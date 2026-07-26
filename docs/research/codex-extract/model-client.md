# codex 模型客户端与 Provider 配置精读报告（面向 nipa-agent）

> 精读范围：`/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/model-provider-info/src/lib.rs`（554 行）、`core/src/client.rs`（2440 行）、`core/src/client_common.rs`（127 行），以及顺藤摸瓜读到的 `core/src/responses_retry.rs`、`core/src/util.rs`（backoff）、`protocol/src/error.rs`（错误分类）、`core/src/session/turn.rs`（流消费循环，L1161-L2530）。
>
> **重要提示**：本 checkout 中 `codex-api` crate（真正的 HTTP/SSE 传输层与 SSE→ResponseEvent 解析器）**缺失**（workspace Cargo.toml 引用 `codex-api = { path = "codex-api" }` 但目录不存在）。SSE 逐行解析代码读不到，但其对外契约（`ResponseEvent` 枚举、`RetryConfig`、`stream_idle_timeout` 的传入点）在 client.rs 的调用侧完全可见，本报告据此还原，并针对 Chat Completions 给出等价状态机设计。

---

## 一、codex 的分层（先建立地图）

```
turn.rs (agent loop)                 ← 流重试循环（stream_max_retries），每次重试用最新 history 重建 Prompt
  └─ ModelClientSession::stream()    ← 每 turn 一个；选 transport（WS/HTTP），401 恢复循环
       └─ ModelClient (session 级)   ← 持有 provider/auth/线程 id，跨 turn 共享
            └─ codex-api crate       ← HTTP 请求级重试（request_max_retries）、SSE 解析、idle 超时
                 └─ ModelProviderInfo → to_api_provider() → ApiProvider（纯数据）
```

关键设计：**两层重试互不知晓**。传输层（codex-api）只管"一次 HTTP 请求"的重试（连接失败/5xx）；agent 层（turn.rs）只管"一条流断掉后整轮重发"的重试。两层各自有独立预算，职责清晰。这是最值得抄的骨架。

---

## 二、ModelProviderInfo 字段表

定义于 `model-provider-info/src/lib.rs` L89-L144。精简后：

```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelProviderInfo {
    pub name: String,                                    // 展示名
    pub base_url: Option<String>,                        // OpenAI 兼容 API 根 URL
    pub env_key: Option<String>,                         // 存 API key 的环境变量名
    pub env_key_instructions: Option<String>,            // 缺 key 时给用户的提示文案
    pub experimental_bearer_token: Option<String>,       // 直接内联 bearer（程序化场景）
    pub auth: Option<ModelProviderAuthInfo>,             // 命令行子进程产 token（如 gcloud auth print-token）
    pub aws: Option<ModelProviderAwsAuthInfo>,           // AWS SigV4（Bedrock）
    pub wire_api: WireApi,                               // 协议枚举（现只剩 Responses）
    pub query_params: Option<HashMap<String, String>>,   // URL 附加 query（Azure 的 api-version）
    pub http_headers: Option<HashMap<String, String>>,   // 固定附加 header
    pub env_http_headers: Option<HashMap<String, String>>,// header 名→环境变量名，变量为空则不发
    pub request_max_retries: Option<u64>,                // HTTP 请求级重试次数
    pub stream_max_retries: Option<u64>,                 // 流断线整轮重发次数
    pub stream_idle_timeout_ms: Option<u64>,             // 流上两个事件之间的空闲超时
    pub websocket_connect_timeout_ms: Option<u64>,       // [死重] WS 连接超时
    pub requires_openai_auth: bool,                      // [死重] ChatGPT 登录流
    pub supports_websockets: bool,                       // [死重]
    pub supports_standalone_web_search: bool,            // [死重]
}
```

### 字段语义与 nipa 取舍

