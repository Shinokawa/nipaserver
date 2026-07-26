# 评审结论

## 最终排序与推荐

**排序：手写 loop > rig-core > genai(作为手写方案的底层可选件) > pi(仅 RPC 后门/设计借鉴) > 跨语言方案(排除)**

### 主推荐：手写 Rust tool-calling loop（维持现有决策，但修正预算与设计要求）

三个硬约束逐一检验后，只有 Rust 进程内方案存活，而在 Rust 进程内方案中手写 loop 综合最优：

1. **单二进制分发**：pi 需要 Node/Bun 运行时（Bun 独立二进制 ~90MB）+ 常驻子进程；Python 系（smolagents/OpenAI Agents SDK）需要打包 Python 运行时且 PyO3 无一等静态链接支持；nanobot 是独立 Go sidecar。全部与"rust-embed 单文件分发到家用 NAS/Docker"冲突。手写 loop 与 rig/genai 均为纯 crate，零新增运行时。
2. **回流 Flutter 客户端 via flutter_rust_bridge**：这是最强的排他约束——iOS 禁止 spawn 子进程，所有"子进程/sidecar"形态（pi RPC、Python、Node、Go）在移动端直接不可行；PyO3 叠加 iOS 静态库/Android NDK 交叉编译是噩梦级别。刮削 agent 若要在客户端复用，runtime 必须是可交叉编译的纯 Rust 代码。手写 loop（reqwest+serde）依赖树最小，交叉编译风险最低；rig-core 理论可行（支持 WASM、rustls）但依赖面更大。
3. **flash 模型 OpenAI 兼容端点**：handwritten-baseline 报告的怪癖清单（DeepSeek ~20% 概率 content 伪 tool call、reasoning_content 强制回传、Qwen enable_thinking 非标参数、Gemini thought_signature、finish_reason 语义不一致、DeepSeek/Qwen 不支持 json_schema strict）说明**真正的工作量在 provider 兼容层，不在 loop 本身**。关键洞察是：这些坑无论选什么框架都逃不掉——rig/genai 也未证实覆盖 DeepSeek 伪 tool call 检测、空响应重试这类兜底；而框架会让"per-provider extra 字段透传 + 整轮重试"这类补丁隔着一层抽象打。薄封装（reqwest + serde flatten）反而最好打补丁。此外**第一版放弃 LLM token 流式**（刮削是后台任务，推给前端的是刮削进度事件而非 token 流）可直接砍掉 bug 密度最高的 streaming delta 拼接层，把总量控制在 ~800–1300 行。

四份报告有一个罕见的共识：pi 报告承认"pi 的核心恰好验证了手写小 loop 是对的"；embed-others 报告指出 HF tiny agents 官方自己说 agent 就是"MCP client 之上的一个 while 循环"；handwritten-baseline 确认核心 loop 300–500 行成立。tool-calling loop 本身已不是技术风险点，风险点全部在兼容层，而兼容层的裁剪空间（非流式、短会话、固定 5–10 tools、粗估 token）恰恰是手写方案独有的优势。

**预算修正**：按 ~1000–1300 行（非流式）+ 1–2 周含测试排期，而非 300–500 行；把 5 家 provider 的真实 tool-calling 冒烟测试（每家 ~20 文件 fixture）纳入 CI——这与 2.2 节已有的 fixture 回归集要求天然合并。

### 备选：rig-core 0.40（MIT）

唯一同时满足全部硬条件的现成框架：MIT、月更发版、`CompletionsClient` + `base_url`/`OPENAI_BASE_URL` 直通任意兼容端点、Gemini/DeepSeek/Groq/Zai(GLM) 原生 adapter、`#[rig_tool]` 宏 + `.max_turns(n)` 内置 loop、mock model/VCR 离线测试。适用场景：(a) 一天内快速原型验证刮削 prompt 与工具设计；(b) 团队试用后认为 provider 兼容层维护成本超预期时整体切换。风险：0.x 每 minor 有 breaking，需锁版本并隔离在 `scraper-agent` 模块后。**两个方向迁移成本都低**——tool schema 双方都是 serde_json，先手写、撞墙再上 rig 完全可行。

### 明确排除

