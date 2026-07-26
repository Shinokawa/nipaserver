//! 媒体库与条目 API（M1：§8.1 的 /libraries 与 /items 组）。

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api_userdata::USER_ID;
use crate::state::AppState;

/// (id, name, path, kind, file_count)
type LibraryDbRow = (i64, Option<String>, String, Option<String>, i64);
/// (task_id, rel_path, result, confidence, evidence)
type PendingDbRow = (i64, Option<String>, Option<String>, Option<String>, Option<String>);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/libraries", get(list_libraries).post(create_library))
        .route("/api/v1/libraries/{id}/scan", post(trigger_scan))
        .route("/api/v1/items", get(list_items))
        .route("/api/v1/items/{id}", get(get_item))
        .route("/api/v1/scrape/pending", get(list_pending))
}

#[derive(Debug, Serialize)]
struct Library {
    id: i64,
    name: Option<String>,
    path: String,
    kind: Option<String>,
    file_count: i64,
}

async fn list_libraries(
    State(state): State<AppState>,
) -> Result<Json<Vec<Library>>, (StatusCode, String)> {
    let rows: Vec<LibraryDbRow> = sqlx::query_as(
        "SELECT l.id, l.name, l.path, l.kind,
                (SELECT COUNT(*) FROM media_files m WHERE m.library_id = l.id)
         FROM libraries l ORDER BY l.id",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, path, kind, file_count)| Library { id, name, path, kind, file_count })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateLibrary {
    name: String,
    path: String,
    /// anime | movie | tv（扫描策略与默认排序提示，M1 仅存储）
    kind: Option<String>,
}

async fn create_library(
    State(state): State<AppState>,
    Json(req): Json<CreateLibrary>,
) -> Result<Json<Library>, (StatusCode, String)> {
    let path = std::path::Path::new(&req.path);
    if !path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("目录不存在: {}", req.path)));
    }
    // canonicalize：拒绝相对路径注入，存绝对路径（§8.4 路径安全）
    let canonical = path
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("路径无效: {e}")))?
        .display()
        .to_string();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO libraries (name, path, kind) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(&req.name)
    .bind(&canonical)
    .bind(&req.kind)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(Library {
        id,
        name: Some(req.name),
        path: canonical,
        kind: req.kind,
        file_count: 0,
    }))
}

#[derive(Debug, Serialize)]
struct ScanStarted {
    library_id: i64,
    hint: &'static str,
}

async fn trigger_scan(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ScanStarted>, (StatusCode, String)> {
    let row: Option<(String,)> = sqlx::query_as("SELECT path FROM libraries WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;
    let Some((path,)) = row else {
        return Err((StatusCode::NOT_FOUND, format!("库 {id} 不存在")));
    };
    // 后台执行；进度经 SSE ScanProgress 事件
    let db = state.db.clone();
    let events = state.events.clone();
    let scrape = state.scrape.clone();
    let dandan = state.dandan.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::scan::scan_library(
            &db,
            &events,
            dandan.as_ref(),
            scrape.as_ref(),
            crate::api::SCRAPE_SYSTEM_PROMPT,
            id,
            &path,
        )
        .await
        {
            tracing::error!(library_id = id, error = %e, "scan failed");
        }
    });
    Ok(Json(ScanStarted {
        library_id: id,
        hint: "subscribe /api/v1/events (scan_progress) for status",
    }))
}

