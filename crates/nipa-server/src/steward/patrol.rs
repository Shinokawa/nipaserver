//! 管家巡检（docs/06-管家设计.md §2 "主动唤醒"）。
//!
//! 定时以固定 prompt 唤醒管家做库健康检查，产出"管家报告"：
//! - 存入 steward_reports 表（WebUI 管家页 feed 的数据源）；
//! - 经 SSE 广播（顶栏铃铛角标）。
//!
//! 与对话共用同一 StewardService（同一工具集、同一模型），仅 [5] 会话层
//! 换成巡检指令——报告因此也能引用真实库数据（工具查询驱动）。

use std::sync::Arc;
use std::time::Duration;

use nipa_core::EventMsg;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::StewardService;

/// 巡检间隔（v1 固定每 24h；首次启动后 5 分钟先跑一轮，让新装用户立刻看到报告）。
const PATROL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST_PATROL_DELAY: Duration = Duration::from_secs(5 * 60);

const PATROL_PROMPT: &str = "\
[巡检任务] 请检查库的健康状况并产出一份简短的管家报告（面向主人阅读）：
1. 用 library_stats 看整体状态；
2. 有待确认(needs_review)任务时列出最需要主人处理的（最多 3 条，带 task_id）；
3. 有 failed 任务时提示可以重识别；
4. 一切正常时就简短汇报今日成果。
直接输出报告正文（不超过 200 字），不要问问题——这是单向汇报。";

pub fn spawn_patrol(
    steward: Arc<StewardService>,
    db: SqlitePool,
    events: broadcast::Sender<EventMsg>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(FIRST_PATROL_DELAY) => {}
            _ = cancel.cancelled() => return,
        }
        loop {
            if let Err(e) = run_patrol(&steward, &db, &events).await {
                warn!(error = %e, "patrol failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(PATROL_INTERVAL) => {}
                _ = cancel.cancelled() => return,
            }
        }
    });
}

async fn run_patrol(
    steward: &StewardService,
    db: &SqlitePool,
    events: &broadcast::Sender<EventMsg>,
) -> anyhow::Result<()> {
    // 巡检用独立会话（不污染用户对话；session 命名固定前缀便于 UI 过滤）
    let session_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO chat_sessions (title, created_at, updated_at)
         VALUES ('[巡检]', unixepoch(), unixepoch()) RETURNING id",
    )
    .fetch_one(db)
    .await?;
    let result = steward.chat(Some(session_id), PATROL_PROMPT.to_string()).await?;

    sqlx::query(
        "INSERT INTO steward_reports (session_id, report, created_at)
         VALUES (?, ?, unixepoch())",
    )
    .bind(session_id)
    .bind(&result.reply)
    .execute(db)
    .await?;
    let _ = events.send(EventMsg::StewardReport {
        report: result.reply.clone(),
    });
    info!(session_id, "patrol report generated");
    Ok(())
}
