# codex 事件协议精读报告（面向 nipa-agent 精简实现）

精读范围：
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/protocol/src/protocol.rs`（Submission/Op、Event/EventMsg 全部变体及 payload struct）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/protocol/src/dynamic_tools.rs`
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/protocol/src/items.rs`（TurnItem）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/exec/src/exec_events.rs`、`event_processor.rs`、`event_processor_with_jsonl_output.rs`、`lib.rs` 主循环

---

## 1. 总体架构：SQ/EQ 双队列 + 双层事件协议

codex 的会话协议是经典的 **Submission Queue / Event Queue** 模式：客户端向 agent 提交 `Submission`（带唯一 id），agent 异步回吐 `Event`（携带关联的 submission id）。这是整个协议的骨架，非常值得抄：

```rust
// protocol.rs:1259（精简）
pub struct Submission {
    pub id: String,      // 唯一 id，用于和 Event 关联
    pub op: Op,          // 载荷
}

pub struct Event {
    pub id: String,      // 关联的 Submission id
    pub msg: EventMsg,   // 载荷
}

// EventMsg 的 serde 标注 —— 这一行是 wire format 的关键
#[derive(Debug, Clone, Deserialize, Serialize, Display, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]   // {"type": "turn_started", ...}
pub enum EventMsg { /* 80+ 变体 */ }
```

更重要的发现是 codex 有**两层事件协议**：

1. **内层 `EventMsg`**（protocol crate）：细粒度、扁平、80+ 变体，是 core 内部与所有前端共享的"全量"事件。
2. **外层 `ThreadEvent`**（exec crate 的 `exec_events.rs`）：对外 JSONL 输出用的**粗粒度、8 个变体**的稳定公共 API，由 `EventProcessorWithJsonOutput` 从内层事件**翻译**而来。

对 nipa-agent 来说，最值得抄的恰恰是外层这个 8 变体的设计（见第 4 节），而不是内层的 80+ 变体巨型 enum。

## 2. Op（Submission 侧）变体速览

`Op`（protocol.rs:522）共 27 个变体，对刮削场景只有 4 类有意义：

| 变体 | 说明 | 刮削场景 |
|---|---|---|
| `UserInput { items, final_output_json_schema, .. }` | 开启一个 turn，可带最终输出的 JSON Schema 约束 | **要抄**（= "开始刮削任务"） |
| `Interrupt` | 中止当前任务，回 `TurnAborted` | **要抄**（用户取消刮削） |
| `Shutdown` | 关闭 agent 实例，回 `ShutdownComplete` | 可选 |
| `ExecApproval` / `PatchApproval` / `DynamicToolResponse` 等审批/回应类 | 客户端对 agent 挂起请求的应答 | 不需要（只读工具无审批） |
| 其余（Realtime 语音会话 6 个、`Compact`、`ThreadRollback`、`Review`、`RefreshMcpServers`、`ThreadSettings`、`InterAgentCommunication`、Guardian 等） | 交互式 IDE/CLI 功能 | **死重** |

值得注意的小设计：`Op::kind() -> &'static str` 给每个变体一个稳定字符串名，方便打点/日志。

## 3. EventMsg 变体全表（约 80 个，按功能分组，一句话/个）

标注：★ = 对刮削场景有直接意义；☆ = 可借鉴思想但不必抄；— = 死重。

### 生命周期 ★
| 变体 | 一句话 | 标注 |
|---|---|---|
| `SessionConfigured` | 会话建立 ack，携带 session_id/thread_id/model 等配置快照 | ★（任务开始时播报一次配置） |
| `TurnStarted` | 一个 turn 开始（含 turn_id、started_at、context window） | ★ |
| `TurnComplete` | turn 完成，含 `last_agent_message`、可选终态 `error`、started_at/completed_at/duration_ms/首 token 延迟 | ★（成功/失败都走这里，字段设计值得抄） |
| `TurnAborted` | turn 被中止，`reason: Interrupted/Replaced/ReviewEnded/BudgetLimited` | ★（用户取消 + 16 轮预算耗尽正好对应 `BudgetLimited`） |
| `ShutdownComplete` | agent 已关停 | ☆ |

