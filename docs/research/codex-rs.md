# OpenAI Codex CLI (openai/codex) 调研报告

调研日期：2026-07-26，基于 main 分支实际代码（本地 sparse clone 核对）。

## 1. 仓库与 workspace 结构

仓库根目录：TypeScript 部分已基本淘汰，核心全部在 `codex-rs/`（Cargo workspace）。workspace 规模已非常庞大：**约 120 个成员 crate，2738 个 .rs 文件，约 122 万行 Rust 代码**（含测试；core 约 25 万行、tui 约 19 万行、app-server 约 4.9 万行）。

核心 crate 划分（按职责）：

**协议 / 类型层**
- `protocol`（发布名 codex-protocol，~2.2 万行）：SQ/EQ 协议核心类型 —— `Op`（Submission 枚举：UserTurn / Interrupt / ExecApproval / UserInputAnswer…）、`EventMsg`（事件枚举：AgentMessage / AgentMessageContentDelta / ExecApprovalRequest / TurnStarted / TurnComplete / Error…）、`AuthMode`、`ResponseItem` 模型消息类型、`dynamic_tools.rs`（外部注入工具的 spec/call/response 类型）。**该 crate 已发布到 crates.io（codex-protocol 0.63.0）**。
- `app-server-protocol`：JSON-RPC 风格的 app-server 线协议（v1/v2），供 VS Code 扩展等 GUI 使用，含 `thread/start` 的 `dynamic_tools` 实验字段。

**引擎层**
- `core`（codex-core，src 约 17.5 万行）：agent 引擎本体。Session/Task/Turn 状态机（`codex_thread.rs`、`session/`、`ThreadManager`）、模型客户端（`client.rs`）、工具注册与分发（`src/tools/`：registry.rs、router.rs、orchestrator.rs、parallel.rs、handlers/、runtimes/）、MCP 集成（mcp.rs、mcp_tool_call.rs）、压缩/compact、rollout 持久化桥接、沙箱策略。依赖 **68 个内部 codex-* crate**（这是剪枝的最大难点）。
- `tools`（codex-tools）：core 外可复用的工具原语 —— `ToolSpec`（enum：Function(ResponsesApiTool) / WebSearch / Freeform）、`ToolExecutor` trait、`ToolExposure`（Direct/Deferred/DirectModelOnly/Hidden）、JSON Schema 解析、MCP tool → Responses API tool 转换。
- `codex-api` + `codex-client` + `http-client` + `websocket-client`：对模型后端的 HTTP/SSE/WebSocket 传输，Provider 抽象、重试、限流。
- `model-provider-info` + `models-manager`：provider 注册表（见第 3 节）。

**前端 / 入口**
- `cli`：`codex` 多工具入口（arg0 分发）。
- `tui`（~19 万行）：ratatui 交互界面。
- `exec`：`codex exec` 非交互/headless 模式，含 `event_processor_with_jsonl_output`（JSONL 事件流输出，适合程序化消费）。
- `app-server` / `app-server-daemon` / `app-server-client`：给 IDE 的长驻 JSON-RPC 服务。

**MCP 相关**
- `mcp-server`：把 Codex 自身作为 MCP server 暴露（`codex mcp-server`）。
- `rmcp-client`：MCP 客户端（连外部 MCP server，把其 tools 注入模型工具列表）。
- `codex-mcp`：core 内 MCP 管理（McpManager、工具缓存、审批模板）。

**执行 / 沙箱**
- `exec-server`、`execpolicy`、`linux-sandbox`（Landlock+seccomp）、`bwrap`、`windows-sandbox-rs`、`sandboxing`（macOS Seatbelt 等）、`apply-patch`（自研 patch 格式 + lark 语法）、`shell-command`、`utils/pty`。

