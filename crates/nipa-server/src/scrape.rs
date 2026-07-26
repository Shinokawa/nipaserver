//! 刮削服务：任务队列 + nipa-agent 桥接（契约：docs/03-agent接口契约.md §5）。
//!
//! M0.5 最小实现：
//! - 单 worker 顺序消费（并发与档位 RPM 联动是 M2a 的事，§2.2）；
//! - agent 事件 → 双写：SSE 总线（EventMsg::Scrape 透传）+ scrape_tasks.transcript（JSONL）；
//! - 结果按置信度闸门落库状态（high → done；medium/low → needs_review，§4.2）。
//!
//! TODO(M2a): 断点续扫（启动时捞 queued/running 任务重新入队）、并发与限流联动、
//!            质量档升级重试、few-shot 改正复用（scrape_corrections）。

use std::sync::Arc;

use nipa_agent::{Agent, AgentConfig, ModelProviderInfo, TaskInput, TaskOutcome, Tool};
use nipa_core::{EventMsg, ModelSection};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// 一次刮削请求：server 侧组装好证据后入队。
#[derive(Debug)]
pub struct ScrapeRequest {
    /// scrape_tasks.id（入队前由调用方 INSERT 生成）。
    pub task_id: i64,
    pub system_prompt: String,
    /// evidence bundle（§4.2：文件路径、ffprobe、字幕采样、兄弟文件）。
    pub user_message: String,
}

/// 刮削服务句柄：入队 + 停机。
#[derive(Clone)]
pub struct ScrapeService {
    tx: mpsc::Sender<ScrapeRequest>,
    cancel: CancellationToken,
}

impl ScrapeService {
    /// 启动 worker。`model` 未配置时返回 None（capabilities.ai_scrape=false，
    /// L2 降级——§4.3 传统兜底不在本模块）。
    pub fn start(
        model: &ModelSection,
        tools: Vec<Arc<dyn Tool>>,
        db: SqlitePool,
        events: broadcast::Sender<EventMsg>,
    ) -> Option<Self> {
        if !model.is_configured() {
            info!("[model] not configured; AI scraping disabled");
            return None;
        }
        let (tx, rx) = mpsc::channel::<ScrapeRequest>(256);
        let cancel = CancellationToken::new();
        tokio::spawn(worker(
            model.clone(),
            tools,
            db,
            events,
            rx,
            cancel.clone(),
        ));
        Some(Self { tx, cancel })
    }

    pub async fn enqueue(&self, req: ScrapeRequest) -> Result<(), &'static str> {
        self.tx.send(req).await.map_err(|_| "scrape worker stopped")
    }

    /// 优雅停机：取消当前任务并停止 worker。
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

fn provider_from_config(model: &ModelSection) -> ModelProviderInfo {
    let mut p = ModelProviderInfo::new("configured", model.base_url.clone());
    if !model.api_key.trim().is_empty() {
        p.api_key = Some(model.api_key.clone());
    } else if !model.api_key_env.trim().is_empty() {
        p.api_key_env = Some(model.api_key_env.clone());
    }
    p
}

/// submit_result 的参数 schema（§4.2）。
pub fn result_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "media_type": {"type": "string", "enum": ["tv_episode", "movie", "ova", "unknown"]},
            "title": {"type": "string", "description": "作品名，优先中文"},
            "original_title": {"type": "string"},
            "season": {"type": "integer"},
            "episode": {"type": "integer"},
            "ids": {
                "type": "object",
                "properties": {
                    "tmdb": {"type": "integer"},
                    "bangumi": {"type": "integer"},
                    "dandanplay_anime": {"type": "integer"}
                },
                "description": "填入所有已核实的 id，能查到的都要填"
            },
            "overview": {
                "type": "string",
                "description": "作品简介（从 provider 详情获取后一并提交，查不到可省略）"
            },
            "genres": {
                "type": "array", "items": {"type": "string"},
                "description": "类型标签，如 [\"科幻\", \"日常\"]（provider 详情可得时提交，查不到可省略）"
            },
            "studios": {
                "type": "array", "items": {"type": "string"},
                "description": "制作公司/动画工作室（provider 详情可得时提交，查不到可省略）"
            },
            "air_date": {
                "type": "string",
                "description": "首播日期 YYYY-MM-DD（本集/本片；provider 详情可得时提交，查不到可省略）"
            },
            "runtime_minutes": {
                "type": "integer",
                "description": "单集/影片时长（分钟；provider 详情可得时提交，查不到可省略）"
            },
            "people": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "kind": {"type": "string", "enum": ["actor", "director", "writer"]},
                        "role": {"type": "string", "description": "配音角色名/职位描述"}
                    },
                    "required": ["name", "kind"]
                },
                "description": "主要演职员（声优 kind=actor、role=角色名；从 provider 详情获取后一并提交，查不到可省略）"
            },
            "confidence": {"type": "string", "enum": ["high", "medium", "low"]},
            "reasoning": {"type": "string", "description": "一句话依据"}
        },
        "required": ["media_type", "title", "confidence", "reasoning"]
    })
}