### 错误与重试 ★
| 变体 | 一句话 | 标注 |
|---|---|---|
| `Error(ErrorEvent)` | 致命错误：`{ message, codex_error_info: Option<CodexErrorInfo> }`，错误分类枚举含 ContextWindowExceeded/Unauthorized/流断开/重试耗尽等 | ★（错误分类枚举思路必抄） |
| `Warning` / `GuardianWarning` | 非致命警告，turn 继续但需告知用户 | ★（Warning 要抄，Guardian 版死重） |
| `StreamError` | 模型流断开/出错、系统正在重试（含 backoff 语义） | ★（flash 模型 + 兼容网关很容易断流，"正在重试"事件对 UI 体验很关键） |
| `DeprecationNotice` | 弃用提示 | — |

### 模型输出 ★
| 变体 | 一句话 | 标注 |
|---|---|---|
| `AgentMessage` | agent 完整文本消息 | ★ |
| `AgentMessageContentDelta` | 文本流式增量（thread_id/turn_id/item_id/delta） | ☆（刮削不需要逐字流式，可不做） |
| `UserMessage` | 回显发给模型的用户输入（含图片/音频附件路径） | ☆（落库 transcript 有用） |
| `AgentReasoning` / `AgentReasoningRawContent` / `AgentReasoningSectionBreak` / `ReasoningContentDelta` / `ReasoningRawContentDelta` | 推理摘要/原始 CoT 及其增量与分节 | —（Chat Completions 无 reasoning 流；某些兼容网关有 reasoning_content，可留一个非流式变体） |

### 工具调用 ★（核心关注区）
| 变体 | 一句话 | 标注 |
|---|---|---|
| `McpToolCallBegin` | MCP 工具调用开始：`{ call_id, invocation: {server, tool, arguments} }` | ★（begin/end 成对 + call_id 配对的模式必抄） |
| `McpToolCallEnd` | MCP 工具调用结束：`{ call_id, invocation, duration, result: Result<CallToolResult, String> }`，带 `is_success()` | ★ |
| `DynamicToolCallRequest` | 动态（客户端实现的）工具调用请求：`{ call_id, turn_id, started_at_ms, namespace, tool, arguments }` —— agent 挂起等客户端用 `Op::DynamicToolResponse` 应答 | ☆（这是"工具执行权反转"设计，nipa 工具在进程内实现则不需要挂起等待，但字段形状值得抄） |
| `DynamicToolCallResponse` | 动态工具调用完成：`{ call_id, tool, arguments, content_items, success, error, duration }` | ★（字段形状 = nipa 的 ToolCallEnd 蓝本） |
| `ExecCommandBegin` / `ExecCommandOutputDelta` / `ExecCommandEnd` / `TerminalInteraction` | shell 命令执行三部曲 + PTY 交互（OutputDelta 用 base64 传原始字节） | —（无 shell 工具；但 End 里 `exit_code/duration/status/aggregated_output` 的字段组合可参考） |
| `WebSearchBegin` / `WebSearchEnd` | 内建 web 搜索开始/结束 | —（nipa 的 search_tmdb 走统一工具事件即可） |
| `ImageGenerationBegin` / `ImageGenerationEnd`、`ViewImageToolCall` | 图片生成/查看 | — |
| `PatchApplyBegin` / `PatchApplyUpdated` / `PatchApplyEnd`、`TurnDiff` | 文件补丁生命周期 | — |

### 审批/交互（全部死重，理由：只读工具 + 无人值守）
`ExecApprovalRequest`、`ApplyPatchApprovalRequest`、`RequestPermissions`、`RequestUserInput`、`ElicitationRequest`、`GuardianAssessment` —— 均为"agent 挂起 → 人审批 → Op 应答"模式。刮削是全自动只读流程，**整个审批家族都不需要**。这是相对 codex 最大的一块可砍面积（连带 `ReviewDecision`、`AskForApproval`、SandboxPolicy 等一大片类型）。

### token 用量 ★
| 变体 | 一句话 | 标注 |
|---|---|---|
| `TokenCount(TokenCountEvent)` | `{ info: Option<TokenUsageInfo>, rate_limits: Option<RateLimitSnapshot> }`；`TokenUsageInfo = { total_token_usage, last_token_usage, model_context_window }` | ★（"累计 + 最近一次"双计数设计必抄；rate_limits 是 OpenAI 账号体系专用，死重） |
| `RawResponseCompleted` | 单次 API 响应的原始未累计用量 | ☆ |
| `RawResponseItem` | 原始响应条目透传 | — |

