//! M4 下载/订阅 API 与后台对账任务。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use nipa_download::{
    AddDownloadRequest, DownloadService, DownloadSnapshot, FeedItem, SubscriptionFilter,
};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(10);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/downloads", get(list_downloads).post(add_download))
        .route(
            "/api/v1/downloads/{info_hash}",
            get(get_download).delete(delete_download),
        )
        .route("/api/v1/downloads/{info_hash}/pause", post(pause_download))
        .route(
            "/api/v1/downloads/{info_hash}/resume",
            post(resume_download),
        )
        .route(
            "/api/v1/subscriptions",
            get(list_subscriptions).post(create_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}",
            put(update_subscription).delete(delete_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}/poll",
            post(poll_one_subscription),
        )
}

pub fn spawn_background(state: AppState, cancel: CancellationToken) {
    let reconcile_state = state.clone();
    let reconcile_cancel = cancel.child_token();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = reconcile_cancel.cancelled() => break,
                _ = ticker.tick() => {
                    if let Err(error) = reconcile_and_ingest(&reconcile_state).await {
                        tracing::error!(%error, "download projection reconciliation failed");
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    if let Err(error) = poll_all_subscriptions(&state).await {
                        tracing::error!(%error, "subscription polling failed");
                    }
                }
            }
        }
    });
}

fn downloads(state: &AppState) -> Result<&Arc<DownloadService>, (StatusCode, String)> {
    state.downloads.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "download engine is unavailable".into(),
        )
    })
}

async fn list_downloads(
    State(state): State<AppState>,
) -> Result<Json<Vec<DownloadSnapshot>>, (StatusCode, String)> {
    Ok(Json(downloads(&state)?.list()))
}

async fn get_download(
    State(state): State<AppState>,
    Path(info_hash): Path<String>,
) -> Result<Json<DownloadSnapshot>, (StatusCode, String)> {
    downloads(&state)?
        .get(&info_hash)
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "download not found".into()))
}

async fn add_download(
    State(state): State<AppState>,
    Json(req): Json<AddDownloadRequest>,
) -> Result<(StatusCode, Json<DownloadSnapshot>), (StatusCode, String)> {
    if req.source.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "source must not be empty".into()));
    }
    let item = downloads(&state)?.add(req).await.map_err(download_error)?;
    reconcile_projection(&state).await.map_err(internal)?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn pause_download(
    State(state): State<AppState>,
    Path(info_hash): Path<String>,
) -> Result<Json<DownloadSnapshot>, (StatusCode, String)> {
    downloads(&state)?
        .pause(&info_hash)
        .await
        .map(Json)
        .map_err(download_error)
}

async fn resume_download(
    State(state): State<AppState>,
    Path(info_hash): Path<String>,
) -> Result<Json<DownloadSnapshot>, (StatusCode, String)> {
    downloads(&state)?
        .resume(&info_hash)
        .await
        .map(Json)
        .map_err(download_error)
}

#[derive(Debug, Deserialize)]
struct DeleteDownloadQuery {
    #[serde(default)]
    delete_files: bool,
}

