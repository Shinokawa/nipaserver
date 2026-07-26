# 手写 tool-calling loop 的现实复杂度评估（作为方案选型基准）

## 1. OpenAI 兼容端点间 tool calling 的实际差异清单（均有实锤 issue/文档）

### 1.1 Streaming 时 tool_calls 分片拼接 —— 最大的坑区

OpenAI 的流式契约：`delta.tool_calls[]` 中只有第一个 chunk 带 `id` 和 `function.name`，后续 chunk 靠 `index` 字段定位归属，`arguments` 是任意切分的 JSON 字符串片段需逐段拼接。各家对该契约的偏离：

- **Gemini OpenAI 兼容层：完全不发 `index` 字段**。Vercel AI SDK 为此专门把 schema 改成 `index: z.number().nullish()` 并注释 "google does not send index"（opencode #17902）。严格按 OpenAI SDK 逻辑写的解析器会直接抛验证错误。
- **Gemini 3 系列新坑：`thought_signature` 丢失**。多轮 tool call 时若不把 thought_signature 原样传回，Gemini 3 返回 400 "missing a thought_signature"；OpenAI 兼容层的标准 message 结构没有这个字段的位置，openai/codex #7519 就栽在这里。这意味着"纯 OpenAI 格式"的 loop 对 Gemini 3 flash 需要额外的 provider-specific 字段透传。
- **部分实现所有 tool call 的 `index` 恒为 0**（Ollama #15457）或干脆不填（Ollama #7881），并行 tool calls 时第二个调用被合并或静默丢弃。
- **`function.name` 可能出现在后续 chunk 而非首个 chunk**（opencode #24137），或后续 chunk 里 `name: null` 触发严格校验失败（vLLM 场景）。
- **单个 delta 可同时含 `content` + `reasoning_content` + `tool_calls`**，三者需独立累积，无顺序保证。
- 连 Codex 0.64.0 这种成熟客户端都出过"把每个 arguments chunk 当成独立 tool call"的回归 bug（openai/codex #7517）——说明拼接逻辑本身就容易写错。

**正确姿势**：按 `index` 累积、缺 index 时按 id/位置回退；arguments 只拼不解析，直到 finish_reason=tool_calls 或流结束才 parse+校验；不做逐 chunk 严格校验。

**重要缓解**：刮削场景不需要向用户实时展示 token，**可以直接用非流式请求**，绕开上面 80% 的坑。但注意 Qwen 的部分思考模型只支持流式（见下）。

### 1.2 各家专属坑

- **DeepSeek**：
  - 非确定性地把 tool call 以纯文本形式输出到 `content` 字段（`tool_calls` 为 null、finish_reason="stop"），同一会话约 20% 概率触发（DeepSeek-V3 #1244、sglang #17561）→ 需要检测+重试兜底。
  - V4/思考模式要求 `reasoning_content` 必须在多轮中原样回传，否则 400 或静默提前终止（opencode #35689）——标准 OpenAI message 结构没有这个字段。
  - 喂回 tool 结果后偶发返回完全空响应（#1453），重试也复现 → 需要空响应检测。
  - deepseek-reasoner 不支持 `tool_choice`。
- **Qwen/DashScope**：非流式请求必须显式带 `enable_thinking: false`（非标准参数，需 extra_body），否则被拒（openclaw #21114）；QwQ/部分 Qwen3 模型只支持流式；思考内容在 `<think>` 内联时会影响 tool call 解析。
- **GLM/智谱**：官方兼容端点对 message 内容有额外校验（"content 不能为空"报错，zed #37302）；自托管/代理场景 GLM 的 XML 式 tool call 格式解析器 bug 频发。z.ai 另提供 Anthropic 兼容端点作为逃生口。
- **Groq**：兼容性最好的一家。仅 n=1、无 presence_penalty 等小差异；坑在 reasoning 模型上 tool call/JSON mode 必须设 `reasoning_format: parsed/hidden`；强制 tool_choice 下模型不从命会直接 400。
- **finish_reason 语义**：DeepSeek 有非标准值 `insufficient_system_resource`；多家在实际发出 tool call 时仍返回 "stop"（sglang/DeepSeek 场景）→ 不能只依赖 finish_reason 判断是否有 tool call，要同时检查 tool_calls 字段和 content 中的伪 tool call 文本。

