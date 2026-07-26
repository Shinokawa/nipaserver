# Codex Core Agent 主循环与会话状态机 — 精读报告（为 nipa-agent 提炼）

精读范围（均为绝对路径）：
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/codex_thread.rs`（对外 facade，711 行）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/session/handlers.rs`（submission_loop 分发）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/tasks/mod.rs` + `tasks/regular.rs`（Task 生命周期/中断）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/session/turn.rs`（run_turn 主循环，2581 行）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/stream_events_utils.rs`（流事件→工具调度）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/state/turn.rs` + `state/session.rs`（状态机数据）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/session/input_queue.rs`（steer/追加输入）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/session/context_window.rs` + `compact.rs`（token 上限与压缩）
- `/Users/sakiko/Desktop/nipaserver/reference/codex/codex-rs/core/src/responses_retry.rs` + `tools/parallel.rs`

---

## 一、整体架构：五层嵌套的循环

```
CodexThread (facade)                    // 对外 API：submit(Op) / next_event()
  └─ SessionIo { tx_sub, rx_event }     // 两条 async_channel：SQ(提交, bounded 512) / EQ(事件, unbounded)
       └─ submission_loop               // tokio 后台任务，逐个 recv Op 并分发
            └─ Session::spawn_task      // 同一时刻最多 1 个活跃 Task（active_turn: Mutex<Option<ActiveTurn>>）
                 └─ RegularTask::run    // tokio::spawn 出去的 Task
                      └─ run_turn       // ★ 核心：Turn 循环（每轮 = 一次采样请求）
                           └─ run_sampling_request        // 重试循环（网络级）
                                └─ try_run_sampling_request  // SSE 流事件循环（单次请求）
                                     └─ handle_output_item_done → ToolCallRuntime  // 工具执行
```

关键洞察：**codex 的 "Turn" 是用户视角的一次对话回合，内部包含 N 次采样请求（sampling request / step）**。每次采样后模型要么发工具调用（→ 回填输出继续下一次采样），要么发纯 assistant message（→ Turn 结束）。这正对应你们 "单任务 ≤16 轮" 里的"轮"。

## 二、状态机描述

### 会话级状态机

```
Idle ──(Op::UserInput, 无活跃 turn)──> Active(RegularTask)
Active ──(Op::UserInput, 有活跃 turn)──> steer_input: 追加进 pending_input 队列（不新建 Task）
Active ──(Op::Interrupt)──> cancel token → 100ms 优雅等待 → handle.abort() → 写入"被中断"标记到历史 → TurnAborted 事件 → Idle
Active ──(task 自然结束)──> on_task_finished: TurnComplete{last_agent_message} → Idle
```

核心数据（`state/turn.rs`）：

```rust
// Session 上：active_turn: Mutex<Option<ActiveTurn>>
pub(crate) struct ActiveTurn {
    pub task: Option<RunningTask>,
    pub turn_state: Arc<Mutex<TurnState>>,
}
pub(crate) struct RunningTask {
    pub done: Arc<Notify>,                  // 任务完成通知（优雅中断用）
    pub cancellation_token: CancellationToken,
    pub handle: AbortOnDropHandle<()>,      // 兜底强杀
    pub turn_context: Arc<TurnContext>,
    // ...
}
#[derive(Default)]
pub(crate) struct TurnState {
    pub pending_input: TurnInputQueue,      // 运行中被 steer 进来的输入
    pub tool_calls: u64,
    pub token_usage_at_turn_start: TokenUsage,
    // pending_approvals / pending_user_input 等 oneshot 等待者（nipa 不需要）
}
```

### Turn 级状态机（`session/turn.rs::run_turn`，简化后）

