# Codex 工具系统精读报告（面向 nipa-agent 精简实现）

精读范围：
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/tools/src/`（`tool_executor.rs`、`tool_spec.rs`、`tool_output.rs`、`tool_payload.rs`、`function_call_error.rs`、`json_schema.rs`）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/tools/`（`registry.rs`、`router.rs`、`orchestrator.rs`、`parallel.rs`、`context.rs`）
- 最简 handler 样例：`core/src/tools/handlers/current_time.rs`、`sleep.rs`
- 调用侧：`core/src/session/turn.rs`（turn loop）、`core/src/stream_events_utils.rs`（tool call 分发点）

---

## 1. 分层总览（谁负责什么）

Codex 把工具系统切成 5 层，每层职责单一：

```
ToolSpec        —— 模型可见的声明（JSON schema，序列化进请求的 tools 数组）
ToolExecutor    —— trait：spec + handle 绑在同一个对象上（声明和实现不分家）
ToolRegistry    —— HashMap<ToolName, Arc<dyn Runtime>>，按名字查 handler + dispatch
ToolRouter      —— 把模型流式输出的 ResponseItem 解析成 ToolCall，转给 registry
ToolCallRuntime —— 并发控制（parallel.rs）：spawn 任务、读写锁门控、取消
```

turn loop（`turn.rs`）在 SSE 流上每收到一个完成的 output item，就问 `ToolRouter::build_tool_call` 是不是工具调用；是则立即 `tokio::spawn` 执行并把 future 塞进 `FuturesOrdered`，流读完后按顺序 drain 结果、回填历史、进入下一轮采样。

---

## 2. 核心 trait 与类型（精简后的实际定义）

### 2.1 ToolExecutor（`tools/src/tool_executor.rs`）

```rust
pub type ToolExecutorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ToolOutput>, FunctionCallError>> + Send + 'a>>;

pub enum ToolExposure {
    Direct,          // 出现在模型可见 tools 列表
    Deferred,        // 注册但不进初始列表（tool_search 用，我们不需要）
    DirectModelOnly, // code-mode 相关，我们不需要
    Hidden,          // 可 dispatch 但模型看不到
}

pub trait ToolExecutor<Invocation>: Send + Sync {
    fn tool_name(&self) -> ToolName;
    fn spec(&self) -> ToolSpec;
    fn exposure(&self) -> ToolExposure { ToolExposure::Direct }
    fn supports_parallel_tool_calls(&self) -> bool { false }   // 默认串行！
    fn handle(&self, invocation: Invocation) -> ToolExecutorFuture<'_>;
}
```

要点：
- **spec 和 handle 在同一 trait 上**——注册一个 handler 就同时拿到"模型看到什么"和"调用时跑什么"，永远不会声明/实现漂移。这是最值得抄的一条。
- `Invocation` 是泛型参数，core 里实例化为 `ToolInvocation`（携带 session/turn 上下文、call_id、payload、CancellationToken）。
- `supports_parallel_tool_calls` 默认 false，每个工具自己声明可并行（只读工具都应返回 true）。

### 2.2 ToolSpec（`tools/src/tool_spec.rs` + `responses_api.rs`）

```rust
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    // Namespace / ToolSearch / WebSearch / Freeform —— 全是 Responses API 特性，对我们是死重
}

pub struct ResponsesApiTool {
    pub name: String,
    pub description: String,
    pub strict: bool,
    pub parameters: JsonSchema,          // 手写的 JSON-Schema 子集 struct
    #[serde(skip)]
    pub output_schema: Option<Value>,
}
```

Codex 自己维护了一个 `JsonSchema` struct（`json_schema.rs`，含 type/description/enum/items/properties/required/additionalProperties 等字段），带 `JsonSchema::object(properties, required, additional_properties)` 之类的构造器。**对我们**：直接用 `serde_json::json!` 内联 schema 即可，不必抄这个 300 行的类型——它的价值在多 crate 复用与测试断言，5-10 个工具用不上。

### 2.3 ToolPayload 与 ToolCall（`tool_payload.rs`、`router.rs`）

```rust
pub enum ToolPayload {
    Function { arguments: String },   // 原始 JSON 字符串，延迟解析
    ToolSearch { .. },                // 死重
    Custom { input: String },         // Responses API custom tool，死重
}

pub struct ToolCall {
    pub tool_name: ToolName,   // { name: String, namespace: Option<String> }
    pub call_id: String,
    pub payload: ToolPayload,
}
```

要点：**arguments 保持原始字符串直到 handler 内部才 `serde_json::from_str` 解析**（`handlers/mod.rs` 的 `parse_arguments` helper）。解析失败转成 `RespondToModel`，让模型看到具体的 serde 错误并自我修正：

