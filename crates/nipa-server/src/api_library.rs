//! 媒体库与条目 API（M1：§8.1 的 /libraries 与 /items 组）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

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
    /// air_date | added_at | title
    sort: Option<String>,
    air_year: Option<i32>,
    air_month: Option<u8>,
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

/// 海报墙数据（§8.1）。默认只返回顶层实体（series/movie），episode 经详情页取。
async fn list_items(
    State(state): State<AppState>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Vec<ItemRow>>, (StatusCode, String)> {
    let mut sql = String::from(
        "SELECT id, kind, parent_id, title, original_title, year, season_no, episode_no,
                air_date, poster_path
         FROM items WHERE deleted_at IS NULL",
    );
    let mut binds: Vec<String> = Vec::new();
    match &q.kind {
        Some(k) => {
            sql.push_str(" AND kind = ?");
            binds.push(k.clone());
        }
        None => sql.push_str(" AND kind IN ('series','movie')"),
    }
    if let Some(lib) = q.library {
        sql.push_str(" AND library_id = ?");
        binds.push(lib.to_string());
    }
    if let Some(y) = q.air_year {
        match q.air_month {
            Some(m) => {
                sql.push_str(" AND air_date LIKE ?");
                binds.push(format!("{y:04}-{m:02}%"));
            }
            None => {
                sql.push_str(" AND air_date LIKE ?");
                binds.push(format!("{y:04}%"));
            }
        }
    }
    let sort = match q.sort.as_deref() {
        Some("air_date") => "air_date DESC NULLS LAST",
        Some("title") => "title ASC",
        _ => "added_at DESC",
    };
    sql.push_str(&format!(" ORDER BY {sort} LIMIT ? OFFSET ?"));

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
    Ok(Json(
        rows.into_iter()
            .map(|(id, kind, parent_id, title, original_title, year, season_no, episode_no, air_date, poster_path)| ItemRow {
                id, kind, parent_id, title, original_title, year, season_no, episode_no, air_date, poster_path,
            })
            .collect(),
    ))
}

#[derive(Debug, Serialize)]
struct ItemDetail {
    #[serde(flatten)]
    item: ItemRow,
    external_ids: Vec<(String, String)>,
    children: Vec<ItemRow>,
    files: Vec<FileRow>,
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
    let external_ids: Vec<(String, String)> =
        sqlx::query_as("SELECT provider, external_id FROM item_ids WHERE item_id = ?")
            .bind(id)
            .fetch_all(&state.db)
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

    Ok(Json(ItemDetail {
        item: to_item(row),
        external_ids,
        children: children.into_iter().map(to_item).collect(),
        files: files
            .into_iter()
            .map(|(id, rel_path, size)| FileRow { id, rel_path, size })
            .collect(),
    }))
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