```
进入 run_turn
  ├─ [pre-sampling compact] token 超限 → 先压缩历史
  └─ loop {
       1. drain pending_input（steer 进来的用户消息）→ 记入历史
       2. 检查 token 状态（context_window_token_status）
       3. input = clone_history().for_prompt()      // 全量历史作为请求输入
       4. run_sampling_request(input)               // 含网络重试
       ├─ Ok { needs_follow_up, last_agent_message }
       │    ├─ token_limit_reached && needs_follow_up → run_auto_compact() → continue（mid-turn 压缩）
       │    ├─ needs_follow_up == true  → continue   // 有工具调用输出待回填 or 有 pending input
       │    └─ needs_follow_up == false → break      // 模型只发了 assistant message，Turn 结束
       ├─ Err(TurnAborted)  → return Err            // interrupt 传播
       └─ Err(其他)         → emit Error 事件; break // 让用户能继续对话，不 crash
     }
  return Ok(last_agent_message)
```

**决定"继续下一轮还是结束"的唯一信号是 `needs_follow_up`**，由三处置位：
1. 模型输出中出现 `FunctionCall`（工具调用）→ `handle_output_item_done` 设 `needs_follow_up = true` 并把工具执行 future 推入 `FuturesOrdered`；
2. 工具调用解析失败（`FunctionCallError::RespondToModel`）→ 把错误文本作为 `FunctionCallOutput` 记入历史，`needs_follow_up = true`（让模型自己纠错，flash 模型下很重要）；
3. 采样期间有新 pending input（steer）。

`Completed { end_turn: Some(false) }` 也会置 follow_up（服务端要求继续），Chat Completions 下可忽略。

### 采样请求级：流事件循环（`try_run_sampling_request`，简化）

```rust
let mut stream = client.stream(prompt, ...).or_cancel(&cancel_token).await??;
let mut in_flight: FuturesOrdered<BoxFuture<Result<ResponseInputItem>>> = FuturesOrdered::new();
let mut needs_follow_up = false;
let mut last_agent_message = None;

let outcome = loop {
    let event = match stream.next().or_cancel(&cancel_token).await {
        Ok(ev) => ev,
        Err(Cancelled) => break Err(CodexErr::TurnAborted),   // ★ interrupt 感知点
    };
    match event {
        Some(Ok(ev)) => match ev {
            OutputItemDone(item) => {
                let r = handle_output_item_done(&mut ctx, item).await?; // 立即记录历史+起工具future
                if let Some(fut) = r.tool_future { in_flight.push_back(fut); }
                needs_follow_up |= r.needs_follow_up;
                last_agent_message = r.last_agent_message.or(last_agent_message);
            }
            OutputTextDelta(d) => emit(AgentMessageDelta(d)),   // 流式转发给客户端
            Completed { token_usage, .. } => {
                sess.record_token_usage_info(token_usage).await;
                break Ok(SamplingRequestResult { needs_follow_up, last_agent_message });
            }
            _ => { /* reasoning delta / rate limits 等 */ }
        },
        Some(Err(e)) => break Err(e),
        None => break Err(CodexErr::Stream("stream closed before completed".into())),
    }
};
// ★ 流结束后才 drain 工具 futures —— 工具与流接收并发，输出按序回填历史
drain_in_flight(&mut in_flight, ...).await?;   // 每个结果 record_conversation_items()
if cancel_token.is_cancelled() { return Err(CodexErr::TurnAborted); }
outcome
```

值得抄的细节：**工具执行与流接收是并发的**（模型还在吐 token 时，前面的 FunctionCall 已经开始跑），用 `FuturesOrdered` 保证输出按调用顺序回填历史。并行门用 `RwLock<()>`：支持并行的工具拿读锁并发跑，不支持的拿写锁独占（`tools/parallel.rs:133`）——比信号量优雅。

### 工具错误分级（`tools/src/function_call_error.rs`）

```rust
pub enum FunctionCallError {
    RespondToModel(String),  // 回给模型让它重试/纠错（参数解析失败、工具软错误）
    Fatal(String),           // 终止整个 Turn
}
```

这个二分是 codex 里最值得抄的错误设计：绝大部分工具失败都走 `RespondToModel`，把错误文本作为工具输出回填，模型下一轮自己修正。

## 三、错误 / 重试 / 中断