`TokenUsage` 本体（protocol.rs:2056）：

```rust
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}
// 配套方法：add_assign 累加、blended_total()（非缓存输入+输出）、
// percent_of_context_window_remaining(context_window)（扣除 BASELINE_TOKENS 基线再算百分比）
```

### item 生命周期（v2 协议，值得抄思想）
| 变体 | 一句话 | 标注 |
|---|---|---|
| `ItemStarted` / `ItemCompleted` | 以 `TurnItem`（tagged enum：AgentMessage/Reasoning/CommandExecution/DynamicToolCall/McpToolCall/FileChange/WebSearch/Plan...）为载荷的统一条目生命周期，带 started_at_ms/completed_at_ms | ☆（codex 正从"每种活动一对 Begin/End 事件"迁移到"统一 Item 生命周期 + item 内部带 status 字段"，nipa 一步到位学后者更省） |
| `PlanUpdate` / `PlanDelta` | 计划（todo list）更新 | — |

### 其余（全部死重）
Realtime 语音会话 4 个 + `RealtimeConversationListVoicesResponse`；Collab 多 agent 10 个（SpawnBegin/End、InteractionBegin/End、WaitingBegin/End、CloseBegin/End、ResumeBegin/End）+ `SubAgentActivity`；`ModelReroute`/`ModelVerification`/`TurnModerationMetadata`/`SafetyBuffering`（OpenAI 后端安全审查）；`ContextCompacted`/`ThreadRolledBack`/`ThreadSettingsApplied`/`ThreadGoalUpdated`；`McpStartupUpdate`/`McpStartupComplete`；`EnvironmentConnected`/`Disconnected`；`HookStarted`/`HookCompleted`；`EnteredReviewMode`/`ExitedReviewMode`。16 轮单任务刮削场景全都用不上。

## 4. dynamic_tools.rs 提炼

这个文件是"客户端注册自定义工具"的完整最小蓝本，与 nipa 的自定义工具高度同构：

```rust
// 工具声明（发给 agent 用来构建 API 请求里的 tools 数组）
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DynamicToolSpec {
    Function(DynamicToolFunctionSpec),
    Namespace(DynamicToolNamespaceSpec),   // 命名空间分组，nipa 5-10 个工具不需要
}
pub struct DynamicToolFunctionSpec {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,          // 直接存 JSON Schema，不做强类型 —— 抄
    pub defer_loading: bool,
}

// 调用请求（agent → 客户端）
pub struct DynamicToolCallRequest {
    pub call_id: String,
    pub turn_id: String,
    pub started_at_ms: i64,
    pub namespace: Option<String>,
    pub tool: String,
    pub arguments: JsonValue,
}

// 调用响应（客户端 → agent）
pub struct DynamicToolResponse {
    pub content_items: Vec<DynamicToolCallOutputContentItem>,  // InputText/InputImage/InputAudio
    pub success: bool,
}
```

值得抄的点：
- **`input_schema` 直接用 `serde_json::Value`**：不给 JSON Schema 建强类型，直通到 Chat Completions 的 `tools[].function.parameters`。
- **`call_id` 贯穿请求/响应/事件**：与 Chat Completions 的 `tool_call_id` 天然对齐。
- **`success: bool` 与 content 分离**：失败时 content_items 仍可携带错误说明文本回喂模型。
- 文件后半段的 legacy 格式归一化（`normalize_dynamic_tool_specs`）是历史包袱，不抄。

## 5. exec 的 JSONL 事件输出（对外协议层）—— 最值得整体照抄的部分

### 5.1 ThreadEvent：8 变体的公共事件面

`exec_events.rs` 定义的对外 JSONL 协议只有 8 个变体，事件名用 `资源.动作` 风格：

