<script lang="ts">
  // 管家：对话界面（POST /chat）+ SSE steward 工具行实时渲染 + 会话历史侧栏
  // （docs/06-管家设计.md §6）
  import { onMount, tick } from 'svelte';
  import { api } from '../lib/api';
  import { sse } from '../lib/sse.svelte';
  import type { ChatSession, ToolEventSnap } from '../lib/types';
  import { fmtArgs, fmtDuration, fmtTime, renderMarkdown } from '../lib/format';

  interface ToolLine {
    tool: string;
    args: unknown;
    running: boolean;
    success?: boolean;
    duration?: number;
  }
  interface Msg {
    role: 'user' | 'steward';
    text: string;
    tools?: ToolLine[];
  }

  let messages = $state<Msg[]>([]);
  let sessions = $state<ChatSession[]>([]);
  let sessionId = $state<number | null>(null);
  let input = $state('');
  let sending = $state(false);
  let liveTools = $state<ToolLine[]>([]);
  let scrollEl = $state<HTMLDivElement | null>(null);
  let error = $state('');

  const suggestions = [
    '最近有什么没看完的？',
    '这季有什么新番值得追',
    '检查库里缺集',
    '今天识别了多少',
  ];

  onMount(() => {
    loadSessions();
    // SSE steward 事件 → 实时工具行
    const un = sse.subscribe((msg) => {
      if (msg.type !== 'steward') return;
      const ev = msg.agent;
      if (ev.type === 'tool_call_begin') {
        liveTools.push({ tool: ev.tool, args: ev.arguments, running: true });
        scrollDown();
      } else if (ev.type === 'tool_call_end') {
        const t = liveTools.find((l) => l.running && l.tool === ev.tool);
        if (t) {
          t.running = false;
          t.success = ev.success;
          t.duration = ev.duration_ms;
        }
      }
    });
    return un;
  });

  function loadSessions() {
    api.chatSessions().then((s) => (sessions = s)).catch(() => {});
  }

  async function openSession(id: number) {
    sessionId = id;
    messages = [];
    liveTools = [];
    error = '';
    try {
      const rows = await api.chatHistory(id);
      const out: Msg[] = [];
      for (const r of rows) {
        if (r.role === 'user') {
          out.push({ role: 'user', text: String(r.content) });
        } else if (r.role === 'steward') {
          out.push({ role: 'steward', text: String(r.content) });
        } else if (r.role === 'tool') {
          // tool 行挂到下一条 steward 消息前：先攒着，遇到 steward 消息时并入
          const t = r.content as ToolEventSnap;
          let last = out[out.length - 1];
          if (!last || last.role !== 'steward' || last.text !== '') {
            // 用空 steward 占位消息承载 tools，等真正回复来时填 text
            last = { role: 'steward', text: '', tools: [] };
            out.push(last);
          }
          (last.tools ??= []).push({
            tool: t.tool,
            args: t.arguments,
            running: false,
            success: t.success,
          });
        }
      }
      // 合并：把空文本 steward 占位与紧随的 steward 回复合并
      messages = out.reduce<Msg[]>((acc, m) => {
        const prev = acc[acc.length - 1];
        if (prev && prev.role === 'steward' && prev.text === '' && m.role === 'steward') {
          prev.text = m.text; // tool 占位与紧随回复合并为一条消息
          return acc;
        }
        acc.push(m);
        return acc;
      }, []);
      scrollDown();
    } catch (e) {
      error = String(e);
    }
  }

  function newSession() {
    sessionId = null;
    messages = [];
    liveTools = [];
    error = '';
  }

  async function send(text?: string) {
    const msg = (text ?? input).trim();
    if (!msg || sending) return;
    input = '';
    error = '';
    messages.push({ role: 'user', text: msg });
    sending = true;
    liveTools = [];
    sse.clearStewardEvents();
    scrollDown();
    try {
      const res = await api.chat(msg, sessionId);
      sessionId = res.session_id;
      // 快照兜底：SSE 可能漏（断线期间），以响应里的 tool_events 为准
      const tools: ToolLine[] =
        res.tool_events.length > 0
          ? res.tool_events.map((t) => ({
              tool: t.tool,
              args: t.arguments,
              running: false,
              success: t.success,
            }))
          : liveTools.map((t) => ({ ...t, running: false }));
      messages.push({ role: 'steward', text: res.reply, tools });
      loadSessions();
    } catch (e) {
      error = String(e);
      messages.push({ role: 'steward', text: '（请求失败）' + String(e) });
    } finally {
      sending = false;
      liveTools = [];
      scrollDown();
    }
  }

  async function scrollDown() {
    await tick();
    scrollEl?.scrollTo({ top: scrollEl.scrollHeight });
  }
