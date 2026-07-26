# pi (badlogic/pi-mono → earendil-works/pi) 调研报告

## 1) 它是什么

- **作者/归属**：Mario Zechner（badlogic，libGDX 作者）。2026 年 4 月他加入 Armin Ronacher 联合创办的 Earendil（公益公司），仓库从 `badlogic/pi-mono` 迁移为 `earendil-works/pi`（旧 URL 仍重定向），核心保持 MIT。
- **语言**：100% TypeScript，npm workspaces monorepo，lockstep 版本，运行时为 Node/Bun（官方发布也提供 Bun 编译的独立二进制）。
- **monorepo 包结构**（当前四个核心包，npm scope 已从 `@mariozechner/*` 改为 `@earendil-works/*`）：
  - `@earendil-works/pi-ai`（packages/ai）：统一多 provider LLM API（流式、tool calling、token/成本追踪、上下文序列化、跨模型 handoff）。
  - `@earendil-works/pi-agent-core`（packages/agent）：Agent 运行时——tool-calling loop、状态管理、事件流（agent_start/turn_start/message_update/tool_execution_* 等）、steering/follow-up 队列、compaction 钩子。
  - `@earendil-works/pi-coding-agent`（packages/coding-agent）：交互式编码 agent CLI（`pi`），含 SDK（`AgentSession`）、RPC/JSON headless 模式、extensions/skills 体系。
  - `@earendil-works/pi-tui`（packages/tui）：差分渲染终端 UI 库。
  - SQLite 会话后端拆为独立包 `@earendil-works/pi-storage-sqlite-node`。Slack/chat 自动化在另一个仓库 `earendil-works/pi-chat`。
- **设计哲学**：极简 + 自我可扩展。默认只有 4 个工具（read/write/edit/bash），系统提示 <1000 token；刻意不内置 MCP、sub-agents、plan mode、权限弹窗、todo、后台 bash——这些都通过 TypeScript extensions 自建。核心 agent loop 本身很小（`agentLoop()` 是一个可直接调用的低层函数：prompts + context + streamFn，循环"LLM 调用 → 执行 tool calls → 把 toolResult 塞回 context → 再调 LLM"，带 `shouldStopAfterTurn`、`beforeToolCall`/`afterToolCall`、`terminate: true` 提前终止等钩子），与我们设想的"手写 300-500 行 loop"是同一个形态，只是打磨过并加了事件流/并行工具执行/steering。

## 2) 模型接入

- `pi-ai` 抽象为 provider（目录+auth）与 API 实现（wire protocol）两层。内置 API 实现：`openai-completions`、`openai-responses`、`anthropic-messages`、`google-generative-ai`、`google-vertex`、`mistral-conversations`、`bedrock-converse-stream`、azure/codex 变体。
- 内置 provider 覆盖极广：OpenAI、Anthropic、Google、**DeepSeek、Groq**、Cerebras、xAI、OpenRouter、Mistral、Together、Fireworks、**ZAI(GLM)、Moonshot/Kimi、MiniMax**、Bedrock、Copilot 等，且明确支持 "**Any OpenAI-compatible API**: Ollama, vLLM, LM Studio, etc."。
- 通过 `createProvider()` 可以指着任意 baseUrl 建自定义 provider（README 给了 Ollama 完整示例），并有 `compat` 标志处理各家 OpenAI 兼容实现的差异（`supportsDeveloperRole`、`supportsReasoningEffort`、`supportsStore`、`supportsUsageInStreaming`、`supportsStrictMode` 等）——这正是我们需要的"任意 OpenAI 兼容端点 + flash 级模型（Gemini/DeepSeek/Qwen/GLM/Groq）"能力，且已内置针对 DeepSeek、Cerebras、xAI 等的兼容性自动检测。注意：库的模型目录**只收录支持 function calling 的模型**。

## 3) Tool 注册机制

- 工具是纯数据+函数对象：`{ name, description, parameters: TypeBox schema, execute(toolCallId, params, signal, onUpdate) }`（`AgentTool`），参数用 TypeBox（re-export 自 pi-ai）定义并自动校验；`execute` 返回 `{ content: [{type:'text'|...}], details }`，抛错即成为 `isError: true` 的 tool result。设置 `agent.state.tools = [...]` 即完成注册。
- 完全与"编码工具"解耦：read/write/edit/bash 只是 coding-agent 包的默认工具；直接用 `pi-agent-core` 时工具列表由你提供，换成 `search_tmdb`/`search_bangumi`/`submit_result` 这类非编程工具是一等公民用法，无需任何 hack。还支持 parallel/sequential 执行、`terminate: true`（例如 `submit_result` 返回后直接终止 loop，正好匹配我们的场景）。

## 4) 被非 TypeScript 宿主（Rust）使用的途径