```rust
#[serde(tag = "type")]
pub enum ThreadEvent {
    #[serde(rename = "thread.started")]  ThreadStarted(ThreadStartedEvent),   // { thread_id }
    #[serde(rename = "turn.started")]    TurnStarted(TurnStartedEvent),       // {}
    #[serde(rename = "turn.completed")]  TurnCompleted(TurnCompletedEvent),   // { usage: Usage }
    #[serde(rename = "turn.failed")]     TurnFailed(TurnFailedEvent),         // { error: { message } }
    #[serde(rename = "item.started")]    ItemStarted(ItemStartedEvent),       // { item: ThreadItem }
    #[serde(rename = "item.updated")]    ItemUpdated(ItemUpdatedEvent),
    #[serde(rename = "item.completed")]  ItemCompleted(ItemCompletedEvent),
    #[serde(rename = "error")]           Error(ThreadErrorEvent),             // 不可恢复错误
}

pub struct ThreadItem {
    pub id: String,
    #[serde(flatten)]
    pub details: ThreadItemDetails,   // 内层再 tag = "type"：agent_message / reasoning /
}                                     // command_execution / mcp_tool_call / web_search / todo_list / error ...
```

设计要点（照抄清单）：
1. **turn 三终态**：`turn.completed`（带用量汇总）/ `turn.failed`（带 error）/ 中断时静默关停。消费方只需盯这三个信号判定任务结束。
2. **item 生命周期承载一切活动**：工具调用不是独立事件对，而是同一个 item 以 `started(status=in_progress)` → `completed(status=completed/failed)` 出现两次，item 内部有自己的 status 枚举。SSE 消费端据 item.id 做 upsert 即可渲染进度。
3. **`ThreadItem { id, #[serde(flatten)] details }`**：id 在外层、类型细节 flatten 进来，JSON 形如 `{"id":"item_0","type":"mcp_tool_call","server":...,"status":"completed"}`，SQLite 落库和前端消费都很舒服。
4. **每种 item 自带小状态机**：`CommandExecutionStatus { InProgress, Completed, Failed, Declined }`、`McpToolCallStatus { InProgress, Completed, Failed }` —— 状态放 item 里而非事件名里，事件名只表达生命周期。

### 5.2 EventProcessor trait 与主循环驱动

```rust
// event_processor.rs（完整）
pub enum CodexStatus { Running, InitiateShutdown }

pub(crate) trait EventProcessor {
    fn print_config_summary(&mut self, config: &Config, prompt: &str,
                            session_configured: &SessionConfiguredEvent);
    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus;
    fn process_warning(&mut self, message: String) -> CodexStatus;
    fn print_final_output(&mut self) {}
}
```

主循环（lib.rs:967）用 `tokio::select!` 同时收内部事件流和中断信号，**由 processor 的返回值 `CodexStatus::InitiateShutdown` 驱动循环退出**——即"收到 TurnCompleted/TurnFailed 才结束"的终止判定内聚在事件处理器里，而不是散在循环体里。这个控制流值得抄。

### 5.3 EventProcessorWithJsonOutput 里值得抄的状态管理

- **id 重映射**（`raw_to_exec_item_id: HashMap<String, String>`）：内部 call_id 映射为顺序的 `item_0, item_1...`，`started_item_id` 插入、`completed_item_id` remove 取出，保证 begin/end 配对且对外 id 稳定紧凑。
- **`reconcile_unfinished_started_items`**：turn 结束时，把发过 `item.started` 但没发 `item.completed` 的 item 强制补发 completed —— 保证消费端永远不会有悬空的 in_progress。**必抄**（模型中途出错或被打断时前端不会卡转圈）。
- **用量缓存**：token 用量事件只更新 `last_total_token_usage`，到 `turn.completed` 时一次性汇总输出，避免对外事件噪音。
- **final_message 提取**：`final_message_from_turn_items` 从 turn items 倒序找最后一条 AgentMessage 作为最终输出。
- **序列化失败降级**：`emit()` 里 `serde_json::to_string` 失败时输出一条 error 事件而不是 panic。
- 错误信息格式统一为 `"{message} ({details})"`。

## 6. 死重清单（相对 nipa-agent）

