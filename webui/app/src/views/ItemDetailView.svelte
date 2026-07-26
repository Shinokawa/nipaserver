<script lang="ts">
  // 条目详情独立路由页 #/item/{id}（Jellyfin 详情页形态，webui-audit §3）
  import { api, imageUrl } from '../lib/api';
  import { nav } from '../lib/nav.svelte';
  import { userdata } from '../lib/userdata.svelte';
  import { toast } from '../lib/toast.svelte';
  import type { Item, ItemDetail } from '../lib/types';
  import {
    artClass,
    fmtRuntime,
    fmtSize,
    fmtYearSpan,
    progressPct,
    remainingMinutes,
  } from '../lib/format';

  let { itemId }: { itemId: number } = $props();

  let detail = $state<ItemDetail | null>(null);
  let error = $state('');
  let overviewExpanded = $state(false);
  let identOpen = $state(false);
  /** 选中的季 tab：season item id 或（按 season_no 分组时）`no:{n}` */
  let seasonKey = $state<string | null>(null);
  /** season item id → 该季集列表（懒取） */
  let seasonEps = $state<Record<number, Item[]>>({});

  $effect(() => {
    const id = itemId;
    detail = null;
    error = '';
    overviewExpanded = false;
    identOpen = false;
    seasonKey = null;
    seasonEps = {};
    api
      .item(id)
      .then((d) => {
        if (itemId === id) detail = d;
      })
      .catch((e) => (error = String(e)));
  });

  // ===== 季/集结构：children 可能是 season 列表或 episode 平铺 =====
  const childSeasons = $derived(
    (detail?.children ?? []).filter((c) => c.kind === 'season')
  );
  const childEpisodes = $derived(
    (detail?.children ?? []).filter((c) => c.kind === 'episode')
  );
  /** episode 平铺时按 season_no 分组的 tab 键列表 */
  const episodeGroups = $derived.by(() => {
    const set = new Set<number | null>();
    for (const e of childEpisodes) set.add(e.season_no ?? null);
    return [...set].sort((a, b) => (a ?? 0) - (b ?? 0));
  });

  interface SeasonTab {
    key: string;
    label: string;
  }
  const seasonTabs = $derived.by<SeasonTab[]>(() => {
    if (childSeasons.length > 0) {
      return childSeasons.map((s) => ({
        key: `id:${s.id}`,
        label: s.title ?? (s.season_no != null ? `第 ${s.season_no} 季` : `季 #${s.id}`),
      }));
    }
    if (episodeGroups.length > 1) {
      return episodeGroups.map((no) => ({
        key: `no:${no ?? 'x'}`,
        label: no != null ? (no === 0 ? '特别篇' : `第 ${no} 季`) : '未分季',
      }));
    }
    return [];
  });

  const activeSeasonKey = $derived(seasonKey ?? seasonTabs[0]?.key ?? null);

  const visibleEpisodes = $derived.by<Item[]>(() => {
    if (childSeasons.length > 0) {
      const key = activeSeasonKey;
      if (!key?.startsWith('id:')) return [];
      const sid = Number(key.slice(3));
      return seasonEps[sid] ?? [];
    }
    if (episodeGroups.length > 1) {
      const key = activeSeasonKey;
      const no = key === 'no:x' ? null : Number(key?.slice(3));
      return childEpisodes.filter((e) => (e.season_no ?? null) === no);
    }
    return childEpisodes;
  });

  // season 子层懒取
  $effect(() => {
    const key = activeSeasonKey;
    if (!key?.startsWith('id:')) return;
    const sid = Number(key.slice(3));
    if (seasonEps[sid]) return;
    api
      .item(sid)
      .then((d) => {
        seasonEps[sid] = d.children.filter((c) => c.kind === 'episode');
      })
      .catch(() => {
        seasonEps[sid] = [];
      });
  });

  // ===== meta 行 =====
  const played = $derived(detail ? userdata.isPlayed(detail) : false);
  const fav = $derived(detail ? userdata.isFavorite(detail) : false);
  const positionMs = $derived(detail ? userdata.positionMs(detail) : 0);
  const remainMin = $derived(detail ? remainingMinutes(detail.runtime_ms, positionMs) : null);
  const yearSpan = $derived(
    detail
      ? detail.kind === 'series'
        ? fmtYearSpan(detail.year, detail.series_status, detail.end_date)
        : detail.year != null
          ? String(detail.year)
          : ''
      : ''
  );

  const backdrop = $derived(detail ? imageUrl(detail, 'backdrop', 1280) : null);
  const poster = $derived(detail ? imageUrl(detail, 'poster', 400) : null);

  const overviewLong = $derived((detail?.overview ?? '').length > 260);

  const taskId = $derived(detail?.scrape_task_id ?? detail?.task_id ?? null);

  function idLink(provider: string, id: string): string | null {
    switch (provider) {
      case 'tmdb':
        return `https://www.themoviedb.org/${detail?.kind === 'movie' ? 'movie' : 'tv'}/${id}`;
      case 'bangumi':
        return `https://bgm.tv/subject/${id}`;
      case 'dandanplay':
        return `https://www.dandanplay.com/anime/${id}`;
      default:
        return null;
    }
  }

  function hasDanmaku(ep: Item): boolean {
    return (ep.external_ids ?? []).some(([p]) => p === 'dandanplay' || p === 'dandan');
  }

  function onPlay() {
    if (detail?.files?.length) {
      const p = detail.files[0].rel_path;
      navigator.clipboard
        ?.writeText(p)
        .then(() => toast.show(`已复制文件路径：${p.length > 48 ? p.slice(0, 48) + '…' : p}`, 'good'))
        .catch(() => toast.show('播放功能 M3 到来', 'info'));
    } else {
      toast.show('播放功能 M3 到来', 'info');
    }
  }

  function goGenre(g: string) {
    nav.go('library', { genre: g });
  }

  function personInitial(name: string): string {
    return name.trim().slice(0, 1).toUpperCase() || '?';
  }

  function epLabel(ep: Item): string {
    const parts: string[] = [];
    if (ep.episode_no != null) parts.push(`${ep.episode_no}.`);
    parts.push(ep.title ?? '（无标题）');
    return parts.join(' ');
  }