```rust
pub(crate) fn parse_arguments<T: DeserializeOwned>(arguments: &str) -> Result<T, FunctionCallError> {
    serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })
}
```

配合 handler 参数 struct 上的 `#[serde(deny_unknown_fields)]`（见 `sleep.rs` 的 `SleepArgs`），模型编造字段名也会得到明确报错。flash 级模型格式错误率高，这套"serde 错误直接回喂"是廉价而有效的纠错回路，必抄。

### 2.4 FunctionCallError —— 两级错误语义（`function_call_error.rs`）

```rust
#[derive(Debug, Error, PartialEq)]
pub enum FunctionCallError {
    #[error("{0}")]
    RespondToModel(String),   // 可恢复：把文本作为工具输出回给模型，turn 继续
    #[error("Fatal error: {0}")]
    Fatal(String),            // 不可恢复：终止整个 turn
}
```

这是整个错误设计的核心，只有两个变体。消费点在 `parallel.rs::handle_tool_call`：

```rust
match future.await {
    Ok(response) => Ok(response.into_response()),                    // 正常输出
    Err(FunctionCallError::Fatal(msg)) => Err(CodexErr::Fatal(msg)), // 冒泡终止 turn
    Err(other) => Ok(Self::failure_response(error_call, other)),     // RespondToModel →
}                                                                    // 变成 success:false 的工具输出

fn failure_response(call: ToolCall, err: FunctionCallError) -> ResponseInputItem {
    ResponseInputItem::FunctionCallOutput {
        call_id: call.call_id,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(err.to_string()),
            success: Some(false),
        },
    }
}
```

即：**错误不是异常，而是一种工具输出**。模型下一轮看到 `"failed to parse function arguments: missing field `query` at line 1"` 这样的 role=tool 消息，就会重发修正后的调用。找不到工具名也走同一路径（registry 里 `unsupported call: {tool_name}` → RespondToModel）。

### 2.5 ToolOutput（`tool_output.rs`）

```rust
pub trait ToolOutput: Send {
    fn log_preview(&self) -> String;                 // 遥测截断预览（2KB/64行）
    fn success_for_logging(&self) -> bool;
    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem;
    // post_tool_use_* / code_mode_result —— hook 与 code-mode 用，死重
}
```

回填给模型的最终形态（`protocol/src/models.rs`）：

```rust
pub struct FunctionCallOutputPayload {
    pub body: FunctionCallOutputBody,   // 直接序列化为 function_call_output.output
    pub success: Option<bool>,          // 内部元数据，不上 wire
}
#[serde(untagged)]
pub enum FunctionCallOutputBody {
    Text(String),
    ContentItems(Vec<FunctionCallOutputContentItem>),  // 图文混排，我们不需要
}
```

现成的实现 `JsonToolOutput`（handler 返回 `serde_json::Value`，序列化成字符串回填）和 `FunctionToolOutput::from_text(text, success)` 覆盖了几乎所有场景。**对我们**：整个 `ToolOutput` trait 可以坍缩成一个 struct：

```rust
pub struct ToolOutput { pub content: String, pub success: bool }
```

因为 Chat Completions 的 tool 消息只有 `{role:"tool", tool_call_id, content:String}` 一种形态，没有 content items / MCP / tool_search 之分。

---

## 3. Registry 与 dispatch（`core/src/tools/registry.rs`）

去掉 hooks/telemetry/sandbox 后的骨架：

```rust
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<dyn CoreToolRuntime>>,   // CoreToolRuntime: ToolExecutor + 元数据扩展
}

impl ToolRegistry {
    fn from_tools(tools: impl IntoIterator<Item = Arc<dyn CoreToolRuntime>>) -> Self {
        // 逐个 insert，重名即 error_or_panic —— 构建期就炸，值得抄
    }

    async fn dispatch(&self, invocation: ToolInvocation) -> Result<AnyToolResult, FunctionCallError> {
        let tool = self.tool(&invocation.tool_name)
            .ok_or_else(|| FunctionCallError::RespondToModel(
                format!("unsupported call: {}", invocation.tool_name)))?;   // ← 未知工具名回喂模型
        let output = tool.handle(invocation.clone()).await?;
        Ok(AnyToolResult { call_id, payload, result: output })
    }
}
```

原文件 780 行里，dispatch 主体 90% 是 PreToolUse/PostToolUse hooks、OTel 遥测、active-turn 计数、外部上下文污染标记——**对我们全部是死重**。真正的 dispatch 逻辑就是"查表 → handle → 包装结果"三行。