| 死重 | 体量/影响 |
|---|---|
| 审批全家桶（Exec/Patch/Permissions/UserInput/Elicitation/Guardian 的事件 + Op 应答 + ReviewDecision/AskForApproval/SandboxPolicy） | protocol.rs 约三分之一的类型 |
| Realtime 语音会话（Op 6 个 + 事件 5 个 + Voice 枚举等） | 约 500 行 |
| Collab 多 agent / SubAgent（事件 11 个 + 配套 struct） | 约 400 行 |
| 命令执行/补丁/diff/PTY（ExecCommand*/PatchApply*/TurnDiff/TerminalInteraction/ParsedCommand） | 只读工具用不到 |
| OpenAI 账号体系（RateLimitSnapshot/CreditsSnapshot/PlanType/SafetyBuffering/ModelReroute/ModelVerification） | 自架网关无此概念 |
| Compact/Rollback/Review/Hook/Plugin/MCP startup/Environment 连接 | 16 轮短任务不需要上下文管理 |
| 双协议派生（`JsonSchema` + `ts_rs::TS` + strum，及 legacy 事件兼容层 `legacy_events.rs`、dynamic_tools 的 legacy 归一化） | nipa 走 flutter_rust_bridge，Rust 类型直接生成 Dart，serde 一套即可 |
| 流式 delta 族（AgentMessageContentDelta/Reasoning*Delta/ExecCommandOutputDelta） | 刮削播报到"工具调用"粒度即可，省掉一整层增量协议 |

## 7. nipa-agent 最小事件集设计（Rust enum，SSE + transcript 落库两用）

设计原则（均从 codex 提炼）：
- 外层信封带 `task_id / seq / ts_ms`（对应 codex `Event.id` 关联 + item 时间戳），SSE 断线重连可用 `Last-Event-ID = seq` 续传，落库时整行 JSON 即 transcript 的一行；
- `#[serde(tag = "type", rename_all = "snake_case")]` 内部 tag，Dart/JS 端好解析；
- 工具调用 begin/end 用 `call_id` 配对，end 内含 status（学 `DynamicToolCallResponseEvent`）；
- 三终态互斥且必有其一：`task_completed` / `task_failed` / `task_aborted`（学 turn.completed/failed + TurnAborted，`reason` 里含 `RoundBudgetExhausted` 对应 codex 的 `BudgetLimited`）；
- token 用量学 `TokenUsageInfo` 的 total+last 双计数，但只在每轮结束发一次；
- 终止前必须补齐所有未完成的 tool_call_end（学 `reconcile_unfinished_started_items`）。

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// 事件信封：SSE 推送一条 = transcript 落库一行。
/// SSE: id = seq, event = event.type_name(), data = 整个信封 JSON。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeEventEnvelope {
    pub task_id: String,     // 对应 scrape_tasks.id
    pub seq: u64,            // 任务内单调递增，SSE Last-Event-ID / 落库排序键
    pub ts_ms: i64,          // Unix 毫秒
    #[serde(flatten)]
    pub event: ScrapeEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScrapeEvent {
    /// 任务开始（对应 SessionConfigured + TurnStarted 合并）
    TaskStarted {
        file_path: String,        // 待刮削的媒体文件
        model: String,
        max_rounds: u32,
    },
    /// 第 N 轮模型调用开始（刮削独有：16 轮预算的进度条数据源）
    RoundStarted { round: u32, max_rounds: u32 },
    /// 模型的文字输出（flash 模型偶尔在工具调用外说话；非流式，整条下发）
    AssistantMessage { text: String },
    /// 工具调用开始（学 McpToolCallBegin / DynamicToolCallRequest）
    ToolCallBegin {
        call_id: String,          // = Chat Completions 的 tool_call_id
        tool: String,             // "search_tmdb" | "search_bangumi" | ...
        arguments: Json,
    },
    /// 工具调用结束（学 DynamicToolCallResponseEvent；与 Begin 按 call_id 配对）
    ToolCallEnd {
        call_id: String,
        tool: String,
        success: bool,
        /// 回喂模型的输出的截断预览（UI 播报用，全文在工具自己的日志里）
        output_preview: String,
        /// success=false 时的错误说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        duration_ms: u64,
    },
    /// 每轮结束后的用量（学 TokenUsageInfo 的 total+last 双计数）
    TokenUsage {
        last_input_tokens: u64,
        last_output_tokens: u64,
        total_input_tokens: u64,
        total_output_tokens: u64,
    },
    /// 非致命：请求流断开/超时，正在第 attempt 次重试（学 StreamError）
    StreamRetry { attempt: u32, max_attempts: u32, message: String },
    /// 非致命警告（学 Warning：任务继续但用户应知晓）
    Warning { message: String },
    /// 成功终态：模型调用了 submit_result（学 TurnComplete，result 即工具入参）
    TaskCompleted {
        result: Json,             // submit_result 的 arguments，直接入库
        rounds_used: u32,
        duration_ms: u64,
    },
    /// 失败终态（学 TurnComplete.error + CodexErrorInfo 的错误分类）
    TaskFailed {
        reason: FailReason,
        message: String,
        rounds_used: u32,
    },
    /// 中止终态（学 TurnAborted）
    TaskAborted { reason: AbortReason },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    RoundBudgetExhausted,     // 16 轮内没有调 submit_result（对应 BudgetLimited）
    ApiError,                 // 上游 4xx/5xx 且重试耗尽（对应 ResponseTooManyFailedAttempts）
    ContextWindowExceeded,
    InvalidToolCall,          // 模型持续输出无法解析的工具调用
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    UserCancelled,            // 对应 Op::Interrupt → TurnAborted(Interrupted)
    Shutdown,
}