### 网络重试（`run_sampling_request` + `responses_retry.rs` + `util.rs::backoff`）

```rust
let max_retries = provider.stream_max_retries();
let mut retries = 0;
loop {
    match try_run_sampling_request(...).await {
        Ok(out) => return Ok(out),
        Err(e) if matches!(e, ContextWindowExceeded) => { sess.set_total_tokens_full(); return Err(e); } // 不重试，交给上层压缩
        Err(e) if !e.is_retryable() => return Err(e),
        Err(e) => {
            if retries >= max_retries { return Err(e); }
            retries += 1;
            let delay = e.retry_delay().unwrap_or_else(|| backoff(retries)); // 指数退避+0.9~1.1抖动
            sess.notify_stream_error(format!("Reconnecting... {retries}/{max_retries}"), e).await; // ★ 给UI发事件
            tokio::time::sleep(delay).await;
        }
    }
}
// backoff: INITIAL_DELAY_MS * FACTOR^(n-1) * jitter(0.9..1.1)
```

要点：重试时**重新从历史构建 prompt**（因为部分已流出的 item 可能已记录）；`ContextWindowExceeded` 和 `UsageLimitReached` 是不可重试的特殊分支。

### 中断（`tasks/mod.rs::abort_all_tasks / handle_task_abort`）

```rust
async fn handle_task_abort(&self, task: RunningTask, reason: TurnAbortReason) {
    task.cancellation_token.cancel();                 // 1. 协作式取消（所有 .or_cancel() 点感知）
    select! {
        _ = task.done.notified() => {}                // 2. 等 100ms 优雅退出
        _ = sleep(Duration::from_millis(100)) => { warn!("not graceful"); }
    }
    task.handle.abort();                              // 3. 兜底强杀 tokio task
    // 4. 往历史里写一条"上一轮被用户中断"的标记消息（模型下一轮可见！）
    self.record_conversation_items(&[interrupted_marker()]).await;
    // 5. 发 TurnAborted 事件
    self.send_event(EventMsg::TurnAborted { reason, .. }).await;
}
```

三级取消（token → 宽限期 → abort）+ **中断后往历史写模型可见标记**，这两点都值得抄。工具侧取消在 `ToolCallRuntime`：`select! { res = dispatch_handle, _ = token.cancelled() => 返回 "aborted after Ns" 的合成输出 }`。

### 轮数上限：codex 没有！

**run_turn 的 loop 是无界的**——没有 max_turns/max_iterations（我全库 grep 过）。codex 只靠 token 上限 + 压缩控制。你们的"≤16 轮"需要自己加：在 loop 里挂个 `step_count`，超限时 break 并产出失败结果，这是对 codex 的净新增。

## 四、Token 上限与 compact

### 触发判定（`session/context_window.rs`）

核心：`auto_compact_token_limit = min(配置值, context_window * 9 / 10)`（`protocol/src/openai_models.rs:459`），每次 `Completed` 事件后累计 usage，超过即 `token_limit_reached`。

### 两个触发时机（`session/turn.rs`）

1. **Pre-turn**（`run_pre_sampling_compact`，run_turn 开头）：上一轮遗留超限 → 先压缩再采样，压缩后历史不注入 initial context（下一轮全量重注入）。
2. **Mid-turn**（主循环内，采样后）：`needs_follow_up && token_limit_reached` → 压缩后 `continue`（工具链继续跑，不打断任务）。

### 本地压缩算法（`compact.rs::run_compact_task_inner_impl`，值得抄的精华）

```rust
// 1. 向模型发一条总结请求：历史 + SUMMARIZATION_PROMPT 作为 user message
// 2. 若总结请求本身 ContextWindowExceeded：从历史头部逐条删除（保缓存前缀+保近期）再试
//    history.remove_first_item(); retries = 0; continue;
// 3. 取回 summary 后重建历史：
let summary_text = format!("{SUMMARY_PREFIX}\n{summary}");
let user_messages = collect_user_messages(&old_history);  // 抽出所有历史 user 消息
let new_history = build_compacted_history(initial_context, &user_messages, &summary_text);
//    ├─ 保留最近的 user 消息，从后往前选，总额 ≤ 20k tokens（COMPACT_USER_MESSAGE_MAX_TOKENS）
//    └─ 追加 summary 作为最后一条
sess.replace_compacted_history(new_history).await;
sess.recompute_token_usage().await;  // 用估算值重置计数
```

