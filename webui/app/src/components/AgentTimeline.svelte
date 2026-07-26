<script lang="ts">
  // 事件流 timeline 渲染器（docs/05 §4.3 中栏）。
  // SSE 实时与 transcript 回放同构（契约 §4 不变量）。
  import type { AgentEnvelope } from '../lib/types';
  import { fmtArgs, fmtDuration } from '../lib/format';

  let { events }: { events: AgentEnvelope[] } = $props();

  type Block =
    | { kind: 'round'; round: number; max: number }
    | { kind: 'msg'; text: string }
    | {
        kind: 'tool';
        tool: string;
        args: unknown;
        running: boolean;
        success?: boolean;
        preview?: string;
        error?: string | null;
        duration?: number;
      }
    | { kind: 'note'; tone: 'warn' | 'crit' | 'good' | 'acc'; text: string; body?: string };

  const blocks = $derived.by(() => {
    const out: Block[] = [];
    const open = new Map<string, Block & { kind: 'tool' }>();
    for (const ev of events) {
      switch (ev.type) {
        case 'task_started':
          out.push({ kind: 'note', tone: 'acc', text: `任务开始 · ${ev.model} · 最多 ${ev.max_rounds} 轮` });
          break;
        case 'round_started':
          out.push({ kind: 'round', round: ev.round, max: ev.max_rounds });
          break;
        case 'assistant_message':
          out.push({ kind: 'msg', text: ev.text });
          break;
        case 'tool_call_begin': {
          const b: Block & { kind: 'tool' } = {
            kind: 'tool',
            tool: ev.tool,
            args: ev.arguments,
            running: true,
          };
          open.set(ev.call_id, b);
          out.push(b);
          break;
        }
        case 'tool_call_end': {
          const b = open.get(ev.call_id);
          if (b) {
            b.running = false;
            b.success = ev.success;
            b.preview = ev.output_preview;
            b.error = ev.error;
            b.duration = ev.duration_ms;
            open.delete(ev.call_id);
          } else {
            out.push({
              kind: 'tool',
              tool: ev.tool,
              args: null,
              running: false,
              success: ev.success,
              preview: ev.output_preview,
              error: ev.error,
              duration: ev.duration_ms,
            });
          }
          break;
        }
        case 'retrying':
          out.push({ kind: 'note', tone: 'warn', text: `重试 ${ev.attempt}/${ev.max_attempts}`, body: ev.message });
          break;
        case 'warning':
          out.push({ kind: 'note', tone: 'warn', text: '警告', body: ev.message });
          break;
        case 'task_completed':
          out.push({
            kind: 'note',
            tone: 'good',
            text: `任务完成 · ${ev.rounds_used} 轮`,
            body: JSON.stringify(ev.result, null, 2),
          });
          break;
        case 'task_failed':
          out.push({
            kind: 'note',
            tone: 'crit',
            text: `任务失败 · ${String(ev.reason)}`,
            body: ev.message,
          });
          break;
        case 'task_aborted':
          out.push({ kind: 'note', tone: 'crit', text: `任务中止 · ${String(ev.reason)}` });
          break;
        case 'token_usage':
          // 汇总在面板头显示，不进 timeline
          break;
      }
    }
    return out;
  });

  const toneBorder = { warn: 'rgba(250,178,25,.4)', crit: 'rgba(208,59,59,.45)', good: 'rgba(12,163,12,.4)', acc: 'rgba(57,135,229,.4)' };
  const toneColor = { warn: '#ffd479', crit: '#ff9d9d', good: '#7fd97f', acc: '#9cc4f2' };
</script>

{#each blocks as b}
  {#if b.kind === 'round'}
    <div class="tl-round">Round {b.round}{#if b.max}&nbsp;/ {b.max}{/if}</div>
  {:else if b.kind === 'msg'}
    <div class="msg-bubble">{b.text}</div>
  {:else if b.kind === 'tool'}
    <div class="tool-card" class:done-ok={!b.running && b.success} class:done-fail={!b.running && b.success === false}>
      <div class="head">
        {#if b.running}
          <div class="tool-icon running"></div>
        {:else}
          <div class="tool-icon">{b.success ? '⌕' : '✗'}</div>
        {/if}
        <span class="tname">{b.tool}</span>
        <span class="targs">{fmtArgs(b.args)}</span>
        <span class="dur">{b.running ? 'running' : b.duration !== undefined ? fmtDuration(b.duration) : ''}</span>
      </div>
      {#if !b.running && (b.preview || b.error)}
        <div class="tout">→ {b.error ? b.error : b.preview}</div>
      {/if}
    </div>
  {:else}
    <div class="tool-card" style="border-color:{toneBorder[b.tone]}">
      <div class="head">
        <span style="color:{toneColor[b.tone]};font-size:12.5px;font-weight:650">{b.text}</span>
      </div>
      {#if b.body}
        <div class="tout" style="white-space:pre-wrap">{b.body}</div>
      {/if}
    </div>
  {/if}
{/each}