- **pi 整机嵌入**：为 300–500 行 loop 引入第二运行时，与否决 codex 剪枝同构，且 iOS 回流路径直接堵死。但应**抄它的设计**：provider `compat` 标志集（supportsDeveloperRole/reasoning_effort/stream usage/strict mode）、`terminate: true`（submit_result 返回即终止 loop）、`shouldStopAfterTurn` 护栏、turn/tool_execution 事件粒度（与 SQ/EQ 事件流设计互补）。`pi --mode rpc` 仅保留为未来"订阅账号刮削"的可选外部 provider，不进核心。
- **全部跨语言方案**（smolagents/OpenAI Agents SDK/Vercel AI SDK/mastra/nanobot/rquickjs 嵌入）：最低代价的 rquickjs 也需自写 host 桥接，工作量不小于直接写 Rust loop；其余全部破坏单二进制或移动端回流。Claude Agent SDK 因专有 license + 仅支持 Claude 双重出局。
- **goose**：Apache-2.0 虽兼容，但核心 crate 不发 crates.io、依赖极重（rmcp/axum/sqlx/oauth2/keyring），是"桌面产品内脏"，与 codex 剪枝同类问题。
- **swiftide/autoagents/llm/agentai**：方向不对口、过重或已停滞。

## 对比表

| 方案 | 嵌入方式 | 模型灵活性（flash 级 OpenAI 兼容端点） | tool 注册 | 体积/分发影响 | 维护风险 | License | 结论 |
|---|---|---|---|---|---|---|---|
| **手写 loop（reqwest/serde，底层可选 genai 或 async-openai BYOT）** | 进程内纯 Rust，可经 flutter_rust_bridge 交叉编译回流客户端 | 完全可控：单一 Chat Completions 协议 + per-provider compat 开关 + serde flatten 透传 extra 字段（reasoning_content/thought_signature/enable_thinking） | 自定义 `ToolExecutor` trait（spec/handle 分离），5–10 个工具手写 schema 无负担 | 零新增依赖，单二进制无影响 | 自担 provider 怪癖（~800–1300 行非流式，1–2 周含测试）；但怪癖兜底无论选谁都要写 | 自有代码 MIT | **主推荐**：三个硬约束全满足，裁剪空间最大，零锁定 |
| **rig-core 0.40（Rust 原生框架代表）** | 进程内 crate，支持 WASM/rustls，理论可回流客户端 | 优：`CompletionsClient`+`base_url` 直通任意兼容端点，另有 Gemini/DeepSeek/Groq/Zai 原生 adapter | 优：`Tool` trait / `DynamicTool` / `#[rig_tool]` 宏三种，`.max_turns(n)` 内置 loop + hook | 中等偏轻（feature 可裁），依赖树大于手写 | 0.x 每 minor 有 breaking（有 MIGRATING.md）；怪癖兜底覆盖度未验证，仍需自补 | MIT | **备选**：原型验证首选；手写撞墙时的整体退路 |
| **genai 0.5/0.7-beta** | 进程内 crate | 良：多家原生 adapter，含 Gemini schema 规范化；自定义端点走 ServiceTargetResolver | 手写 JSON schema，无宏无 trait，**无内置 loop** | 极轻 | 0.7 beta API 变动期；tool 生态薄 | MIT/Apache-2.0 | 不独立成方案，作为方案一的底层薄客户端选项 |
| **pi（TS，earendil-works）** | 仅 `pi --mode rpc` 子进程 + JSONL，需 Node/Bun 运行时；无 Rust binding | 极优：内置全部目标厂商 + compat 标志自动检测 | 需写 TS extension 注册工具并禁用默认编码工具 | Bun 独立二进制 ~90MB，破坏单二进制；**iOS 禁子进程，回流路径堵死** | 极活跃（77.8k stars）但对本项目是第二运行时 | MIT | 不集成；**抄设计**（compat 标志集、terminate:true、事件粒度）；RPC 留作未来可选外部 provider |
| **跨语言方案（smolagents/OpenAI Agents SDK/Vercel AI SDK/nanobot/PyO3/rquickjs 等）** | 子进程/sidecar/嵌入解释器，均需第二运行时或大量桥接 | 各自尚可，但与嵌入代价不成比例 | 各异 | Python ~15–30MB+ 且 PyO3 无静态链接；Node/Bun 50–90MB；均破坏单二进制与移动端回流 | 供应链风险（litellm 投毒事件）、跨语言双层调试 | 混杂（Claude Agent SDK 专有直接出局） | **全部排除**：核心价值（几百行 while 循环）不值运行时代价 |

## 文档 2.1 节修改建议