官方支持三种集成层：
1. **TypeScript SDK**（`createAgentSession` / `Agent` / `agentLoop`）——仅限 Node/Bun 宿主，对 Rust 不直接可用。
2. **`pi --mode json`**：单次 headless 运行，JSON 事件输出。
3. **`pi --mode rpc`**：**stdin/stdout JSONL 协议的常驻子进程**，官方文档明确定位为"embed the agent in other applications... integrate from any language"。命令集：`prompt`（带 id 关联、可带图片）、`steer`、`follow_up`、`abort`、`new_session`、`set_model`、`compact`、`get_state`、`get_messages`、fork 等；事件以 JSON lines 流式输出；严格 LF 分帧。
- **对 Rust 的现实代价**：RPC 模式跑的是完整 coding-agent（自带 read/write/edit/bash 工具、session 文件、extensions 发现机制）。要换成我们的 5-10 个刮削工具，需写 TypeScript extension 注册自定义工具并禁用默认工具，然后 Rust 侧 spawn `pi --mode rpc` 子进程、tokio 管道读写 JSONL。这意味着：**部署时必须携带 Node/Bun 运行时（或 ~90MB+ 的 Bun 独立二进制）**、每个刮削会话一个子进程、跨语言调试两层日志。API key/baseUrl 配置也要从 Rust 侧透传给 pi 的 provider 体系。没有官方 Rust binding/FFI。

## 5) License、活跃度、社区

- **License：MIT**（整个 monorepo）。
- **活跃度**（GitHub API，2026-07-25）：约 **77.8k stars、9.6k forks、79 open issues**，仓库创建于 2025-08，最后 push 2026-07-25——极度活跃，有 Discord 社区、RFC 流程、供应链加固（依赖 pin、shrinkwrap、audit CI）。已商业化托管于 Earendil，但对新贡献者 PR/issue 默认自动关闭（维护者每日复审）。
- **社区评价**：多个 HN 热帖（"Pi – A minimal terminal coding harness"、"What I learned building an opinionated and minimal coding agent"）；被称赞 "does not flicker and is VERY hackable"、树形会话是 SOTA。火的原因：对 Claude Code 复杂化的反叛叙事 + libGDX 作者信誉 + 成为 **OpenClaw**（一周 14.5 万 star）的底层引擎 + Terminal-Bench 上以极简配置打赢重型 harness。域名 pi.dev / shittycodingagent.ai。

## 6) 对 NipaServer 场景的适用性结论

**不建议整机嵌入，但建议"抄 pi-ai/pi-agent-core 的设计"或仅在允许 Node 依赖时用 RPC 模式。**

- **反对整机集成的理由**：pi 是 TypeScript 生态，嵌入 Rust 只能走子进程 + JSONL RPC，代价是给一个 Rust 媒体服务器引入 Node/Bun 运行时依赖（对家用 NAS/Docker 单镜像分发是明显负担），且 RPC 模式承载的是 coding-agent 全套（session、extensions、默认编码工具），我们要用 extension 剥掉大半功能——为一个 300-500 行就能写好的 loop 引入一整个第二运行时，得不偿失。这与否决 codex 剪枝的逻辑一致，只是量级小得多。
- **它证明了什么**：pi 的核心恰好验证了"手写小 loop 是对的"——它的 agent loop 本身就是极简循环 + 钩子，复杂度都在多 provider 兼容层（pi-ai）。**我们真正该借鉴的清单**：(a) provider/API 两层抽象与 `compat` 标志集（supportsDeveloperRole/reasoning_effort/stream usage/strict mode 是 OpenAI 兼容端点实测最常踩的坑，pi 已替我们列全）；(b) tool = schema + execute + 抛错即 isError 的约定；(c) `terminate: true`（submit_result 直接收尾）与 `shouldStopAfterTurn`（限制最大轮数/预算）；(d) 事件流粒度设计（turn/message/tool_execution）便于 WebUI 展示刮削过程。
- **Rust 侧落地建议**：用 `async-openai`（或直接 reqwest + serde 手写 Chat Completions 请求）实现 openai-completions 单协议 loop，把 Gemini/DeepSeek/Qwen/GLM/Groq 全部走各家的 OpenAI 兼容端点，参照 pi 的 compat 标志做少量端点差异开关。工作量与"手写 300-500 行"预估一致，零新运行时，MIT 无碍。
- **保留选项**：若未来要做"用户可用订阅账号（Claude/ChatGPT OAuth）刮削"或复杂 agent 特性，`pi --mode rpc` 子进程是文档完善、协议稳定（HN 上已有其他项目仿制此协议）的现成后门，可作为可选外部 provider 而非核心依赖。

## Sources
https://github.com/badlogic/pi-mono
https://github.com/earendil-works/pi
https://raw.githubusercontent.com/badlogic/pi-mono/main/README.md
https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/ai/README.md
https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/agent/README.md
https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/rpc.md
https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/sdk.md
https://pi.dev/docs/latest/rpc
https://news.ycombinator.com/item?id=47143754
https://news.ycombinator.com/item?id=46844822
https://news.ycombinator.com/item?id=46629341
https://www.npmjs.com/package/@mariozechner/pi-coding-agent
https://explainx.ai/blog/pi-minimal-agent-harness-mario-zechner-guide-2026
https://shivamagarwal7.medium.com/agentic-ai-pi-anatomy-of-a-minimal-coding-agent-powering-openclaw-5ecd4dd6b440
https://dev.to/arshtechpro/pi-the-open-source-ai-coding-agent-you-probably-havent-tried-yet-2h0h
https://pyshine.com/Pi-Mono-Full-Stack-AI-Agent-Toolkit/