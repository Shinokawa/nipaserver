# 媒体刮削 Agent 低成本 LLM 方案调研（2026-07）

场景假设：可靠 tool calling、读文件名/ffprobe/字幕片段推断影视作品、单任务几轮到十几轮工具调用、不需要深度推理。上下文量小（每轮几 K tokens），成本主要由轮数 × 输入重复（对话历史+工具定义）决定 → **prompt caching / 缓存命中价** 对这类 agent 影响很大。

---

## 1) 各家 flash/mini 级模型现状与价格（$ / 1M tokens，输入/输出）

### OpenAI
- **GPT-5 mini**（2025-08 发布）：$0.25 → 现最低 $0.125 输入 / $1.00 输出，400K 上下文。
- **GPT-5 nano**：$0.05 / $0.40，400K 上下文。
- **GPT-5.4 mini**（2026-03-17 发布）：$0.75 / $4.50，cached input $0.075；支持 tool calling / computer use。
- **GPT-5.4 nano**：$0.20 / $1.25，cached input $0.02；定位分类/抽取/高频子 agent 任务，API only。
- 结论：老一代 GPT-5 mini/nano 极便宜且仍在服务；5.4 代能力更强但涨价 3–4 倍。刮削场景 GPT-5 mini 或 5.4 nano 足够。
- 来源：pricepertoken.com（gpt-5-mini / gpt-5-nano 页）、tokencost.app GPT-5.4 对比、cloudzero/benchlm。

### Google Gemini
- **Gemini 2.5 Flash-Lite**：$0.10 / $0.40（最便宜，1M 上下文，已进入 legacy）。
- **Gemini 2.5 Flash**：$0.30 / $2.50（legacy 中端）。
- **Gemini 3.1 Flash-Lite**（2026-05-07 GA）：$0.25 / $1.50。
- **Gemini 3 Flash**（preview→主推）：约 $0.50 输入档。
- 2026-06-01 起 Gemini 2.0 Flash/Flash-Lite 弃用；2026-04-01 起 Pro 系列免费档取消。
- 来源：pricepertoken.com、tldl.io google pricing、metacto、aipricing.guru。

### Anthropic
- **Claude Haiku 4.5**（`claude-haiku-4-5`）：$1.00 / $5.00，200K 上下文，64K 最大输出。tool calling 完整支持（含 tool runner、strict tool use、prompt caching，缓存读 ~0.1×）。是本清单里最贵的"小模型"，但工具调用可靠性口碑好。注意：走 Anthropic 原生 Messages API，**不提供官方 OpenAI 兼容 endpoint**（社区有 LiteLLM 等转换层）。
- 来源：Anthropic 官方定价（platform.claude.com/docs/en/pricing）。

### DeepSeek
- 2026-07-24 起 `deepseek-chat`/`deepseek-reasoner` 旧名退役，映射到 **V4-Flash**（non-thinking / thinking 模式）。
- **DeepSeek-V4-Flash**：$0.14 输入（cache miss）/ $0.0028（cache hit！）/ $0.28 输出，1M 上下文。
- **DeepSeek-V4-Pro**：促销价 $0.435 / $0.87（cache hit $0.0036），原价 $1.74/$3.48。
- 自动前缀磁盘缓存（无需手动标记），对多轮 agent 反复重发工具定义+历史极其友好——cache hit 价近乎免费。
- Function calling 无附加费；endpoint `api.deepseek.com`，**OpenAI 兼容**。注册送 5M tokens 免费额度（30 天有效）。
- 历史注意点：DeepSeek 早期版本 function calling 稳定性一般（有循环调用问题），V4 代需实测验证，但价格优势巨大。
- 来源：deepseek.ai/pricing、nxcode.io、pricepertoken.com。

### Qwen（阿里 Model Studio）
- **Qwen-Flash**：$0.05 / $0.40 起（按输入长度分档计价，长 prompt 涨价）。
- **Qwen-Plus**：约 $0.26–0.40 输入 / $0.78–1.20 输出，1M 上下文。
- 国际站（新加坡）新账号每个可用模型送 100 万 tokens（90 天有效，per-model 额度）；**2026-04-15 起旧开发者免费档取消**。北京 endpoint 便宜 60–70% 但无免费额度。
- Batch 5 折、context caching 折扣（两者不可叠加）。DashScope 提供 OpenAI 兼容 endpoint（`dashscope.aliyuncs.com/compatible-mode/v1`）。
- 来源：pricepertoken.com/provider/qwen、benchlm.ai/alibaba、developer.puter.com。

