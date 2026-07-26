<script lang="ts">
  // 媒体库首页：Hero → 继续观看 → Next Up → 每库最新添加 → 全部条目网格
  // （筛选行 + 排序菜单 + 无限滚动；批次 B 端点缺席时各 section 优雅隐藏）
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { nav } from '../lib/nav.svelte';
  import { sse } from '../lib/sse.svelte';
  import type { Item, Library, NextUpItem, ResumeItem } from '../lib/types';
  import { artClass, progressPct } from '../lib/format';
  import PosterCard from '../components/PosterCard.svelte';

  const PAGE = 60;

  // ===== 全部条目网格（筛选 + 排序 + 无限滚动） =====
  let items = $state<Item[]>([]);
  let total = $state<number | null>(null);
  let loading = $state(false);
  let loadingMore = $state(false);
  let reachedEnd = $state(false);
  let error = $state('');

  let kindFilter = $state<string | null>(null);
  let yearFilter = $state<number | null>(null);
  let playedFilter = $state<'all' | 'played' | 'unplayed'>('all');
  let favoriteOnly = $state(false);
  let sort = $state<'added_at' | 'sort_name' | 'premiere' | 'rating' | 'random'>('added_at');
  let sortMenuOpen = $state(false);
  let genreFilter = $state<string | null>(nav.query.genre ?? null);

  const SORT_LABELS: Record<string, string> = {
    added_at: '最新入库',
    sort_name: '按名称',
    premiere: '按首播',
    rating: '按评分',
    random: '随机',
  };

  const YEARS = (() => {
    const now = new Date().getFullYear();
    const out: number[] = [];
    for (let y = now; y >= 1980; y--) out.push(y);
    return out;
  })();

  function buildParams(offset: number) {
    return {
      kind: kindFilter ?? undefined,
      sort,
      year: yearFilter ?? undefined,
      genre: genreFilter ?? undefined,
      is_played:
        playedFilter === 'all' ? undefined : playedFilter === 'played',
      is_favorite: favoriteOnly ? true : undefined,
      limit: PAGE,
      offset,
    };
  }

  let loadSeq = 0;
  async function load() {
    const my = ++loadSeq;
    loading = true;
    error = '';
    reachedEnd = false;
    try {
      const { rows, total: t } = await api.itemsPaged(buildParams(0));
      if (my !== loadSeq) return;
      items = rows;
      total = t;
      if (rows.length < PAGE || (t !== null && rows.length >= t)) reachedEnd = true;
    } catch (e) {
      if (my !== loadSeq) return;
      error = String(e);
      items = [];
    } finally {
      if (my === loadSeq) loading = false;
    }
  }

  async function loadMore() {
    if (loading || loadingMore || reachedEnd || sort === 'random') return;
    const my = loadSeq;
    loadingMore = true;
    try {
      const { rows, total: t } = await api.itemsPaged(buildParams(items.length));
      if (my !== loadSeq) return;
      if (t !== null) total = t;
      if (rows.length === 0) {
        reachedEnd = true;
      } else {
        const seen = new Set(items.map((i) => i.id));
        items = [...items, ...rows.filter((r) => !seen.has(r.id))];
        if (rows.length < PAGE || (total !== null && items.length >= total)) reachedEnd = true;
      }
    } catch {
      reachedEnd = true; // offset 分页不可用时停止
    } finally {
      loadingMore = false;
    }
  }

  // 无限滚动哨兵
  let sentinel = $state<HTMLDivElement | null>(null);
  $effect(() => {
    const el = sentinel;
    if (!el) return;
    const ob = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) loadMore();
      },
      { rootMargin: '600px' }
    );
    ob.observe(el);
    return () => ob.disconnect();
  });

  // ===== sections =====
  let resume = $state<ResumeItem[]>([]);
  let nextUp = $state<NextUpItem[]>([]);
  let libraries = $state<Library[]>([]);
  /** library_id → 最新条目；-1 = 退化的全局“最新添加” */
  let latestByLib = $state<Record<number, Item[]>>({});

  async function loadSections() {
    api.resume().then((r) => (resume = Array.isArray(r) ? r : [])).catch(() => (resume = []));
    api.nextUp().then((r) => (nextUp = Array.isArray(r) ? r : [])).catch(() => (nextUp = []));
    let libs: Library[] = [];
    try {
      libs = await api.libraries();
    } catch {
      libs = [];
    }
    libraries = libs;
    if (libs.length === 0) return;
    let anyOk = false;
    await Promise.all(
      libs.map(async (lib) => {
        try {
          const rows = await api.latest(16, lib.id);
          latestByLib[lib.id] = Array.isArray(rows) ? rows : [];
          anyOk = true;
        } catch {
          latestByLib[lib.id] = [];
        }
      })
    );
    if (!anyOk) {
      // /items/latest 未上线：退化为单个“最新添加”区（sort=added_at）
      try {
        latestByLib[-1] = await api.items({ sort: 'added_at', limit: 16 });
      } catch {
        latestByLib[-1] = [];
      }
    }
  }

  onMount(() => {
    load();
    loadSections();
    const un = sse.subscribe((msg) => {
      if (msg.type === 'scrape_update' && msg.state === 'done') {
        load();
        loadSections();
      }
      if (msg.type === 'scan_progress' && /完成|done/i.test(msg.message)) load();
    });
    const closeMenu = () => (sortMenuOpen = false);
    window.addEventListener('click', closeMenu);
    return () => {
      un();
      window.removeEventListener('click', closeMenu);
    };
  });

  // #/library?genre=X 变化时同步
  $effect(() => {
    const g = nav.query.genre ?? null;
    if (g !== genreFilter) {
      genreFilter = g;
      load();
    }
  });

  const hero = $derived(items.find((i) => i.kind === 'series') ?? items[0] ?? null);
  const scanningTasks = $derived(
    Object.entries(sse.scrapeStates)
      .filter(([, s]) => s === 'running' || s === 'queued')
      .map(([id, s]) => ({ id: Number(id), state: s }))
  );

  const latestSections = $derived.by(() => {
    const out: { key: number; name: string; items: Item[] }[] = [];
    for (const lib of libraries) {
      const rows = latestByLib[lib.id];
      if (rows?.length) out.push({ key: lib.id, name: lib.name ?? lib.path, items: rows });
    }
    const fallback = latestByLib[-1];
    if (out.length === 0 && fallback?.length) {
      out.push({ key: -1, name: '', items: fallback });
    }
    return out;
  });

  function setKind(k: string | null) {
    kindFilter = k;
    load();
  }
  function setSort(s: typeof sort) {
    sort = s;
    sortMenuOpen = false;
    load();
  }
  function cyclePlayed() {
    playedFilter = playedFilter === 'all' ? 'played' : playedFilter === 'played' ? 'unplayed' : 'all';
    load();
  }
  function clearGenre() {
    genreFilter = null;
    nav.go('library');
    load();
  }