</script>

{#if error}
  <section class="view">
    <div class="empty"><div class="e-icon">!</div>{error}</div>
    <div style="text-align:center"><a class="btn btn-ghost btn-sm" href="#/library">← 返回媒体库</a></div>
  </section>
{:else if !detail}
  <section class="view"><div class="empty">加载中…</div></section>
{:else}
  <div class="detail-page">
    <!-- ===== backdrop 头部 ===== -->
    <div class="dp-backdrop">
      {#if backdrop}
        <img class="dp-bg" src={backdrop} alt="" />
      {:else if poster}
        <img class="dp-bg blurred" src={poster} alt="" />
      {:else}
        <div class="dp-bg {artClass(detail.id)}"></div>
      {/if}
      <div class="dp-fade"></div>
      <a class="dp-back" href="#/library" aria-label="返回">← 媒体库</a>
    </div>

    <div class="dp-body">
      <div class="dp-head">
        <div class="dp-poster {poster ? '' : artClass(detail.id)}">
          {#if poster}<img src={poster} alt={detail.title ?? ''} />{/if}
        </div>

        <div class="dp-meta">
          <h1 class="dp-title">{detail.title ?? '（未识别）'}</h1>
          {#if detail.original_title && detail.original_title !== detail.title}
            <div class="dp-orig">{detail.original_title}</div>
          {/if}

          <div class="dp-metarow">
            {#if yearSpan}<span>{yearSpan}</span>{/if}
            {#if fmtRuntime(detail.runtime_ms)}<span>{fmtRuntime(detail.runtime_ms)}</span>{/if}
            {#if detail.official_rating}<span class="rating-badge">{detail.official_rating}</span>{/if}
            {#if detail.rating != null}
              <span class="star-rating" title="社区评分">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="#fab219"><path d="M12 2l2.9 6.3 6.9.8-5.1 4.7 1.4 6.8L12 17.2 5.9 20.6l1.4-6.8L2.2 9.1l6.9-.8z"/></svg>
                {detail.rating.toFixed(1)}
              </span>
            {/if}
            {#if detail.air_date && detail.kind === 'episode'}<span>首播 {detail.air_date}</span>{/if}
          </div>

          {#if detail.tagline}
            <div class="dp-tagline">{detail.tagline}</div>
          {/if}

          {#if detail.overview}
            <div class="dp-overview" class:clamped={!overviewExpanded && overviewLong}>
              {detail.overview}
            </div>
            {#if overviewLong}
              <button class="dp-more" onclick={() => (overviewExpanded = !overviewExpanded)}>
                {overviewExpanded ? '收起 ▴' : '展开更多 ▾'}
              </button>
            {/if}
          {/if}

          <!-- 播放按钮组 -->
          <div class="dp-actions">
            <button class="btn btn-primary" onclick={onPlay}>
              <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
              {#if positionMs > 0}
                继续{remainMin != null ? ` · 剩余 ${remainMin} 分钟` : ''}
              {:else}
                播放
              {/if}
            </button>
            <button
              class="btn btn-ghost act-toggle"
              class:on-good={played}
              title={played ? '标记未看' : '标记已看'}
              onclick={() => detail && userdata.togglePlayed(detail)}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6"><path d="M4 12.5l5 5L20 6.5"/></svg>
              {played ? '已看' : '标记已看'}
            </button>
            <button
              class="btn btn-ghost act-toggle"
              class:on-crit={fav}
              title={fav ? '取消收藏' : '收藏'}
              onclick={() => detail && userdata.toggleFavorite(detail)}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill={fav ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2"><path d="M12 21s-7.5-4.7-9.5-9C1 8.5 3 5 6.5 5c2 0 3.5 1 5.5 3.2C14 6 15.5 5 17.5 5 21 5 23 8.5 21.5 12c-2 4.3-9.5 9-9.5 9z"/></svg>
              {fav ? '已收藏' : '收藏'}
            </button>
          </div>

          <!-- genres / studios 链接行 -->
          {#if detail.genres?.length || detail.studios?.length}
            <div class="dp-links">
              {#if detail.genres?.length}
                <div class="dpl-row">
                  <span class="dpl-k">类型</span>
                  {#each detail.genres as g, i (g)}
                    {#if i > 0}<span class="dpl-sep">·</span>{/if}
                    <button class="dpl-link" onclick={() => goGenre(g)}>{g}</button>
                  {/each}
                </div>
              {/if}
              {#if detail.studios?.length}
                <div class="dpl-row">
                  <span class="dpl-k">制作</span>
                  {#each detail.studios as s, i (s)}
                    {#if i > 0}<span class="dpl-sep">·</span>{/if}
                    <span class="dpl-plain">{s}</span>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}

          <!-- 外部 id 徽章行 -->
          {#if detail.external_ids.length}
            <div class="d-tags" style="margin-top:12px">
              {#each detail.external_ids as [provider, exId] (provider + exId)}
                {@const href = idLink(provider, exId)}
                {#if href}
                  <a {href} target="_blank" rel="noreferrer" style="text-decoration:none">
                    <span class="badge good"><span class="bdot"></span>{provider} {exId}</span>
                  </a>
                {:else}
                  <span class="badge"><span class="bdot"></span>{provider} {exId}</span>
                {/if}
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <!-- ===== 季 Tab + 集列表 ===== -->
      {#if seasonTabs.length > 0 || visibleEpisodes.length > 0}
        <div class="dp-sec">
          <div class="dp-sec-head">
            <h2>集列表</h2>
            {#if seasonTabs.length > 0}
              <div class="season-tabs">
                {#each seasonTabs as t (t.key)}
                  <button
                    class="chip"
                    class:on={activeSeasonKey === t.key}
                    onclick={() => (seasonKey = t.key)}
                  >{t.label}</button>
                {/each}
              </div>
            {/if}
          </div>
          {#if visibleEpisodes.length === 0}
            <div class="empty" style="padding:26px">该季暂无集数据</div>
          {:else}
            <div class="ep-list">
              {#each visibleEpisodes as ep (ep.id)}
                {@const epPoster = imageUrl(ep, 'poster', 300)}
                {@const epPlayed = userdata.isPlayed(ep)}
                {@const epPos = userdata.positionMs(ep)}
                {@const epPct = ep.runtime_ms ? progressPct(epPos, ep.runtime_ms) : 0}
                <div
                  class="ep-card"
                  role="button"
                  tabindex="0"
                  onclick={() => nav.goItem(ep.id)}
                  onkeydown={(e) => e.key === 'Enter' && nav.goItem(ep.id)}
                >
                  <div class="ep-thumb {epPoster || poster ? '' : artClass(ep.id)}">
                    {#if epPoster}
                      <img src={epPoster} alt="" loading="lazy" />
                    {:else if poster}
                      <img src={poster} alt="" loading="lazy" />
                    {/if}
                    {#if epPlayed}<span class="corner-check" title="已看">✓</span>{/if}
                    {#if epPct > 0 && epPct < 100}
                      <div class="watch-bar"><i style="width:{epPct}%"></i></div>
                    {/if}
                  </div>
                  <div class="ep-info">
                    <div class="ep-head-row">
                      <b class="ep-name">{epLabel(ep)}</b>
                      {#if hasDanmaku(ep)}
                        <span class="badge acc" title="弹幕已关联"><span class="bdot"></span>弹幕</span>
                      {/if}
                    </div>
                    {#if ep.overview}
                      <div class="ep-ov">{ep.overview}</div>
                    {/if}
                    <div class="ep-meta">
                      {#if fmtRuntime(ep.runtime_ms)}<span>{fmtRuntime(ep.runtime_ms)}</span>{/if}
                      {#if ep.air_date}<span>{ep.air_date}</span>{/if}
                    </div>
                  </div>
                  <div class="ep-acts">
                    <button
                      class="ha-btn"
                      class:on={epPlayed}
                      title={epPlayed ? '标记未看' : '标记已看'}
                      aria-label={epPlayed ? '标记未看' : '标记已看'}
                      onclick={(e) => { e.stopPropagation(); userdata.togglePlayed(ep); }}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><path d="M4 12.5l5 5L20 6.5"/></svg>
                    </button>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- ===== 演职员横滚 ===== -->
      {#if detail.people?.length}
        <div class="dp-sec">
          <div class="dp-sec-head"><h2>演职员</h2></div>
          <div class="people-scroll">
            {#each detail.people as p, i (p.name + i)}
              <div class="person-card">
                <div class="person-avatar {p.image_url ? '' : artClass(i + detail.id)}">
                  {#if p.image_url}
                    <img src={p.image_url} alt={p.name} loading="lazy" />
                  {:else}
                    <span>{personInitial(p.name)}</span>
                  {/if}
                </div>
                <div class="person-name">{p.name}</div>
                <div class="person-role">
                  {#if p.role}饰 {p.role}{:else if p.kind}{p.kind}{/if}
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- ===== 文件 ===== -->
      {#if detail.files.length}
        <div class="dp-sec">
          <div class="dp-sec-head"><h2>文件</h2></div>
          {#each detail.files as f (f.id)}
            <div class="file-row">
              <span style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{f.rel_path}</span>
              <span class="fsize">{fmtSize(f.size)}</span>
            </div>
          {/each}
        </div>
      {/if}

      <!-- ===== 识别信息折叠区 ===== -->
      <div class="dp-sec">
        <button class="ident-toggle" onclick={() => (identOpen = !identOpen)}>
          <span class="it-caret" class:open={identOpen}>▸</span>
          识别信息
          <span class="it-hint">这个条目是怎么被认出来的</span>
        </button>
        {#if identOpen}
          <div class="ident-body">
            {#if taskId != null}
              <p>该条目由识别任务 <b style="font-family:var(--mono)">#{taskId}</b> 产出。</p>
              <a class="btn btn-ghost btn-sm" href="#/console?task={taskId}">在控制台查看识别过程 →</a>
            {:else}
              <p style="color:var(--ink-3)">暂无关联识别任务记录（可能为手动入库或后端尚未输出关联字段）。</p>
              <a class="btn btn-ghost btn-sm" href="#/console">打开 Agent 控制台 →</a>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}
