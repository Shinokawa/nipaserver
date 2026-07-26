//! M3 播放 API：PlaybackInfo、签名 Direct Play（Range）与按需 HLS。

use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{Request, Response, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use nipa_stream::{
    DeviceProfile, HlsSessionSpec, MediaSource, PlayMethod, TranscodeReason, decide_video,
    normalize_container,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/playback/info", post(playback_info))
        .route("/api/v1/stream/direct/{file_id}", get(direct_stream))
        .route(
            "/api/v1/stream/hls/{session}/master.m3u8",
            get(hls_playlist),
        )
        .route(
            "/api/v1/stream/hls/{session}/{segment_file}",
            get(hls_segment),
        )
}

#[derive(Debug, Deserialize)]
struct PlaybackInfoRequest {
    #[serde(alias = "FileId")]
    file_id: i64,
    #[serde(default, alias = "DeviceProfile")]
    device_profile: DeviceProfile,
}

#[derive(Debug, Serialize)]
struct PlaybackInfoResponse {
    media_sources: Vec<PlaybackMediaSource>,
    play_session_id: Option<String>,
    error_code: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct PlaybackMediaSource {
    id: i64,
    name: String,
    size: Option<i64>,
    container: String,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    runtime_ticks: Option<i64>,
    supports_direct_play: bool,
    supports_direct_stream: bool,
    supports_transcoding: bool,
    transcode_reasons: Vec<TranscodeReason>,
    direct_url: Option<String>,
    transcode_url: Option<String>,
}

#[derive(Debug)]
struct FileSource {
    id: i64,
    path: PathBuf,
    name: String,
    size: Option<i64>,
    probe: Option<Value>,
}

async fn playback_info(
    State(state): State<AppState>,
    Json(req): Json<PlaybackInfoRequest>,
) -> Result<Json<PlaybackInfoResponse>, (StatusCode, String)> {
    let source = load_file_source(&state, req.file_id).await?;
    let probe = ensure_probe(
        &state,
        &source,
        req.device_profile.client == nipa_stream::ClientKind::Nipa,
    )
    .await?;
    let summary = nipa_stream::probe::summarize(&probe);
    let format = probe.get("format");
    let bitrate = format.and_then(|f| f.get("bit_rate")).and_then(value_u64);
    let container = summary
        .container
        .as_deref()
        .map(normalize_container)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| normalize_container(&extension(&source.path)));
    let media = MediaSource {
        container,
        video_codec: summary.video.as_ref().map(|v| v.codec.clone()),
        audio_codec: summary.audio_tracks.first().map(|a| a.codec.clone()),
        bitrate,
        hdr: summary.video.as_ref().is_some_and(|v| v.hdr),
    };
    let decision = decide_video(&req.device_profile, &media);
    let direct_query = state
        .stream_tokens
        .sign("direct", source.id, Duration::from_secs(60 * 60));
    let direct_url = format!("/api/v1/stream/direct/{}?{}", source.id, direct_query);
    let duration = summary.duration_secs;

    let (session_id, transcode_url) = if decision.method == PlayMethod::DirectPlay {
        (None, None)
    } else {
        let hls = state.hls.as_ref().ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "ffmpeg 不可用，该文件只能 Direct Play".into(),
        ))?;
        let duration_secs = duration.filter(|d| d.is_finite() && *d > 0.0).ok_or((
            StatusCode::UNPROCESSABLE_ENTITY,
            "ffprobe 未返回有效时长，无法生成 HLS".into(),
        ))?;
        let id = hls
            .create_session(HlsSessionSpec {
                source: source.path.clone(),
                duration_secs,
                method: decision.method,
            })
            .await
            .map_err(stream_error)?;
        // subject 直接绑定 128-bit session id，segment 与 playlist 共用此签名。
        let query = state
            .stream_tokens
            .sign_subject("hls", &id, Duration::from_secs(60 * 60));
        let url = format!("/api/v1/stream/hls/{id}/master.m3u8?{query}");
        (Some(id), Some(url))
    };
    Ok(Json(PlaybackInfoResponse {
        media_sources: vec![PlaybackMediaSource {
            id: source.id,
            name: source.name,
            size: source.size,
            container: media.container,
            video_codec: media.video_codec,
            audio_codec: media.audio_codec,
            runtime_ticks: duration.map(|d| (d * 10_000_000.0) as i64),
            supports_direct_play: decision.method == PlayMethod::DirectPlay,
            supports_direct_stream: decision.method == PlayMethod::Remux,
            supports_transcoding: state.hls.is_some(),
            transcode_reasons: decision.reasons,
            direct_url: (decision.method == PlayMethod::DirectPlay).then_some(direct_url),
            transcode_url,
        }],
        play_session_id: session_id,
        error_code: None,
    }))
}

#[derive(Debug, Deserialize)]
struct SignatureQuery {
    exp: i64,
    sig: String,
}

async fn direct_stream(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(sig): Query<SignatureQuery>,
    request: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    if !state
        .stream_tokens
        .verify("direct", file_id, sig.exp, &sig.sig)
    {
        return Err((StatusCode::UNAUTHORIZED, "播放 URL 签名无效或已过期".into()));
    }
    // Direct Play 不依赖 ffprobe，保持 §6.3 的降级契约。
    let source = load_file_source(&state, file_id).await?;
    // ServeFile 原生实现 RFC Range/If-Range/HEAD，不把整个媒体读入内存。
    let response = ServeFile::new(source.path)
        .oneshot(request)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(response.map(Body::new))
}