impl ScrapeEvent {
    /// 稳定字符串名（学 Op::kind()）：SSE 的 event: 字段 / 日志打点用
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::TaskStarted { .. }      => "task_started",
            Self::RoundStarted { .. }     => "round_started",
            Self::AssistantMessage { .. } => "assistant_message",
            Self::ToolCallBegin { .. }    => "tool_call_begin",
            Self::ToolCallEnd { .. }      => "tool_call_end",
            Self::TokenUsage { .. }       => "token_usage",
            Self::StreamRetry { .. }      => "stream_retry",
            Self::Warning { .. }          => "warning",
            Self::TaskCompleted { .. }    => "task_completed",
            Self::TaskFailed { .. }       => "task_failed",
            Self::TaskAborted { .. }      => "task_aborted",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TaskCompleted { .. } | Self::TaskFailed { .. } | Self::TaskAborted { .. })
    }
}
```

配套约定（实现时的不变量，均源自 codex 的做法）：
1. **传输**：runtime 内部用 `tokio::sync::broadcast::Sender<ScrapeEventEnvelope>`（或每任务一个 mpsc + fanout），axum 侧 `Sse::new(stream)` 直接映射，frb 侧用 `StreamSink<ScrapeEventEnvelope>` 转发同一条流——事件类型只定义一次，两端复用。
2. **终态唯一且必达**：每个任务恰好以 `task_completed | task_failed | task_aborted` 之一结束；发终态前先为所有未配对的 `ToolCallBegin` 补发 `success=false` 的 `ToolCallEnd`（学 `reconcile_unfinished_started_items`）。
3. **落库**：`scrape_tasks.transcript` 存信封 JSON 的 JSONL（或 JSON 数组），重放/前端回填历史 = 按 seq 排序重新消费同一套 enum，与 SSE 消费代码完全同构——这正是 codex `SessionConfiguredEvent.initial_messages: Option<Vec<EventMsg>>` 复用事件类型做历史回放的思路。
4. **序列化失败降级**：emit 时 `to_string` 失败输出一条 `Warning` 而非 panic（学 `EventProcessorWithJsonOutput::emit`）。
5. 事件数 11 个，去掉可选的 `AssistantMessage`/`Warning`/`StreamRetry` 后最小可用集为 8 个，与 codex 对外 ThreadEvent 的规模一致——**证明对外协议收敛到 10 个左右变体是 codex 自己验证过的正确粒度**。

## 8. 取舍建议总结

抄：SQ/EQ 双队列骨架、`#[serde(tag="type")]` 内部 tag 枚举、call_id 配对的 Begin/End、三终态互斥、total+last 双 token 计数、`StreamRetry` 非致命重试播报、错误分类枚举（`CodexErrorInfo` 的精简版）、终止前 reconcile 未完成项、EventProcessor trait + 状态驱动主循环退出、`input_schema: Json` 直通、事件类型同时服务实时流与历史回放。

不抄：内层 80+ 变体巨型 enum（直接一步到位设计对外事件集）、审批/沙箱/Realtime/Collab/补丁/账号限额全家桶、流式 delta 层、JsonSchema+ts-rs 双派生、legacy 兼容层、DynamicTool 的"挂起等客户端应答"机制（nipa 工具进程内执行，同步 await 即可）。