结构：**新历史 = [近期 user 消息(截断到 20k)] + [summary 消息]**，assistant/tool 全丢。压缩期间发 `ContextCompaction` turn item 事件让 UI 显示进度。

## 五、对外事件流（对应你们的 SSE）

- `Session.tx_event: async_channel::Sender<Event>`（unbounded），`Event { id: turn_id, msg: EventMsg }`。
- `send_event()` 同时做：写 rollout（持久化）+ 发给客户端 —— 事件即事实源。
- 生命周期事件序列：`TurnStarted → (AgentMessageContentDelta* | ToolCall 相关 | TokenCount | Warning)* → TurnComplete{last_agent_message} | TurnAborted{reason} | Error`。
- `CodexThread::next_event()` 就是消费端。你们把 rx_event 桥到 axum SSE / flutter_rust_bridge 的 StreamSink 即可，一套事件两个出口。

## 六、nipa-agent 精简版伪代码（可直接照抄的骨架）

```rust
// ===== 事件（SSE / FRB 共用）=====
#[derive(Serialize, Clone)]
#[serde(tag = "type")]
pub enum AgentEvent {
    TaskStarted { task_id: String },
    StepStarted { step: u32 },                       // 对应 codex 的 sampling request
    AssistantDelta { text: String },                 // 流式文本
    ToolCallStarted { name: String, args_preview: String },
    ToolCallCompleted { name: String, output_preview: String },
    Compacted { tokens_before: i64, tokens_after: i64 },
    Retrying { attempt: u32, max: u32, error: String },
    TaskCompleted { result: ScrapeResult },          // submit_result 的载荷
    TaskFailed { reason: FailReason },               // MaxSteps | Fatal | Aborted
}

// ===== 工具（codex 的 FunctionCallError 二分照抄）=====
pub enum ToolError {
    RespondToModel(String),   // 回填给模型自己纠错（TMDB 查不到、参数错）
    Fatal(String),            // 终止任务
}
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;   // OpenAI function schema
    async fn call(&self, args: serde_json::Value, cx: &TaskCtx) -> Result<ToolOutput, ToolError>;
}
pub enum ToolOutput {
    Text(String),                          // 普通只读工具
    Terminal(ScrapeResult),                // ★ submit_result：codex 没有的概念，你们的终止信号
}

// ===== 主循环（run_turn 的裁剪版）=====
pub struct RunCfg { pub max_steps: u32 /*16*/, pub max_retries: u32 /*3*/, pub compact_limit: i64 }

pub async fn run_task(
    llm: &ChatClient, tools: &ToolSet, mut history: Vec<ChatMessage>,
    events: mpsc::UnboundedSender<AgentEvent>, cancel: CancellationToken, cfg: RunCfg,
) -> Result<ScrapeResult, FailReason> {
    let mut total_tokens: i64 = 0;
    for step in 0..cfg.max_steps {                                   // ★ codex 无界，你们加界
        let _ = events.send(AgentEvent::StepStarted { step });
        // -- token 上限：flash + 16 轮通常到不了，保守起见留个截断 --
        if total_tokens > cfg.compact_limit {
            history = compact(llm, history).await.map_err(|e| FailReason::Fatal(e))?;
        }
        // -- 网络重试循环（codex run_sampling_request 裁剪）--
        let resp = {
            let mut attempt = 0;
            loop {
                match stream_chat(llm, &history, tools, &events, &cancel).await {
                    Ok(r) => break r,
                    Err(e) if cancel.is_cancelled() => return Err(FailReason::Aborted),
                    Err(e) if e.retryable() && attempt < cfg.max_retries => {
                        attempt += 1;
                        let _ = events.send(AgentEvent::Retrying { attempt, max: cfg.max_retries, error: e.to_string() });
                        tokio::time::sleep(backoff(attempt)).await;   // 指数退避+抖动
                    }
                    Err(e) => return Err(FailReason::Fatal(e.to_string())),
                }
            }
        };
        total_tokens = resp.total_tokens_so_far;
        history.push(resp.assistant_message.clone());                 // 先记录 assistant（含 tool_calls）
        if resp.tool_calls.is_empty() {
            // codex 语义：无工具调用 = turn 结束。你们语义：没 submit_result 就是"模型迷路"
            history.push(nudge_message("请调用工具继续，完成后必须调用 submit_result"));
            continue;                                                 // 温和推回，而不是直接失败
        }
        // -- 顺序执行工具（5-10 个只读工具没必要上 FuturesOrdered 并发）--
        for call in resp.tool_calls {
            let _ = events.send(AgentEvent::ToolCallStarted { name: call.name.clone(), args_preview: preview(&call.args) });
            let out = match tools.dispatch(&call, &cancel).await {
                Ok(ToolOutput::Terminal(result)) => {                 // ★ 唯一成功出口
                    let _ = events.send(AgentEvent::TaskCompleted { result: result.clone() });
                    return Ok(result);
                }
                Ok(ToolOutput::Text(t)) => t,
                Err(ToolError::RespondToModel(msg)) => msg,           // 错误文本回填，模型自纠
                Err(ToolError::Fatal(msg)) => return Err(FailReason::Fatal(msg)),
            };
            history.push(ChatMessage::tool(call.id, out));            // 回填 → 天然 needs_follow_up
        }
        // 有工具输出回填 → 隐式 continue 下一 step
    }
    Err(FailReason::MaxSteps)                                         // 16 轮内没 submit_result
}

// ===== 会话包装（Session/Task 的裁剪：单任务不需要 submission_loop）=====
pub struct ScrapeTaskHandle {
    pub events: mpsc::UnboundedReceiver<AgentEvent>,  // → axum SSE / FRB StreamSink 各桥一次
    pub cancel: CancellationToken,                     // interrupt = cancel()
    pub join: JoinHandle<Result<ScrapeResult, FailReason>>,
}
pub fn spawn_scrape(req: ScrapeRequest, deps: Deps) -> ScrapeTaskHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(run_task(..., tx.clone(), cancel.child_token(), cfg));
    ScrapeTaskHandle { events: rx, cancel, join }
}
```