| 字段 | codex 语义 | nipa 建议 |
|---|---|---|
| `name` | 展示 + `is_openai()` 判别 | 保留（日志/事件播报用） |
| `base_url` | `None` 时按 auth 模式选默认 | 保留，必填即可（用户配自己的兼容端点） |
| `env_key` | 运行时读 env，空串视为未设并报 `EnvVarError{var, instructions}` | **改成直接存 key 字符串**。你们配置来自 Flutter/服务器配置文件，不是 shell env。但"空白串视为未配置 + 带解决指引的错误"值得抄 |
| `env_key_instructions` | 报错时附带"去哪申请 key" | 保留思想：错误里带可行动指引 |
| `experimental_bearer_token` / `auth` / `aws` | 三种备选鉴权 | 全删，一个 `api_key: Option<String>` 够了 |
| `wire_api` | 枚举，deserialize 时对已删除的 `"chat"` 值给出**定向迁移报错**（L72-84） | 你们只有 Chat Completions，可删枚举；但"废弃配置值给迁移文案而非 unknown variant"这招值得记住 |
| `query_params` | Azure `?api-version=...` | **保留**——很多自建网关/Azure 兼容端点需要 |
| `http_headers` | 每请求附加 | **保留**——私有网关常要自定义 header |
| `env_http_headers` | 值来自环境变量 | 可删（嵌入场景无意义） |
| `request_max_retries` | 默认 4，硬上限 100 | 保留 |
| `stream_max_retries` | 默认 5，硬上限 100 | 保留（flash 模型流不稳，这个必须有） |
| `stream_idle_timeout_ms` | 默认 300_000（5 分钟） | 保留但**默认调小**：flash 模型 + 只读工具，建议 60s 甚至 30s |
| 其余 websocket/openai_auth/web_search | Responses/ChatGPT 特有 | 全删 |

### 有效值收敛函数（值得抄的小模式，L305-330）

配置字段全部 `Option`，语义（默认值 + 上限钳制）收敛在 getter 里，调用方永远拿到可用值：

```rust
pub fn request_max_retries(&self) -> u64 {
    self.request_max_retries.unwrap_or(4).min(100)   // 用户配置有硬上限，防配错打爆上游
}
pub fn stream_max_retries(&self) -> u64 {
    self.stream_max_retries.unwrap_or(5).min(100)
}
pub fn stream_idle_timeout(&self) -> Duration {
    self.stream_idle_timeout_ms.map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(300_000))
}
```

### ProviderInfo → 运行时 Provider 的降解（L244-281）

配置结构和运行时结构分离：`ModelProviderInfo`（serde 友好、全 Option）→ `ApiProvider`（headers 已构建成 `HeaderMap`、retry 已实例化）。传输层不认识配置格式：

```rust
pub fn to_api_provider(&self) -> Result<ApiProvider> {
    Ok(ApiProvider {
        name, base_url, query_params,
        headers: self.build_header_map()?,     // http_headers + env_http_headers 合并
        retry: ApiRetryConfig {
            max_attempts: self.request_max_retries(),
            base_delay: Duration::from_millis(200),
            retry_429: false,       // 注意：429 不在传输层重试！留给上层按 Retry-After 处理
            retry_5xx: true,
            retry_transport: true,  // 连接失败/DNS/TLS 可重试
        },
        stream_idle_timeout: self.stream_idle_timeout(),
    })
}
```

`retry_429: false` 是刻意的：429 带 `Retry-After`，盲目按固定 backoff 重试会撞墙；codex 把它归到 `UsageLimitReached`（不可重试，直接报给用户）或经 `CodexErr::retry_delay()`（服务器指定延迟）走流级重试。**nipa 建议同样把 429 从传输层排除，在流级重试里尊重 Retry-After。**

---

## 三、请求组装（协议无关的部分）

### Prompt（client_common.rs L18-49）——每轮采样的输入包

```rust
pub struct Prompt {
    pub input: Vec<ResponseItem>,        // 完整对话历史（codex 无状态重发全量）
    pub tools: Vec<ToolSpec>,
    pub parallel_tool_calls: bool,
    pub base_instructions: BaseInstructions,   // system prompt
    pub output_schema: Option<Value>,          // 结构化输出 schema
    pub output_schema_strict: bool,
}
```

对应到 Chat Completions：`input` → `messages[]`，`base_instructions` → system message，`tools` → `tools[]`，`output_schema` → `response_format: {type: "json_schema"}`。nipa 直接照抄这个形状。

### ResponseItem（protocol/src/models.rs L799+）——统一的历史条目模型

