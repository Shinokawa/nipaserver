<script lang="ts">
  // 海报卡：hover 按钮组（▶/✓/♥）+ 已看角标 + 未看数角标 + 可选底部进度条
  import { nav } from '../lib/nav.svelte';
  import { artClass, progressPct } from '../lib/format';
  import { imageUrl } from '../lib/api';
  import { userdata } from '../lib/userdata.svelte';
  import { toast } from '../lib/toast.svelte';
  import type { Item } from '../lib/types';

  let {
    item,
    sub,
    progress = null,
    fixedWidth = false,
  }: {
    item: Item;
    /** 覆盖副标题（如 "剩余 23 分钟" / series_title） */
    sub?: string;
    /** 进度 0-100（继续观看横滚）；null=不显示 */
    progress?: number | null;
    /** 横滚里用固定宽度 */
    fixedWidth?: boolean;
  } = $props();

  const poster = $derived(imageUrl(item, 'poster', 300));
  const played = $derived(userdata.isPlayed(item));
  const fav = $derived(userdata.isFavorite(item));
  const pos = $derived(userdata.positionMs(item));
  const pct = $derived(
    progress ?? (pos > 0 && item.runtime_ms ? progressPct(pos, item.runtime_ms) : null)
  );

  function open() {
    nav.goItem(item.id);
  }
  function playStub(e: MouseEvent) {
    e.stopPropagation();
    toast.show('播放功能 M3 到来', 'info');
  }
</script>

<div
  class="poster"
  class:fixed-w={fixedWidth}
  role="button"
  tabindex="0"
  onclick={open}
  onkeydown={(e) => e.key === 'Enter' && open()}
>
  <div class="art {poster ? '' : artClass(item.id)}">
    {#if poster}
      <img src={poster} alt={item.title ?? ''} loading="lazy" />
    {/if}
    {#if played}
      <span class="corner-check" title="已看">✓</span>
    {:else if item.unplayed_count != null && item.unplayed_count > 0}
      <span class="ep-badge">{item.unplayed_count}</span>
    {/if}
    <div class="hover-meta">
      <b>{item.title ?? '（未识别）'}</b>
      <span>{item.year ?? ''}{item.year ? ' · ' : ''}{item.kind === 'series' ? 'TV' : item.kind}</span>
    </div>
    <div class="hover-actions">
      <button class="ha-btn primary" title="播放" aria-label="播放" onclick={playStub}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
      </button>
      <button
        class="ha-btn"
        class:on={played}
        title={played ? '标记未看' : '标记已看'}
        aria-label={played ? '标记未看' : '标记已看'}
        onclick={(e) => { e.stopPropagation(); userdata.togglePlayed(item); }}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><path d="M4 12.5l5 5L20 6.5"/></svg>
      </button>
      <button
        class="ha-btn"
        class:on={fav}
        title={fav ? '取消收藏' : '收藏'}
        aria-label={fav ? '取消收藏' : '收藏'}
        onclick={(e) => { e.stopPropagation(); userdata.toggleFavorite(item); }}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill={fav ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.2"><path d="M12 21s-7.5-4.7-9.5-9C1 8.5 3 5 6.5 5c2 0 3.5 1 5.5 3.2C14 6 15.5 5 17.5 5 21 5 23 8.5 21.5 12c-2 4.3-9.5 9-9.5 9z"/></svg>
      </button>
    </div>
    {#if pct !== null && pct > 0 && pct < 100}
      <div class="watch-bar"><i style="width:{pct}%"></i></div>
    {/if}
  </div>
  <div class="p-title">{item.title ?? '（未识别）'}</div>
  <div class="p-sub">{sub ?? `${item.year ?? ''}${item.air_date ? (item.year ? ' · ' : '') + item.air_date : ''}`}</div>
</div>