</script>

<section class="view">
  <div class="view-title">管家</div>
  <div class="view-sub">你的私人媒体管理员 — 说出你的需求，或让它汇报家里的情况</div>

  <div class="steward-layout">
    <!-- 对话面板 -->
    <div class="card chat-panel">
      <div class="chat-scroll" bind:this={scrollEl}>
        {#if messages.length === 0 && !sending}
          <div class="empty">
            <div class="e-icon">n</div>
            跟管家说点什么 — 订阅、识别、整理、找片子都行
          </div>
        {/if}
        {#each messages as m}
          <div class="msg" class:user={m.role === 'user'} class:steward={m.role === 'steward'}>
            <div class="m-avatar">{m.role === 'user' ? 'S' : 'n'}</div>
            <div class="m-body">
              {#if m.tools?.length}
                {#each m.tools as t}
                  <div class="chat-tool">
                    <span>⌕</span><span class="tname">{t.tool}</span>
                    <span class="targs">{fmtArgs(t.args, 60)}</span>
                    {#if t.success === false}
                      <span class="fail">✗ 失败</span>
                    {:else}
                      <span class="ok">✓{t.duration !== undefined ? ' ' + fmtDuration(t.duration) : ''}</span>
                    {/if}
                  </div>
                {/each}
              {/if}
              {#if m.text}
                <!-- eslint-disable-next-line svelte/no-at-html-tags — renderMarkdown 已做 HTML 转义 -->
                <div class="m-text">{@html renderMarkdown(m.text)}</div>
              {/if}
            </div>
          </div>
        {/each}
        {#if sending}
          <div class="msg steward">
            <div class="m-avatar">n</div>
            <div class="m-body">
              {#each liveTools as t}
                <div class="chat-tool">
                  <span>⌕</span><span class="tname">{t.tool}</span>
                  <span class="targs">{fmtArgs(t.args, 60)}</span>
                  {#if t.running}
                    <span class="ok" style="color:var(--accent-2)">运行中…</span>
                  {:else if t.success === false}
                    <span class="fail">✗</span>
                  {:else}
                    <span class="ok">✓{t.duration !== undefined ? ' ' + fmtDuration(t.duration) : ''}</span>
                  {/if}
                </div>
              {/each}
              <div class="m-text" style="color:var(--ink-3)">思考中…</div>
            </div>
          </div>
        {/if}
      </div>

      <div class="chat-input-wrap">
        <div class="chat-suggest">
          {#each suggestions as s}
            <button class="sug" onclick={() => send(s)} disabled={sending}>{s}</button>
          {/each}
        </div>
        <div class="chat-input">
          <input
            placeholder="跟管家说点什么… 订阅、识别、整理、找片子都行"
            bind:value={input}
            onkeydown={(e) => e.key === 'Enter' && !e.isComposing && send()}
            disabled={sending}
          />
          <button class="send-btn" onclick={() => send()} disabled={sending || !input.trim()} aria-label="发送">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z"/></svg>
          </button>
        </div>
        {#if error}
          <div style="margin-top:8px;font-size:11.5px;color:#ff9d9d">{error}</div>
        {/if}
      </div>
    </div>

    <!-- 会话历史侧栏 -->
    <div>
      <div class="card">
        <div class="panel-title">
          会话历史
          <span class="count">{sessions.length} 个</span>
        </div>
        <div
          class="session-row"
          class:sel={sessionId === null}
          role="button"
          tabindex="0"
          onclick={newSession}
          onkeydown={(e) => e.key === 'Enter' && newSession()}
        >
          <b>＋ 新会话</b>
          <span>开始新的对话</span>
        </div>
        {#each sessions as s (s.id)}
          <div
            class="session-row"
            class:sel={sessionId === s.id}
            role="button"
            tabindex="0"
            onclick={() => openSession(s.id)}
            onkeydown={(e) => e.key === 'Enter' && openSession(s.id)}
          >
            <b>{s.title ?? `会话 #${s.id}`}</b>
            <span>{fmtTime(s.updated_at)}</span>
          </div>
        {/each}
        {#if sessions.length === 0}
          <div class="empty" style="padding:24px 14px">还没有历史会话</div>
        {/if}
      </div>
    </div>
  </div>
</section>