**其他外围**：`login`（OAuth）、`ollama`/`lmstudio`（本地模型探测）、`rollout`/`thread-store`/`state`（会话持久化）、`otel`（遥测）、`skills`/`hooks`/`plugin`/`connectors`/`memories`/`cloud-tasks`/`code-mode`（工具调用编译为代码执行的实验模式）、`v8-poc` 等。

## 2. Agent loop 与工具调用机制

### 事件流协议（codex-rs/docs/protocol_v1.md）
- UI 与引擎通过一对队列通信：**SQ（Submission Queue，UI→Codex，载荷为 `Op`）/ EQ（Event Queue，Codex→UI，载荷为 `EventMsg`）**。事件可序列化为 newline-delimited JSON，可跑在线程 channel、stdio、TCP、gRPC 等任意双向流上。
- 层级：`Session`（配置+状态）→ `Task`（一次用户输入触发的工作，Session 同时最多 1 个）→ `Turn`（一次模型请求循环：prompt→SSE 流式响应→执行工具→输出作为下一 Turn 输入；Turn 无输出则 Task 结束）。
- 每个 Turn 结束保存 Responses API 的 `response_id` 作为续接/分叉书签（`EventMsg::TurnComplete` 返回）。审批（ExecApprovalRequest→Op::ExecApproval）内嵌在事件流里。

### 工具注册与执行
- 基础抽象在 `tools` crate：`pub trait ToolExecutor<Invocation>: Send + Sync { fn tool_name(); fn spec() -> ToolSpec; fn exposure() -> ToolExposure; fn supports_parallel_tool_calls(); fn handle(&self, invocation) -> ToolExecutorFuture; }`，返回 `Result<Box<dyn ToolOutput>, FunctionCallError>`。
- core 内 `core/src/tools/`：`ToolRegistry`（`HashMap<ToolName, Arc<dyn CoreToolRuntime>>`，`from_tools()` 构建，重名 panic）→ `ToolRouter::build_tool_call()` 把模型返回的 `ResponseItem`（function_call / custom_tool_call / mcp 调用）解析为 `ToolCall` → `dispatch_tool_call` 分发到 handler，支持并行工具调用（parallel.rs）、pre/post-tool-use hooks、审批、遥测。orchestrator.rs 负责一个 Turn 内的多工具编排。
- 内置 handler（`core/src/tools/handlers/`）：shell / unified_exec（PTY 会话）/ apply_patch / view_image / plan / request_user_input / request_permissions / mcp（外部 MCP 工具透传）/ tool_search（Deferred 工具按需发现）/ multi_agents（子 agent）等。
- 工具暴露给模型时被转成 Responses API 的 tool 定义（`tool_definition_to_responses_api_tool`、`mcp_tool_to_responses_api_tool`）。`ToolExposure::Deferred` 支持"先隐藏、模型用 tool_search 按需加载"，对控制小模型的 token 开销有用。
- **三条注入自定义工具的官方途径**：(a) 在 core 里实现 `ToolExecutor` 并注册（需改源码/自建 registry）；(b) 通过 MCP server 提供工具（config.toml `mcp_servers`，零改码）；(c) app-server v2 协议 `thread/start` 的实验字段 `dynamic_tools: Vec<DynamicToolSpec>`（`{name, description, input_schema}`），工具调用以 `DynamicToolCallRequest` 事件发回宿主，由宿主执行后回传 `DynamicToolResponse` —— 这正是"宿主进程注册工具、Codex 只做 runtime"的形态，但标注为 experimental。

## 3. 模型接入方式

由 `model-provider-info` crate 管理。`ModelProviderInfo` 字段：`name / base_url / env_key / env_key_instructions / experimental_bearer_token / auth（command 生成 token）/ aws（SigV4）/ wire_api / query_params / http_headers / env_http_headers / request_max_retries / stream_max_retries / stream_idle_timeout_ms / requires_openai_auth / supports_websockets` 等。

