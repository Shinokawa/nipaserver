<script lang="ts">
  // ⌘K 全局搜索覆盖层：300ms 防抖 → /search → 按 kind 分组，↑↓ 选择、回车跳详情
  import { api } from '../lib/api';
  import { nav } from '../lib/nav.svelte';
  import { artClass } from '../lib/format';
  import { imageUrl } from '../lib/api';
  import type { Item, SearchGroups } from '../lib/types';

  let { onclose }: { onclose: () => void } = $props();

  let q = $state('');
  let groups = $state<SearchGroups | null>(null);
  let searching = $state(false);
  let failed = $state(false);
  let selIndex = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let seq = 0;

  $effect(() => {
    inputEl?.focus();
  });

  function onInput() {
    if (debounceTimer) clearTimeout(debounceTimer);
    const query = q.trim();
    if (!query) {
      groups = null;
      failed = false;
      return;
    }
    debounceTimer = setTimeout(() => run(query), 300);
  }

  async function run(query: string) {
    const my = ++seq;
    searching = true;
    failed = false;
    try {
      const g = await api.search(query);
      if (my !== seq) return;
      groups = g;
      selIndex = 0;
    } catch {
      if (my !== seq) return;
      // /search 未上线：退化到 /items?search=
      try {
        const rows = await api.items({ search: query, limit: 30 });
        if (my !== seq) return;
        const g: SearchGroups = { series: [], movies: [], episodes: [], other: [] };
        for (const it of rows) {
          if (it.kind === 'series') g.series.push(it);
          else if (it.kind === 'movie') g.movies.push(it);
          else if (it.kind === 'episode') g.episodes.push(it);
          else g.other.push(it);
        }
        groups = g;
        selIndex = 0;
      } catch {
        if (my !== seq) return;
        groups = null;
        failed = true;
      }
    } finally {
      if (my === seq) searching = false;
    }
  }

  interface Row {
    kind: 'header' | 'item';
    label?: string;
    item?: Item;
  }
  const rows = $derived.by<Row[]>(() => {
    if (!groups) return [];
    const out: Row[] = [];
    const sections: [string, Item[]][] = [
      ['剧集', groups.series],
      ['电影', groups.movies],
      ['单集', groups.episodes],
      ['其他', groups.other],
    ];
    for (const [label, items] of sections) {
      if (!items.length) continue;
      out.push({ kind: 'header', label });
      for (const it of items.slice(0, 8)) out.push({ kind: 'item', item: it });
    }
    return out;
  });
  const itemRows = $derived(rows.filter((r) => r.kind === 'item'));

  function open(it: Item) {
    onclose();
    nav.goItem(it.id);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      selIndex = Math.min(selIndex + 1, itemRows.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selIndex = Math.max(selIndex - 1, 0);
    } else if (e.key === 'Enter') {
      const it = itemRows[selIndex]?.item;
      if (it) open(it);
    }
  }

  const hasResults = $derived(itemRows.length > 0);
</script>

<div
  class="overlay search-overlay"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && onclose()}
>
  <div class="search-panel" role="dialog" aria-modal="true" aria-label="搜索">
    <div class="sp-input">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>
      <input
        bind:this={inputEl}
        bind:value={q}
        oninput={onInput}
        onkeydown={onKeydown}
        placeholder="搜索条目…"
        spellcheck="false"
      />
      {#if searching}<span class="sp-spin"></span>{/if}
      <kbd>esc</kbd>
    </div>
    <div class="sp-results">
      {#if !q.trim()}
        <div class="sp-hint">输入以搜索 · ↑↓ 选择 · 回车打开</div>
      {:else if failed}
        <div class="sp-hint">搜索暂不可用（后端搜索端点尚未就绪）</div>
      {:else if !hasResults && !searching}
        <div class="sp-hint">没有匹配 “{q.trim()}” 的条目</div>
      {:else}
        {#each rows as row, ri (ri)}
          {#if row.kind === 'header'}
            <div class="sp-group">{row.label}</div>
          {:else if row.item}
            {@const it = row.item}
            {@const idx = itemRows.indexOf(row)}
            {@const poster = imageUrl(it, 'poster', 100)}
            <div
              class="sp-row"
              class:sel={idx === selIndex}
              role="button"
              tabindex="0"
              onclick={() => open(it)}
              onkeydown={(e) => e.key === 'Enter' && open(it)}
              onmousemove={() => (selIndex = idx)}
            >
              <div class="sp-poster {poster ? '' : artClass(it.id)}">
                {#if poster}<img src={poster} alt="" loading="lazy" />{/if}
              </div>
              <div class="sp-info">
                <b>{it.title ?? '（未识别）'}</b>
                <span>
                  {it.kind === 'series' ? 'TV' : it.kind}{it.year ? ` · ${it.year}` : ''}
                  {#if it.kind === 'episode' && it.season_no != null && it.episode_no != null}
                    · S{String(it.season_no).padStart(2, '0')}E{String(it.episode_no).padStart(2, '0')}
                  {/if}
                </span>
              </div>
              <span class="sp-enter">↵</span>
            </div>
          {/if}
        {/each}
      {/if}
    </div>
  </div>
</div>
