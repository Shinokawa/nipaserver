//! 管家工具第一批（docs/06-管家设计.md §2.1）。
//!
//! 级别纪律：本文件当前全部为只读或可逆写；破坏性操作（文件整理/删除）
//! 只能以"计划"形态出现，v1.x 实现。

use std::sync::Arc;

use nipa_agent::{BoxFuture, Tool, ToolError, ToolOutput};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use nipa_core::EventMsg;

use crate::scrape::{ScrapeRequest, ScrapeService};

/// (id, state, result, confidence, transcript, tokens_in, tokens_out)
type ScrapeTaskRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

fn db_err(e: sqlx::Error) -> ToolError {
    ToolError::RespondToModel(format!("数据库查询失败: {e}"))
}

/// query_library：结构化查询条目（只读）。
pub struct QueryLibrary {
    pub db: SqlitePool,
}

impl Tool for QueryLibrary {
    fn name(&self) -> &str {
        "query_library"
    }
    fn description(&self) -> &str {
        "查询媒体库条目。支持按标题模糊搜索、按类型(series/movie/episode)、按状态、\
         按首播年份/月份过滤，按 air_date/added_at/title 排序。返回条目清单（含 id）。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "标题模糊匹配（可选）"},
                "kind": {"type": "string", "enum": ["series", "movie", "episode"]},
                "air_year": {"type": "integer", "description": "首播年份过滤"},
                "air_month": {"type": "integer", "description": "首播月份（1-12，与 air_year 联用）"},
                "sort": {"type": "string", "enum": ["air_date", "added_at", "title"], "default": "added_at"},
                "limit": {"type": "integer", "default": 20, "maximum": 50}
            }
        })
    }
    fn call(&self, args: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let mut sql = String::from(
                "SELECT id, kind, title, original_title, year, season_no, episode_no, air_date \
                 FROM items WHERE deleted_at IS NULL",
            );
            let mut binds: Vec<String> = Vec::new();
            if let Some(t) = args["title"].as_str() {
                sql.push_str(" AND (title LIKE ? OR original_title LIKE ?)");
                let pat = format!("%{t}%");
                binds.push(pat.clone());
                binds.push(pat);
            }
            if let Some(k) = args["kind"].as_str() {
                sql.push_str(" AND kind = ?");
                binds.push(k.to_string());
            }
            if let Some(y) = args["air_year"].as_i64() {
                match args["air_month"].as_i64() {
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
            let sort = match args["sort"].as_str() {
                Some("air_date") => "air_date DESC",
                Some("title") => "title ASC",
                _ => "added_at DESC",
            };
            sql.push_str(&format!(" ORDER BY {sort} LIMIT ?"));
            let limit = args["limit"].as_i64().unwrap_or(20).clamp(1, 50);

            let mut q = sqlx::query_as::<
                _,
                (
                    i64,
                    String,
                    Option<String>,
                    Option<String>,
                    Option<i64>,
                    Option<i64>,
                    Option<i64>,
                    Option<String>,
                ),
            >(&sql);
            for b in &binds {
                q = q.bind(b);
            }
            let rows = q.bind(limit).fetch_all(&self.db).await.map_err(db_err)?;

            let items: Vec<Value> = rows
                .into_iter()
                .map(|(id, kind, title, orig, year, s, e, air)| {
                    json!({
                        "id": id, "kind": kind, "title": title, "original_title": orig,
                        "year": year, "season": s, "episode": e, "air_date": air
                    })
                })
                .collect();
            Ok(ToolOutput::json(
                &json!({ "count": items.len(), "items": items }),
            ))
        })
    }
}

/// get_scrape_task：读任务结论与 transcript 摘要（只读）。
pub struct GetScrapeTask {
    pub db: SqlitePool,
}