Router 的解析侧（`router.rs::build_tool_call`）负责把模型输出转成 `ToolCall`；我们的对应物是解析 Chat Completions 的 `message.tool_calls[]` 数组（`id` + `function.name` + `function.arguments`），比 Responses API 的流式 item 匹配简单得多。

## 4. 并行工具调用的实现（`parallel.rs` + `turn.rs`）—— 最精妙的部分

三个机制叠加：

**(a) 边流边跑**：turn loop 收到一个完成的 tool call item 就立刻 spawn 执行，不等整条 SSE 流读完（`stream_events_utils.rs:318`）：

```rust
let tool_future = Box::pin(tool_runtime.clone().handle_tool_call(call, cancellation_token));
in_flight.push_back(tool_future);   // FuturesOrdered
```

**(b) FuturesOrdered 保序 drain**：并发执行，但结果按模型发出 tool_calls 的顺序回填历史（`turn.rs::drain_in_flight`）。API 要求 tool 消息与 tool_calls 顺序对应，`FuturesOrdered` 一个类型就同时解决"并发执行"和"顺序回填"。

**(c) RwLock 作并发门**（`parallel.rs`）——单个 `RwLock<()>` 实现"只读工具并行、写工具独占"：

```rust
pub(crate) struct ToolCallRuntime {
    router: Arc<ToolRouter>,
    parallel_execution: Arc<RwLock<()>>,   // 每个 turn 一把
}

// spawn 出的任务里：
let _guard = if supports_parallel {
    Either::Left(lock.read().await)    // 可并行 → 读锁，互不阻塞
} else {
    Either::Right(lock.write().await)  // 串行工具 → 写锁，独占
};
router.dispatch(...).await
```

**(d) 取消**：每个工具任务包在 `AbortOnDropHandle`（tokio_util）里，外层 `tokio::select!` 竞争 `cancellation_token.cancelled()`；取消时 abort 任务并合成一条 `"aborted by user after {secs}s"` 的工具输出回填（历史保持完整，不留悬空 tool_call）。

**对我们**：(a)(b)(c) 全部照抄，总共 ~60 行。我们的工具全部只读，`supports_parallel` 可以全 true，甚至可以退化为 `futures::future::join_all`——但保留 RwLock 门有个好处：`submit_result` 声明为串行（写锁），天然保证它执行时没有其他工具还在飞。取消机制在 ≤16 轮、每轮几秒的场景下可以简化为整个任务级别的一个 CancellationToken。

## 5. Orchestrator（`orchestrator.rs`）—— 对我们 100% 死重

它做的是：审批（approval）→ 沙箱选择 → 执行 → 沙箱拒绝后升级重试（免二次审批）→ 网络审批。全部围绕"在用户机器上跑不可信命令"这一前提。nipa-agent 的工具是进程内只读函数，无审批、无沙箱、无升级重试，**整个文件跳过**。唯一可借鉴的抽象：文件头注释里"approval → select sandbox → attempt → retry with escalation"这种把重试策略集中在一个 orchestrator、handler 保持无知的分层思想——如果将来 TMDB/Bangumi 调用要做限流重试，应该放在类似的中间层而不是散在各 handler 里。

## 6. 值得抄 / 死重清单

**值得抄：**
1. `ToolExecutor` trait 把 spec+handle 绑定（声明实现不分家）。
2. `FunctionCallError::{RespondToModel, Fatal}` 两级错误 + "错误即工具输出"回喂模型。
3. `parse_arguments` + `deny_unknown_fields`：serde 错误原文回喂，flash 模型自我修正。
4. `FuturesOrdered` 并发执行 + 保序回填。
5. `RwLock<()>` 读/写锁并发门（只读并行、终结工具独占）。
6. 未知工具名不 panic，回喂 `unsupported call: X`。
7. Registry 构建期重名检测。
8. 取消时合成 aborted 工具输出，保持消息历史配对完整。
9. `supports_parallel_tool_calls` 由工具自声明，默认保守（false）。

**死重（不要抄）：**
- `ToolSpec` 里 Namespace/ToolSearch/WebSearch/Freeform 变体、`ToolExposure::Deferred`、tool_search 全套（`tool_search.rs`、`tool_discovery.rs`）——Responses API + 大工具集专用。
- Pre/PostToolUse hooks、`ToolArgumentDiffConsumer`（流式参数 diff）、OTel 遥测、`ToolDispatchTrace`、`ToolCallTimingGuard`。
- `orchestrator.rs`、`sandboxing.rs`、`approvals.rs`、network_approval 全部。
- code_mode、MCP、extension tools、multi_agents handlers。
- `ToolOutput` trait 的多态（对 Chat Completions 一个 struct 足够）、`ContentItems` 图文输出。
- 手写 `JsonSchema` struct（用 `json!` 内联）。

## 7. nipa-agent 对应设计（可直接照写）