流式处理里对每个 `cancel` 敏感点抄 codex 的模式：`stream.next() => ...` 与 `cancel.cancelled()` 做 `select!`（codex 用 `OrCancelExt::or_cancel` 扩展，一个 ~20 行的小 trait，可以直接抄）。

## 七、保留 / 丢弃清单

### 值得抄（保留）
| 设计 | 出处 | 理由 |
|---|---|---|
| 双通道模型：提交 channel + 事件 channel，事件即事实源 | `session/mod.rs:562` | SSE 与 FRB 共用一条事件流，天然解耦 |
| `needs_follow_up` 单信号驱动循环续/止 | `turn.rs:321-476` | 状态机极简：有工具输出回填就续，纯文本就止 |
| `FunctionCallError::RespondToModel / Fatal` 二分 | `tools/src/function_call_error.rs` | flash 模型出错率高，软错误回填让模型自纠是关键 |
| 工具调用/输出**立即**记入历史（中断也不丢） | `stream_events_utils.rs:188` 注释 | 保证历史与事件流一致 |
| 三级中断：CancellationToken → 100ms Notify 宽限 → handle.abort() | `tasks/mod.rs:868-917` | 只依赖 tokio + tokio_util，FRB 环境可用 |
| 网络重试：is_retryable 判定 + 指数退避带抖动 + 每次重试发 UI 事件 | `responses_retry.rs`、`util.rs:86` | "Reconnecting 2/5" 事件对进度播报体验很重要 |
| `ContextWindowExceeded` 不重试、直接触发压缩 | `turn.rs:1233` | 分清网络错误与容量错误 |
| 压缩重建公式：近期 user 消息(限额截断) + summary，压缩时超窗则从头删条目重试 | `compact.rs:240-393, 611` | 20 行能实现的实用算法 |
| auto_compact_limit = 90% context window | `openai_models.rs:459` | 简单可靠的阈值 |
| TurnComplete/TurnAborted 携带 `last_agent_message` + 时长 | `tasks/mod.rs:785-812` | 终态事件自带结果摘要 |
| Turn 级错误不 crash：emit Error 事件后 break，会话可继续 | `turn.rs:499-509` | 服务器场景必需 |

