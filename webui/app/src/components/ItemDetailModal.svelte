<script lang="ts">
  // 条目详情浮层（GET /items/{id}：external_ids 徽章 / children 集列表 / files）
  import { api } from '../lib/api';
  import type { ItemDetail } from '../lib/types';
  import { artClass, fmtSize } from '../lib/format';

  let { itemId, onclose }: { itemId: number; onclose: () => void } = $props();

  let detail = $state<ItemDetail | null>(null);
  let error = $state('');

  $effect(() => {
    detail = null;
    error = '';
    api
      .item(itemId)
      .then((d) => (detail = d))
      .catch((e) => (error = String(e)));
  });

  function idLink(provider: string, id: string): string | null {
    switch (provider) {
      case 'tmdb':
        return `https://www.themoviedb.org/${detail?.kind === 'movie' ? 'movie' : 'tv'}/${id}`;
      case 'bangumi':
        return `https://bgm.tv/subject/${id}`;
      default:
        return null;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="overlay" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="detail-modal" role="dialog" aria-modal="true">
    <button class="detail-close" onclick={onclose} aria-label="关闭">✕</button>
    {#if error}
      <div class="empty"><div class="e-icon">!</div>{error}</div>
    {:else if !detail}
      <div class="empty">加载中…</div>
    {:else}
      <div class="detail-head">
        <div class="d-poster {detail.poster_path ? '' : artClass(detail.id)}">
          {#if detail.poster_path}
            <img src={detail.poster_path} alt={detail.title ?? ''} />
          {/if}
        </div>
        <div style="min-width:0">
          <div class="d-title">{detail.title ?? '（未识别）'}</div>
          {#if detail.original_title && detail.original_title !== detail.title}
            <div class="d-orig">{detail.original_title}</div>
          {/if}
          <div class="d-tags">
            <span class="badge acc"><span class="bdot"></span>{detail.kind}</span>
            {#if detail.year}<span class="badge">{detail.year}</span>{/if}
            {#if detail.air_date}<span class="badge">首播 {detail.air_date}</span>{/if}
            {#if detail.children.length}<span class="badge">{detail.children.length} 集</span>{/if}
          </div>
          <div class="d-tags">
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
        </div>
      </div>

      {#if detail.children.length}
        <div class="detail-sec">
          <h3>集列表</h3>
          {#each detail.children as ep (ep.id)}
            <div class="ep-row">
              <span class="ep-no">
                {#if ep.season_no != null && ep.episode_no != null}
                  S{String(ep.season_no).padStart(2, '0')}E{String(ep.episode_no).padStart(2, '0')}
                {:else if ep.episode_no != null}
                  E{String(ep.episode_no).padStart(2, '0')}
                {:else}
                  {ep.kind}
                {/if}
              </span>
              <span class="ep-t">{ep.title ?? '（无标题）'}</span>
              {#if ep.air_date}<span class="ep-d">{ep.air_date}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}

      {#if detail.files.length}
        <div class="detail-sec">
          <h3>文件</h3>
          {#each detail.files as f (f.id)}
            <div class="file-row">
              <span style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis">{f.rel_path}</span>
              <span class="fsize">{fmtSize(f.size)}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</div>
