# Rust 原生 LLM Agent 框架调研（2026-07 现状）

面向 NipaServer「AI 刮削 agent」场景：嵌入 Rust 进程、任意 OpenAI 兼容端点（Gemini/DeepSeek/Qwen/GLM/Groq，flash 级模型）、5-10 个自定义 tools、multi-turn loop、MIT 兼容、轻量、活跃维护。

---

## 1) rig-core（0xPlaygrounds/rig）— 重点候选

- **版本/活跃度**：`rig-core` **0.40.0**（2026-07-11 发布），仓库 8.0k stars，最近 commit 2026-07-25（调研当天前一天），releases 节奏约每月一个 minor（0.38→0.39→0.40）。累计下载 180 万+。生产用户含 Dria、Nethermind、Neon（app.build V2）、VT Code。
- **License**：**MIT**。
- **依赖重量**：中等偏轻。核心依赖为 reqwest、tokio(rt,sync)、serde/serde_json、schemars、futures、eventsource-stream、thiserror、tracing 等，无重型依赖（vector store、bedrock、candle 等全部拆到独立 crate：rig-qdrant、rig-bedrock 等，不用不引入）。可裁 feature（default = reqwest + derive + rustls）。支持 WASM。
- **Provider 支持**：内置 provider 模块极全：`openai`（Responses API 和 **Completions API 双通道**，`CompletionsClient`）、`gemini`、`deepseek`、`groq`、`moonshot`、`zai`（GLM）、`openrouter`、`ollama`、`anthropic`、`mistral`、`xai`、`together` 等 25+。**自定义 base_url 一等公民**：`Client::builder().api_key(k).base_url(&base).build()`，且 `from_env()` 自动读 `OPENAI_BASE_URL`——即任意 OpenAI 兼容端点直接用 CompletionsClient + base_url 即可，Gemini/DeepSeek 也有原生 adapter 可选。
- **Multi-turn agent loop**：**内置**。`agent.prompt("...").max_turns(20).await`，另有 `default_max_turns()`、`AgentRunner`、hook 系统 v2（tool-call 审批、`Flow::RewriteArgs/RewriteResult/OverrideRequest`）、`agent_run_stepping`、typed errors、mock model 与 VCR cassette 测试。examples 里有 `multi_turn_agent`、`agent_with_tools`、`manual_tool_calls`、`openai_streaming_with_tools_otel` 等 60+ 个。
- **Tool 定义方式**（两种，另有 `#[rig_tool]` 宏）：

  静态 trait 实现（0.40 实际 API，来自官方 `multi_turn_agent` 示例）：
  ```rust
  use rig::tool::Tool;

  #[derive(Deserialize)]
  struct OperationArgs { x: i32, y: i32 }

  struct Add;
  impl Tool for Add {
      const NAME: &'static str = "add";
      type Error = MathError;         // impl std::error::Error
      type Args = OperationArgs;      // serde 反序列化自 LLM JSON
      type Output = i32;              // serde 序列化回 tool result

      fn description(&self) -> String { "Add x and y together".into() }
      fn parameters(&self) -> serde_json::Value {
          json!({ "type": "object", "properties": {
              "x": {"type": "number"}, "y": {"type": "number"} } })
      }
      async fn call(&self, _ctx: &mut rig::tool::ToolContext, args: Self::Args)
          -> Result<Self::Output, Self::Error> { Ok(args.x + args.y) }
  }

  let agent = client.agent("deepseek-chat")
      .preamble("...")
      .tool(Add).tool(Subtract)
      .build();
  let result = agent.prompt("...").max_turns(10).await?;
  ```
  运行时动态 tool（`DynamicTool::new(name, desc, json_schema, |ctx, args| Box::pin(async {...}))`，返回 `ToolOutput::json(...)`）。此外 `rig-derive`（default feature）提供 **`#[rig_tool]` 属性宏**：`#[rig_tool(description = "...")] fn add(a: i32, b: i32) -> Result<i32, ToolExecutionError>` 直接把函数变成 Tool。
- **Streaming tool_calls 容错**：有专门投入——`eventsource-stream` SSE 解析、SSE retry backoff、`gemini_default_api_recovery`/`gemini_stream_kill_token_count` 等示例表明对非标准 provider 行为做过防御；不 stream 时（刮削场景可不 stream）完全没有此问题。

## 2) genai（jeremychone/rust-genai）