impl Tool for GetScrapeTask {
    fn name(&self) -> &str {
        "get_scrape_task"
    }
    fn description(&self) -> &str {
        "查询识别任务：按任务 id 或文件名（模糊）。返回状态、结论、置信度、\
         识别过程摘要（调过哪些工具）——用于回答\"这个文件为什么被认成 X\"。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "integer"},
                "file": {"type": "string", "description": "文件名模糊匹配"}
            }
        })
    }
    fn call(&self, args: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let row: Option<ScrapeTaskRow> = if let Some(id) = args["task_id"].as_i64() {
                sqlx::query_as(
                        "SELECT t.id, t.state, t.result, t.confidence, t.transcript, t.tokens_in, t.tokens_out
                         FROM scrape_tasks t WHERE t.id = ?",
                    )
                    .bind(id)
                    .fetch_optional(&self.db)
                    .await
                    .map_err(db_err)?
            } else if let Some(f) = args["file"].as_str() {
                sqlx::query_as(
                        "SELECT t.id, t.state, t.result, t.confidence, t.transcript, t.tokens_in, t.tokens_out
                         FROM scrape_tasks t
                         LEFT JOIN media_files m ON m.id = t.file_id
                         WHERE m.rel_path LIKE ? OR t.evidence LIKE ?
                         ORDER BY t.id DESC LIMIT 1",
                    )
                    .bind(format!("%{f}%"))
                    .bind(format!("%{f}%"))
                    .fetch_optional(&self.db)
                    .await
                    .map_err(db_err)?
            } else {
                return Err(ToolError::RespondToModel(
                    "需要 task_id 或 file 参数之一".into(),
                ));
            };

            let Some((id, state, result, confidence, transcript, tin, tout)) = row else {
                return Ok(ToolOutput::text("未找到匹配的识别任务"));
            };
            // transcript 是信封 JSONL 数组：摘要出工具调用轨迹（省 token）
            let trace: Vec<String> = transcript
                .as_deref()
                .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                        .filter_map(|v| match v["type"].as_str() {
                            Some("tool_call_end") => Some(format!(
                                "{}({}) {}",
                                v["tool"].as_str().unwrap_or("?"),
                                v["duration_ms"],
                                if v["success"].as_bool().unwrap_or(false) {
                                    "ok"
                                } else {
                                    "err"
                                }
                            )),
                            Some("task_failed") => {
                                Some(format!("FAILED: {}", v["message"].as_str().unwrap_or("")))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(ToolOutput::json(&json!({
                "task_id": id, "state": state, "confidence": confidence,
                "result": result.and_then(|r| serde_json::from_str::<Value>(&r).ok()),
                "tool_trace": trace,
                "tokens": {"in": tin, "out": tout}
            })))
        })
    }
}

/// library_stats：库统计（只读）。
pub struct LibraryStats {
    pub db: SqlitePool,
}

impl Tool for LibraryStats {
    fn name(&self) -> &str {
        "library_stats"
    }
    fn description(&self) -> &str {
        "库整体统计：条目数（按类型）、识别任务状态分布、今日识别量。"
    }
    fn parameters(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    fn call(&self, _args: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let by_kind: Vec<(String, i64)> = sqlx::query_as(
                "SELECT kind, COUNT(*) FROM items WHERE deleted_at IS NULL GROUP BY kind",
            )
            .fetch_all(&self.db)
            .await
            .map_err(db_err)?;
            let by_state: Vec<(Option<String>, i64)> =
                sqlx::query_as("SELECT state, COUNT(*) FROM scrape_tasks GROUP BY state")
                    .fetch_all(&self.db)
                    .await
                    .map_err(db_err)?;
            let today: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM scrape_tasks WHERE state IN ('done','needs_review')
                 AND created_at >= unixepoch('now','start of day')",
            )
            .fetch_one(&self.db)
            .await
            .map_err(db_err)?;
            Ok(ToolOutput::json(&json!({
                "items_by_kind": by_kind.into_iter().collect::<std::collections::BTreeMap<_,_>>(),
                "tasks_by_state": by_state.into_iter()
                    .map(|(s, c)| (s.unwrap_or_else(|| "unknown".into()), c))
                    .collect::<std::collections::BTreeMap<_,_>>(),
                "identified_today": today.0
            })))
        })
    }
}

/// requeue_scrape：带 hint 重新识别（可逆写）。
pub struct RequeueScrape {
    pub db: SqlitePool,
    pub scrape: ScrapeService,
    pub system_prompt: String,
}

