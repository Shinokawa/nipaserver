//! 事件类型（EventMsg）——SSE 事件流的载荷（开发文档 §2.1 SQ/EQ 设计、§8.1 /events）。
//!
//! TODO(M2a): 对齐 agent 事件流（工具调用、结论、置信度、审批内嵌），
//! 当前仅提供骨架所需的最小变体。

use serde::{Deserialize, Serialize};

/// 推送给前端的事件（JSONL / SSE data 载荷）。
///
/// 以 `type` 字段区分变体，payload 内联，贴近 codex `EventMsg` 风格。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsg {
    /// 保活心跳（SSE 连接层，同时防中间设备断连）。
    Heartbeat {
        /// Unix 时间戳（秒）。
        ts: i64,
    },
    /// 扫描进度占位。TODO(M1): 细化字段（library_id、已扫/总数等）。
    ScanProgress { library_id: i64, message: String },
    /// 刮削任务状态变化（队列层：queued/running/done/needs_review/failed）。
    ScrapeUpdate { task_id: i64, state: String },
    /// 刮削 agent 事件透传：`agent` 为 nipa-agent 的 AgentEventEnvelope 原样
    /// JSON（契约见 docs/03-agent接口契约.md §4）。不在此处强类型化，
    /// 避免 nipa-core 依赖 nipa-agent；消费方按契约解析。
    Scrape {
        task_id: i64,
        agent: serde_json::Value,
    },
    /// 管家对话过程事件透传（工具调用进度，WebUI 对话页实时渲染）。
    Steward { agent: serde_json::Value },
}
