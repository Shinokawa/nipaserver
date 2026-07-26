<script lang="ts">
  // 媒体库：Hero（最新 series）+ 海报墙 + 扫描中占位卡 + 详情浮层（docs/05 §4.1）
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { sse } from '../lib/sse.svelte';
  import type { Item } from '../lib/types';
  import { artClass } from '../lib/format';
  import ItemDetailModal from '../components/ItemDetailModal.svelte';

  let items = $state<Item[]>([]);
  let loading = $state(true);
  let error = $state('');
  let kindFilter = $state<string | null>(null); // null=全部
  let sort = $state<'added_at' | 'air_date' | 'title'>('added_at');
  let selectedId = $state<number | null>(null);
  /** 每个 series 的集数（列表接口不带，从 hero 详情外不额外拉；用 SSE 后刷新） */

  async function load() {
    loading = true;
    error = '';
    try {
      items = await api.items({
        kind: kindFilter ?? undefined,
        sort,
        limit: 100,
      });
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    load();
    // 任务完成（done）→ 可能有新条目入库，轻量刷新
    const un = sse.subscribe((msg) => {
      if (msg.type === 'scrape_update' && msg.state === 'done') load();
      if (msg.type === 'scan_progress' && /完成|done/i.test(msg.message)) load();
    });
    return un;
  });

  const hero = $derived(items.find((i) => i.kind === 'series') ?? items[0] ?? null);
  const gridItems = $derived(items.filter((i) => i.id !== hero?.id));
  /** 扫描中占位卡：SSE scrape_update 里 state=running/queued 的任务 */
  const scanningTasks = $derived(
    Object.entries(sse.scrapeStates)
      .filter(([, s]) => s === 'running' || s === 'queued')
      .map(([id, s]) => ({ id: Number(id), state: s }))
  );

  function setKind(k: string | null) {
    kindFilter = k;
    load();
  }
</script>

<section class="view">
  {#if hero}
    <div
      class="hero"
      role="button"
      tabindex="0"
      onclick={() => (selectedId = hero.id)}
      onkeydown={(e) => e.key === 'Enter' && (selectedId = hero.id)}
    >
      <div class="h-art {hero.poster_path ? '' : artClass(hero.id)}"></div>
      <div class="h-meta">
        <div class="h-tag">
          <span class="badge acc"><span class="bdot"></span>最新入库</span>
          <span class="badge">{hero.kind === 'series' ? 'TV' : hero.kind}{hero.year ? ` · ${hero.year}` : ''}</span>
          {#if hero.air_date}<span class="badge">首播 {hero.air_date}</span>{/if}
        </div>
        <div class="h-title">{hero.title ?? '（未识别）'}</div>
        <div class="h-sub">
          {hero.original_title && hero.original_title !== hero.title ? hero.original_title : ''}
        </div>
        <div class="h-actions">
          <button class="btn btn-primary" onclick={(e) => { e.stopPropagation(); selectedId = hero.id; }}>查看详情</button>
        </div>
      </div>
    </div>
  {/if}

  <div class="chip-row">
    <button class="chip" class:on={kindFilter === null} onclick={() => setKind(null)}>全部</button>
    <button class="chip" class:on={kindFilter === 'series'} onclick={() => setKind('series')}>剧集</button>
    <button class="chip" class:on={kindFilter === 'movie'} onclick={() => setKind('movie')}>电影</button>
    <div style="flex:1"></div>
    <button
      class="chip"
      onclick={() => {
        sort = sort === 'added_at' ? 'air_date' : sort === 'air_date' ? 'title' : 'added_at';
        load();
      }}
    >
      {sort === 'added_at' ? '最新入库' : sort === 'air_date' ? '按首播' : '按标题'} ▾
    </button>
  </div>

  <div class="sec-title">最新入库</div>
  {#if loading && items.length === 0}
    <div class="empty">加载中…</div>
  {:else if error}
    <div class="empty"><div class="e-icon">!</div>{error}</div>
  {:else if gridItems.length === 0 && scanningTasks.length === 0 && !hero}
    <div class="empty">
      <div class="e-icon">◫</div>
      库里还没有条目 — 先在设置里添加媒体库并触发扫描
      <div class="e-act"><a class="btn btn-ghost btn-sm" href="#/settings">去设置 →</a></div>
    </div>
  {:else}
    <div class="poster-grid">
      {#each gridItems as item (item.id)}
        <div
          class="poster"
          role="button"
          tabindex="0"
          onclick={() => (selectedId = item.id)}
          onkeydown={(e) => e.key === 'Enter' && (selectedId = item.id)}
        >
          <div class="art {item.poster_path ? '' : artClass(item.id)}">
            {#if item.poster_path}
              <img src={item.poster_path} alt={item.title ?? ''} loading="lazy" />
            {/if}
            <div class="hover-meta">
              <b>{item.title ?? '（未识别）'}</b>
              <span>{item.year ?? ''}{item.year ? ' · ' : ''}{item.kind === 'series' ? 'TV' : item.kind}</span>
            </div>
          </div>
          <div class="p-title">{item.title ?? '（未识别）'}</div>
          <div class="p-sub">{item.year ?? ''}{item.air_date ? ` · ${item.air_date}` : ''}</div>
        </div>
      {/each}
      {#each scanningTasks as t (t.id)}
        <div class="poster scanning">
          <div class="art">
            <div class="scan-tag">
              <div>
                <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="#898781" stroke-width="1.6"><circle cx="12" cy="12" r="3.2"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9L17 7M7 17l-2.1 2.1"/></svg>
                <span>{t.state === 'running' ? 'AI 识别中…' : '队列中'}</span>
              </div>
            </div>
          </div>
          <div class="p-title" style="color:var(--ink-3)">任务 #{t.id}</div>
          <div class="p-sub">{t.state === 'running' ? 'L2 · 识别中' : '等待调度'}</div>
        </div>
      {/each}
    </div>
  {/if}
</section>

{#if selectedId !== null}
  <ItemDetailModal itemId={selectedId} onclose={() => (selectedId = null)} />
{/if}