nipa 需要的最小子集（Chat Completions 语义完全覆盖）：

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    Message { role: String, content: Vec<ContentItem> },      // user/assistant/system
    FunctionCall {
        name: String,
        arguments: String,   // ★ 保持原始 JSON 字符串，不预解析——留给 handler 解析并把解析错误
        call_id: String,     //   作为 FunctionCallOutput 回喂给模型（codex 注释 L869-871 明确此设计）
    },
    FunctionCallOutput { call_id: String, output: FunctionCallOutputPayload },
}
```

`arguments` 存字符串而非 `Value` 很关键：flash 模型常吐坏 JSON，解析失败要把错误文本作为 tool output 回给模型让它重试，而不是在反序列化层崩掉。

### build_responses_request（client.rs L838-925）中协议无关的要点

- `tool_choice: "auto"` 写死；`stream: true` 写死。
- `prompt_cache_key`: 用 session_id 做提示缓存 key（Chat Completions 对应字段是 `prompt_cache_key`，OpenAI 已支持；对通用兼容端点无害）。nipa 每个刮削任务用 task_id 即可。
- verbosity/reasoning 按 `ModelInfo` 能力位裁剪：模型不支持就不发字段并 warn。**"能力表驱动请求字段"** 值得抄——nipa 可以给每个 provider/model 配 `supports_parallel_tool_calls`、`supports_json_schema` 等位。

---

## 四、流式处理：两级流水线（协议无关，nipa 核心要抄的）

### 4.1 ResponseStream 包装器（client_common.rs L104-123）

```rust
pub struct ResponseStream {
    rx_event: mpsc::Receiver<Result<ResponseEvent>>,
    consumer_dropped: CancellationToken,   // 消费者提前 drop 时通知后台任务
}
impl Stream for ResponseStream {
    type Item = Result<ResponseEvent>;
    fn poll_next(...) { self.rx_event.poll_recv(cx) }
}
impl Drop for ResponseStream {
    fn drop(&mut self) { self.consumer_dropped.cancel(); }
}
```

设计点：网络流的解析在**独立 tokio task** 里跑，通过有界 mpsc（容量 1600）把事件递给消费者。消费者（agent loop）drop 掉流（用户取消/16 轮上限触发），`Drop` 里 cancel token，后台任务在 `tokio::select!` 里感知并立即退出，不泄漏连接。**这是嵌 axum + FRB 都需要的形状：SSE 对外播报的 axum handler drop 时上游模型请求自动终止。**

### 4.2 map_response_events：聚合/转发任务（client.rs L1916-2088）

精简后的骨架（去掉 telemetry/trace）：

```rust
fn map_response_events(api_stream, ...) -> (ResponseStream, oneshot::Receiver<LastResponse>) {
    let (tx_event, rx_event) = mpsc::channel(1600);
    let consumer_dropped = CancellationToken::new();
    tokio::spawn(async move {
        let mut items_added: Vec<ResponseItem> = Vec::new();
        loop {
            let event = tokio::select! {
                _ = consumer_dropped.cancelled() => return,       // 消费者跑了，立即收工
                event = api_stream.next() => event,
            };
            let Some(event) = event else { break };               // 流自然结束（无 Completed）→ 掉到下面报错
            match event {
                Ok(ResponseEvent::OutputItemDone(item)) => {
                    items_added.push(item.clone());               // 聚合本次响应产生的完整 item
                    if tx_event.send(Ok(OutputItemDone(item))).await.is_err() { return; }
                }
                Ok(ResponseEvent::Completed { response_id, token_usage, .. }) => {
                    // 记录 usage / TTFT；转发 Completed；这是唯一正常终态
                    tx_event.send(...).await;
                }
                Ok(other) => { /* delta 类事件直接转发（记录首个 item 的 TTFT） */ }
                Err(err) => { tx_event.send(Err(map(err))).await; }
            }
        }
        // ★ 循环因流关闭而退出（没收到 Completed）→ 上层会看到 stream 提前结束
    });
    (ResponseStream { rx_event, consumer_dropped }, rx_last_response)
}
```

关键契约：**"流在 `Completed` 之前关闭"不是成功，是可重试错误。** 消费侧（turn.rs L2096-2104）：

```rust
let event = match stream.next().await {
    Some(Ok(event)) => event,
    Some(Err(err)) => break Err(err),
    None => break Err(CodexErr::Stream("stream closed before response.completed".into())),
};
```

nipa 必须照抄这条：Chat Completions 里对应 "没收到 `finish_reason` / `data: [DONE]` 就 EOF"。flash 模型托管端点半路掉流很常见，缺了这条会把半截响应当完整响应。

### 4.3 ResponseEvent 枚举（codex-api 定义，从调用侧还原）

codex 用到的变体（core 内 grep 统计）中，nipa 需要的最小集：

```rust
pub enum ResponseEvent {
    Created,                                   // 流已建立（可对外播报 "thinking..."）
    OutputItemAdded(ResponseItem),             // 一个输出项开始（含空壳 FunctionCall：有 name/call_id 无完整 arguments）
    OutputTextDelta(String),                   // 文本增量（对外播报进度）
    ToolCallInputDelta { call_id, delta },     // 工具参数增量（nipa 可选：仅播报"正在调 search_tmdb"够用）
    OutputItemDone(ResponseItem),              // ★ 一个输出项完整了（含完整 FunctionCall）
    Completed { response_id: String, token_usage: Option<TokenUsage> },  // ★ 唯一成功终态
}
// codex 还有 Reasoning*/RateLimits/ServerModel/ModelsEtag/SafetyBuffering 等 → 全是 Responses/OpenAI 内部特有，删
```

Agent loop 只依赖 `OutputItemDone` 和 `Completed` 驱动状态；delta 事件纯粹用于 UI 播报。**"驱动逻辑只吃完整 item、播报走 delta"的分离**让 loop 逻辑与流式细节解耦，nipa 照抄。

### 4.4 映射到 Chat Completions：delta.tool_calls 聚合状态机设计

codex 的 Responses API 由服务器直接推送结构化事件（`response.output_item.added` / `response.function_call_arguments.delta` / `response.output_item.done`），**分片聚合是服务器做好的**。Chat Completions 里聚合要自己做，分片语义：

- 每个 chunk：`choices[0].delta.tool_calls: [{index, id?, type?, function: {name?, arguments?}}]`
- **`index` 是聚合 key**（并行调用时多个 index 交错）；`id` 和 `function.name` 只在该 index 的**首个分片**出现；`function.arguments` 是需逐片拼接的字符串
- `delta.content` 与 `tool_calls` 可能交替出现
- 终态：`finish_reason: "tool_calls" | "stop" | "length" | "content_filter"`；usage 在 `stream_options: {include_usage: true}` 时于最后一个 chunk 给出；然后 `data: [DONE]`

nipa 聚合器（把 Chat Completions 分片翻译成上面 4.3 的 ResponseEvent 流）：

```rust
#[derive(Default)]
struct ChatStreamAggregator {
    content: String,                              // 累积文本
    tool_calls: BTreeMap<u32, PartialToolCall>,   // index → 分片累积
    emitted_created: bool,
}
#[derive(Default)]
struct PartialToolCall { id: String, name: String, arguments: String }

