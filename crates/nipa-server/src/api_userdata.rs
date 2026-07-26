//! 用户数据与首页查询 API（Jellyfin 对标批次 B，docs/07）。
//!
//! 语义来源 docs/research/jellyfin-full/userdata-playback.md：
//! - /playback/progress：三合一简化版（start/progress/stop），§2；
//! - /items/{id}/played、/favorite：MarkPlayed/MarkUnplayed，§2.3/§4；
//! - /items/resume、/shows/next-up、/items/latest、/search：三大首页查询，§3。
//!
//! auth 未实现（§8.4）前 user_id 固定为 1（迁移 0006 落了默认用户）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::userdata;

/// auth 落地前的固定单用户（§8.4）。
pub const USER_ID: i64 = 1;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/playback/progress", post(playback_progress))
        // 静态段路由优先于 /items/{id}（axum/matchit 语义）
        .route("/api/v1/items/resume", get(items_resume))
        .route("/api/v1/items/latest", get(items_latest))
        .route("/api/v1/shows/next-up", get(shows_next_up))
        .route("/api/v1/search", get(search))
        .route("/api/v1/items/{id}/played", post(mark_played).delete(mark_unplayed))
        .route("/api/v1/items/{id}/favorite", post(mark_favorite).delete(unmark_favorite))
}

// ===== 播放进度上报（§2 三端点合一） =====

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProgressEvent {
    Start,
    Progress,
    Stop,
}

#[derive(Debug, Deserialize)]
struct ProgressBody {
    item_id: i64,
    file_id: Option<i64>,
    #[serde(default)]
    position_ms: i64,
    duration_ms: Option<i64>,
    event: ProgressEvent,
}