async fn delete_download(
    State(state): State<AppState>,
    Path(info_hash): Path<String>,
    Query(query): Query<DeleteDownloadQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    downloads(&state)?
        .delete(&info_hash, query.delete_files)
        .await
        .map_err(download_error)?;
    reconcile_projection(&state).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct SubscriptionRow {
    id: i64,
    rss_url: String,
    title: String,
    filters: SubscriptionFilter,
    enabled: bool,
    last_check: Option<i64>,
    last_error: Option<String>,
}

type SubscriptionDbRow = (
    i64,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<String>,
);

async fn list_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<Vec<SubscriptionRow>>, (StatusCode, String)> {
    let rows: Vec<SubscriptionDbRow> = sqlx::query_as(
        "SELECT id, rss_url, COALESCE(title, ''), COALESCE(filters, '{}'),
                COALESCE(enabled, 0), last_check, last_error
         FROM subscriptions ORDER BY id",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    let result = rows
        .into_iter()
        .map(subscription_from_db)
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct SubscriptionRequest {
    rss_url: String,
    title: String,
    #[serde(default)]
    filters: SubscriptionFilter,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

async fn validate_subscription_request(
    req: &SubscriptionRequest,
) -> Result<(), (StatusCode, String)> {
    if req.title.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "title must not be empty".into()));
    }
    nipa_download::validate_public_http_url(&req.rss_url)
        .await
        .map_err(download_error)?;
    nipa_download::select_feed_items(Vec::new(), &req.filters).map_err(download_error)?;
    Ok(())
}

async fn create_subscription(
    State(state): State<AppState>,
    Json(req): Json<SubscriptionRequest>,
) -> Result<(StatusCode, Json<SubscriptionRow>), (StatusCode, String)> {
    validate_subscription_request(&req).await?;
    let filters = serde_json::to_string(&req.filters).map_err(internal)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO subscriptions (rss_url, title, filters, enabled)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(req.rss_url.trim())
    .bind(req.title.trim())
    .bind(filters)
    .bind(req.enabled)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(SubscriptionRow {
            id,
            rss_url: req.rss_url,
            title: req.title,
            filters: req.filters,
            enabled: req.enabled,
            last_check: None,
            last_error: None,
        }),
    ))
}

async fn update_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<SubscriptionRequest>,
) -> Result<Json<SubscriptionRow>, (StatusCode, String)> {
    validate_subscription_request(&req).await?;
    let filters = serde_json::to_string(&req.filters).map_err(internal)?;
    let changed = sqlx::query(
        "UPDATE subscriptions SET rss_url = ?, title = ?, filters = ?, enabled = ?, last_error = NULL
         WHERE id = ?",
    )
    .bind(req.rss_url.trim())
    .bind(req.title.trim())
    .bind(filters)
    .bind(req.enabled)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?
    .rows_affected();
    if changed == 0 {
        return Err((StatusCode::NOT_FOUND, "subscription not found".into()));
    }
    Ok(Json(SubscriptionRow {
        id,
        rss_url: req.rss_url,
        title: req.title,
        filters: req.filters,
        enabled: req.enabled,
        last_check: None,
        last_error: None,
    }))
}