### 1.3 Strict JSON schema（与 submit_result 强相关）

- **DeepSeek、Qwen 只支持 `json_object`，不支持 OpenAI 的 `json_schema`/strict**；schema 需注入 prompt，mismatch 率 5-12%（DeepSeek JSON mode 实测）。
- Groq 支持 `json_schema` + `strict: true`（约束解码，比 OpenAI 还严）。
- Gemini 走自己的 responseSchema 机制，经兼容层的支持因模型而异。
- Requesty 测了 244 个模型：不少提供商"接受 json_schema 参数但输出不符合 schema"或返回空/坏 JSON。
- **结论**：submit_result 不能依赖 strict schema，必须自带 serde 反序列化失败 → 把错误喂回模型重试的循环。这本身就是 tool loop 的一部分。

## 2. 需要自己实现的周边设施

- **重试/退避**：指数退避 + jitter（基准 ~500ms、上限 60s、5-7 次），尊重 `Retry-After` 头作为下限。
- **错误分类**：可重试 = 429/500/502/503/504/529/超时/空响应/坏 JSON tool call；不可重试 = 400（除 Gemini thought_signature 类可修复 400）/401/403/quota 型 429（需检查 429 body 是否含 "quota"/"billing" 字样）。
- **限流**：单机媒体服务器场景较简单，一个 semaphore + 每 provider 的 RPM 令牌桶即可；免费档 Gemini/Groq 的 RPM 很低，批量刮削时这是必需品而非可选项。
- **超时**：连接超时 + 首 token 超时 + 总超时分开设；思考模型首 token 可能 30-90s。
- **循环护栏**：max turns（如 15）+ max tokens 预算 + 同 tool 同参数重复调用检测。freeCodeCamp 引用的真实案例：2025 年 Claude Code 递归循环 5 小时烧掉 $16k-50k——护栏不是理论需求。
- **上下文控制**：刮削任务单次会话短（10-20 轮内结束），基本不需要复杂的 context 压缩；token 计数可以用 `chars/4 + 响应里的 usage 字段` 粗估，**不需要引入 tiktoken 类依赖**（跨 5 家 tokenizer 各不同，精确计数本来就不可能，粗估+usage 回读足够）。这是相对通用 agent 明显省掉的一块。
- **并行 tool calls 的响应回填**：Gemini 要求并行调用的所有 tool 结果一次性全部回传，否则报错；每个 tool_call_id 必须有对应 tool message。

## 3. "手写 loop" 的经验教训（2025-2026 实践共识）

- 核心 loop 本身确实简单且与框架内部实现同构："model call → 执行 tools → 回填 → 循环直到无 tool_calls"（Victor Dibia、boot.dev、Inngest 教程一致）。**框架的价值不在 loop，在于消化了上述 provider 怪癖**——而这正是 litellm/AI SDK 常年在修 bug 的地方（它们自己也经常修错）。
- Agent 不会大声失败：模型遇到歧义会"尽力帮忙"地空转重试，所以退出条件、预算和审计日志是第一优先级，不是事后补的。
- 每次 tool 执行独立包裹（独立重试、独立日志），tool 内部错误以字符串形式喂回模型而不是终止 loop——模型自我纠错能力是 loop 可靠性的主要来源。
- 针对"79% 正常 / 21% 输出坏格式"这类非确定性 provider bug，检测+整轮重试比试图容错解析更可靠。

## 4. 结论：真实工作量估计

**"300-500 行"是理想化数字，现实是分层的：**

| 层 | 内容 | 估计 |
|---|---|---|
| 核心 loop + tool trait/注册 + message 类型 | serde 结构体、dispatch、循环护栏 | 300-500 行（这部分确实如预期） |
| HTTP 层：重试/退避/超时/错误分类/限流 | 用 reqwest + backoff/自写 | 200-400 行 |
| **非流式**模式下的 provider 怪癖层 | DeepSeek content 伪 tool call 检测、Qwen enable_thinking、DeepSeek reasoning_content 回传、Gemini thought_signature 透传、submit_result JSON 校验重试 | 200-400 行 |
| 流式支持（如果做） | SSE 解析 + delta 累积 + 各家 index/name 缺失容错 | +300-600 行，且是 bug 密度最高的部分 |

