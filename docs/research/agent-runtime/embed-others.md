# 轻量 Agent 框架调研：是否值得为 NipaServer 引入跨语言集成

## 1) HuggingFace smolagents（Python）

- **规模/License**：核心 agent 逻辑约 1,000 行（agents.py），Apache-2.0（MIT 兼容），HF 官方活跃维护。安装用 extras 拆分：`smolagents[openai]`（openai SDK）、`[litellm]`、`[toolkit]`、`[mcp]` 等；核心包本身依赖不重，但实际可用配置（openai + toolkit）会拉入 openai/requests/jinja2 等一批 Python 依赖。
- **CodeAgent vs ToolCallingAgent**：
  - **CodeAgent**（主打）：LLM 把"动作"写成 Python 代码片段执行，工具即 Python 函数。强大但带来代码执行安全问题——官方建议 E2B/Modal/Docker/Blaxel 沙箱。对刮削这种"查 TMDB → 提交结果"的固定小工具集场景是**过度设计且引入攻击面**。
  - **ToolCallingAgent**：经典 JSON tool-calling loop，本质上就是你们打算手写的那 300-500 行东西的 Python 版。
- **OpenAI 兼容端点**：`OpenAIServerModel`（新版别名 `OpenAIModel`）支持任意 `api_base`，实测覆盖 Gemini OpenAI 端点、Together、LM Studio、Ollama 等，完全满足 Gemini/DeepSeek/Qwen/GLM/Groq 需求。
- **嵌入非 Python 宿主的代价**：smolagents 没有任何 serve/RPC 形态，嵌入 Rust 只能：(a) 子进程跑 Python 脚本（要求用户机器有 Python 3.10+ 环境，或你打包 ~几十 MB 的 Python 运行时）；(b) PyO3 嵌入（见第 6 点，单二进制分发基本不可行）。**为一个 1,000 行、其中你只需要 ToolCallingAgent 那几百行的库，引入整套 Python 运行时，性价比为负。**

## 2) mini-swe-agent（普林斯顿/斯坦福）

- **设计**：agent 本体约 100 行 Python（+环境/模型/入口共约 200 行），MIT license。设计哲学极有参考价值：**bash-only（不用 tool-calling API，动作是 shell 命令）、`subprocess.run` 无状态执行、完全线性的消息历史**。SWE-bench verified >74%，证明"能力在模型不在脚手架"。
- **可复用性**：它是**范式证明而非可复用库**——核心依赖 litellm（2026 年还出过 1.82.7/1.82.8 供应链投毒事件，PR #794 才排除），且 bash-only 设计与你们"5-10 个结构化 tools（search_tmdb/submit_result）"的需求方向相反：媒体刮削需要结构化 JSON 结果，不是 shell 自由发挥。**结论：读它的源码来指导手写 loop（linear history、simple while loop），但不集成它。**

## 3) OpenAI Agents SDK 与 Claude Agent SDK

- **OpenAI Agents SDK**（Python 27k+ stars / TS 3.1k+）：**MIT license**，不绑定 OpenAI 模型——可用 `OpenAIChatCompletionsModel` + 自定义 `AsyncOpenAI(base_url=...)` 指向任意 Chat Completions 端点，或 LiteLLM 扩展覆盖 100+ 提供商。但默认路径（Responses API、tracing 上报 OpenAI dashboard）都拉向 OpenAI 平台；第三方模型走的是"best-effort beta adapter"。且它是 Python/TS，嵌入 Rust 的代价与 smolagents 相同。对你们的需求（单一 agent、固定工具集），Handoffs/Guardrails/Sessions/Tracing 全是死重。
- **Claude Agent SDK**：**专有 license（受 Anthropic Commercial ToS 约束），非 MIT 兼容**；构建在 Claude Code 之上，模型只支持 Claude（Anthropic API/Bedrock/Vertex/Azure Foundry 上的 Claude），**不支持 Gemini/DeepSeek/Qwen 等任意 OpenAI 兼容端点**。直接排除。