impl Tool for RequeueScrape {
    fn name(&self) -> &str {
        "requeue_scrape"
    }
    fn description(&self) -> &str {
        "把一个识别任务重新入队，可附带提示（例如用户告知的正确作品名）。\
         识别是异步的，入队后告知用户稍后查看结果即可。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "integer"},
                "hint": {"type": "string", "description": "重识别线索，如\"用户确认这是《XX》第2季\""}
            },
            "required": ["task_id"]
        })
    }
    fn call(&self, args: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let Some(task_id) = args["task_id"].as_i64() else {
                return Err(ToolError::RespondToModel("task_id 必填".into()));
            };
            let row: Option<(Option<String>,)> =
                sqlx::query_as("SELECT evidence FROM scrape_tasks WHERE id = ?")
                    .bind(task_id)
                    .fetch_optional(&self.db)
                    .await
                    .map_err(db_err)?;
            let Some((Some(evidence),)) = row else {
                return Err(ToolError::RespondToModel(format!(
                    "任务 {task_id} 不存在或没有保存证据，无法重识别"
                )));
            };
            let mut user_message = evidence;
            if let Some(hint) = args["hint"].as_str() {
                user_message.push_str(&format!("\n\n[管家提示] {hint}"));
            }
            sqlx::query("UPDATE scrape_tasks SET state = 'queued' WHERE id = ?")
                .bind(task_id)
                .execute(&self.db)
                .await
                .map_err(db_err)?;
            self.scrape
                .enqueue(ScrapeRequest {
                    task_id,
                    system_prompt: self.system_prompt.clone(),
                    user_message,
                })
                .await
                .map_err(|e| ToolError::RespondToModel(e.to_string()))?;
            Ok(ToolOutput::json(&json!({"requeued": task_id})))
        })
    }
}

/// confirm_pending：代用户确认待确认任务（可逆写；对话中的明确指示视为确认）。
pub struct ConfirmPending {
    pub db: SqlitePool,
    pub events: broadcast::Sender<EventMsg>,
}

impl Tool for ConfirmPending {
    fn name(&self) -> &str {
        "confirm_pending"
    }
    fn description(&self) -> &str {
        "确认一个待确认(needs_review)的识别结论，使其正式入库。\
         只有当用户在对话中明确表示认可时才调用。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"task_id": {"type": "integer"}},
            "required": ["task_id"]
        })
    }
    fn call(&self, args: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let Some(task_id) = args["task_id"].as_i64() else {
                return Err(ToolError::RespondToModel("task_id 必填".into()));
            };
            let updated = sqlx::query(
                "UPDATE scrape_tasks SET state = 'done' WHERE id = ? AND state = 'needs_review'",
            )
            .bind(task_id)
            .execute(&self.db)
            .await
            .map_err(db_err)?;
            if updated.rows_affected() == 0 {
                return Err(ToolError::RespondToModel(format!(
                    "任务 {task_id} 不在待确认状态"
                )));
            }
            // TODO(M2b): 经 §4.5 合并流程写 items/file_item（当前仅状态翻转）。
            let _ = self.events.send(EventMsg::ScrapeUpdate {
                task_id,
                state: "done".into(),
            });
            Ok(ToolOutput::json(&json!({"confirmed": task_id})))
        })
    }
}

/// 组装管家工具集：管家专属 + 复用 providers 的全部只读元数据工具。
pub fn build_steward_tools(
    db: SqlitePool,
    events: broadcast::Sender<EventMsg>,
    scrape: Option<ScrapeService>,
    provider_tools: Vec<Arc<dyn Tool>>,
    scrape_system_prompt: &str,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(QueryLibrary { db: db.clone() }),
        Arc::new(GetScrapeTask { db: db.clone() }),
        Arc::new(LibraryStats { db: db.clone() }),
        Arc::new(ConfirmPending {
            db: db.clone(),
            events,
        }),
    ];
    if let Some(scrape) = scrape {
        tools.push(Arc::new(RequeueScrape {
            db,
            scrape,
            system_prompt: scrape_system_prompt.to_string(),
        }));
    }
    tools.extend(provider_tools);
    tools
}