- **内置 provider 仅 4 个**：`openai`、`amazon-bedrock`（bedrock-mantle OpenAI 兼容端点）、`ollama`（http://localhost:11434/v1）、`lmstudio`（:1234/v1）。用户可在 `~/.codex/config.toml` 的 `[model_providers.<id>]` 下任意扩展（内置项除 bedrock 部分字段外不可覆盖）。
- **关键限制：`wire_api = "chat"`（Chat Completions）已被移除**（代码中反序列化直接报错，指向 discussion #7782；`WireApi` enum 现在只剩 `Responses`）。也就是说 **main 分支的 Codex 只支持 OpenAI Responses API（/v1/responses）方言**。接入第三方需要：端点原生支持 Responses（新版 Ollama、LM Studio、vLLM 新版、Azure OpenAI），或经 LiteLLM 之类网关转换。直接把 Gemini Flash 的 OpenAI-compat chat endpoint 填进 base_url 在当前版本行不通（旧版本 tag 仍有 chat 支持，可 pin 旧版）。
- **认证**：`AuthMode` 枚举 = ApiKey / Chatgpt（OAuth，token 存 auth.json 并自动刷新）/ ChatgptAuthTokens（外部宿主提供）/ Headers / AgentIdentity / PersonalAccessToken / BedrockApiKey。第三方 provider 走 `env_key` 环境变量（Bearer）或 `auth.command`（执行命令取 token）；`requires_openai_auth=true` 才会触发 ChatGPT 登录流程。ChatGPT 订阅额度只能用于 openai 官方 provider。

配置示例（写入开发文档可直接用）：
```toml
[model_providers.myproxy]
name = "my-proxy"
base_url = "http://127.0.0.1:4000/v1"   # 需支持 /v1/responses
env_key = "MYPROXY_API_KEY"
wire_api = "responses"

model = "gemini-flash-latest"
model_provider = "myproxy"
```

## 4. 许可证与规模

- **Apache-2.0**（可商用、可修改、可闭源分发，需保留 NOTICE/许可声明）。
- GitHub API：repo 约 512 MB（含历史），~10.15 万 star。codex-rs：~120 crate / 2738 个 .rs / ~122 万行（约 40% 是测试）。核心引擎 core 单 crate src 17.5 万行、直接依赖 68 个内部 crate。

## 5. 嵌入媒体服务器作为"刮削 agent runtime"的评估

### 5.1 若剪枝嵌入 codex-rs
理论保留集：`protocol`（类型）、`tools`（ToolExecutor/ToolSpec）、`codex-api`+`codex-client`（Responses SSE 客户端）、`model-provider-info`、core 中的 turn loop + tool router。理论移除集：tui、cli、app-server*、exec-server、全部沙箱（linux-sandbox/bwrap/windows-sandbox/execpolicy/sandboxing）、apply-patch、login/ChatGPT OAuth、cloud-tasks、skills/plugins/connectors/memories/hooks、rollout/thread-store、otel、ollama/lmstudio、code-mode、multi-agents。

**结论：不建议**。理由：
1. `codex-core` 与上述"应移除"部分深度耦合（68 个内部依赖，session 初始化强绑沙箱策略、审批、rollout、AGENTS.md、skills），无 feature flag 可裁剪，剪枝等于长期维护一个 fork；上游迭代极快（monorepo 式大改频繁，如 wire_api chat 直接删除），rebase 成本高。
2. 仅 `codex-protocol` 在 crates.io 上发布；`codex-core` 不发布，只能 git dependency 整仓引入（编译时间、供应链体积都不可接受：全 workspace 编译产物数 GB）。
3. 它的工具面向"改代码 + 执行 shell"，媒体刮削需要的只是 function-calling 循环，Codex 的 90% 能力（沙箱、patch、PTY、审批 UI）都是死重。
4. 只支持 Responses API，与"flash 级模型驱动"目标冲突（Gemini/多数廉价模型的 OpenAI 兼容层是 chat completions）。