- **版本**：稳定 0.5.3，最新 **0.7.0-beta.14**（2026-07-20），仓库 840 stars，最近 push 2026-07-22，活跃。**MIT OR Apache-2.0**。下载 27.7 万。
- **定位**：多 provider **统一 chat 客户端**，不是 agent 框架。原生 adapter：OpenAI、Anthropic、**Gemini（含 tool schema 规范化：自动处理 Gemini 拒收的 JSON-Schema 关键字）**、**DeepSeek**、**Groq**、**Zai/BigModel（GLM）**、Ollama、xAI、Cohere、Together、Fireworks 等；自定义端点走 `ServiceTargetResolver`（可覆写 auth/endpoint/headers）。
- **Tool calling**：支持但**无宏、无 trait**——手写 JSON schema：`Tool::new("get_weather").with_description(...).with_schema(json!({...}))`，响应 `chat_res.into_tool_calls()` → 自己执行 → `ToolResponse::new(call_id, result)` → `chat_req.append_message(...)` 再调用。**没有内置 agent loop**，multi-turn 循环要自己写（正好就是你们说的 300-500 行那种）。有 `with_tool_choice`、streaming tool-use 示例（c21）、0.5 起换了"更健壮的内部 streaming 引擎"。
- **依赖轻**（reqwest/serde/tokio 级别）。缺点：0.7 处于 beta，API 有变动期；tool 生态薄。

## 3) swiftide（bosun-ai）

- **版本**：0.32.1（2025-11-15 发布 crates.io，仓库 push 2026-07-24 仍活跃），723 stars，**MIT**。
- **定位**：确实以 **RAG/indexing pipeline 起家**（loaders/transformers/embedders/vector stores，Qdrant 等），但 `swiftide-agents` 是完整 agent harness：loop + tools + lifecycle hooks + stop conditions + 暂停/恢复 + MCP tools + typed task graphs。
- **Tool 定义最省事**：`#[swiftide::tool(description = "...", param(...))]` 属性宏直接标注 async fn，参数结构体用 `schemars::JsonSchema` 自动生成 schema；也有 `Tool` derive 宏。
- **顾虑**：默认 provider 集合偏 OpenAI/Ollama/Bedrock 系（openai feature 走 async-openai，可配 base_url），agent 概念绑定 `AgentContext`/`ToolExecutor`（面向"跑 shell 命令改代码"场景），对纯 API 刮削略重；自述"heavy development、会 breaking"；发版频率低于 rig。

## 4) 纯客户端 + 手写 loop

- **async-openai（64bit）**：0.41.1（2026-06-18），**MIT**，6.6M 下载，1972 stars，稳定维护。支持自定义 `api_base`；**BYOT（bring-your-own-types）feature** 是杀手锏——`create_byot` 收发 `serde_json::Value`，专治 OpenAI 兼容端点字段不完全一致导致的反序列化崩溃（DeepSeek/GLM/Qwen 常见坑），也可 `extra_body` flatten 扩展。无 agent loop、无 tool 宏（第三方 `openai-func-enums` 提供宏）。约 23K SLoC，依赖轻。
- **openai-api-rs**：10.0.1（2026-04-17），MIT，更简单的 API 面，支持 `OPENAI_API_BASE`/OpenRouter，功能与容错都弱于 async-openai。选这条路线的话 async-openai 明显更优。

## 5) 其他新兴框架

- **llm（graniet/llm）**：1.3.8（2026-04-19），MIT，359 stars，最近 push 2026-06-06。统一 11+ 后端（OpenAI/Claude/Gemini/Ollama…）+ 链式 workflow + 语音 + REST serving + CLI。功能面宽但杂（TTS/STT/serving 都在一个 crate），agent 概念是"共享内存反应式 agent"，与本场景 tool-loop 不对口；维护单人、迭代慢于 rig。
- **agentai（AdamStrojek/rust-agentai）**：0.1.5，**最近 push 2025-09-22，已停滞约 10 个月**，168 stars。底层用 genai，有 `#[toolbox]` 宏，但太早期且不再活跃，排除。
- **autoagents（liquidos-ai/AutoAgents）**：0.4.0（2026-07-08），MIT OR Apache-2.0，716 stars，push 2026-07-24，活跃。基于 Ractor actor 模型的多 agent 框架，ReAct executor、guardrails、telemetry、WASM、mistral-rs/llama.cpp 本地推理。工作区 crate 众多，面向"多 agent 协作系统"，对单一刮削 agent 明显过重。
- **kowalski（yarenty）**：多 crate、Ollama 为中心、Postgres/pgvector 记忆、联邦化，本地优先全栈框架，社区小、方向偏离，排除。

## 6) Block goose 的复用性