impl ChatStreamAggregator {
    /// 每收到一个 SSE data chunk 调一次，返回要向下游发的事件
    fn on_chunk(&mut self, chunk: ChatChunk) -> Vec<ResponseEvent> {
        let mut out = vec![];
        if !self.emitted_created { self.emitted_created = true; out.push(ResponseEvent::Created); }
        let Some(choice) = chunk.choices.into_iter().next() else {
            // 无 choices 的 chunk = usage-only 尾包，暂存 usage
            self.usage = chunk.usage; return out;
        };
        if let Some(text) = choice.delta.content {
            self.content.push_str(&text);
            out.push(ResponseEvent::OutputTextDelta(text));
        }
        for tc in choice.delta.tool_calls.unwrap_or_default() {
            let slot = self.tool_calls.entry(tc.index).or_default();
            if let Some(id) = tc.id { slot.id = id; }                    // 首分片
            if let Some(f) = tc.function {
                if let Some(name) = f.name {
                    slot.name = name;
                    out.push(/* 可选：OutputItemAdded(空壳 FunctionCall) 用于播报 "调用 search_tmdb..." */);
                }
                if let Some(args) = f.arguments {
                    out.push(ResponseEvent::ToolCallInputDelta { call_id: slot.id.clone(), delta: args.clone() });
                    slot.arguments.push_str(&args);                       // ★ 纯字符串拼接，不做增量 JSON 解析
                }
            }
        }
        if let Some(reason) = choice.finish_reason {
            out.extend(self.finish(reason));                              // 见下
        }
        out
    }