/// start → play_count+1 & last_played_at=now（Jellyfin：播放次数在开始时 +1）；
/// progress/stop → 跑 §1.2 判定后 upsert watch_history。
async fn playback_progress(
    State(state): State<AppState>,
    Json(body): Json<ProgressBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    if body.position_ms < 0 {
        return Err((StatusCode::BAD_REQUEST, "position_ms 不能为负".into()));
    }
    // 条目必须存在（顺带拿 runtime_ms 作为 duration 兜底）
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT runtime_ms FROM items WHERE id = ? AND deleted_at IS NULL")
            .bind(body.item_id)
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?;
    let Some((runtime_ms,)) = row else {
        return Err((StatusCode::NOT_FOUND, format!("条目 {} 不存在", body.item_id)));
    };

    match body.event {
        ProgressEvent::Start => {
            sqlx::query(
                "INSERT INTO watch_history
                   (user_id, item_id, file_id, position_ms, played, play_count,
                    updated_at, last_played_at)
                 VALUES (?, ?, ?, 0, 0, 1, unixepoch(), unixepoch())
                 ON CONFLICT(user_id, item_id) DO UPDATE SET
                   file_id = COALESCE(excluded.file_id, file_id),
                   play_count = play_count + 1,
                   updated_at = excluded.updated_at,
                   last_played_at = excluded.last_played_at",
            )
            .bind(USER_ID)
            .bind(body.item_id)
            .bind(body.file_id)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        }
        ProgressEvent::Progress | ProgressEvent::Stop => {
            // duration：客户端上报优先，items.runtime_ms 兜底（§5.3）
            let duration = body.duration_ms.filter(|d| *d > 0).or(runtime_ms);
            let ps = userdata::resolve_play_state(body.position_ms, duration);
            sqlx::query(
                "INSERT INTO watch_history
                   (user_id, item_id, file_id, position_ms, duration_ms, played,
                    updated_at, last_played_at)
                 VALUES (?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
                 ON CONFLICT(user_id, item_id) DO UPDATE SET
                   file_id = COALESCE(excluded.file_id, file_id),
                   position_ms = excluded.position_ms,
                   duration_ms = COALESCE(excluded.duration_ms, duration_ms),
                   played = MAX(played, excluded.played),  -- 只置 true 不置 false（§1.2）
                   updated_at = excluded.updated_at,
                   last_played_at = excluded.last_played_at",
            )
            .bind(USER_ID)
            .bind(body.item_id)
            .bind(body.file_id)
            .bind(ps.position_ms)
            .bind(duration)
            .bind(ps.mark_played as i64)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ===== 手动标记（§2.3 MarkPlayed/MarkUnplayed + §4 收藏） =====

/// MarkPlayed：played=1、position=0、play_count=max(play_count,1)、last_played_at=now。
async fn mark_played(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_item(&state, id).await?;
    sqlx::query(
        "INSERT INTO watch_history
           (user_id, item_id, position_ms, played, play_count, updated_at, last_played_at)
         VALUES (?, ?, 0, 1, 1, unixepoch(), unixepoch())
         ON CONFLICT(user_id, item_id) DO UPDATE SET
           played = 1,
           position_ms = 0,
           play_count = MAX(play_count, 1),
           updated_at = unixepoch(),
           last_played_at = unixepoch()",
    )
    .bind(USER_ID)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// MarkUnplayed：清 played 与续播点（显式清除是 played 归零的唯一途径）。
async fn mark_unplayed(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_item(&state, id).await?;
    sqlx::query(
        "UPDATE watch_history SET played = 0, position_ms = 0, updated_at = unixepoch()
         WHERE user_id = ? AND item_id = ?",
    )
    .bind(USER_ID)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mark_favorite(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_item(&state, id).await?;
    sqlx::query(
        "INSERT INTO watch_history (user_id, item_id, is_favorite, updated_at)
         VALUES (?, ?, 1, unixepoch())
         ON CONFLICT(user_id, item_id) DO UPDATE SET
           is_favorite = 1, updated_at = unixepoch()",
    )
    .bind(USER_ID)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unmark_favorite(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    ensure_item(&state, id).await?;
    sqlx::query(
        "UPDATE watch_history SET is_favorite = 0, updated_at = unixepoch()
         WHERE user_id = ? AND item_id = ?",
    )
    .bind(USER_ID)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_item(state: &AppState, id: i64) -> Result<(), (StatusCode, String)> {
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM items WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?;
    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, format!("条目 {id} 不存在")));
    }
    Ok(())
}

// ===== 三大首页查询（§3） =====

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_home_limit")]
    limit: i64,
}

fn default_home_limit() -> i64 {
    12
}

/// 继续观看（§3.2）：position>0 且未看完，最近播放在前。
async fn items_resume(
    State(state): State<AppState>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<userdata::ResumeRow>>, (StatusCode, String)> {
    let rows = userdata::query_resume(&state.db, USER_ID, q.limit.clamp(1, 100))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

/// NextUp（§3.1 两阶段）：最近在看的剧 → 每剧下一未看集。
async fn shows_next_up(
    State(state): State<AppState>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<userdata::NextUpRow>>, (StatusCode, String)> {
    let rows = userdata::query_next_up(&state.db, USER_ID, q.limit.clamp(1, 100))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

/// 最新添加（§3.3）：按库分组的最新 series/movie。
async fn items_latest(
    State(state): State<AppState>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<userdata::LatestRow>>, (StatusCode, String)> {
    let rows = userdata::query_latest(&state.db, q.limit.clamp(1, 50))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

// ===== 搜索（§4 简化：title/original_title LIKE，series/movie 优先） =====

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct SearchHit {
    id: i64,
    kind: String,
    title: Option<String>,
    original_title: Option<String>,
    year: Option<i64>,
    season_no: Option<i64>,
    episode_no: Option<i64>,
    series_title: Option<String>,
    poster_path: Option<String>,
}

async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchHit>>, (StatusCode, String)> {
    let term = q.q.trim();
    if term.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let pattern = format!("%{}%", like_escape(term));
    let rows: Vec<SearchHit> = sqlx::query_as(
        "SELECT i.id, i.kind, i.title, i.original_title, i.year,
                i.season_no, i.episode_no, sr.title AS series_title,
                COALESCE(i.poster_path, sr.poster_path) AS poster_path
         FROM items i
         LEFT JOIN items sr ON sr.id = i.series_id
         WHERE i.deleted_at IS NULL
           AND i.kind IN ('series', 'movie', 'episode')
           AND (i.title LIKE ?1 ESCAPE '\\' OR i.original_title LIKE ?1 ESCAPE '\\')
         ORDER BY CASE WHEN i.kind = 'episode' THEN 1 ELSE 0 END, i.title
         LIMIT 20",
    )
    .bind(&pattern)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(rows))
}

/// LIKE 通配符转义（用户输入是数据不是模式）。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn internal(e: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