对 `/Users/sakiko/Desktop/nipaserver/docs/01-开发文档.md` 2.1 节的修改建议（现有决策方向正确，主要是修正预算、补充兼容层设计、更新借鉴对象）：

1. **修正工作量表述（第 46 行）**：将"约 300–500 行核心代码"改为分层表述——"核心 loop + tool trait + 消息类型约 300–500 行；加上 HTTP 重试/退避/错误分类/限流（200–400 行）与 provider 怪癖层（200–400 行），**第一版（非流式）总计约 800–1300 行、1–2 周含测试**"。避免按 300 行做排期。

2. **新增决策点：v1 LLM 请求非流式**。刮削是后台任务，推给前端的 SSE/WebSocket 是刮削进度事件（SQ/EQ），不是 LLM token 流，两者解耦。非流式可整体绕开 streaming delta 拼接层（index 缺失/恒 0、name 晚到、arguments 切片——各家 issue 密度最高的坑区）。相应地，**伪代码第 58 行删去"拼接流式 arguments 分片"**（该句描述的是流式坑，与非流式决策矛盾），改为"容错：JSON 修复、检测 content 中伪 tool call、整轮重试"。代价注明：Qwen 个别仅支持流式的思考模型不可用，选非思考 flash 模型规避。

3. **伪代码第 57 行修正**：`if finish_reason == "tool_calls"` 不可靠——DeepSeek/sglang 等在实际发出 tool call 时仍返回 "stop"，DeepSeek 还有非标值 `insufficient_system_resource`。应改为"同时检查 `tool_calls` 字段非空 + content 中伪 tool call 文本检测"。

4. **新增小节：provider 兼容层设计要求**（这是手写方案的真实工作量所在）：
   - per-provider compat 标志集（借鉴 pi-ai 的 supportsDeveloperRole/supportsReasoningEffort/supportsUsageInStreaming/supportsStrictMode）；
   - extra 字段透传机制（serde `flatten` + `Map<String, Value>`）：DeepSeek `reasoning_content` 多轮回传、Gemini 3 `thought_signature`、Qwen `enable_thinking: false`、Groq `reasoning_format`；
   - 兜底三件套：DeepSeek ~20% 概率 tool call 落入 content 的检测+整轮重试、空响应检测、坏 JSON arguments 喂回模型自纠错；
   - `submit_result` 不得依赖 `json_schema`/strict（DeepSeek/Qwen 只支持 json_object）：serde 反序列化失败 → 错误作为 tool result 喂回重试，作为 loop 的一部分。

5. **循环护栏升格为一等需求**：max turns（现有 N=16）之外补 token 预算上限 + 同 tool 同参数重复调用检测 + 审计日志；并行 tool calls 的结果必须按 tool_call_id 一次性全部回填（Gemini 硬性要求）。

6. **借鉴对象从"codex 三个设计"扩为"codex + pi 五个设计"**：保留现有三条，新增 (a) pi 的 `terminate: true` 语义——`submit_result` 执行后直接终止 loop，与刮削收尾天然匹配；(b) pi 的 compat 标志集（见第 4 条）。标注 pi 为 MIT、仅抄设计不引入运行时。

7. **备选段（第 64 行）更新**：
   - rig 信息具体化：`rig-core 0.40（MIT，月更）：CompletionsClient + base_url 直通任意兼容端点，#[rig_tool] 宏，.prompt().max_turns(n) 内置 loop，mock model/VCR 离线测试`；注明双向迁移成本低（tool schema 均为 serde_json），rig 也可用作一天级原型验证；风险为 0.x breaking，需锁版本并隔离在 scraper-agent 模块后；
   - 补充：底层 HTTP 客户端可选 `genai`（多 provider 原生 adapter、含 Gemini schema 规范化，MIT/Apache 双许可）或 `async-openai`（BYOT feature 专治兼容端点反序列化崩溃），也可直接 reqwest+serde 手拼（兼容性最可控）；
   - 补一句排除记录："已评估并排除：pi 等 TS/Python 方案（第二运行时破坏单二进制、iOS 禁子进程堵死 flutter_rust_bridge 回流）、goose（不发 crates.io、依赖极重）、swiftide/autoagents 等（过重或方向不对口）"，避免未来重复调研。

8. **与 2.2 节联动**：2.2 已要求"每个预设模型 ~20 文件 fixture 回归集"，建议在 2.1 明确该回归集需覆盖真实 tool-calling 冒烟测试（5 家 provider 各跑一遍），纳入 CI。