    /// finish_reason 到达：吐出完整 item + Completed
    fn finish(&mut self, reason: FinishReason) -> Vec<ResponseEvent> {
        let mut out = vec![];
        if !self.content.is_empty() {
            out.push(ResponseEvent::OutputItemDone(ResponseItem::Message {
                role: "assistant".into(),
                content: vec![ContentItem::OutputText { text: std::mem::take(&mut self.content) }],
            }));
        }
        for (_, tc) in std::mem::take(&mut self.tool_calls) {             // BTreeMap 保证按 index 有序
            out.push(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                call_id: tc.id, name: tc.name,
                arguments: tc.arguments,   // 原始字符串；坏 JSON 由 tool 分发层报回模型
            }));
        }
        out.push(ResponseEvent::Completed { finish_reason: reason, token_usage: self.usage.take() });
        out
    }
}
// 外层：收到 `data: [DONE]` 前 EOF/出错，且尚未 finish → Err(Stream("closed before completion")) → 可重试
```

与 codex 对齐的三个不变量：
1. **`OutputItemDone(FunctionCall)` 里 arguments 一定完整**——聚合器保证，agent loop 不碰分片。
2. **`Completed` 是唯一成功终态**，缺了就是 `Stream` 可重试错误。
3. **arguments 只做字符串拼接**，解析推迟到工具分发（对应 codex models.rs L869-871 注释）。

额外建议（codex 没有、但 flash 模型需要）：有些兼容端点 `finish_reason` 给了但从不发 `[DONE]`；聚合器应以 finish_reason 为准终结，`[DONE]`/EOF 只做兜底。以及防御 `tool_calls` 首分片缺 `id` 的端点（用 `call_{index}` 合成）。

### 4.5 idle 超时的落点

`stream_idle_timeout` 不是整个响应的超时，而是**相邻两个 SSE 事件之间的**超时（codex-api 里用 `tokio::time::timeout` 包每次 poll，从 `SseTelemetry::on_sse_poll` 的签名 `Result<Option<Result<Event, _>>, Elapsed>` 可证，client.rs L2374-2385）。unary 请求（compact 端点）则用 `idle × 4` 当整体超时（L630-633 及注释 L162-164）。nipa 实现：

```rust
loop {
    match tokio::time::timeout(idle_timeout, sse_stream.next()).await {
        Ok(Some(ev)) => aggregator.on_chunk(parse(ev)?),
        Ok(None) => /* EOF */,
        Err(_Elapsed) => return Err(Error::Stream("idle timeout".into())),  // 可重试
    }
}
```

---

## 五、重试/超时策略清单

### 三个参数的实际用途（总结）

| 参数 | 默认 | 上限 | 生效层 | 语义 |
|---|---|---|---|---|
| `request_max_retries` | 4 | 100 | codex-api 传输层 | 单次 HTTP 请求：连接失败/5xx 重试（429 除外），base_delay 200ms |
| `stream_max_retries` | 5 | 100 | turn.rs agent 层 | 流建立后断掉：**整轮重发**（重新构建 Prompt、重新走 HTTP） |
| `stream_idle_timeout_ms` | 300 000 | — | codex-api SSE poll | 相邻事件间空闲超时，超时算流断 → 计入 stream 重试预算 |

### backoff（util.rs L86-91）——直接抄

```rust
const INITIAL_DELAY_MS: u64 = 200;
const BACKOFF_FACTOR: f64 = 2.0;
pub fn backoff(attempt: u64) -> Duration {   // attempt 从 1 起
    let exp = BACKOFF_FACTOR.powi(attempt.saturating_sub(1) as i32);
    let base = (INITIAL_DELAY_MS as f64 * exp) as u64;
    let jitter = rand::rng().random_range(0.9..1.1);   // ±10% 抖动
    Duration::from_millis((base as f64 * jitter) as u64)
}
// 200ms, 400ms, 800ms, 1.6s, 3.2s ...
```

### 流级重试循环（turn.rs L1198-1267 + responses_retry.rs L22-74）——精简后骨架

```rust
let max_retries = provider.stream_max_retries();
let mut retries = 0u64;
loop {
    let prompt = build_prompt(latest_history(), ...);   // ★ 每次重试都从最新 history 重建
    let err = match try_run_sampling_request(&prompt).await {
        Ok(output) => return Ok(output),
        Err(err) if fatal(&err) => return Err(err),     // ContextWindowExceeded / UsageLimit → 直接失败
        Err(err) => err,
    };
    if !err.is_retryable() { return Err(err); }
    if retries >= max_retries { return Err(err); }
    retries += 1;
    let delay = err.retry_delay().unwrap_or_else(|| backoff(retries));  // ★ 服务器 Retry-After 优先
    emit_warning_event(format!("Reconnecting... {retries}/{max_retries}"));  // ★ 播报给 UI，别让用户看冻屏
    tokio::time::sleep(delay).await;
}
```

三个必抄的点：
1. **重试重建 Prompt 而不是重发旧请求**：一条流断掉前可能已产出若干 `OutputItemDone` 并进了 history，重试用最新 history，服务器返回过的 item 不会重复算。
2. **`err.retry_delay()`（服务器 Retry-After，`CodexErr` 上的 `with_retry_delay` 携带）优先于本地 backoff**。
3. **每次重试发一条对外事件**（对应你们 SSE 播报流里加一个 `retrying {n}/{max}` 事件类型）。

### 401 处理（client.rs L1406-1504, handle_unauthorized L2168+）

独立于上述两层：`stream_responses_api` 外面套一个 `loop`，收到 401 → 尝试 token refresh（`UnauthorizedRecovery`，只有 next step 可用时执行）→ 成功则 `continue` 重发，失败/无恢复手段则报错。**nipa 用静态 API key，401 不可恢复，直接归为不可重试错误即可**——但保留这个"401 单独处理、不占重试预算"的结构位置，将来接 OAuth 的网关时有用。

### 错误分类（protocol/src/error.rs L358-396）——`is_retryable()` 完整清单

```rust
pub fn is_retryable(&self) -> bool {
    match self.details() {
        // 不可重试：
        // TurnAborted / Interrupted（用户取消）
        // Fatal / EnvVar（配置错）
        // InvalidRequest / InvalidImageRequest（4xx 请求本身错，重发也错）
        // UsageLimitReached / QuotaExceeded / UsageNotIncluded（配额，重试无意义）
        // ServerOverloaded（模型满载，codex 让用户换模型）
        // ContextWindowExceeded（要压缩历史，不是重发）
        // RetryLimit（已经重试穷尽）
        // RefreshTokenFailed / Sandbox / ...
        ... => false,
        // 可重试：
        CodexErrorDetails::Stream(..)              // 流中途断（含 idle 超时）
        | CodexErrorDetails::Timeout | RequestTimeout
        | CodexErrorDetails::UnexpectedStatus(_)   // 意外 HTTP 状态（注意：含未归类的 4xx！见下）
        | CodexErrorDetails::ResponseStreamFailed(_)  // reqwest 读 body 错
        | CodexErrorDetails::ConnectionFailed(_)      // 连接建立失败
        | CodexErrorDetails::InternalServerError      // 5xx
        | CodexErrorDetails::Io(_) | Json(_) | TokioJoin(_) => true,
    }
}
```

nipa 的错误枚举建议（够用的最小集）：

```rust
pub enum AgentError {
    Stream(String),           // 可重试：流断/idle 超时/EOF 无终态
    Connection(String),       // 可重试：连接失败
    ServerError(u16, String), // 可重试：5xx
    RateLimited { retry_after: Option<Duration> },  // 可重试但走 retry_after
    BadRequest(u16, String),  // 不可重试：4xx（模型名错/schema 错）
    Auth(String),             // 不可重试：401/403
    ContextWindowExceeded,    // 不可重试（nipa 场景直接判任务失败）
    Cancelled,
    RetryLimit { last: Box<AgentError> },
}
```

注意一个 codex 的坑别抄：它把 `UnexpectedStatus`（含未识别 4xx）归为可重试，是因为传输层已把已知 4xx 提前归类走了；nipa 如果自己实现，**务必 400-499（除 408/429）不重试**，否则模型名写错会白烧 4 次。另外 codex 对 JSON 解析错也重试（`Json(_) => true`）——对 SSE 中途坏包合理，对请求序列化不合理，nipa 按错误来源区分。

---

## 六、值得抄 vs 死重 总表

### 值得抄（协议无关）

1. **配置/运行时结构分离**：`ModelProviderInfo`（全 Option + serde）→ `to_api_provider()` → 运行时结构；默认值和上限钳制收敛在 getter。
2. **两层重试**：传输层管单请求（5xx/连接错，200ms 指数退避），agent 层管流断（整轮重发、从最新 history 重建、尊重 Retry-After、对外播报重试进度）。
3. **流聚合双通道**：后台 task 解析 → 有界 mpsc → `ResponseStream` 实现 `Stream` + `Drop` cancel token。天然适配 axum SSE handler 和 FRB StreamSink，取消传播免费获得。
4. **"OutputItemDone 驱动逻辑、delta 驱动播报"的事件分层**；`Completed` 为唯一成功终态，提前 EOF = 可重试错误。
5. **idle 超时按事件间隔计**，不是整响应超时。
6. **`FunctionCall.arguments` 保持原始字符串**，解析失败作为 tool output 回喂模型。
7. **错误枚举带 `is_retryable()` + `retry_delay()`**，重试决策一处收口。
8. **能力表驱动请求字段**（ModelInfo 的 supports_* 位）。
9. 细节：`env_key` 空白串视为未配置且报错带指引；用户配置的重试次数设硬上限（100）；重试通知第一次静默、之后才打扰用户（responses_retry.rs L56-58 的思路，nipa 可简化为全部播报）。

### 死重（Responses/codex 特有，nipa 全删）

- **WebSocket 传输全套**（约占 client.rs 一半）：`WebsocketSession`、连接缓存/复用、`prewarm`（generate=false 预热）、增量请求（`previous_response_id` + `get_incremental_items` 前缀比对）、HTTP fallback 状态机（`disable_websockets: AtomicBool`）。Chat Completions 无此概念。
- **`x-codex-turn-state` sticky routing**（`Arc<OnceLock<String>>` 回放）——codex 后端专属契约。
- **ChatGPT 登录 / token refresh / UnauthorizedRecovery / AgentIdentity / attestation / AWS SigV4**——nipa 一个 Bearer key。
- **遥测三件套**（`RequestTelemetry`/`SseTelemetry`/`WebsocketTelemetry` + feedback_tags）——占了 client.rs 后半 500 行，nipa 用 tracing 打日志即可；但 `SseTelemetry::on_sse_poll` 揭示的"每次 poll 包 timeout"实现方式要留下。
- **reasoning/verbosity/service_tier/responses-lite/encrypted_content**——Responses 请求体特有。
- **compact / memories / realtime 三个端点**的 client 方法。
- `ResponseItem` 的十余个变体（Reasoning/WebSearchCall/CustomToolCall/Compaction/...）——nipa 留 Message/FunctionCall/FunctionCallOutput 三个。
- `merge_configured_model_providers` 的 Bedrock 特判、内置 provider 目录（ollama/lmstudio 端口探测）——nipa 的 provider 来自自己的配置系统。

### 对 nipa 约束的对齐说明

- **≤16 轮**：codex 无硬轮次上限（预算在 token/rollout 层），nipa 在 agent loop 外层数 `Completed` 次数即可；超限时 drop `ResponseStream`，`consumer_dropped` 机制保证连接立即关闭。
- **submit_result 终止**：对应 codex "OutputItemDone(FunctionCall) → 分发 → 不再 needs_follow_up" 的判断位；nipa 在收到 `FunctionCall{name:"submit_result"}` 时不回喂 output、直接终止循环。
- **FRB/tokio 约束**：上述设计只用 `tokio::{spawn, select, time, sync::{mpsc, oneshot}}` + `tokio_util::sync::CancellationToken` + `reqwest` + `futures::Stream`，无其他运行时依赖，FRB 侧把 `ResponseStream` 的事件转投 `StreamSink` 即可。
- **SSE 对外播报**：ResponseEvent（4.3 的精简枚举）可以近乎一比一地序列化成对外 SSE 事件（`created`/`text_delta`/`tool_call_started`/`tool_call_done`/`retrying`/`completed`/`failed`），聚合器一处产出、axum 与 FRB 两个出口共用。

### 关键文件路径索引

- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/model-provider-info/src/lib.rs` — ProviderInfo 全部字段/getter/to_api_provider
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/client_common.rs` — Prompt / ResponseStream + Drop cancel
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/client.rs` — L838 请求组装；L1395 HTTP 流 + 401 循环；L1919-2088 map_response_events 聚合任务
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/responses_retry.rs` — 流级重试决策
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/util.rs` L86 — backoff
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/protocol/src/error.rs` L358 — is_retryable 分类
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/session/turn.rs` L1198（重试循环）、L2068-2530（流消费 match）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/protocol/src/models.rs` L799 — ResponseItem 定义