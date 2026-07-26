//! 扫描编排（M1 管线）：walk → diff → 指纹/hash → L1(弹弹play) → L2(AI) 入队。
//!
//! 管线（开发文档 §4.1）：
//! - L0：`(size, dandan_hash)` 命中旧识别 → 迁移关联不重刮（rel_path 变更即移动检测）；
//! - L1：弹弹play match（无凭证时整级跳过——降级路径）；
//! - L2：组 evidence 入 AI 队列。
//!
//! TODO(M1.x): watcher 增量触发；IO 节流用 nipa-scanner::IoLimiter 全局共享；
//! TODO(M3): ffprobe 摘要进 evidence（当前无 ffmpeg 依赖，evidence 降级形态）。

use nipa_core::EventMsg;
use nipa_match::{classify, DandanClient, MatchOutcome, MatchRequest};
use nipa_scanner::{build_evidence, dandan_hash, fingerprint, walk_library, EvidenceParams};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::scrape::{ScrapeRequest, ScrapeService};

pub struct ScanOutcome {
    pub discovered: usize,
    pub new_files: usize,
    pub moved: usize,
    pub matched_l1: usize,
    pub queued_l2: usize,
}

/// 全量扫描一个库（阻塞式完成 walk 与 hash，适合 spawn 后台执行）。
pub async fn scan_library(
    db: &SqlitePool,
    events: &broadcast::Sender<EventMsg>,
    dandan: Option<&Arc<DandanClient>>,
    scrape: Option<&ScrapeService>,
    scrape_system_prompt: &str,
    library_id: i64,
    library_path: &str,
) -> anyhow::Result<ScanOutcome> {
    let root = std::path::PathBuf::from(library_path);
    let _ = events.send(EventMsg::ScanProgress {
        library_id,
        message: "开始扫描".into(),
    });

    // walk 是同步 IO，丢进 blocking 池
    let discovered = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || walk_library(&root)).await?
    };
    info!(library_id, count = discovered.len(), "walk complete");

    let mut outcome = ScanOutcome {
        discovered: discovered.len(),
        new_files: 0,
        moved: 0,
        matched_l1: 0,
        queued_l2: 0,
    };

    for file in &discovered {
        let fp = fingerprint(file.size, file.mtime);
        // 已知且未变更 → 跳过
        let known: Option<(i64, Option<String>, String)> = sqlx::query_as(
            "SELECT id, fingerprint, status FROM media_files
             WHERE library_id = ? AND rel_path = ?",
        )
        .bind(library_id)
        .bind(&file.rel_path)
        .fetch_optional(db)
        .await?;
        if let Some((_, Some(known_fp), _)) = &known
            && *known_fp == fp
        {
            continue;
        }

        // 计算弹弹play hash（前 16MB MD5；L0 缓存主键成分）
        let abs = root.join(&file.rel_path);
        let hash = {
            let abs = abs.clone();
            tokio::task::spawn_blocking(move || dandan_hash(&abs)).await?
        };
        let hash = match hash {
            Ok(h) => h,
            Err(e) => {
                warn!(file = %file.rel_path, error = %e, "hash failed; skip");
                continue;
            }
        };

        // L0 移动检测：同 (size, dandan_hash) 的旧行且 rel_path 不同 → 迁移
        let moved: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM media_files WHERE size = ? AND dandan_hash = ?
             AND rel_path != ? AND library_id = ?",
        )
        .bind(file.size as i64)
        .bind(&hash)
        .bind(&file.rel_path)
        .bind(library_id)
        .fetch_optional(db)
        .await?;
        if let Some((old_id,)) = moved {
            sqlx::query(
                "UPDATE media_files SET rel_path = ?, mtime = ?, fingerprint = ? WHERE id = ?",
            )
            .bind(&file.rel_path)
            .bind(file.mtime)
            .bind(&fp)
            .bind(old_id)
            .execute(db)
            .await?;
            outcome.moved += 1;
            continue;
        }

        // upsert 文件行
        let file_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO media_files (library_id, rel_path, size, mtime, fingerprint, dandan_hash, status)
             VALUES (?, ?, ?, ?, ?, ?, 'pending')
             ON CONFLICT(library_id, rel_path) DO UPDATE SET
               size = excluded.size, mtime = excluded.mtime,
               fingerprint = excluded.fingerprint, dandan_hash = excluded.dandan_hash,
               status = 'pending'
             RETURNING id",
        )
        .bind(library_id)
        .bind(&file.rel_path)
        .bind(file.size as i64)
        .bind(file.mtime)
        .bind(&fp)
        .bind(&hash)
        .fetch_one(db)
        .await?;
        outcome.new_files += 1;

        // ===== L1：弹弹play hash match（凭证可用时；§4.1）=====
        let mut l1_candidates_note: Option<String> = None;
        if let Some(client) = dandan {
            let name_no_ext = file_name(&file.rel_path)
                .rsplit_once('.')
                .map(|(n, _)| n)
                .unwrap_or_else(|| file_name(&file.rel_path));
            let req = MatchRequest::new(name_no_ext, &hash, file.size as i64);
            match client.match_file(req).await {
                Ok(resp) => match classify(resp) {
                    MatchOutcome::Exact(m) => {
                        // 精确命中 → 直接入库（弹弹play 结论视同 high 置信度）
                        let result = dandan_to_result(&m);
                        let task_id = sqlx::query_scalar::<_, i64>(
                            "INSERT INTO scrape_tasks (file_id, tier, state, result, confidence, model, created_at)
                             VALUES (?, 'l1', 'done', ?, 'high', 'dandanplay-hash', unixepoch()) RETURNING id",
                        )
                        .bind(file_id)
                        .bind(result.to_string())
                        .fetch_one(db)
                        .await?;
                        if let Err(e) =
                            crate::ingest::ingest_result(db, library_id, Some(file_id), &result).await
                        {
                            warn!(task_id, error = %e, "L1 ingest failed");
                        } else {
                            sqlx::query("UPDATE media_files SET status = 'matched' WHERE id = ?")
                                .bind(file_id)
                                .execute(db)
                                .await?;
                            outcome.matched_l1 += 1;
                            let _ = events.send(EventMsg::ScrapeUpdate {
                                task_id,
                                state: "done".into(),
                            });
                            continue; // L1 命中，无需 L2
                        }
                    }
                    MatchOutcome::Candidates(list) => {
                        // 候选给 L2 当线索（§4.1）
                        let names: Vec<String> = list
                            .iter()
                            .take(5)
                            .map(|m| {
                                format!(
                                    "{}(animeId={})",
                                    m.anime_title.as_deref().unwrap_or("?"),
                                    m.anime_id
                                )
                            })
                            .collect();
                        l1_candidates_note =
                            Some(format!("弹弹play hash 未精确命中，候选: {}", names.join("; ")));
                    }
                    MatchOutcome::NoMatch => {}
                },
                Err(e) => warn!(file = %file.rel_path, error = %e, "L1 match error; falling to L2"),
            }
        }

        // ===== L2：组 evidence 入队 =====
        let Some(scrape) = scrape else { continue };
        let siblings: Vec<String> = discovered
            .iter()
            .filter(|f| {
                f.rel_path != file.rel_path
                    && parent_dir(&f.rel_path) == parent_dir(&file.rel_path)
            })
            .map(|f| file_name(&f.rel_path).to_string())
            .collect();
        let mut evidence = build_evidence(&EvidenceParams {
            rel_path: &file.rel_path,
            ffprobe: None, // TODO(M3)
            subtitle_sample: None,
            siblings: &siblings,
        });
        if let Some(note) = &l1_candidates_note {
            evidence.push_str(&format!("\n{note}"));
        }
        let task_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO scrape_tasks (file_id, tier, state, evidence, created_at)
             VALUES (?, 'l2', 'queued', ?, unixepoch()) RETURNING id",
        )
        .bind(file_id)
        .bind(&evidence)
        .fetch_one(db)
        .await?;
        if scrape
            .enqueue(ScrapeRequest {
                task_id,
                system_prompt: scrape_system_prompt.to_string(),
                user_message: evidence,
            })
            .await
            .is_ok()
        {
            outcome.queued_l2 += 1;
        }
    }

    // 消失文件：库内在册但本次未发现 → 软删除宽限（§9 生命周期）
    // TODO(M1.x): missing → items.deleted_at 标记；当前先记日志。
    let _ = events.send(EventMsg::ScanProgress {
        library_id,
        message: format!(
            "扫描完成：发现 {}，新增 {}，移动 {}，hash 命中 {}，AI 入队 {}",
            outcome.discovered,
            outcome.new_files,
            outcome.moved,
            outcome.matched_l1,
            outcome.queued_l2
        ),
    });
    Ok(outcome)
}

/// 弹弹play 精确命中 → submit_result 同构 JSON（进同一 ingest 管线）。
/// episodeId 规则：animeId*10000 + 集数（官方约定），据此还原集号。
fn dandan_to_result(m: &nipa_match::MatchResultV2) -> serde_json::Value {
    let episode_no = m.episode_id % 10000;
    let is_movie = matches!(
        m.anime_type,
        nipa_match::AnimeType::Movie | nipa_match::AnimeType::TmdbMovie | nipa_match::AnimeType::JpMovie
    );
    serde_json::json!({
        "media_type": if is_movie { "movie" } else { "tv_episode" },
        "title": m.anime_title.clone().unwrap_or_else(|| format!("弹弹play {}", m.anime_id)),
        "season": 1,
        "episode": episode_no,
        "ids": { "dandanplay_anime": m.anime_id, "dandanplay_episode": m.episode_id },
        "confidence": "high",
        "reasoning": format!("弹弹play hash 精确命中：{}", m.episode_title.as_deref().unwrap_or("")),
        "image_url": m.image_url,
    })
}

fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(_, f)| f).unwrap_or(rel)
}