async fn delete_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let changed = sqlx::query("DELETE FROM subscriptions WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(internal)?
        .rows_affected();
    if changed == 0 {
        return Err((StatusCode::NOT_FOUND, "subscription not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct PollResult {
    discovered: usize,
    added: usize,
}

async fn poll_one_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PollResult>, (StatusCode, String)> {
    let row = load_subscription(&state, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "subscription not found".into()))?;
    poll_subscription(&state, &row)
        .await
        .map(Json)
        .map_err(internal)
}

fn subscription_from_db(row: SubscriptionDbRow) -> anyhow::Result<SubscriptionRow> {
    Ok(SubscriptionRow {
        id: row.0,
        rss_url: row.1,
        title: row.2,
        filters: serde_json::from_str(&row.3)?,
        enabled: row.4 != 0,
        last_check: row.5,
        last_error: row.6,
    })
}

async fn load_subscription(state: &AppState, id: i64) -> anyhow::Result<Option<SubscriptionRow>> {
    let row: Option<SubscriptionDbRow> = sqlx::query_as(
        "SELECT id, rss_url, COALESCE(title, ''), COALESCE(filters, '{}'),
                COALESCE(enabled, 0), last_check, last_error
         FROM subscriptions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    row.map(subscription_from_db).transpose()
}

async fn poll_all_subscriptions(state: &AppState) -> anyhow::Result<()> {
    let ids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM subscriptions WHERE enabled = 1")
        .fetch_all(&state.db)
        .await?;
    for (id,) in ids {
        let Some(subscription) = load_subscription(state, id).await? else {
            continue;
        };
        if let Err(error) = poll_subscription(state, &subscription).await {
            tracing::warn!(subscription_id = id, %error, "subscription poll failed");
        }
    }
    Ok(())
}

async fn poll_subscription(state: &AppState, sub: &SubscriptionRow) -> anyhow::Result<PollResult> {
    let service = state
        .downloads
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("download engine unavailable"))?;
    let result = async {
        let items = service.fetch_feed(&sub.rss_url).await?;
        let discovered = items.len();
        let selected = nipa_download::select_feed_items(items, &sub.filters)?;
        let mut added = 0;
        for item in selected {
            if claim_entry(&state.db, sub.id, &item).await? {
                match service
                    .add(AddDownloadRequest {
                        source: item.enclosure_url.clone(),
                        save_path: None,
                    })
                    .await
                {
                    Ok(download) => {
                        sqlx::query(
                            "UPDATE subscription_entries SET info_hash = ?
                             WHERE subscription_id = ? AND entry_key = ?",
                        )
                        .bind(download.info_hash)
                        .bind(sub.id)
                        .bind(&item.entry_key)
                        .execute(&state.db)
                        .await?;
                        added += 1;
                    }
                    Err(error) => {
                        // 释放占位，下次轮询可恢复重试。
                        sqlx::query(
                            "DELETE FROM subscription_entries WHERE subscription_id = ? AND entry_key = ?",
                        )
                        .bind(sub.id)
                        .bind(&item.entry_key)
                        .execute(&state.db)
                        .await?;
                        return Err(anyhow::Error::new(error));
                    }
                }
            }
        }
        Ok::<_, anyhow::Error>(PollResult { discovered, added })
    }
    .await;
    match result {
        Ok(result) => {
            sqlx::query(
                "UPDATE subscriptions SET last_check = unixepoch(), last_error = NULL WHERE id = ?",
            )
            .bind(sub.id)
            .execute(&state.db)
            .await?;
            Ok(result)
        }
        Err(error) => {
            sqlx::query(
                "UPDATE subscriptions SET last_check = unixepoch(), last_error = ? WHERE id = ?",
            )
            .bind(error.to_string())
            .bind(sub.id)
            .execute(&state.db)
            .await?;
            Err(error)
        }
    }
}

async fn claim_entry(db: &sqlx::SqlitePool, sub_id: i64, item: &FeedItem) -> anyhow::Result<bool> {
    let changed = sqlx::query(
        "INSERT OR IGNORE INTO subscription_entries
         (subscription_id, entry_key, title, source_url, created_at)
         VALUES (?, ?, ?, ?, unixepoch())",
    )
    .bind(sub_id)
    .bind(&item.entry_key)
    .bind(&item.title)
    .bind(&item.enclosure_url)
    .execute(db)
    .await?
    .rows_affected();
    Ok(changed == 1)
}

async fn reconcile_and_ingest(state: &AppState) -> anyhow::Result<()> {
    reconcile_projection(state).await?;
    let Some(service) = &state.downloads else {
        return Ok(());
    };
    let library_id = ensure_download_library(state, service.output_dir()).await?;
    for torrent in service.list() {
        let Some(manifest_hash) = torrent.manifest_hash else {
            continue;
        };
        let done: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM torrent_ingests
             WHERE info_hash = ? AND manifest_hash = ? AND state = 'done'",
        )
        .bind(&torrent.info_hash)
        .bind(&manifest_hash)
        .fetch_optional(&state.db)
        .await?;
        if done.is_some() {
            continue;
        }
        sqlx::query(
            "INSERT INTO torrent_ingests (info_hash, manifest_hash, state, updated_at)
             VALUES (?, ?, 'pending', unixepoch())
             ON CONFLICT(info_hash, manifest_hash) DO UPDATE SET updated_at = unixepoch()",
        )
        .bind(&torrent.info_hash)
        .bind(&manifest_hash)
        .execute(&state.db)
        .await?;

        let root = service.output_dir().to_string_lossy().into_owned();
        match crate::scan::scan_library(
            &state.db,
            &state.events,
            state.dandan.as_ref(),
            state.scrape.as_ref(),
            crate::api::SCRAPE_SYSTEM_PROMPT,
            library_id,
            &root,
        )
        .await
        {
            Ok(_) => {
                sqlx::query(
                    "UPDATE torrent_ingests SET state = 'done', last_error = NULL, updated_at = unixepoch()
                     WHERE info_hash = ? AND manifest_hash = ?",
                )
                .bind(&torrent.info_hash)
                .bind(&manifest_hash)
                .execute(&state.db)
                .await?;
            }
            Err(error) => {
                sqlx::query(
                    "UPDATE torrent_ingests SET last_error = ?, updated_at = unixepoch()
                     WHERE info_hash = ? AND manifest_hash = ?",
                )
                .bind(error.to_string())
                .bind(&torrent.info_hash)
                .bind(&manifest_hash)
                .execute(&state.db)
                .await?;
            }
        }
    }
    Ok(())
}

async fn ensure_download_library(state: &AppState, path: &std::path::Path) -> anyhow::Result<i64> {
    let path = path.canonicalize()?.to_string_lossy().into_owned();
    sqlx::query(
        "INSERT INTO libraries (name, path, kind, options)
         SELECT '下载', ?, 'anime', '{\"managed_by\":\"downloads\"}'
         WHERE NOT EXISTS (SELECT 1 FROM libraries WHERE path = ?)",
    )
    .bind(&path)
    .bind(&path)
    .execute(&state.db)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM libraries WHERE path = ? ORDER BY id LIMIT 1")
            .bind(path)
            .fetch_one(&state.db)
            .await?,
    )
}

