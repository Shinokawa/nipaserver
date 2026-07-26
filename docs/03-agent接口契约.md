# nipa-agent ↔ nipaserver 接口契约 v1（nipa-agent 0.1.1）

> 2026-07-26。双方并行开发的对齐基准；改动此文件需同步改两边。
> nipa-agent 是独立版本化项目（`nipa-agent/` submodule，不在 workspace members 里），nipaserver 通过精确版本约束和本地 path 接入：`nipa-agent = { version = "=0.1.1", path = "nipa-agent" }`。发布版本以 `vX.Y.Z` 注释标签为准。

## 1. 职责边界

**nipa-agent 负责**（纯运行时，无业务知识）：
- OpenAI 兼容 Chat Completions 客户端（非流式）+ 重试/退避/错误分类；
- provider 配置（`ModelProviderInfo` + compat 标志）；
- tool-calling loop：轮数/token 预算护栏、伪 tool call 检测、坏参数回喂自纠错、`submit_result` 终止语义；
- 事件流产出（`AgentEvent`，SSE 与 transcript 落库两用）。

**nipaserver 负责**（业务侧）：
- 实现具体工具（`search_tmdb` / `search_bangumi` / … 在 `nipa-providers`，实现 nipa-agent 的 `Tool` trait）；
- 组装 evidence bundle（system prompt + user 消息内容）；
- 定义 `submit_result` 的 JSON Schema 与结果落库；
- 任务队列、并发控制、断点续扫、置信度闸门、审批流；
- 把 `AgentEvent` 转发到 axum SSE / 写入 `scrape_tasks.transcript`。

**依赖约束**：nipa-agent 只依赖 reqwest/serde/tokio(rt,time,sync,macros)/tokio-util/thiserror/tracing/futures/fastrand——可交叉编译、可经 flutter_rust_bridge 回流移动端。不依赖 axum/sqlx。

## 2. nipa-agent 公开 API

```rust
// ===== 入口 =====
pub struct Agent { /* ... */ }

impl Agent {
    pub fn new(cfg: AgentConfig) -> Result<Self, AgentError>;

    /// 运行一次刮削任务直至终态。事件经 cfg.event_tx 实时推出；
    /// 返回值与终态事件语义一致（Completed 携带 submit_result 的参数）。
    pub async fn run(&self, task: TaskInput) -> TaskOutcome;
}

pub struct AgentConfig {
    pub provider: ModelProviderInfo,
    pub model: String,                       // "deepseek-chat" / "gemini-3-flash" / ...
    pub tools: Vec<Arc<dyn Tool>>,           // 业务工具，不含 submit_result
    /// submit_result 的参数 schema（JSON Schema，serde_json::Value）。
    /// runtime 自动注册名为 "submit_result" 的终结工具。
    pub result_schema: serde_json::Value,
    pub max_rounds: u32,                     // 默认 16
    pub max_total_tokens: Option<u64>,       // token 预算护栏（按 usage 累计）
    pub max_tool_output_bytes: usize,         // 单次工具输出上限，默认 16 KiB
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEventEnvelope>>,
    pub cancel: tokio_util::sync::CancellationToken,
    pub task_id: String,                     // 事件信封回带，server 侧对应 scrape_tasks.id
}

pub struct TaskInput {
    pub system_prompt: String,               // server 侧组装（含反注入声明）
    pub user_message: String,                // evidence bundle
}

// ===== 终态 =====
pub enum TaskOutcome {
    /// 模型调用了 submit_result 且参数通过 schema/serde 校验
    Completed { result: serde_json::Value, rounds_used: u32, usage: TokenTotals },
    Failed    { reason: FailReason, message: String, rounds_used: u32, usage: TokenTotals },
    Aborted   { reason: AbortReason },
}

pub enum FailReason { RoundBudgetExhausted, TokenBudgetExhausted, ApiError,
                      ContextWindowExceeded, InvalidToolCall, Other }
pub enum AbortReason { UserCancelled, Shutdown }
```

### 2.1 管家多轮对话 API

`Agent` 面向必须调用 `submit_result` 的结构化刮削任务；常驻管家使用同 crate 的
`Conversation`，允许模型以普通文本结束一轮，并把上下文持久化职责留给 server：