</script>

<section class="view">
  {#if hero}
    <div
      class="hero"
      role="button"
      tabindex="0"
      onclick={() => nav.goItem(hero.id)}
      onkeydown={(e) => e.key === 'Enter' && nav.goItem(hero.id)}
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
          <button class="btn btn-primary" onclick={(e) => { e.stopPropagation(); nav.goItem(hero.id); }}>查看详情</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- ===== 继续观看 ===== -->
  {#if resume.length}
    <div class="sec-title">继续观看</div>
    <div class="h-scroll">
      {#each resume as r (r.id)}
        <PosterCard
          item={r}
          fixedWidth
          progress={progressPct(r.position_ms, r.duration_ms ?? r.runtime_ms)}
          sub={r.series_title ?? undefined}
        />
      {/each}
    </div>
  {/if}

  <!-- ===== Next Up ===== -->
  {#if nextUp.length}
    <div class="sec-title">接下来看</div>
    <div class="h-scroll">
      {#each nextUp as n (n.id)}
        <PosterCard item={n} fixedWidth sub={n.series_title ?? undefined} />
      {/each}
    </div>
  {/if}

  <!-- ===== 每库最新添加 ===== -->
  {#each latestSections as sec (sec.key)}
    <div class="sec-title">{sec.name ? `最新添加 · ${sec.name}` : '最新添加'}</div>
    <div class="h-scroll">
      {#each sec.items as it (it.id)}
        <PosterCard item={it} fixedWidth />
      {/each}
    </div>
  {/each}

  <!-- ===== 全部条目 ===== -->
  <div class="sec-title" style="margin-top:32px">
    全部条目
    {#if total !== null}<span class="count-hint">{total} 项</span>{/if}
  </div>
  <div class="chip-row" style="flex-wrap:wrap">
    <button class="chip" class:on={kindFilter === null} onclick={() => setKind(null)}>全部</button>
    <button class="chip" class:on={kindFilter === 'series'} onclick={() => setKind('series')}>剧集</button>
    <button class="chip" class:on={kindFilter === 'movie'} onclick={() => setKind('movie')}>电影</button>

    {#if genreFilter}
      <button class="chip on" title="清除类型筛选" onclick={clearGenre}>{genreFilter} ✕</button>
    {/if}

    <select
      class="year-select"
      value={yearFilter === null ? '' : String(yearFilter)}
      onchange={(e) => {
        const v = (e.currentTarget as HTMLSelectElement).value;
        yearFilter = v ? Number(v) : null;
        load();
      }}
    >
      <option value="">全部年份</option>
      {#each YEARS as y (y)}
        <option value={String(y)}>{y}</option>
      {/each}
    </select>

    <button
      class="chip"
      class:on={playedFilter !== 'all'}
      title="已看状态筛选（三态）"
      onclick={cyclePlayed}
    >
      {playedFilter === 'all' ? '已看状态' : playedFilter === 'played' ? '✓ 已看' : '未看'}
    </button>
    <button
      class="chip"
      class:on={favoriteOnly}
      onclick={() => { favoriteOnly = !favoriteOnly; load(); }}
    >♥ 收藏</button>

    <div style="flex:1"></div>

    <div class="sort-wrap">
      <button
        class="chip"
        onclick={(e) => { e.stopPropagation(); sortMenuOpen = !sortMenuOpen; }}
      >{SORT_LABELS[sort]} ▾</button>
      {#if sortMenuOpen}
        <div class="sort-menu" role="menu">
          {#each Object.entries(SORT_LABELS) as [key, label] (key)}
            <button
              class="sort-opt"
              class:on={sort === key}
              role="menuitem"
              onclick={() => setSort(key as typeof sort)}
            >{label}{#if sort === key}<span>✓</span>{/if}</button>
          {/each}
        </div>
      {/if}
    </div>
  </div>

  {#if loading && items.length === 0}
    <div class="empty">加载中…</div>
  {:else if error}
    <div class="empty"><div class="e-icon">!</div>{error}</div>
  {:else if items.length === 0 && scanningTasks.length === 0}
    <div class="empty">
      <div class="e-icon">◫</div>
      {#if kindFilter || yearFilter || genreFilter || playedFilter !== 'all' || favoriteOnly}
        没有符合筛选条件的条目
      {:else}
        库里还没有条目 — 先在设置里添加媒体库并触发扫描
        <div class="e-act"><a class="btn btn-ghost btn-sm" href="#/settings">去设置 →</a></div>
      {/if}
    </div>
  {:else}
    <div class="poster-grid">
      {#each items as item (item.id)}
        <PosterCard {item} />
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
    <div bind:this={sentinel} style="height:1px"></div>
    {#if loadingMore}
      <div class="empty" style="padding:18px">加载更多…</div>
    {/if}
  {/if}
</section>