## 4) Vercel AI SDK / mastra（TS）简评

- **Vercel AI SDK**：Apache-2.0。tool 定义用 zod schema + execute 函数，AI SDK 6 的 `ToolLoopAgent` 就是现成的 tool-execution loop（默认 20 步上限，支持 needsApproval）。设计干净，是 TS 生态里最贴合你们需求形态的。
- **mastra**：核心 Apache-2.0（ee/ 目录为企业 license），24k+ stars，YC 公司。全家桶框架（memory/workflow/vector/dev UI），对你们是大炮打蚊子。
- 两者共同问题：需要 Node.js 运行时。作为 sidecar 意味着分发 Node 或用 bun/deno 编译出 ~50-90 MB 的独立可执行文件——对"单二进制 + Flutter 客户端回流"是显著负担。

## 5) MCP 驱动的极简 agent

- **nanobot（nanobot-ai/obot 团队）**：Go + Svelte，Apache-2.0，定位是**独立部署的 MCP host**（YAML 定义 agent，HTTP 服务于 :8080），OpenAI/Anthropic 内置 provider。作为 sidecar 可行但意味着多分发一个 Go 二进制 + 一套 YAML 运维面，且它的价值（MCP-UI、多 agent 编排）你们用不上。注意与港大 HKUDS 的同名 Python "Nanobot"（~30k stars）是两个项目。
- **HF tiny agents**：Python 版在 huggingface_hub 里（`MCPClient` + ~70 行 Agent while-loop），JS 版 `@huggingface/tiny-agents` MIT。它最大的价值同样是**教学性**：官方博客明说 agent 就是"MCP client 之上的一个 while 循环"。这反向印证了手写 loop 的合理性——如果连 HF 都认为 70 行够用，你们的 300-500 行 Rust 版毫无不妥。
- MCP 本身对你们也是可选项：tools 全部是进程内函数（查 TMDB API、写数据库），没有跨进程工具共享需求，引入 MCP 只是加一层 JSON-RPC 间接。

## 6) 跨语言集成模式对比（针对单二进制 + flutter_rust_bridge 回流）

| 模式 | 分发 | 体积 | 运维/复杂度 |
|---|---|---|---|
| **子进程 + stdio JSONL**（Python/Node agent） | 破坏单二进制：要么要求用户装 Python/Node，要么打包运行时（Python embeddable ~15-30MB 且平台各异；bun/deno compile 50-90MB）| 大 | 进程生命周期管理、崩溃恢复、跨平台路径/信号处理；**在 Flutter 移动端（iOS）子进程直接被平台禁止** |
| **本地 HTTP sidecar** | 同上再加端口占用/防火墙/启动顺序问题 | 大 | 最重；适合服务器部署，完全不适合回流进 Flutter 客户端 |
| **PyO3 嵌入 Python** | 官方明确**没有一等静态链接 libpython 支持**（issue #416），Windows 基本不可行；PyOxidizer 已近弃维护 | 大且脆弱 | 与 flutter_rust_bridge 叠加时（iOS 静态库、Android NDK 交叉编译 CPython）是噩梦级别 |
| **rquickjs 嵌入 JS** | 真单二进制，`embed!` 宏编译期打包 JS 为字节码 | 引擎仅 ~1MB 开销 | 最轻的跨语言方案，但没有现成 JS agent 框架能在 QuickJS（无 Node API、无 fetch）里直接跑——Vercel AI SDK 依赖 fetch/Node，需自己 polyfill，等于把工作量换了个地方 |
| **deno_core (V8)** | 二进制 +几十 MB，bare-bones（无 Node std/模块解析），社区反馈集成"问题缠身" | 很大 | 高 |