**总计：非流式约 800-1300 行可控 Rust；加流式约 1500-2000 行。** 关键决策是**第一版放弃流式**（刮削是后台任务，无 UI 展示需求），可砍掉整个最易踩坑的层；代价是 Qwen 个别思考模型不可用（选非思考模型即可规避）。

**最容易踩坑的三处**（按实际 issue 密度排序）：
1. 流式 delta 累积（index 缺失/恒 0、name 晚到、arguments 切片）——不做流式即免疫；
2. 思考模型的隐藏协议（DeepSeek reasoning_content 回传、Gemini thought_signature、Qwen enable_thinking、Groq reasoning_format）——每接一家新 provider 都可能撞上，需要留 per-provider 的 extra fields 透传机制（serde `flatten` + `Map<String, Value>` 即可）；
3. "模型没按格式出牌"的兜底（tool call 落在 content 里、空响应、坏 JSON arguments）——必须假设会发生并整轮重试。

**对比结论**：这个工作量（1000 行级、约 1-2 周含测试）仍显著小于引入任何多语言桥接或剪枝大型框架的成本，且刮削场景的裁剪空间（非流式、短会话、固定 5-10 个 tools、无需精确 token 计数）让手写方案比通用 agent 框架的适配面小得多。手写方案成立，但预算应按 ~1000 行而非 300 行做计划，并把 provider 兼容性测试（对 5 家各跑真实 tool-calling 冒烟测试）纳入 CI。


## Sources
https://discuss.ai.google.dev/t/gemini-openai-compatibility-issue-with-tool-call-streaming/59886
https://github.com/openai/codex/issues/7519
https://github.com/BerriAI/litellm/issues/9686
https://github.com/agno-agi/agno/issues/3001
https://github.com/openai/openai-python/issues/2806
https://github.com/deepseek-ai/DeepSeek-V3/issues/1244
https://github.com/deepseek-ai/DeepSeek-V3/issues/1453
https://github.com/sgl-project/sglang/issues/17561
https://github.com/anomalyco/opencode/issues/35689
https://github.com/sgl-project/sglang/issues/15721
https://github.com/zed-industries/zed/issues/37302
https://github.com/openai/codex/issues/7517
https://console.groq.com/docs/openai
https://console.groq.com/docs/tool-use
https://console.groq.com/docs/structured-outputs
https://github.com/anomalyco/opencode/issues/24137
https://github.com/ollama/ollama/issues/7881
https://github.com/ollama/ollama/issues/15457
https://github.com/anomalyco/opencode/issues/17902
https://github.com/BerriAI/litellm/pull/14587
https://github.com/BerriAI/litellm/issues/20711
https://github.com/vllm-project/vllm/issues/16340
https://github.com/openclaw/openclaw/issues/21114
https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope
https://www.alibabacloud.com/help/en/model-studio/deep-thinking
https://www.requesty.ai/blog/structured-outputs-across-llm-providers-the-compatibility-mess
https://www.dataleadsfuture.com/make-microsoft-agent-frameworks-structured-output-work-with-qwen-and-deepseek-models/
https://docs.litellm.ai/docs/completion/json_mode
https://newsletter.victordibia.com/p/the-agent-execution-loop-how-to-build
https://www.freecodecamp.org/news/how-to-build-a-production-safe-agent-loop-from-exit-conditions-to-audit-trails
https://www.inngest.com/docs/ai-patterns/agent-tool-loops
https://help.openai.com/en/articles/5955604-how-can-i-solve-429-too-many-requests-errors
https://www.grizzlypeaksoftware.com/library/llm-api-error-handling-and-retry-patterns-bpk0jmvq
https://www.getmaxim.ai/articles/handle-429-errors-in-production-llm-applications/