#[derive(Debug, Deserialize)]
struct ItemsQuery {
    library: Option<i64>,
    kind: Option<String>,
    /// air_date | added_at | title | sort_name | random
    sort: Option<String>,
    air_year: Option<i32>,
    air_month: Option<u8>,
    /// 标题/原名 LIKE（批次 B §11）
    search: Option<String>,
    /// genre 名（JOIN item_values）
    genre: Option<String>,
    /// 已看状态过滤（JOIN watch_history user 1）
    is_played: Option<bool>,
    is_favorite: Option<bool>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
struct ItemRow {
    id: i64,
    kind: String,
    parent_id: Option<i64>,
    title: Option<String>,
    original_title: Option<String>,
    year: Option<i64>,
    season_no: Option<i64>,
    episode_no: Option<i64>,
    air_date: Option<String>,
    poster_path: Option<String>,
}

/// LIKE 通配符转义（用户输入是数据不是模式）。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 海报墙数据（§8.1 + 批次 B §11 扩参）。默认只返回顶层实体（series/movie）。
/// 响应头 `X-Total-Count` = 同条件不分页总数（客户端无限滚动依赖）。
async fn list_items(
    State(state): State<AppState>,
    Query(q): Query<ItemsQuery>,
) -> Result<(HeaderMap, Json<Vec<ItemRow>>), (StatusCode, String)> {
    // WHERE 片段：全部"枚举→静态 SQL + bind"模式，用户输入不进 format!（§6.4）
    let mut cond = String::from(" WHERE deleted_at IS NULL");
    let mut binds: Vec<String> = Vec::new();
    match &q.kind {
        Some(k) => {
            cond.push_str(" AND kind = ?");
            binds.push(k.clone());
        }
        None => cond.push_str(" AND kind IN ('series','movie')"),
    }
    if let Some(lib) = q.library {
        cond.push_str(" AND library_id = ?");
        binds.push(lib.to_string());
    }
    if let Some(y) = q.air_year {
        match q.air_month {
            Some(m) => {
                cond.push_str(" AND air_date LIKE ?");
                binds.push(format!("{y:04}-{m:02}%"));
            }
            None => {
                cond.push_str(" AND air_date LIKE ?");
                binds.push(format!("{y:04}%"));
            }
        }
    }
    if let Some(term) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cond.push_str(" AND (title LIKE ? ESCAPE '\\' OR original_title LIKE ? ESCAPE '\\')");
        let pattern = format!("%{}%", like_escape(term));
        binds.push(pattern.clone());
        binds.push(pattern);
    }
    if let Some(genre) = q.genre.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cond.push_str(
            " AND EXISTS (SELECT 1 FROM item_value_map m
                          JOIN item_values v ON v.id = m.value_id
                          WHERE m.item_id = items.id AND v.kind = 'genre' AND v.value = ?)",
        );
        binds.push(genre.to_string());
    }
    // 用户态过滤（auth 前固定 user 1）：无 watch_history 行视为未看/未收藏
    if let Some(played) = q.is_played {
        let frag = if played {
            " AND EXISTS (SELECT 1 FROM watch_history w
                          WHERE w.item_id = items.id AND w.user_id = ? AND w.played = 1)"
        } else {
            " AND NOT EXISTS (SELECT 1 FROM watch_history w
                              WHERE w.item_id = items.id AND w.user_id = ? AND w.played = 1)"
        };
        cond.push_str(frag);
        binds.push(USER_ID.to_string());
    }
    if let Some(fav) = q.is_favorite {
        let frag = if fav {
            " AND EXISTS (SELECT 1 FROM watch_history w
                          WHERE w.item_id = items.id AND w.user_id = ? AND w.is_favorite = 1)"
        } else {
            " AND NOT EXISTS (SELECT 1 FROM watch_history w
                              WHERE w.item_id = items.id AND w.user_id = ? AND w.is_favorite = 1)"
        };
        cond.push_str(frag);
        binds.push(USER_ID.to_string());
    }

    // 总数（同条件不分页）
    let count_sql = format!("SELECT COUNT(*) FROM items{cond}");
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_query = count_query.bind(b);
    }
    let total = count_query.fetch_one(&state.db).await.map_err(internal)?;

    let sort = match q.sort.as_deref() {
        Some("air_date") => "air_date DESC NULLS LAST",
        Some("title") => "title ASC",
        Some("sort_name") => "COALESCE(sort_name, title) ASC",
        Some("random") => "RANDOM()",
        _ => "added_at DESC",
    };
    let sql = format!(
        "SELECT id, kind, parent_id, title, original_title, year, season_no, episode_no,
                air_date, poster_path
         FROM items{cond} ORDER BY {sort} LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, (i64, String, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<String>, Option<String>)>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows = query
        .bind(q.limit.clamp(1, 200))
        .bind(q.offset.max(0))
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Total-Count",
        HeaderValue::from_str(&total.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    Ok((
        headers,
        Json(
            rows.into_iter()
                .map(|(id, kind, parent_id, title, original_title, year, season_no, episode_no, air_date, poster_path)| ItemRow {
                    id, kind, parent_id, title, original_title, year, season_no, episode_no, air_date, poster_path,
                })
                .collect(),
        ),
    ))
}

#[derive(Debug, Serialize)]
struct ItemDetail {
    #[serde(flatten)]
    item: ItemRow,
    // 批次 B §12：0005 新列
    overview: Option<String>,
    rating: Option<f64>,
    backdrop_path: Option<String>,
    series_status: Option<String>,
    runtime_ms: Option<i64>,
    end_date: Option<String>,
    tagline: Option<String>,
    external_ids: Vec<(String, String)>,
    genres: Vec<String>,
    studios: Vec<String>,
    people: Vec<PersonRow>,
    user_data: UserData,
    children: Vec<ItemRow>,
    files: Vec<FileRow>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PersonRow {
    name: String,
    kind: String,
    role: Option<String>,
    image_url: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct UserData {
    position_ms: i64,
    played: bool,
    is_favorite: bool,
}

#[derive(Debug, Serialize)]
struct FileRow {
    id: i64,
    rel_path: String,
    size: i64,
}

async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ItemDetail>, (StatusCode, String)> {
    type Row = (i64, String, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<String>, Option<String>);
    // 详情扩展列（§12）：overview/rating/backdrop + 0005 新列。
    type ExtraRow = (Option<String>, Option<f64>, Option<String>, Option<String>, Option<i64>, Option<String>, Option<String>);
    let to_item = |r: Row| ItemRow {
        id: r.0, kind: r.1, parent_id: r.2, title: r.3, original_title: r.4,
        year: r.5, season_no: r.6, episode_no: r.7, air_date: r.8, poster_path: r.9,
    };
    const COLS: &str = "id, kind, parent_id, title, original_title, year, season_no, episode_no, air_date, poster_path";

    let row: Option<Row> =
        sqlx::query_as(&format!("SELECT {COLS} FROM items WHERE id = ? AND deleted_at IS NULL"))
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?;
    let Some(row) = row else {
        return Err((StatusCode::NOT_FOUND, format!("条目 {id} 不存在")));
    };
    let extra: ExtraRow = sqlx::query_as(
        "SELECT overview, rating, backdrop_path, series_status, runtime_ms, end_date, tagline
         FROM items WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    let external_ids: Vec<(String, String)> =
        sqlx::query_as("SELECT provider, external_id FROM item_ids WHERE item_id = ?")
            .bind(id)
            .fetch_all(&state.db)
            .await
            .map_err(internal)?;
    let genres = fetch_values(&state, id, "genre").await?;
    let studios = fetch_values(&state, id, "studio").await?;
    let people: Vec<PersonRow> = sqlx::query_as(
        "SELECT p.name, p.kind, ip.role, p.image_url
         FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = ? ORDER BY ip.sort_order",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    let user_data: Option<(Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT position_ms, played, is_favorite FROM watch_history
         WHERE user_id = ? AND item_id = ?",
    )
    .bind(USER_ID)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;
    let children: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM items WHERE parent_id = ? AND deleted_at IS NULL
         ORDER BY season_no, episode_no"
    ))
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    let files: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT m.id, m.rel_path, m.size FROM media_files m
         JOIN file_item fi ON fi.file_id = m.id WHERE fi.item_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let (overview, rating, backdrop_path, series_status, runtime_ms, end_date, tagline) = extra;
    Ok(Json(ItemDetail {
        item: to_item(row),
        overview,
        rating,
        backdrop_path,
        series_status,
        runtime_ms,
        end_date,
        tagline,
        external_ids,
        genres,
        studios,
        people,
        user_data: user_data
            .map(|(position_ms, played, is_favorite)| UserData {
                position_ms: position_ms.unwrap_or(0),
                played: played != 0,
                is_favorite: is_favorite != 0,
            })
            .unwrap_or_default(),
        children: children.into_iter().map(to_item).collect(),
        files: files
            .into_iter()
            .map(|(id, rel_path, size)| FileRow { id, rel_path, size })
            .collect(),
    }))
}

/// item_values 取某类值列表（genre/studio）。
async fn fetch_values(
    state: &AppState,
    item_id: i64,
    kind: &str,
) -> Result<Vec<String>, (StatusCode, String)> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT v.value FROM item_value_map m JOIN item_values v ON v.id = m.value_id
         WHERE m.item_id = ? AND v.kind = ? ORDER BY v.value",
    )
    .bind(item_id)
    .bind(kind)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(rows.into_iter().map(|(v,)| v).collect())
}

#[derive(Debug, Serialize)]
struct PendingRow {
    task_id: i64,
    file: Option<String>,
    result: Option<serde_json::Value>,
    confidence: Option<String>,
    evidence: Option<String>,
}

/// 待确认队列（§8.1 /scrape/pending）。
async fn list_pending(
    State(state): State<AppState>,
) -> Result<Json<Vec<PendingRow>>, (StatusCode, String)> {
    let rows: Vec<PendingDbRow> = sqlx::query_as(
            "SELECT t.id, m.rel_path, t.result, t.confidence, t.evidence
             FROM scrape_tasks t LEFT JOIN media_files m ON m.id = t.file_id
             WHERE t.state = 'needs_review' ORDER BY t.id",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|(task_id, file, result, confidence, evidence)| PendingRow {
                task_id,
                file,
                result: result.and_then(|r| serde_json::from_str(&r).ok()),
                confidence,
                evidence: evidence.map(|e| e.chars().take(300).collect()),
            })
            .collect(),
    ))
}

fn internal(e: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