结论：任何跨语言模式的最低代价（rquickjs）都仍需你自己写 host 侧的 HTTP/tool 桥接，工作量不小于直接用 Rust 写 loop；其余模式直接与"单二进制 + Flutter 回流"目标冲突（iOS 上子进程/sidecar 均不可行）。

## 7) 最终结论

**没有任何一个非 Rust 方案比"Rust 原生库或手写 loop"更适合。** 理由汇总：

1. 这些框架的核心价值（smolagents ~1,000 行、mini-swe-agent 100 行、tiny agents 70 行）恰恰证明 tool-calling loop 本身就是小问题——它们自己的卖点就是"agent = while 循环 + tool dispatch"。为几百行逻辑引入 Python/Node 运行时、破坏单二进制分发、堵死 Flutter 移动端回流路径，是净亏损。
2. Claude Agent SDK 因专有 license + 只支持 Claude 直接出局；OpenAI Agents SDK 虽 MIT 但偏向 OpenAI 平台且是 Python/TS；nanobot 是独立 Go 服务，运维面额外。
3. **推荐路径**：
   - **首选：手写 300-500 行 Rust tool-calling loop**，底层 HTTP 用 `async-openai` 或 `genai` crate（或直接 reqwest + serde 手拼 Chat Completions 请求，兼容性最可控——Gemini/GLM 的 OpenAI 兼容层各有怪癖，薄封装反而好打补丁）。借鉴 mini-swe-agent 的 linear history 和 tiny agents 的 while-loop 结构。
   - **备选：Rust 原生 [rig](https://github.com/0xPlaygrounds/rig)**（0xPlaygrounds，MIT，7.6k+ stars，2026 年活跃）：`rig-core` + `rig-agent` 提供类型安全的 `.tool()` 注册、20+ provider（含 OpenAI/Gemini/Groq/DeepSeek/Ollama，各自 feature flag 隔离）、WASM 兼容。若不想维护 provider 兼容性细节和 streaming/retry 逻辑，rig 是唯一同时满足"嵌入进程、MIT、OpenAI 兼容、活跃维护"的现成方案。它的风险仅是 API 尚在快速演进（rig-core 0.29.x）。
   - 设计上采纳 mini-swe-agent 的教训反面：你们的场景**应该**用结构化 tool-calling（submit_result 需要严格 schema），而非 bash-only。flash 级模型（Gemini Flash/GLM-4-Flash/Groq Llama）的原生 function calling 已足够可靠。


## Sources
https://github.com/huggingface/smolagents
https://huggingface.co/docs/smolagents/en/index
https://huggingface.co/blog/smolagents
https://huggingface.co/docs/smolagents/v1.8.1/en/reference/models
https://github.com/SWE-agent/mini-swe-agent
https://mini-swe-agent.com/latest/
https://docs.litellm.ai/docs/projects/mini-swe-agent
https://openai.github.io/openai-agents-python/models/
https://docs.litellm.ai/docs/tutorials/openai_agents_sdk
https://platform.claude.com/docs/en/agent-sdk/overview
https://github.com/anthropics/claude-agent-sdk-python/blob/main/LICENSE
https://github.com/vercel/ai/blob/main/LICENSE
https://vercel.com/blog/ai-sdk-6
https://github.com/mastra-ai/mastra/blob/main/LICENSE.md
https://mastra.ai/
https://github.com/nanobot-ai/nanobot
https://obot.ai/blog/introducing-nanobot-a-new-framework-for-turning-mcp-servers-into-ai-agents/
https://huggingface.co/blog/python-tiny-agents
https://huggingface.co/docs/huggingface.js/en/tiny-agents/README
https://pyo3.rs/main/building-and-distribution
https://github.com/PyO3/pyo3/discussions/4102
https://docs.rs/rquickjs/latest/rquickjs/index.html
https://docs.rs/deno_core
https://github.com/denoland/deno/discussions/21968
https://github.com/0xPlaygrounds/rig
https://www.rig.rs/
https://lib.rs/crates/rig-core