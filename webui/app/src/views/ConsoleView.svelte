<script lang="ts">
  // Agent 控制台：任务队列（/scrape/pending 轮询 + SSE scrape_update 增量）
  // + 事件流 timeline（SSE scrape 按 task_id 分组）+ 待确认卡（docs/05 §4.3）
  import { onMount, tick } from 'svelte';
  import { api } from '../lib/api';
  import { nav } from '../lib/nav.svelte';
  import { sse } from '../lib/sse.svelte';
  import type { PendingTask } from '../lib/types';
  import { confidenceBadge, artClass } from '../lib/format';
  import AgentTimeline from '../components/AgentTimeline.svelte';

  let { onPendingChange }: { onPendingChange?: (n: number) => void } = $props();

  let pending = $state<PendingTask[]>([]);
  let selectedTask = $state<number | null>(null);
  let timelineEl = $state<HTMLDivElement | null>(null);
  let followTail = $state(true);

  async function loadPending() {
    try {
      pending = await api.scrapePending();
      onPendingChange?.(pending.length);
    } catch {
      /* 后端未起时静默 */
    }
  }

  onMount(() => {
    // #/console?task=N（详情页“识别信息”入口）→ 选中该任务
    const taskParam = nav.query.task;
    if (taskParam && /^\d+$/.test(taskParam)) selectedTask = Number(taskParam);
    loadPending();
    const t = setInterval(loadPending, 15_000);
    const un = sse.subscribe(async (msg) => {
      if (msg.type === 'scrape_update') {
        // 状态迁移到 needs_review/done 时刷新待确认列表
        if (msg.state === 'needs_review' || msg.state === 'done' || msg.state === 'failed') {
          loadPending();
        }
        if (selectedTask === null && msg.state === 'running') selectedTask = msg.task_id;
      }
      if (msg.type === 'scrape' && msg.task_id === selectedTask && followTail) {
        await tick();
        timelineEl?.scrollTo({ top: timelineEl.scrollHeight });
      }
    });
    return () => {
      clearInterval(t);
      un();
    };
  });

  /** 队列 = SSE 已知状态的任务 ∪ 待确认任务 */
  const queue = $derived.by(() => {
    const rows = new Map<number, { id: number; label: string; state: string; confidence?: string | null }>();
    for (const [idStr, state] of Object.entries(sse.scrapeStates)) {
      const id = Number(idStr);
      rows.set(id, { id, label: `任务 #${id}`, state });
    }
    for (const p of pending) {
      rows.set(p.task_id, {
        id: p.task_id,
        label: p.file ?? `任务 #${p.task_id}`,
        state: 'needs_review',
        confidence: p.confidence,
      });
    }
    return [...rows.values()].sort((a, b) => b.id - a.id);
  });

  const runningCount = $derived(queue.filter((q) => q.state === 'running').length);
  const queuedCount = $derived(queue.filter((q) => q.state === 'queued').length);
  const doneToday = $derived(
    Object.values(sse.scrapeStates).filter((s) => s === 'done' || s === 'needs_review').length
  );

  const selectedEvents = $derived(selectedTask !== null ? (sse.scrapeEvents[selectedTask] ?? []) : []);
  const selectedTokens = $derived.by(() => {
    let total = 0;
    for (const ev of selectedEvents) {
      if (ev.type === 'token_usage') total = ev.total_input + ev.total_output;
    }
    return total;
  });
  const selectedModel = $derived.by(() => {
    for (const ev of selectedEvents) {
      if (ev.type === 'task_started') return ev.model;
    }
    return null;
  });

  function stateBadge(state: string): { cls: string; label: string } {
    switch (state) {
      case 'running':
        return { cls: 'acc', label: '运行中' };
      case 'queued':
        return { cls: '', label: '队列中' };
      case 'done':
        return { cls: 'good', label: '完成' };
      case 'needs_review':
        return { cls: 'warn', label: '待确认' };
      case 'failed':
        return { cls: 'crit', label: '失败' };
      default:
        return { cls: '', label: state };
    }
  }

  function resultTitle(r: Record<string, unknown> | null): string {
    if (!r) return '（无结论）';
    const t = (r.title ?? r.name ?? r.series_title) as string | undefined;
    const s = r.season_no ?? r.season;
    const e = r.episode_no ?? r.episode;
    let out = t ?? '（无标题）';
    if (s != null && e != null) out += ` S${String(s).padStart(2, '0')}E${String(e).padStart(2, '0')}`;
    else if (e != null) out += ` E${e}`;
    return out;
  }

  function onTimelineScroll() {
    if (!timelineEl) return;
    const nearBottom =
      timelineEl.scrollHeight - timelineEl.scrollTop - timelineEl.clientHeight < 40;
    followTail = nearBottom;
  }