- goose 2026 年已捐给 Linux Foundation AAIF（仓库现为 aaif-goose/goose，51.7k stars，push 2026-07-25），**Apache-2.0**（MIT 兼容）。
- 核心 `goose` crate（workspace 内，含 agents/providers/session/OAuth/telemetry）理论上可作嵌入式 runtime，官方也宣传"API to embed anywhere"。
- **但不适合**：① crates.io 上的 `goose` 名字被 tag1consulting 的负载测试框架占用，Block 的 goose 核心 crate 未发布到 crates.io，只能 git 依赖整个大 workspace，版本管理痛苦；② 依赖极重：rmcp（MCP 客户端全家桶）、axum-server、sqlx、oauth2、jsonwebtoken、keyring、tiktoken-rs、minijinja、可选 candle/AWS SDK——单 Cargo.toml 300 行，本质上是"桌面 agent 产品的内脏"，和被否决的 codex 剪枝是同一类问题；③ 抽象围绕 session/extension/MCP，不是干净的库 API。**排除。**

---

## 7) 结论与推荐

**首选：rig-core（0.40，MIT）。** 理由：
1. 唯一同时满足全部硬性条件的框架：MIT、活跃（周更级 commit、月更发版、8k stars、多个生产用户）、依赖中轻量且 feature 可裁、内置 multi-turn loop（`.max_turns(n)`）、tool 注册三种姿势（trait / `DynamicTool` / `#[rig_tool]` 宏）均简洁。
2. Provider 覆盖恰好命中需求：OpenAI **Chat Completions 通道**（`CompletionsClient` + `base_url`，`OPENAI_BASE_URL` env 直通任意兼容端点，覆盖 Qwen/GLM/自建网关），另有 Gemini、DeepSeek、Groq、Zai 原生 adapter，flash 级模型随便换。
3. 附赠对刮削 agent 有实际价值的东西：typed error、tool-call hook（可在 `submit_result` 前做校验/重写）、mock model + VCR 测试（离线测刮削逻辑）、OTel tracing。
4. 风险：0.x 语义版本，minor 版本有 breaking（有 MIGRATING.md）；建议锁定版本、把 rig 隔离在一个 `scraper-agent` 模块后面。

**备选：genai + 自写 ~300 行 loop。** 如果试用 rig 后觉得抽象过多（Agent/hook/AgentRunner 对 5-10 个 tool 的单一任务确实偏大），genai 是最好的"薄客户端"：多 provider 原生 adapter（含 Gemini schema 规范化——这是裸 OpenAI 兼容层容易踩坑的地方）、MIT/Apache 双许可、极轻。tool 无宏但你们本来就打算手写 loop，`Tool::new().with_schema(json!)` + `into_tool_calls()` + `ToolResponse` 的手动流程正是原方案的骨架。相比 async-openai+手写，genai 省掉了对每个 provider 兼容性差异的处理；相比 rig 少一层框架锁定。

**不推荐**：swiftide（RAG/代码执行导向，对本场景过重）、goose（git 依赖 + 超重依赖树）、autoagents/llm/kowalski（方向不对口或过重）、agentai（停滞）。

**一句话决策**：先用 **rig-core** 做原型（一天内可跑通：`CompletionsClient::builder().base_url(...)` + 5 个 `#[rig_tool]` + `.prompt().max_turns(8)`）；若框架税不可接受再退到 genai + 手写 loop，两者迁移成本都低，因为 tool schema 都是 serde_json。

## Sources
https://github.com/0xPlaygrounds/rig
https://www.rig.rs/
https://crates.io/crates/rig-core
https://raw.githubusercontent.com/0xPlaygrounds/rig/main/examples/multi_turn_agent/src/main.rs
https://raw.githubusercontent.com/0xPlaygrounds/rig/main/examples/agent_with_tools/src/main.rs
https://raw.githubusercontent.com/0xPlaygrounds/rig/main/crates/rig-core/Cargo.toml
https://raw.githubusercontent.com/0xPlaygrounds/rig/main/crates/rig-derive/src/lib.rs
https://raw.githubusercontent.com/0xPlaygrounds/rig/main/crates/rig-core/src/providers/openai/client.rs
https://github.com/jeremychone/rust-genai
https://crates.io/crates/genai
https://raw.githubusercontent.com/jeremychone/rust-genai/main/examples/c20-tooluse.rs
https://github.com/bosun-ai/swiftide
https://crates.io/crates/swiftide-agents
https://raw.githubusercontent.com/bosun-ai/swiftide/master/examples/hello_agents.rs
https://github.com/64bit/async-openai
https://crates.io/crates/async-openai
https://crates.io/crates/openai-api-rs
https://github.com/graniet/llm
https://github.com/AdamStrojek/rust-agentai
https://github.com/liquidos-ai/AutoAgents
https://crates.io/crates/autoagents
https://github.com/yarenty/kowalski
https://github.com/block/goose
https://raw.githubusercontent.com/aaif-goose/goose/main/crates/goose/Cargo.toml
https://deepwiki.com/block/goose
https://lib.rs/crates/rig-core
https://docs.rs/genai/latest/genai/chat/index.html