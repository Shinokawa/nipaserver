//! 图片本地缓存与伺服（对标批次 C；docs/07 §C、webui-audit P0#9）。
//!
//! 现状问题：poster_path 存 Bangumi/弹弹play 远程 URL，前端热链——
//! 不稳定（防盗链风险）、无缩放、无缓存。本模块改为：
//! `GET /api/v1/items/{id}/images/{type}?width=` → 首次请求时下载远程图到
//! `data/images/{item_id}-{type}.{ext}`，之后直接伺服本地文件；带 width 时
//! 用 image crate 缩放并缓存缩放版（`-w{width}` 后缀）。
//!
//! 下载失败时 302 回源 URL（优雅降级，不阻塞前端）。

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

use crate::state::AppState;

const MAX_IMAGE_BYTES: usize = 15 * 1024 * 1024;
/// 缩放宽度白名单（防任意尺寸打爆缓存目录）。
const ALLOWED_WIDTHS: &[u32] = &[200, 300, 400, 600, 800, 1280];

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
    width: Option<u32>,
}

/// GET /api/v1/items/{id}/images/{type}
pub async fn item_image(
    State(state): State<AppState>,
    AxumPath((item_id, image_type)): AxumPath<(i64, String)>,
    Query(q): Query<ImageQuery>,
) -> Response {
    if !matches!(
        image_type.as_str(),
        "primary" | "backdrop" | "thumb" | "logo" | "banner"
    ) {
        return (StatusCode::BAD_REQUEST, "unknown image type").into_response();
    }

    // 源 URL：item_images 优先，快捷列兜底（primary=poster_path, backdrop=backdrop_path）
    let source: Option<String> = {
        let from_table: Option<(Option<String>,)> =
            sqlx::query_as("SELECT url FROM item_images WHERE item_id = ? AND image_type = ?")
                .bind(item_id)
                .bind(&image_type)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        match from_table {
            Some((Some(url),)) => Some(url),
            _ => {
                let col = match image_type.as_str() {
                    "primary" => "poster_path",
                    "backdrop" => "backdrop_path",
                    _ => return (StatusCode::NOT_FOUND, "no image").into_response(),
                };
                let row: Option<(Option<String>,)> =
                    sqlx::query_as(&format!("SELECT {col} FROM items WHERE id = ?"))
                        .bind(item_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                row.and_then(|(u,)| u)
            }
        }
    };
    let Some(source_url) = source else {
        return (StatusCode::NOT_FOUND, "no image").into_response();
    };

    // 已是本地相对路径（未来 ingest 直接落地的情况）
    let images_dir = state.config.server.data_dir.join("images");
    let base = images_dir.join(format!("{item_id}-{image_type}"));

    // 命中原图缓存？
    let cached = find_cached(&base).await;
    let original = match cached {
        Some(p) => p,
        None => match download(&state, &source_url, &base).await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(item_id, error = %e, "image download failed; redirecting to source");
                return Redirect::temporary(&source_url).into_response();
            }
        },
    };

    // 缩放请求
    if let Some(w) = q.width {
        let Some(&w) = ALLOWED_WIDTHS
            .iter()
            .find(|&&a| a >= w)
            .or(ALLOWED_WIDTHS.last())
        else {
            return serve_file(&original).await;
        };
        let scaled = original.with_file_name(format!(
            "{}-w{w}.jpg",
            original
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("img")
        ));
        if !scaled.exists() {
            let orig = original.clone();
            let scaled_clone = scaled.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let img = image::open(&orig)?;
                let resized = img.resize(w, u32::MAX, image::imageops::FilterType::Lanczos3);
                resized
                    .to_rgb8()
                    .save_with_format(&scaled_clone, image::ImageFormat::Jpeg)?;
                Ok(())
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                return serve_file(&original).await;
            }
        }
        return serve_file(&scaled).await;
    }
    serve_file(&original).await
}

async fn find_cached(base: &std::path::Path) -> Option<PathBuf> {
    for ext in ["jpg", "png", "webp"] {
        let p = base.with_extension(ext);
        if tokio::fs::try_exists(&p).await.unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

async fn download(state: &AppState, url: &str, base: &std::path::Path) -> anyhow::Result<PathBuf> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        anyhow::bail!("not a remote url");
    }
    if let Some(dir) = base.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let resp = state.http.get(url).send().await?.error_for_status()?;
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ext = if content_type.contains("png") {
        "png"
    } else if content_type.contains("webp") {
        "webp"
    } else {
        "jpg"
    };
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_IMAGE_BYTES {
        anyhow::bail!("image too large");
    }
    if bytes.is_empty() {
        anyhow::bail!("empty image");
    }
    let path = base.with_extension(ext);
    let tmp = base.with_extension(format!("{ext}.tmp"));
    let mut f = tokio::fs::File::create(&tmp).await?;
    f.write_all(&bytes).await?;
    f.flush().await?;
    drop(f);
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}

async fn serve_file(path: &std::path::Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let mime = match path.extension().and_then(|e| e.to_str()) {
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                _ => "image/jpeg",
            };
            (
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    // 图片按条目缓存，元数据变更会换 URL 里的条目 id；一天缓存合理
                    (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "image gone").into_response(),
    }
}