</script>

<section class="view">
  <div class="view-title">Agent 控制台</div>
  <div class="view-sub">识别管线的每一步都可追溯 — 这是管家手下工人们的车间</div>

  <div class="tiles">
    <div class="card tile">
      <div class="t-label">本次会话已识别</div>
      <div class="t-row"><div class="t-value">{doneToday}</div></div>
    </div>
    <div class="card tile">
      <div class="t-label">运行中</div>
      <div class="t-row">
        <div class="t-value">{runningCount}</div>
        {#if runningCount > 0}<span class="badge acc"><span class="bdot"></span>live</span>{/if}
      </div>
    </div>
    <div class="card tile">
      <div class="t-label">队列中</div>
      <div class="t-row"><div class="t-value">{queuedCount}</div></div>
    </div>
    <div class="card tile">
      <div class="t-label">待确认</div>
      <div class="t-row">
        <div class="t-value">{pending.length}</div>
        {#if pending.length > 0}<span class="badge warn"><span class="bdot"></span>需要你</span>{/if}
      </div>
    </div>
  </div>

  <div class="console">
    <!-- 左：任务队列 -->
    <div class="card">
      <div class="panel-title">任务队列<span class="count">{runningCount} 运行 · {queuedCount} 排队</span></div>
      {#if queue.length === 0}
        <div class="empty" style="padding:30px 14px">
          暂无任务 — 触发库扫描或在设置里试刮一段证据
        </div>
      {/if}
      {#each queue as q (q.id)}
        {@const b = stateBadge(q.state)}
        <div
          class="task-row"
          class:sel={selectedTask === q.id}
          role="button"
          tabindex="0"
          onclick={() => { selectedTask = q.id; followTail = true; }}
          onkeydown={(e) => e.key === 'Enter' && (selectedTask = q.id)}
        >
          <div class="f">{q.label}</div>
          <div class="m">
            <span class="tier l2">L2</span>
            <span class="badge {b.cls}"><span class="bdot"></span>{b.label}</span>
            {#if q.confidence}<span>{q.confidence}</span>{/if}
          </div>
        </div>
      {/each}
    </div>

    <!-- 中：事件流 timeline -->
    <div class="card">
      <div class="panel-title">
        {#if selectedTask !== null}
          <span style="font-family:var(--mono);font-size:12px">任务 #{selectedTask}</span>
          {#if sse.scrapeStates[selectedTask] === 'running'}
            <span class="badge acc"><span class="bdot"></span>live</span>
          {/if}
          <span class="count">
            {selectedModel ?? ''}{selectedModel && selectedTokens ? ' · ' : ''}{selectedTokens
              ? (selectedTokens / 1000).toFixed(1) + 'k tokens'
              : ''}
          </span>
        {:else}
          事件流
          <span class="count">选择左侧任务查看</span>
        {/if}
      </div>
      <div class="timeline" bind:this={timelineEl} onscroll={onTimelineScroll}>
        {#if selectedTask === null}
          <div class="empty">从左侧选择一个任务，实时查看 agent 的每一步</div>
        {:else if selectedEvents.length === 0}
          <div class="empty">该任务暂无事件 — 事件流仅覆盖本次连接后的实时任务</div>
        {:else}
          <AgentTimeline events={selectedEvents} />
        {/if}
      </div>
    </div>

    <!-- 右：待确认审批 -->
    <div class="card">
      <div class="panel-title">待确认<span class="count">{pending.length} 项</span></div>
      {#if pending.length === 0}
        <div class="empty" style="padding:30px 14px">没有待确认条目 ✓</div>
      {/if}
      {#each pending as p (p.task_id)}
        <div class="approve-card">
          <div class="ev">{p.file ?? `任务 #${p.task_id}`}{#if p.evidence}<br />{p.evidence}{/if}</div>
          <div class="concl">
            <div class="mini-poster {artClass(p.task_id)}" style="width:44px"></div>
            <div>
              <b>{resultTitle(p.result)}</b>
              {#if p.confidence}
                <span class="badge {confidenceBadge(p.confidence)}"><span class="bdot"></span>{p.confidence}</span>
              {/if}
            </div>
          </div>
          <div class="approve-actions">
            <button class="btn btn-primary btn-sm" disabled title="确认端点后续里程碑提供，当前请走管家对话确认">✓ 确认</button>
            <button class="btn btn-ghost btn-sm" disabled>✎ 改正</button>
          </div>
        </div>
      {/each}
      {#if pending.length > 0}
        <div class="kbd-hint">确认操作暂经<b style="color:var(--ink-2)">管家对话</b>完成（confirm_pending）</div>
      {/if}
    </div>
  </div>
</section>