### 5.2 不嵌入、进程外复用（若坚持用 Codex）
- 方案 A：子进程跑 `codex exec --json`（JSONL 事件流）或 `codex app-server`（JSON-RPC，v2 支持 `dynamic_tools` 宿主侧工具），媒体服务器作为宿主执行工具回调。第三方社区已有 `codex-app-server-sdk`（Tokio Rust SDK）、`codex-client-sdk`（CLI-over-JSONL 封装）可参考。
- 方案 B：把刮削工具做成一个内部 MCP server（stdio），在 config.toml 注册给 Codex。零改码，但仍拖着整个 Codex 二进制（>50MB）、Responses-only、模型选择受限。
- 适用性：仅当你本来就要求"用户已装 Codex/有 ChatGPT 订阅"时才有意义；对内嵌媒体服务器场景仍偏重。

### 5.3 推荐方案对比

| 方案 | 依赖体积 | 模型灵活性 | 自定义工具 | 维护成本 | 结论 |
|---|---|---|---|---|---|
| fork 剪枝 codex-rs | 极大（百万行级） | 差（Responses only） | 需改 core | 极高 | 否决 |
| 子进程 codex exec/app-server + dynamic_tools/MCP | 中（外部二进制） | 差 | 好（宿主回调） | 中 | 备选 |
| rig（rig-core 0.40，2026-07 仍活跃更新） | 小（单 crate） | 好：OpenAI/Anthropic/Gemini/Ollama/兼容端点，原生支持 Gemini Flash | `Tool` trait + derive 宏，agent multi-turn 循环内置 | 低 | **推荐**（若想少写代码） |
| 手写 tool-calling loop（async-openai 或裸 reqwest + chat completions tools 参数） | 最小 | 最好（任意 OpenAI-compat base_url） | 完全自控 | 低（~300-500 行：定义 tools JSON schema → 循环：请求→若 finish_reason=tool_calls 则本地 dispatch→把 tool 结果 role=tool 回填→直至文本输出；加步数上限/超时/JSON 修复重试） | **推荐**（刮削场景工具集小而固定，5-10 个 tool：search_tmdb / get_episode_info / rename_file / write_nfo / fetch_artwork 等，循环深度浅，flash 模型完全够用） |

**开发文档建议结论**：媒体刮削 agent runtime 不复用 codex-rs 代码；采用"手写 tool-calling loop（首选，零框架锁定）或 rig-core"，可借鉴 Codex 的三个设计：(1) SQ/EQ 事件流协议（把刮削进度/审批以 EventMsg 风格 JSONL 事件推给前端）；(2) `ToolExecutor` trait 的 spec/exposure/handle 分离（tools crate 单文件即可抄形）；(3) `ModelProviderInfo` 的 provider 配置模型（base_url + env_key + retry/超时字段），照抄字段设计即可支持任意第三方兼容 API。

## Sources
https://github.com/openai/codex
https://raw.githubusercontent.com/openai/codex/main/codex-rs/README.md
https://raw.githubusercontent.com/openai/codex/main/codex-rs/Cargo.toml
https://raw.githubusercontent.com/openai/codex/main/codex-rs/docs/protocol_v1.md
https://raw.githubusercontent.com/openai/codex/main/codex-rs/model-provider-info/src/lib.rs
https://raw.githubusercontent.com/openai/codex/main/codex-rs/tools/src/tool_executor.rs
https://raw.githubusercontent.com/openai/codex/main/codex-rs/protocol/src/auth.rs
https://raw.githubusercontent.com/openai/codex/main/codex-rs/protocol/src/dynamic_tools.rs
https://raw.githubusercontent.com/openai/codex/main/docs/config.md
https://raw.githubusercontent.com/openai/codex/main/codex-rs/responses-api-proxy/README.md
https://github.com/openai/codex/discussions/7782
https://crates.io/crates/codex-protocol
https://crates.io/crates/codex-app-server-sdk
https://crates.io/crates/rig-core