### 死重（丢弃）
| 部分 | 占比/位置 | 说明 |
|---|---|---|
| Responses API 专属：ResponseItem 十几种变体、reasoning summary 流、remote compaction (v1/v2)、WebSocket 传输+HTTPS fallback、sticky routing | turn.rs 约 40%、compact_remote*.rs 全部 | 你们走 Chat Completions，只需 assistant/tool_calls/tool 三种消息 |
| 多 Agent：mailbox / InterAgentCommunication / MailboxDeliveryPhase 状态机 / agent_control | input_queue.rs 一半、state/turn.rs | 单 agent 刮削完全不需要 |
| steer_input（运行中追加用户输入） | mod.rs:3975 | 刮削任务无人工介入；接口上留 interrupt 就够 |
| 审批/许可体系：pending_approvals、sandbox policy、permission profile、exec policy、guardian | TurnState 大半字段 + 多个模块 | 你们工具全只读，无需审批 |
| 扩展/hook 体系：ExtensionData、turn_item_contributors、stop hooks、pre/post compact hooks | run_turn 中十几处调用点 | codex 的插件化需求，你们写死即可 |
| skills / plugins / connectors / mentions 注入 | turn.rs:569-740 | 你们的 system prompt 是静态的 |
| Plan mode 全部流解析状态机（PlanModeStreamState 等） | turn.rs:1424-1955 | 无 plan 概念 |
| rollout 持久化 + thread store + fork/resume | rollout.rs、thread_manager.rs 等 | 刮削任务是短命的；要留痕写自己的日志表即可 |
| MCP 运行时、unified_exec、turn environments、TurnDiffTracker | 各自模块 | 工具是进程内 Rust 函数，无外部工具面 |
| 遥测/analytics（OTel span、事件打点，on_task_finished 里 ~150 行） | tasks/mod.rs:644-767 | 换成 tracing 日志即可 |
| 模型切换压缩（comp_hash、model downshift） | turn.rs:879-1005 | 单模型固定 |
| 工具并行门（RwLock 读写锁分级） | tools/parallel.rs | 只读 API 工具顺序执行足够；若想并发再抄这 10 行 |

### 需要自己新增（codex 没有的）
1. **`max_steps` 硬上限**：codex 的 turn loop 无界，你们必须在循环里加计数器。
2. **`ToolOutput::Terminal` 终止语义**：codex 以"无工具调用"为 turn 结束；你们以 `submit_result` 为唯一成功出口，工具返回值里加终止变体，在 dispatch 处短路 return。建议同时把"模型不调用任何工具"处理为 nudge 回填而非失败（flash 模型常见走神）。
3. **Chat Completions 流解析**：把 codex 的 ResponseEvent 换成 chunk delta 里 `tool_calls[].function.arguments` 的增量拼接（codex 的 `ToolCallInputDelta` 处理思路可参考，但格式不同）。

### 依赖面结论
主循环真正的运行时依赖只有：`tokio`（sync/spawn/time）、`tokio_util`（CancellationToken、AbortOnDropHandle）、`futures`（stream）、`async_channel` 或 `tokio::mpsc`。没有任何阻碍嵌入 axum 或经 flutter_rust_bridge 回流的重型依赖——codex 自己也是这几样撑起整个状态机的。