async fn reconcile_projection(state: &AppState) -> anyhow::Result<()> {
    let Some(service) = &state.downloads else {
        return Ok(());
    };
    let snapshots = service.list();
    let mut tx = state.db.begin().await?;
    let old: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT info_hash, COALESCE(added_at, unixepoch()), save_path FROM torrents",
    )
    .fetch_all(&mut *tx)
    .await?;
    let old: HashMap<_, _> = old
        .into_iter()
        .map(|(hash, added, path)| (hash, (added, path)))
        .collect();
    sqlx::query("DELETE FROM torrents")
        .execute(&mut *tx)
        .await?;
    for item in snapshots {
        let (added_at, save_path) = old.get(&item.info_hash).cloned().unwrap_or_else(|| {
            (
                now_unix(),
                Some(service.output_dir().to_string_lossy().into_owned()),
            )
        });
        insert_projection(&mut tx, &item, added_at, save_path).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn insert_projection(
    tx: &mut Transaction<'_, Sqlite>,
    item: &DownloadSnapshot,
    added_at: i64,
    save_path: Option<String>,
) -> anyhow::Result<()> {
    let state = serde_json::to_value(item.state)?
        .as_str()
        .unwrap_or("error")
        .to_string();
    sqlx::query(
        "INSERT INTO torrents
         (info_hash, name, state, save_path, added_at, session_id,
          progress_bytes, total_bytes, uploaded_bytes, error)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&item.info_hash)
    .bind(&item.name)
    .bind(state)
    .bind(save_path)
    .bind(added_at)
    .bind(item.session_id as i64)
    .bind(item.progress_bytes as i64)
    .bind(item.total_bytes as i64)
    .bind(item.uploaded_bytes as i64)
    .bind(&item.error)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn download_error(error: nipa_download::DownloadError) -> (StatusCode, String) {
    use nipa_download::DownloadError::*;
    let status = match error {
        InvalidSource(_) | UnsafeUrl(_) | FilterRegex(_) | Feed(_) => StatusCode::BAD_REQUEST,
        ResponseTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        Torrent(_) | Http(_) | Io(_) => StatusCode::BAD_GATEWAY,
    };
    (status, error.to_string())
}

fn internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