### GLM / 智谱（Z.ai）
- **GLM-4.7**：官方 $0.60 / $2.20（cached input $0.11）；OpenRouter 上 $0.40 / $1.75，204.8K 上下文。
- **GLM-4.7 Flash / GLM-4.5 Flash：API 直接免费**（非试用），约 1000 req/天限额，无需绑卡。
- Coding Plan 订阅制（约 $10 起/月，Lite/Pro/Max 分档），主要面向 coding agent 场景，API 也走 OpenAI 兼容格式。
- 来源：openrouter.ai/z-ai/glm-4.7、felloai.com/glm-pricing、tokenmix.ai。

---

## 2) 免费/低价方案现实可用性

| 方案 | 免费内容 | 限制 / 风险 |
|---|---|---|
| **Gemini API 免费档** | Gemini 3 Flash：10 RPM / 250K TPM / **1500 req/天**；3.1 Flash-Lite：15 RPM。无需绑卡，AI Studio 拿 key 即用。支持 tool calling。 | Pro 模型 2026-04 起免费档取消；限额按 GCP project 计（多 key 不叠加）；官方声明限额"不保证"，高峰期实测可能远低于纸面值；免费档数据会被用于训练改进。**对个人自用刮削 agent（一晚扫几百个文件）完全够用，是最强免费选项。** |
| **OpenRouter :free 模型** | ~25–29 个免费模型，$0/token；20 RPM 恒定 + 未充值 50 req/天 / 充过 $10 后 1000 req/天。 | 免费模型名单随时变动、高峰限流严重、可能被下线/换路由；当前 DeepSeek/Gemini/Mistral 在 OpenRouter 上均无 $0 模型。适合兜底/测试，不适合做默认依赖。 |
| **Groq** | 免费无需绑卡：30 RPM / 6K–30K TPM / 1000–14.4K req/天（按模型）。OpenAI 兼容 endpoint `api.groq.com/openai/v1`，支持 tool use（"mostly compatible" 非 100%）。绑卡进 Developer 档 = 10× 限额 + 75 折。 | 限额按组织级共享；6K TPM 对长字幕片段可能偏紧；RPD 通常先耗尽。个人刮削够用，速度极快。 |
| **DeepSeek 免费额度** | 注册送 5M tokens，30 天。之后 V4-Flash 付费也近乎免费（见上）。 | 一次性额度，非长期免费档。 |
| **Qwen 国际站** | 每模型 100 万 tokens、90 天。 | 一次性；旧免费档已取消。 |
| **GLM-4.7-Flash** | **长期真免费**，~1000 req/天。 | 国内厂商，海外用户延迟/合规自行评估；能力弱于 GLM-4.7 正式版。 |

现实结论：**"用户自带 key + Gemini 免费档" 是唯一可长期依赖的零成本路线**（1500 req/天对家用 NAS 刮削绰绰有余）；GLM-4.7-Flash 是中国用户的等价免费选项；Groq 免费档适合追求速度的备选。

---

## 3) OpenAI 兼容 API 生态（决定能否一套 runtime 接多家）

- **OpenAI 官方**：原生，注意其主推 **Responses API**，但生态兼容层普遍只实现 **Chat Completions**。
- **Gemini**：官方 OpenAI 兼容层 `https://generativelanguage.googleapis.com/v1beta/openai/`（Bearer = Gemini API key），支持标准 `tools`/`tool_choice`、`finish_reason: "tool_calls"`。**只支持 `/chat/completions`，`/responses` 返回 404** —— 如果 codex runtime 走 Responses API，接 Gemini 需回落到 chat completions 模式（codex 有 `wire_api = "chat"` 配置可切换）。来源：ai.google.dev/gemini-api/docs/openai。
- **DeepSeek**：`api.deepseek.com` 原生 OpenAI 兼容（chat completions），function calling 支持。
- **Qwen**：DashScope compatible-mode，OpenAI 兼容 + tool calling。
- **GLM/Z.ai**：OpenAI 兼容格式 + tool calling。
- **Groq**：`api.groq.com/openai/v1`，OpenAI 兼容（自称 mostly compatible），tool calling 支持，另有内置 Web Search 等 server tools。
- **OpenRouter**：本身就是统一的 OpenAI 兼容聚合层（chat completions），一个 base_url + key 换 model 字符串即可切几百个模型。
- **Anthropic Haiku：唯一的例外**，无官方 OpenAI 兼容 endpoint，需原生 Messages API 或经 LiteLLM/OpenRouter 中转。

