<script lang="ts">
  // 设置：库管理（列表/新建/触发扫描）+ system/info 展示（docs/05 §4.5 的 M1 子集）
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { sse } from '../lib/sse.svelte';
  import type { Library, SystemInfo } from '../lib/types';

  let { sysInfo }: { sysInfo: SystemInfo | null } = $props();

  let libraries = $state<Library[]>([]);
  let error = $state('');
  let creating = $state(false);
  let newName = $state('');
  let newPath = $state('');
  let newKind = $state('anime');
  let scanBusy = $state<Record<number, boolean>>({});

  async function load() {
    try {
      libraries = await api.libraries();
      error = '';
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    load();
    const un = sse.subscribe((msg) => {
      if (msg.type === 'scan_progress') load();
    });
    return un;
  });

  async function create() {
    if (!newName.trim() || !newPath.trim() || creating) return;
    creating = true;
    error = '';
    try {
      await api.createLibrary({ name: newName.trim(), path: newPath.trim(), kind: newKind });
      newName = '';
      newPath = '';
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      creating = false;
    }
  }

  async function scan(id: number) {
    scanBusy[id] = true;
    error = '';
    try {
      await api.scanLibrary(id);
    } catch (e) {
      error = String(e);
    } finally {
      // 触发即返回；实际进度经 SSE scan_progress
      setTimeout(() => (scanBusy[id] = false), 1200);
    }
  }
</script>

<section class="view">
  <div class="view-title">设置</div>
  <div class="view-sub">媒体库管理与系统信息</div>

  <div class="settings-grid">
    <div>
      <div class="card">
        <div class="panel-title">媒体库<span class="count">{libraries.length} 个</span></div>
        {#if libraries.length === 0}
          <div class="empty" style="padding:26px 14px">还没有媒体库 — 在下方添加第一个</div>
        {/if}
        {#each libraries as lib (lib.id)}
          <div class="lib-row">
            <div class="l-icon">
              <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 7h5l2 3h11v9a1 1 0 01-1 1H4a1 1 0 01-1-1V7z"/><path d="M3 7V5a1 1 0 011-1h4l2 2"/></svg>
            </div>
            <div class="l-info">
              <b>{lib.name ?? `库 #${lib.id}`}</b>
              <span>{lib.path}</span>
            </div>
            <span class="badge">{lib.kind ?? '未分类'}</span>
            <span class="badge acc"><span class="bdot"></span>{lib.file_count} 文件</span>
            {#if sse.scanProgress[lib.id]}
              <span class="badge warn"><span class="bdot"></span>{sse.scanProgress[lib.id]}</span>
            {/if}
            <button class="btn btn-ghost btn-sm" onclick={() => scan(lib.id)} disabled={scanBusy[lib.id]}>
              {scanBusy[lib.id] ? '已触发…' : '扫描'}
            </button>
          </div>
        {/each}
      </div>

      <div class="card" style="margin-top:16px; padding:16px 18px">
        <div style="font-size:13px;font-weight:650;color:var(--ink);margin-bottom:13px">新建媒体库</div>
        <div class="form-row">
          <label for="lib-name">名称</label>
          <input id="lib-name" placeholder="动漫库" bind:value={newName} />
        </div>
        <div class="form-row">
          <label for="lib-path">路径（服务器上的绝对路径）</label>
          <input id="lib-path" placeholder="/mnt/media/anime" bind:value={newPath} />
        </div>
        <div class="form-row">
          <label for="lib-kind">类型</label>
          <select id="lib-kind" bind:value={newKind}>
            <option value="anime">动漫</option>
            <option value="tv">剧集</option>
            <option value="movie">电影</option>
          </select>
        </div>
        <button class="btn btn-primary" onclick={create} disabled={creating || !newName.trim() || !newPath.trim()}>
          {creating ? '创建中…' : '＋ 添加媒体库'}
        </button>
        {#if error}
          <div style="margin-top:10px;font-size:12px;color:#ff9d9d">{error}</div>
        {/if}
      </div>
    </div>

    <div>
      <div class="card" style="padding:6px 18px 10px">
        <div class="panel-title" style="padding:13px 0;margin-bottom:2px">系统信息</div>
        {#if sysInfo}
          <div class="kv-row"><span class="k">名称</span><span class="v">{sysInfo.name}</span></div>
          <div class="kv-row"><span class="k">版本</span><span class="v">v{sysInfo.version}</span></div>
          <div class="kv-row"><span class="k">平台</span><span class="v">{sysInfo.platform} / {sysInfo.arch}</span></div>
          <div class="kv-row"><span class="k">数据目录</span><span class="v" style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{sysInfo.data_dir}</span></div>
          <div class="kv-row">
            <span class="k">数据库</span>
            <span class="badge {sysInfo.database_ok ? 'good' : 'crit'}"><span class="bdot"></span>{sysInfo.database_ok ? '正常' : '异常'}</span>
          </div>
          <div class="kv-row">
            <span class="k">AI 刮削</span>
            <span class="badge {sysInfo.capabilities.ai_scrape ? 'good' : ''}"><span class="bdot"></span>{sysInfo.capabilities.ai_scrape ? '可用' : '未配置'}</span>
          </div>
          <div class="kv-row">
            <span class="k">ffmpeg</span>
            <span class="badge {sysInfo.capabilities.ffmpeg ? 'good' : ''}"><span class="bdot"></span>{sysInfo.capabilities.ffmpeg ? '可用' : '未检测'}</span>
          </div>
          <div class="kv-row">
            <span class="k">弹弹play L1</span>
            <span class="badge {sysInfo.capabilities.dandanplay_l1 ? 'good' : ''}"><span class="bdot"></span>{sysInfo.capabilities.dandanplay_l1 ? '可用' : '未配置'}</span>
          </div>
          <div class="kv-row">
            <span class="k">BT 下载</span>
            <span class="badge {sysInfo.capabilities.downloads ? 'good' : 'crit'}"><span class="bdot"></span>{sysInfo.capabilities.downloads ? 'librqbit 已就绪' : '初始化失败'}</span>
          </div>
        {:else}
          <div class="empty" style="padding:20px 0">未连接到服务器</div>
        {/if}
      </div>

      <div class="card" style="margin-top:14px; padding:13px 15px">
        <div style="font-size:12px;color:var(--ink-3);margin-bottom:8px">SSE 连接</div>
        <div class="kv-row">
          <span class="k">状态</span>
          <span class="badge {sse.conn === 'open' ? 'good' : 'warn'}"><span class="bdot"></span>{sse.conn === 'open' ? '已连接' : sse.conn === 'retrying' ? '重连中' : '连接中'}</span>
        </div>
        {#if sse.lastHeartbeat > 0}
          <div class="kv-row"><span class="k">最近心跳</span><span class="v">{new Date(sse.lastHeartbeat * 1000).toLocaleTimeString()}</span></div>
        {/if}
      </div>
    </div>
  </div>
</section>