```rust
pub struct ConversationConfig {
    pub provider: ModelProviderInfo,
    pub model: String,
    pub tools: Vec<Arc<dyn Tool>>,
    pub max_rounds: u32,                     // 默认 12
    pub max_tool_output_bytes: usize,         // 默认 16 KiB
    pub event_tx: Option<UnboundedSender<AgentEventEnvelope>>,
    pub cancel: CancellationToken,
    pub task_id: String,
}

impl Conversation {
    pub fn new(cfg: ConversationConfig) -> Result<Self, AgentError>;
    pub async fn run_turn(&self, messages: Vec<ChatMessage>) -> ConversationOutcome;
}

pub struct ConversationOutcome {
    pub reply: String,
    pub tool_events: Vec<ToolInteraction>,
    pub rounds_used: u32,
    pub usage: TokenTotals,
    pub error: Option<AgentError>,
}
```

## 3. Tool trait（nipaserver 在 nipa-providers 中实现）

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;                       // 注册重名 → panic
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;    // JSON Schema，直通 tools[].function.parameters
    /// 错误二分（codex FunctionCallError 设计）：
    /// - Err(ToolError::RespondToModel(msg)) → msg 作为 tool output 回喂，模型自纠错
    /// - Err(ToolError::Fatal(msg))          → 整个任务失败（FailReason::Other）
    fn call(&self, arguments: serde_json::Value)
        -> BoxFuture<'_, Result<ToolOutput, ToolError>>;
}

pub struct ToolOutput {
    pub content: String,                     // 回喂模型的文本（JSON 序列化结果）
}

pub enum ToolError { RespondToModel(String), Fatal(String) }
```

约定：
- 工具参数解析失败（模型给了坏 JSON）由 **runtime** 处理（喂回错误让模型重试），工具的 `call` 收到的是已解析的 `serde_json::Value`（但字段级校验仍在工具内做，失败走 `RespondToModel`）；
- 工具内部限流/缓存是 nipaserver 的事（egress 层），runtime 不管；
- 并行：一轮内多个 tool_calls 并发执行，结果**按 tool_call_id 全部回填后**才发起下一轮（Gemini 硬性要求）。

## 4. 事件协议（SSE 与 transcript 共用）

```rust
pub struct AgentEventEnvelope {
    pub task_id: String,
    pub seq: u64,            // 任务内单调递增；SSE Last-Event-ID / 落库排序键
    pub ts_ms: i64,
    #[serde(flatten)]
    pub event: AgentEvent,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TaskStarted   { model: String, max_rounds: u32 },
    RoundStarted  { round: u32, max_rounds: u32 },
    AssistantMessage { text: String },
    ToolCallBegin { call_id: String, tool: String, arguments: serde_json::Value },
    ToolCallEnd   { call_id: String, tool: String, success: bool,
                    output_preview: String, error: Option<String>, duration_ms: u64 },
    TokenUsage    { last_input: u64, last_output: u64, total_input: u64, total_output: u64 },
    Retrying      { attempt: u32, max_attempts: u32, message: String },
    Warning       { message: String },
    TaskCompleted { result: serde_json::Value, rounds_used: u32, duration_ms: u64 },
    TaskFailed    { reason: FailReason, message: String, rounds_used: u32 },
    TaskAborted   { reason: AbortReason },
}
```

不变量（server 侧可依赖）：
1. 每任务**恰好一个**终态事件（`task_completed | task_failed | task_aborted`），且是最后一条；
2. 终态前所有未配对的 `ToolCallBegin` 会补发 `success=false` 的 `ToolCallEnd`（不悬空转圈）；
3. `seq` 从 0 起连续递增；
4. SSE 映射：`id: seq` / `event: <type>` / `data: 信封整体 JSON`；transcript 落库 = 信封 JSONL。

## 5. server 侧对接点（nipaserver 内的桥接层）

- `nipa-server` 的 scrape 服务持有任务队列，取任务 → 组 evidence → `Agent::run`；
- `event_tx` 接 `tokio::sync::mpsc` → 广播到 SSE 订阅者 + 逐行 append 到 transcript；
- `TaskOutcome::Completed.result` 反序列化为 `ScrapeResult`（`nipa-core` 定义，字段见开发文档 §4.2 submit_result schema）→ 置信度闸门 → 入库/待确认；
- 取消：server 持有 `CancellationToken`，用户取消/停机时 `.cancel()`。

## 6. 版本与演进

- 本契约随 nipa-agent 0.1.x 生效；0.x 破坏性改动 bump minor，兼容修复 bump patch，并同步更新本文件与双方 changelog；
- 未来回流 Flutter：`AgentEventEnvelope`/`TaskOutcome` 经 flutter_rust_bridge 直接生成 Dart 类型，字段命名保持 snake_case。
