//! 条目合并入库（开发文档 §4.5 最小实现）。
//!
//! 把一个通过闸门的刮削结论（submit_result JSON）合并进 items 树：
//! - tv_episode: series → season → episode 三级，先按 (provider, external_id)
//!   查锚点，命中挂载、未命中建树；
//! - movie: 单节点；
//! - 全程单写事务 + item_ids UNIQUE 约束兜底并发去重；
//! - file_item 关联物理文件（试刮任务无 file_id 时跳过关联）。
//!
//! TODO(M2b): 弹弹play animeId 作为 season 级别名挂载；air_date 从
//! provider 详情回填 episode 行；手动"拆分/合并条目"出口。

use serde_json::Value;
use sqlx::{Sqlite, SqlitePool, Transaction};

/// 刮削结论入库。返回叶子 item id（episode 或 movie）。
pub async fn ingest_result(
    db: &SqlitePool,
    library_id: i64,
    file_id: Option<i64>,
    result: &Value,
) -> anyhow::Result<i64> {
    let media_type = result["media_type"].as_str().unwrap_or("unknown");
    let title = result["title"].as_str().unwrap_or("未知标题");
    let original_title = result["original_title"].as_str();
    let year = result["year"].as_i64();
    let air_date = result["air_date"].as_str();
    let ids = &result["ids"];

    let mut tx = db.begin().await?;
    let leaf_id = match media_type {
        "movie" => {
            find_or_create_item(
                &mut tx, library_id, "movie", None, title, original_title, year, None, None,
                air_date, ids,
            )
            .await?
        }
        // tv_episode 与 ova 同构：series → season → episode
        "tv_episode" | "ova" => {
            let season_no = result["season"].as_i64().unwrap_or(1);
            let episode_no = result["episode"].as_i64();
            let series = find_or_create_item(
                &mut tx, library_id, "series", None, title, original_title, year, None, None,
                None, ids,
            )
            .await?;
            let season = find_or_create_season(&mut tx, library_id, series, season_no).await?;
            find_or_create_episode(
                &mut tx, library_id, season, title, season_no, episode_no, air_date,
            )
            .await?
        }
        _ => {
            // unknown：挂一个 movie 形态的占位节点（可在 UI 改正）
            find_or_create_item(
                &mut tx, library_id, "movie", None, title, original_title, year, None, None,
                air_date, ids,
            )
            .await?
        }
    };

    if let Some(fid) = file_id {
        sqlx::query(
            "INSERT INTO file_item (file_id, item_id) VALUES (?, ?)
             ON CONFLICT(file_id, item_id) DO NOTHING",
        )
        .bind(fid)
        .bind(leaf_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE media_files SET status = 'ai_matched' WHERE id = ?")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(leaf_id)
}

/// 先按外部 id 锚点查、再按 (library, kind, title) 查、最后新建（§4.5 先查后并）。
#[allow(clippy::too_many_arguments)]
async fn find_or_create_item(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    kind: &str,
    parent_id: Option<i64>,
    title: &str,
    original_title: Option<&str>,
    year: Option<i64>,
    season_no: Option<i64>,
    episode_no: Option<i64>,
    air_date: Option<&str>,
    ids: &Value,
) -> anyhow::Result<i64> {
    // 1) 锚点：canonical 优先级 tmdb > bangumi > dandanplay（§4.5）
    for provider in ["tmdb", "bangumi", "dandanplay_anime"] {
        if let Some(eid) = ids[provider].as_i64() {
            let pname = provider.trim_end_matches("_anime");
            let hit: Option<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM item_ids WHERE provider = ? AND external_id = ?",
            )
            .bind(pname)
            .bind(eid.to_string())
            .fetch_optional(&mut **tx)
            .await?;
            if let Some((item_id,)) = hit {
                return Ok(item_id);
            }
        }
    }
    // 2) 同库同名同类兜底（无 id 的结论防分裂）
    let hit: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM items WHERE library_id = ? AND kind = ? AND title = ?
         AND deleted_at IS NULL LIMIT 1",
    )
    .bind(library_id)
    .bind(kind)
    .bind(title)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((item_id,)) = hit {
        attach_ids(tx, item_id, ids).await?;
        return Ok(item_id);
    }
    // 3) 新建
    let item_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO items (library_id, kind, parent_id, title, original_title, year,
                            season_no, episode_no, air_date, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch()) RETURNING id",
    )
    .bind(library_id)
    .bind(kind)
    .bind(parent_id)
    .bind(title)
    .bind(original_title)
    .bind(year)
    .bind(season_no)
    .bind(episode_no)
    .bind(air_date)
    .fetch_one(&mut **tx)
    .await?;
    attach_ids(tx, item_id, ids).await?;
    Ok(item_id)
}

/// 外部 id 挂载（UNIQUE(provider, external_id) 与 UNIQUE(item_id, provider) 兜底）。
async fn attach_ids(
    tx: &mut Transaction<'_, Sqlite>,
    item_id: i64,
    ids: &Value,
) -> anyhow::Result<()> {
    for provider in ["tmdb", "bangumi", "dandanplay_anime", "imdb"] {
        let eid = match &ids[provider] {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        if let Some(eid) = eid {
            let pname = provider.trim_end_matches("_anime");
            // 冲突（该 id 已挂别的条目 / 该条目已有该 provider）→ 静默跳过，
            // 保守不覆盖——错误合并比漏合并更难恢复。
            let _ = sqlx::query(
                "INSERT INTO item_ids (item_id, provider, external_id) VALUES (?, ?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(item_id)
            .bind(pname)
            .bind(eid)
            .execute(&mut **tx)
            .await;
        }
    }
    Ok(())
}

async fn find_or_create_season(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    series_id: i64,
    season_no: i64,
) -> anyhow::Result<i64> {
    let hit: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM items WHERE parent_id = ? AND kind = 'season' AND season_no = ?
         AND deleted_at IS NULL",
    )
    .bind(series_id)
    .bind(season_no)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((id,)) = hit {
        return Ok(id);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO items (library_id, kind, parent_id, title, season_no, added_at)
         VALUES (?, 'season', ?, ?, ?, unixepoch()) RETURNING id",
    )
    .bind(library_id)
    .bind(series_id)
    .bind(format!("第 {season_no} 季"))
    .bind(season_no)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

async fn find_or_create_episode(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    season_id: i64,
    series_title: &str,
    season_no: i64,
    episode_no: Option<i64>,
    air_date: Option<&str>,
) -> anyhow::Result<i64> {
    if let Some(ep) = episode_no {
        let hit: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM items WHERE parent_id = ? AND kind = 'episode' AND episode_no = ?
             AND deleted_at IS NULL",
        )
        .bind(season_id)
        .bind(ep)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((id,)) = hit {
            return Ok(id);
        }
    }
    let title = match episode_no {
        Some(ep) => format!("{series_title} S{season_no:02}E{ep:02}"),
        None => format!("{series_title}（未知集）"),
    };
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO items (library_id, kind, parent_id, title, season_no, episode_no,
                            air_date, added_at)
         VALUES (?, 'episode', ?, ?, ?, ?, ?, unixepoch()) RETURNING id",
    )
    .bind(library_id)
    .bind(season_id)
    .bind(title)
    .bind(season_no)
    .bind(episode_no)
    .bind(air_date)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}