工程结论：**codex runtime 一套代码（base_url + api_key + model 三个配置项，wire_api 固定为 chat completions）可以覆盖 OpenAI / Gemini / DeepSeek / Qwen / GLM / Groq / OpenRouter 全部目标**。tool calling 语义（`tools` 数组 + `tool_calls` 返回 + `role:"tool"` 回传）在这些厂商间基本一致；差异点主要在：并行工具调用支持度、strict/JSON schema 严格模式、streaming 中 tool_call 分片格式——建议 runtime 里对 tool_calls 做容错解析（拼接 arguments 分片、容忍非并行）。Anthropic 若要支持需单独 adapter 或声明"经 OpenRouter 接入"。

---

## 4) 社区版（用户自带 API key）默认模型档位建议

三档预设 + 一个自定义位，全部走 OpenAI 兼容 chat completions：

1. **默认档（推荐）：`gemini-3-flash`（Gemini API，免费档）**
   - 零成本、1500 req/天、tool calling 可靠、1M 上下文、拿 key 门槛最低（AI Studio 免绑卡）。文档里注明：高峰可能限流，建议 agent 内置 429 退避 + 每文件重试。
   - 中国大陆用户等价默认：**DeepSeek V4-Flash**（$0.14/$0.28，cache hit 近免费，自动缓存对多轮 agent 极优）或免费的 GLM-4.7-Flash。

2. **付费性价比档：`deepseek-v4-flash` 或 `gpt-5-mini`（$0.125/$1.0）/ `gpt-5.4-nano`（$0.20/$1.25）**
   - 单部影片刮削（10 轮 × ~3K tokens）成本约 $0.005–0.02，一个 500 部的库不到 $10（有缓存时远低于此）。

3. **质量档（疑难识别/字幕语义推断）：`gpt-5.4-mini`（$0.75/$4.50）或 `gemini-3.1-flash-lite`→`2.5-flash`、`claude-haiku-4-5`（$1/$5，需 adapter）**
   - 建议做成"升级重试"策略：默认档识别置信度低时才升到质量档，而非全程用贵模型。

4. **自定义位：base_url + api_key + model 三字段**，天然覆盖 OpenRouter、Groq、本地 Ollama/vLLM 等一切 OpenAI 兼容服务。

补充工程建议：
- 工具定义 + system prompt 放在消息前缀且保持字节稳定，吃满各家 prefix cache（DeepSeek 自动、OpenAI cached input、Gemini implicit caching）。
- 免费档限流是常态：任务队列 + 指数退避 + 断点续扫应作为一等公民设计。
- 文档中标注各模型实测 tool calling 可靠性（建议发布前跑一个 20 文件的 fixture 集对每个预设模型回归）。

## Sources
https://pricepertoken.com/pricing-page/model/openai-gpt-5-mini
https://pricepertoken.com/pricing-page/model/openai-gpt-5-nano
https://tokencost.app/blog/gpt-5-4-mini-vs-nano-pricing
https://www.cloudzero.com/blog/openai-pricing/
https://benchlm.ai/openai/api-pricing
https://pricepertoken.com/pricing-page/model/google-gemini-2.5-flash-lite
https://www.tldl.io/resources/google-gemini-api-pricing
https://www.metacto.com/blogs/the-true-cost-of-google-gemini-a-guide-to-api-pricing-and-integration
https://www.aipricing.guru/google-ai-pricing/
https://pecollective.com/tools/gemini-free-tier-guide/
https://tokenmix.ai/blog/gemini-api-free-tier-limits
https://www.aifreeapi.com/en/posts/gemini-api-free-tier-complete-guide
https://ai.google.dev/gemini-api/docs/openai
https://deepseek.ai/pricing
https://www.nxcode.io/resources/news/deepseek-api-pricing-complete-guide-2026
https://pricepertoken.com/pricing-page/model/deepseek-deepseek-chat-v3.1
https://pricepertoken.com/pricing-page/provider/qwen
https://benchlm.ai/alibaba/api-pricing
https://developer.puter.com/tutorials/qwen-api-pricing/
https://openrouter.ai/z-ai/glm-4.7
https://felloai.com/glm-pricing/
https://tokenmix.ai/blog/glm-free-api-access-tiers-2026
https://pricepertoken.com/endpoints/openrouter/free
https://klymentiev.com/blog/openrouter-free-tier
https://console.groq.com/docs/rate-limits
https://pricepertoken.com/endpoints/groq/free
https://tokenmix.ai/blog/groq-free-tier-limits-2026
https://www.eesel.ai/blog/groq-pricing
https://platform.claude.com/docs/en/pricing