async fn hls_playlist(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Query(sig): Query<SignatureQuery>,
) -> Result<Response<Body>, (StatusCode, String)> {
    verify_hls(&state, &session, &sig)?;
    let hls = state
        .hls
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "HLS 不可用".into()))?;
    let query = format!("exp={}&sig={}", sig.exp, sig.sig);
    let playlist = hls.playlist(&session, &query).await.map_err(stream_error)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn hls_segment(
    State(state): State<AppState>,
    Path((session, segment_file)): Path<(String, String)>,
    Query(sig): Query<SignatureQuery>,
) -> Result<Response<Body>, (StatusCode, String)> {
    verify_hls(&state, &session, &sig)?;
    let segment = segment_file
        .strip_suffix(".m4s")
        .and_then(|value| value.parse::<i32>().ok())
        .ok_or((StatusCode::NOT_FOUND, "HLS segment 路径无效".into()))?;
    let hls = state
        .hls
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "HLS 不可用".into()))?;
    let segment = hls.segment(&session, segment).await.map_err(stream_error)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, segment.content_type)
        .header(header::CACHE_CONTROL, "private, max-age=3600, immutable")
        .body(Body::from(segment.bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

fn verify_hls(
    state: &AppState,
    session: &str,
    sig: &SignatureQuery,
) -> Result<(), (StatusCode, String)> {
    if state
        .stream_tokens
        .verify_subject("hls", session, sig.exp, &sig.sig)
    {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "HLS URL 签名无效或已过期".into()))
    }
}

async fn load_file_source(
    state: &AppState,
    file_id: i64,
) -> Result<FileSource, (StatusCode, String)> {
    let row: Option<(String, Option<i64>, Option<String>, String)> = sqlx::query_as(
        "SELECT mf.rel_path, mf.size, mf.ffprobe, l.path
         FROM media_files mf JOIN libraries l ON l.id = mf.library_id
         WHERE mf.id = ?",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;
    let Some((rel_path, size, stored_probe, library_path)) = row else {
        return Err((StatusCode::NOT_FOUND, format!("媒体文件 {file_id} 不存在")));
    };
    let path = checked_media_path(FsPath::new(&library_path), &rel_path)?;
    let probe = stored_probe.and_then(|s| serde_json::from_str(&s).ok());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("未命名媒体")
        .to_string();
    Ok(FileSource {
        id: file_id,
        path,
        name,
        size,
        probe,
    })
}

async fn ensure_probe(
    state: &AppState,
    source: &FileSource,
    allow_unknown: bool,
) -> Result<Value, (StatusCode, String)> {
    if let Some(probe) = source.probe.clone().filter(|v| !v.is_null()) {
        return Ok(probe);
    }
    let Some(paths) = state.ffmpeg_paths.as_ref() else {
        if allow_unknown {
            // NipaPlay 直放无需 codec 协商，容器后续由扩展名填充。
            return Ok(Value::Null);
        }
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "ffprobe 不可用且数据库无媒体探测结果".into(),
        ));
    };
    Ok(nipa_stream::probe(&paths.ffprobe, &source.path)
        .await
        .map_err(stream_error)?
        .raw)
}

/// canonicalize 根与候选文件，再做组件边界判定，阻断 `..` 与 symlink 逃逸。
fn checked_media_path(root: &FsPath, rel_path: &str) -> Result<PathBuf, (StatusCode, String)> {
    let root = root
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("媒体库路径无效: {e}")))?;
    let relative = PathBuf::from(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    if relative.is_absolute() {
        return Err((StatusCode::FORBIDDEN, "媒体相对路径不得为绝对路径".into()));
    }
    let candidate = root
        .join(relative)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("媒体文件不可读: {e}")))?;
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return Err((StatusCode::FORBIDDEN, "媒体路径越出已配置媒体库".into()));
    }
    Ok(candidate)
}

fn extension(path: &FsPath) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn value_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn internal(error: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn stream_error(error: nipa_stream::StreamError) -> (StatusCode, String) {
    tracing::warn!(error = %error, "playback failed");
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::RANGE;

    #[test]
    fn playback_router_builds_with_axum_path_syntax() {
        let _ = router();
    }

    #[test]
    fn canonical_guard_accepts_child_and_rejects_traversal() {
        let base = std::env::temp_dir().join(format!("nipa-playback-path-{}", std::process::id()));
        let root = base.join("library");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ok.mp4"), b"x").unwrap();
        std::fs::write(base.join("secret.mp4"), b"x").unwrap();
        assert!(checked_media_path(&root, "ok.mp4").is_ok());
        assert!(checked_media_path(&root, "../secret.mp4").is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn serve_file_honors_byte_ranges() {
        let path = std::env::temp_dir().join(format!("nipa-range-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let request = Request::builder()
            .uri("/stream")
            .header(RANGE, "bytes=2-5")
            .body(Body::empty())
            .unwrap();
        let response = ServeFile::new(&path).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        let _ = std::fs::remove_file(path);
    }
}