```rust
// ---- 类型 ----
pub enum ToolError {
    RespondToModel(String),   // 回喂模型，任务继续
    Fatal(anyhow::Error),     // 终止任务
}

pub struct ToolOutput {
    pub content: String,      // 回填 {role:"tool"} 的 content
    pub success: bool,        // 仅用于日志/SSE 事件
}

/// 终结信号：submit_result 通过它带出结构化结果
pub enum ToolOutcome {
    Continue(ToolOutput),
    Finish { output: ToolOutput, result: ScrapeResult },   // 唯一成功终止路径
}

#[async_trait]                // 或手写 Pin<Box<dyn Future>>；async_trait 更省事
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> serde_json::Value;          // json!({...}) 内联 schema
    fn parallel_safe(&self) -> bool { true }            // 只读默认并行，submit_result 返回 false
    async fn call(&self, ctx: &TaskCtx, args: &str) -> Result<ToolOutcome, ToolError>;
}

// Chat Completions tools 数组：
// {"type":"function","function":{"name":..,"description":..,"parameters":..}}
pub fn tools_json(tools: &[Arc<dyn Tool>]) -> Vec<serde_json::Value> { ... }

// ---- Registry ----
pub struct ToolRegistry { tools: HashMap<&'static str, Arc<dyn Tool>> }
// from_tools 重名 panic；dispatch 查不到 → RespondToModel("unknown tool: {name}")

// ---- 每轮 dispatch（并行 + 保序 + 终结检测）----
async fn run_tool_calls(
    registry: &ToolRegistry, ctx: &TaskCtx, calls: Vec<ToolCallReq>,  // 来自 message.tool_calls
    gate: &tokio::sync::RwLock<()>, events: &EventTx,                 // SSE 播报
) -> (Vec<ChatMessage /*role=tool*/>, Option<ScrapeResult>) {
    let mut futs = FuturesOrdered::new();
    for call in calls {
        futs.push_back(async move {
            let _g = match registry.get(&call.name).map(|t| t.parallel_safe()) {
                Some(true) => Either::Left(gate.read().await),
                _          => Either::Right(gate.write().await),   // submit_result 独占
            };
            events.send(AgentEvent::ToolStart { name, call_id, args_preview });
            let outcome = match registry.dispatch(ctx, &call).await {
                Ok(o) => o,
                Err(ToolError::RespondToModel(msg)) =>
                    ToolOutcome::Continue(ToolOutput { content: msg, success: false }),
                Err(ToolError::Fatal(e)) => return Err(e),
            };
            events.send(AgentEvent::ToolEnd { call_id, success });
            Ok((call.id, outcome))
        });
    }
    // drain：每个 call_id 必须产出一条 tool 消息（含失败），保持与 tool_calls 配对；
    // 遇到 Finish 记下 ScrapeResult，剩余 future 照常 drain 完再返回。
}
```

**submit_result 终结工具的做法**（Codex 没有直接对应物——它靠"无 tool call 即结束"，我们反过来）：
1. `submit_result` 是普通注册工具，schema 里放最终结构（tmdb_id/bgm_id/title/year/episode 映射/confidence 等），description 写明"确认识别结果后必须调用此工具提交"。
2. handler 内做**强校验**（serde `deny_unknown_fields` + 业务校验如 id 必须来自本任务查询过的结果）；校验失败返回 `RespondToModel("submit_result rejected: ...")`，模型还有轮次可修正——这正是 codex 错误回路在终结工具上的复用。
3. 校验通过返回 `ToolOutcome::Finish`，turn loop 看到即停止采样（不再发下一轮请求）。同批其余并发工具照常 drain（`parallel_safe()=false` 的写锁已保证 Finish 执行时无并发）。
4. 兜底：a) 模型只回文本不调工具 → 追加一条 user 消息"你必须调用工具，完成时调用 submit_result"再采样；b) 16 轮耗尽仍无 Finish → 任务以 `ExhaustedTurns` 失败，SSE 播报。
5. SSE 事件枚举建议：`TaskStart / RoundStart / ToolStart / ToolEnd / ModelText / TaskFinish(result) / TaskFailed(reason)`——一个 `tokio::sync::broadcast` 通道即可同时喂 axum SSE 和 flutter_rust_bridge 的 StreamSink，无 tokio 之外依赖（codex 用的 tokio_util 的 `Either`/`AbortOnDropHandle` 也都是纯 tokio 生态，可用可不用）。

规模预估：以上全套（trait + registry + 并行 dispatch + 事件）~300 行，对照 codex 相应区域 ~4000+ 行，裁剪比约 10:1，裁掉的全部是 Responses API 形态、审批/沙箱、hooks 与遥测。