async fn worker(
    model: ModelSection,
    tools: Vec<Arc<dyn Tool>>,
    db: SqlitePool,
    events: broadcast::Sender<EventMsg>,
    mut rx: mpsc::Receiver<ScrapeRequest>,
    cancel: CancellationToken,
) {
    loop {
        let req = tokio::select! {
            r = rx.recv() => match r {
                Some(r) => r,
                None => break,
            },
            _ = cancel.cancelled() => break,
        };
        if let Err(e) = run_one(&model, &tools, &db, &events, req, &cancel).await {
            error!(error = %e, "scrape task failed at infrastructure level");
        }
    }
    info!("scrape worker stopped");
}

async fn run_one(
    model: &ModelSection,
    tools: &[Arc<dyn Tool>],
    db: &SqlitePool,
    events: &broadcast::Sender<EventMsg>,
    req: ScrapeRequest,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let task_id = req.task_id;
    set_task_state(db, task_id, "running", &model.model).await?;
    let _ = events.send(EventMsg::ScrapeUpdate { task_id, state: "running".into() });

    // agent 事件 → SSE 透传 + transcript 收集（信封 JSONL，契约 §4 不变量 4）。
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let events_clone = events.clone();
    let collector = tokio::spawn(async move {
        let mut transcript: Vec<String> = Vec::new();
        while let Some(env) = agent_rx.recv().await {
            if let Ok(line) = serde_json::to_string(&env) {
                let _ = events_clone.send(EventMsg::Scrape {
                    task_id,
                    agent: serde_json::from_str(&line).unwrap_or_default(),
                });
                transcript.push(line);
            }
        }
        transcript
    });

    let mut cfg = AgentConfig::new(
        provider_from_config(model),
        model.model.clone(),
        result_schema(),
    );
    cfg.tools = tools.to_vec();
    if model.max_rounds > 0 {
        cfg.max_rounds = model.max_rounds;
    }
    if model.max_total_tokens > 0 {
        cfg.max_total_tokens = Some(model.max_total_tokens);
    }
    cfg.event_tx = Some(agent_tx);
    cfg.cancel = cancel.child_token();
    cfg.task_id = task_id.to_string();

    let outcome = match Agent::new(cfg) {
        Ok(agent) => {
            agent
                .run(TaskInput {
                    system_prompt: req.system_prompt,
                    user_message: req.user_message,
                })
                .await
        }
        Err(e) => {
            set_task_state(db, task_id, "failed", &model.model).await?;
            let _ = events.send(EventMsg::ScrapeUpdate { task_id, state: "failed".into() });
            warn!(error = %e, "agent init failed (config error)");
            return Ok(());
        }
    };

    let transcript = collector.await.unwrap_or_default();
    let transcript_json = serde_json::to_string(&transcript)?;

    // 置信度闸门（§4.2）：high 直接经 §4.5 合并入库，medium/low 落待确认。
    let (state, result_json, confidence, usage) = match &outcome {
        TaskOutcome::Completed { result, usage, .. } => {
            let confidence = result["confidence"].as_str().unwrap_or("low").to_string();
            let state = if confidence == "high" { "done" } else { "needs_review" };
            if state == "done" {
                // 任务关联的文件与库（试刮任务无 file_id 时 library 取 0 占位）
                let link: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
                    "SELECT t.file_id, m.library_id FROM scrape_tasks t
                     LEFT JOIN media_files m ON m.id = t.file_id WHERE t.id = ?",
                )
                .bind(task_id)
                .fetch_optional(db)
                .await?;
                let (file_id, library_id) = link.unwrap_or((None, None));
                if let Err(e) =
                    crate::ingest::ingest_result(db, library_id.unwrap_or(0), file_id, result)
                        .await
                {
                    warn!(task_id, error = %e, "ingest failed; leaving task done without item link");
                }
            }
            (state, Some(result.to_string()), Some(confidence), *usage)
        }
        TaskOutcome::Failed { usage, .. } => ("failed", None, None, *usage),
        TaskOutcome::Aborted { .. } => ("failed", None, None, Default::default()),
    };

    sqlx::query(
        "UPDATE scrape_tasks SET state = ?, result = ?, confidence = ?, transcript = ?,
         tokens_in = ?, tokens_out = ? WHERE id = ?",
    )
    .bind(state)
    .bind(&result_json)
    .bind(&confidence)
    .bind(&transcript_json)
    .bind(usage.input as i64)
    .bind(usage.output as i64)
    .bind(task_id)
    .execute(db)
    .await?;
    let _ = events.send(EventMsg::ScrapeUpdate { task_id, state: state.into() });
    info!(task_id, state, "scrape task finished");
    Ok(())
}

async fn set_task_state(
    db: &SqlitePool,
    task_id: i64,
    state: &str,
    model: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE scrape_tasks SET state = ?, model = ? WHERE id = ?")
        .bind(state)
        .bind(model)
        .bind(task_id)
        .execute(db)
        .await?;
    Ok(())
}
