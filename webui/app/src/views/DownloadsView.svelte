<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { toast } from '../lib/toast.svelte';
  import type {
    DownloadSnapshot,
    DownloadState,
    Subscription,
    SubscriptionInput,
  } from '../lib/types';

  let { downloadsAvailable }: { downloadsAvailable: boolean | null } = $props();

  type Tab = 'downloads' | 'subscriptions';
  type ConfirmTarget =
    | { kind: 'download'; id: string; name: string }
    | { kind: 'subscription'; id: number; name: string };

  let tab = $state<Tab>('downloads');
  let downloads = $state<DownloadSnapshot[]>([]);
  let subscriptions = $state<Subscription[]>([]);
  let loadingDownloads = $state(true);
  let loadingSubscriptions = $state(true);
  let downloadsError = $state('');
  let subscriptionsError = $state('');
  let busy = $state<Record<string, boolean>>({});

  let showAddDownload = $state(false);
  let source = $state('');
  let savePath = $state('');

  let showSubscriptionForm = $state(false);
  let editingSubscriptionId = $state<number | null>(null);
  let subTitle = $state('');
  let subUrl = $state('');
  let subResolution = $state('1080p');
  let subGroups = $state('');
  let subExclude = $state('');
  let subEnabled = $state(true);

  let confirmTarget = $state<ConfirmTarget | null>(null);
  let deleteFiles = $state(false);

  const activeCount = $derived(
    downloads.filter((d) => d.state === 'downloading' || d.state === 'queued').length
  );
  const completedCount = $derived(
    downloads.filter((d) => d.state === 'completed' || d.state === 'seeding').length
  );
  const enabledSubscriptions = $derived(subscriptions.filter((s) => s.enabled).length);
  const totalBytes = $derived(downloads.reduce((sum, d) => sum + d.total_bytes, 0));
  const progressBytes = $derived(downloads.reduce((sum, d) => sum + d.progress_bytes, 0));
  const overallProgress = $derived(
    totalBytes > 0 ? Math.min(100, Math.round((progressBytes / totalBytes) * 100)) : 0
  );

  onMount(() => {
    loadDownloads();
    loadSubscriptions();
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') loadDownloads(true);
    }, 4000);
    return () => clearInterval(timer);
  });

  function message(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function setBusy(key: string, value: boolean) {
    if (value) busy[key] = true;
    else delete busy[key];
  }

  async function loadDownloads(silent = false) {
    if (!silent) loadingDownloads = true;
    try {
      downloads = await api.downloads();
      downloadsError = '';
    } catch (error) {
      downloadsError = message(error);
    } finally {
      loadingDownloads = false;
    }
  }

  async function loadSubscriptions() {
    loadingSubscriptions = true;
    try {
      subscriptions = await api.subscriptions();
      subscriptionsError = '';
    } catch (error) {
      subscriptionsError = message(error);
    } finally {
      loadingSubscriptions = false;
    }
  }

  function formatBytes(value: number): string {
    if (!Number.isFinite(value) || value <= 0) return '0 B';
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
    const amount = value / 1024 ** index;
    return `${amount >= 100 || index === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[index]}`;
  }

  function progress(download: DownloadSnapshot): number {
    if (download.total_bytes <= 0) return 0;
    return Math.min(100, Math.max(0, (download.progress_bytes / download.total_bytes) * 100));
  }

  function stateMeta(state: DownloadState): { label: string; cls: string } {
    switch (state) {
      case 'queued': return { label: '等待中', cls: '' };
      case 'downloading': return { label: '下载中', cls: 'acc' };
      case 'paused': return { label: '已暂停', cls: 'warn' };
      case 'seeding': return { label: '做种中', cls: 'good' };
      case 'completed': return { label: '已完成', cls: 'good' };
      case 'error': return { label: '失败', cls: 'crit' };
    }
  }

  function formatTime(value: number | null): string {
    if (!value) return '尚未检查';
    return new Date(value * 1000).toLocaleString('zh-CN', {
      month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
    });
  }

  async function addDownload() {
    if (!source.trim() || busy.addDownload) return;
    setBusy('addDownload', true);
    try {
      await api.addDownload({
        source: source.trim(),
        save_path: savePath.trim() || undefined,
      });
      source = '';
      savePath = '';
      showAddDownload = false;
      toast.show('下载任务已添加', 'good');
      await loadDownloads(true);
    } catch (error) {
      toast.show(`添加失败：${message(error)}`, 'crit');
    } finally {
      setBusy('addDownload', false);
    }
  }

  async function pause(download: DownloadSnapshot) {
    const key = `task:${download.info_hash}`;
    setBusy(key, true);
    try {
      await api.pauseDownload(download.info_hash);
      toast.show('任务已暂停', 'good');
      await loadDownloads(true);
    } catch (error) {
      toast.show(`暂停失败：${message(error)}`, 'crit');
    } finally {
      setBusy(key, false);
    }
  }

  async function resume(download: DownloadSnapshot) {
    const key = `task:${download.info_hash}`;
    setBusy(key, true);
    try {
      await api.resumeDownload(download.info_hash);
      toast.show('任务已恢复', 'good');
      await loadDownloads(true);
    } catch (error) {
      toast.show(`恢复失败：${message(error)}`, 'crit');
    } finally {
      setBusy(key, false);
    }
  }

  function newSubscription() {
    editingSubscriptionId = null;
    subTitle = '';
    subUrl = '';
    subResolution = '1080p';
    subGroups = '';
    subExclude = '';
    subEnabled = true;
    showSubscriptionForm = true;
  }

  function editSubscription(subscription: Subscription) {
    editingSubscriptionId = subscription.id;
    subTitle = subscription.title;
    subUrl = subscription.rss_url;
    subResolution = subscription.filters.resolution ?? '';
    subGroups = subscription.filters.subgroup_priority.join(', ');
    subExclude = subscription.filters.exclude_regex ?? '';
    subEnabled = subscription.enabled;
    showSubscriptionForm = true;
  }

  function subscriptionBody(): SubscriptionInput {
    const groups = [...new Set(
      subGroups.split(/[,，\n]/).map((part) => part.trim()).filter(Boolean)
    )];
    return {
      title: subTitle.trim(),
      rss_url: subUrl.trim(),
      enabled: subEnabled,
      filters: {
        subgroup_priority: groups,
        resolution: subResolution.trim() || null,
        exclude_regex: subExclude.trim() || null,
      },
    };
  }

  async function saveSubscription() {
    if (!subTitle.trim() || !subUrl.trim() || busy.saveSubscription) return;
    setBusy('saveSubscription', true);
    try {
      const body = subscriptionBody();
      if (editingSubscriptionId === null) await api.createSubscription(body);
      else await api.updateSubscription(editingSubscriptionId, body);
      showSubscriptionForm = false;
      toast.show(editingSubscriptionId === null ? '订阅已创建' : '订阅已更新', 'good');
      await loadSubscriptions();
    } catch (error) {
      toast.show(`保存失败：${message(error)}`, 'crit');
    } finally {
      setBusy('saveSubscription', false);
    }
  }

  async function toggleSubscription(subscription: Subscription) {
    const key = `sub:${subscription.id}`;
    setBusy(key, true);
    try {
      await api.updateSubscription(subscription.id, {
        title: subscription.title,
        rss_url: subscription.rss_url,
        filters: subscription.filters,
        enabled: !subscription.enabled,
      });
      toast.show(subscription.enabled ? '订阅已停用' : '订阅已启用', 'good');
      await loadSubscriptions();
    } catch (error) {
      toast.show(`操作失败：${message(error)}`, 'crit');
    } finally {
      setBusy(key, false);
    }
  }

  async function pollSubscription(subscription: Subscription) {
    const key = `poll:${subscription.id}`;
    setBusy(key, true);
    try {
      const result = await api.pollSubscription(subscription.id);
      toast.show(`检查完成：发现 ${result.discovered} 项，新增 ${result.added} 个任务`, 'good');
      await Promise.all([loadSubscriptions(), loadDownloads(true)]);
    } catch (error) {
      toast.show(`检查失败：${message(error)}`, 'crit');
      await loadSubscriptions();
    } finally {
      setBusy(key, false);
    }
  }

  async function confirmDelete() {
    if (!confirmTarget || busy.delete) return;
    setBusy('delete', true);
    try {
      if (confirmTarget.kind === 'download') {
        await api.deleteDownload(confirmTarget.id, deleteFiles);
        toast.show(deleteFiles ? '任务和文件已删除' : '下载任务已移除', 'good');
        await loadDownloads(true);
      } else {
        await api.deleteSubscription(confirmTarget.id);
        toast.show('订阅已删除', 'good');
        await loadSubscriptions();
      }
      confirmTarget = null;
      deleteFiles = false;
    } catch (error) {
      toast.show(`删除失败：${message(error)}`, 'crit');
    } finally {
      setBusy('delete', false);
    }
  }
</script>

<section class="view downloads-view">
  <div class="download-heading">
    <div>
      <div class="view-title">下载与订阅</div>
      <div class="view-sub">librqbit 下载任务、Mikan RSS 追番与完成后自动入库</div>
    </div>
    <button
      class="btn btn-primary"
      onclick={() => tab === 'downloads' ? (showAddDownload = true) : newSubscription()}
      disabled={downloadsAvailable === false}
    >
      {tab === 'downloads' ? '＋ 添加下载' : '＋ 新建订阅'}
    </button>
  </div>

  {#if downloadsAvailable === false}
    <div class="engine-alert">
      <b>下载引擎未就绪</b>
      <span>请检查数据目录权限和服务端日志；订阅配置仍可查看，但不能创建下载任务。</span>
    </div>
  {/if}

  <div class="tiles download-tiles">
    <div class="card tile">
      <div class="t-label">活动任务</div>
      <div class="t-row"><div class="t-value">{activeCount}</div>{#if activeCount}<span class="badge acc"><span class="bdot"></span>live</span>{/if}</div>
    </div>
    <div class="card tile">
      <div class="t-label">总体进度</div>
      <div class="t-row"><div class="t-value">{overallProgress}%</div><span class="tile-note">{formatBytes(progressBytes)} / {formatBytes(totalBytes)}</span></div>
    </div>
    <div class="card tile">
      <div class="t-label">已完成</div>
      <div class="t-row"><div class="t-value">{completedCount}</div></div>
    </div>
    <div class="card tile">
      <div class="t-label">启用订阅</div>
      <div class="t-row"><div class="t-value">{enabledSubscriptions}</div><span class="tile-note">共 {subscriptions.length} 个</span></div>
    </div>
  </div>

  <div class="download-tabs" role="tablist" aria-label="下载管理分类">
    <button class:active={tab === 'downloads'} role="tab" aria-selected={tab === 'downloads'} onclick={() => (tab = 'downloads')}>
      下载任务 <span>{downloads.length}</span>
    </button>
    <button class:active={tab === 'subscriptions'} role="tab" aria-selected={tab === 'subscriptions'} onclick={() => (tab = 'subscriptions')}>
      RSS 订阅 <span>{subscriptions.length}</span>
    </button>
  </div>

  {#if tab === 'downloads'}
    {#if showAddDownload}
      <div class="card editor-card">
        <div class="editor-head">
          <div><b>添加下载任务</b><span>支持 magnet 链接和公网 .torrent URL</span></div>
          <button class="icon-button" aria-label="关闭" onclick={() => (showAddDownload = false)}>×</button>
        </div>
        <div class="form-row">
          <label for="download-source">Magnet 或 Torrent URL</label>
          <textarea id="download-source" rows="3" placeholder="magnet:?xt=urn:btih:…" bind:value={source}></textarea>
        </div>
        <div class="form-row">
          <label for="download-path">保存子目录（可选）</label>
          <input id="download-path" placeholder="留空使用默认 downloads 目录" bind:value={savePath} />
          <small>自定义路径必须是服务端下载根目录下已存在的目录。</small>
        </div>
        <div class="editor-actions">
          <button class="btn btn-ghost" onclick={() => (showAddDownload = false)}>取消</button>
          <button class="btn btn-primary" onclick={addDownload} disabled={!source.trim() || busy.addDownload}>
            {busy.addDownload ? '添加中…' : '开始下载'}
          </button>
        </div>
      </div>
    {/if}

    {#if loadingDownloads}
      <div class="card empty"><div class="loading-ring"></div>正在读取下载会话…</div>
    {:else if downloadsError}
      <div class="card empty error-empty">
        <div class="e-icon">!</div><b>无法读取下载任务</b><span>{downloadsError}</span>
        <button class="btn btn-ghost btn-sm" onclick={() => loadDownloads()}>重试</button>
      </div>
    {:else if downloads.length === 0}
      <div class="card empty download-empty">
        <div class="empty-download-icon">↓</div>
        <b>还没有下载任务</b>
        <span>添加 magnet 或 Torrent URL，完成后会自动进入“下载”媒体库。</span>
        <button class="btn btn-primary btn-sm" onclick={() => (showAddDownload = true)}>添加第一个任务</button>
      </div>
    {:else}
      <div class="task-list">
        {#each downloads as download (download.info_hash)}
          {@const meta = stateMeta(download.state)}
          {@const pct = progress(download)}
          <article class="card download-task" class:task-error={download.state === 'error'}>
            <div class="task-icon" class:active={download.state === 'downloading'}>
              {download.state === 'completed' || download.state === 'seeding' ? '✓' : '↓'}
            </div>
            <div class="task-main">
              <div class="task-head">
                <b title={download.name}>{download.name}</b>
                <span class="badge {meta.cls}"><span class="bdot"></span>{meta.label}</span>
              </div>
              <div class="task-progress-row">
                <div class="meter"><i style={`width:${pct}%`}></i></div>
                <span>{pct.toFixed(pct >= 10 ? 0 : 1)}%</span>
              </div>
              <div class="task-meta">
                <span>{formatBytes(download.progress_bytes)} / {formatBytes(download.total_bytes)}</span>
                {#if download.uploaded_bytes > 0}<span>已上传 {formatBytes(download.uploaded_bytes)}</span>{/if}
                <span class="hash" title={download.info_hash}>{download.info_hash.slice(0, 12)}</span>
                {#if download.manifest_hash}<span class="ingested">已生成入库清单</span>{/if}
              </div>
              {#if download.error}<div class="task-error-text">{download.error}</div>{/if}
            </div>
            <div class="task-actions">
              {#if download.state === 'paused'}
                <button class="btn btn-ghost btn-sm" onclick={() => resume(download)} disabled={busy[`task:${download.info_hash}`]}>▶ 恢复</button>
              {:else if download.state === 'downloading' || download.state === 'queued'}
                <button class="btn btn-ghost btn-sm" onclick={() => pause(download)} disabled={busy[`task:${download.info_hash}`]}>Ⅱ 暂停</button>
              {/if}
              <button
                class="icon-button danger"
                aria-label={`删除 ${download.name}`}
                title="删除任务"
                onclick={() => { confirmTarget = { kind: 'download', id: download.info_hash, name: download.name }; deleteFiles = false; }}
              >×</button>
            </div>
          </article>
        {/each}
      </div>
    {/if}
  {:else}
    {#if showSubscriptionForm}
      <div class="card editor-card">
        <div class="editor-head">
          <div><b>{editingSubscriptionId === null ? '新建 Mikan RSS 订阅' : '编辑订阅'}</b><span>每 30 分钟自动检查，匹配条目自动加入下载队列</span></div>
          <button class="icon-button" aria-label="关闭" onclick={() => (showSubscriptionForm = false)}>×</button>
        </div>
        <div class="subscription-form-grid">
          <div class="form-row">
            <label for="sub-title">名称</label>
            <input id="sub-title" placeholder="本季追番" bind:value={subTitle} />
          </div>
          <div class="form-row">
            <label for="sub-resolution">分辨率过滤</label>
            <select id="sub-resolution" bind:value={subResolution}>
              <option value="">不限</option>
              <option value="1080p">1080p</option>
              <option value="2160p">2160p / 4K</option>
              <option value="720p">720p</option>
            </select>
          </div>
        </div>
        <div class="form-row">
          <label for="sub-url">RSS URL</label>
          <input id="sub-url" type="url" placeholder="https://mikanani.me/RSS/MyBangumi?token=…" bind:value={subUrl} />
        </div>
        <div class="form-row">
          <label for="sub-groups">字幕组优先级</label>
          <input id="sub-groups" placeholder="喵萌奶茶屋, 桜都字幕组, LoliHouse" bind:value={subGroups} />
          <small>用逗号分隔，排在前面的字幕组优先。</small>
        </div>
        <div class="form-row">
          <label for="sub-exclude">排除规则（正则，可选）</label>
          <input id="sub-exclude" placeholder="合集|繁体|720P" bind:value={subExclude} />
        </div>
        <label class="switch-row"><input type="checkbox" bind:checked={subEnabled} /><span>创建后立即启用自动检查</span></label>
        <div class="editor-actions">
          <button class="btn btn-ghost" onclick={() => (showSubscriptionForm = false)}>取消</button>
          <button class="btn btn-primary" onclick={saveSubscription} disabled={!subTitle.trim() || !subUrl.trim() || busy.saveSubscription}>
            {busy.saveSubscription ? '保存中…' : editingSubscriptionId === null ? '创建订阅' : '保存修改'}
          </button>
        </div>
      </div>
    {/if}

    {#if loadingSubscriptions}
      <div class="card empty"><div class="loading-ring"></div>正在读取订阅…</div>
    {:else if subscriptionsError}
      <div class="card empty error-empty">
        <div class="e-icon">!</div><b>无法读取订阅</b><span>{subscriptionsError}</span>
        <button class="btn btn-ghost btn-sm" onclick={loadSubscriptions}>重试</button>
      </div>
    {:else if subscriptions.length === 0}
      <div class="card empty download-empty">
        <div class="empty-download-icon rss">⌁</div>
        <b>还没有 RSS 订阅</b>
        <span>添加 Mikan RSS，并按字幕组、分辨率和排除规则自动筛选。</span>
        <button class="btn btn-primary btn-sm" onclick={newSubscription}>新建第一个订阅</button>
      </div>
    {:else}
      <div class="subscription-list">
        {#each subscriptions as subscription (subscription.id)}
          <article class="card subscription-card" class:disabled={!subscription.enabled}>
            <div class="rss-mark">RSS</div>
            <div class="subscription-main">
              <div class="subscription-head">
                <b>{subscription.title}</b>
                <span class="badge {subscription.enabled ? 'good' : ''}"><span class="bdot"></span>{subscription.enabled ? '已启用' : '已停用'}</span>
                {#if subscription.last_error}<span class="badge crit"><span class="bdot"></span>检查失败</span>{/if}
              </div>
              <div class="subscription-url" title={subscription.rss_url}>{subscription.rss_url}</div>
              <div class="filter-chips">
                <span>上次检查：{formatTime(subscription.last_check)}</span>
                {#if subscription.filters.resolution}<span>{subscription.filters.resolution}</span>{/if}
                {#each subscription.filters.subgroup_priority as group}<span>{group}</span>{/each}
                {#if subscription.filters.exclude_regex}<span>排除 /{subscription.filters.exclude_regex}/</span>{/if}
              </div>
              {#if subscription.last_error}<div class="task-error-text">{subscription.last_error}</div>{/if}
            </div>
            <div class="subscription-actions">
              <button class="btn btn-ghost btn-sm" onclick={() => pollSubscription(subscription)} disabled={busy[`poll:${subscription.id}`] || downloadsAvailable === false}>
                {busy[`poll:${subscription.id}`] ? '检查中…' : '立即检查'}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => toggleSubscription(subscription)} disabled={busy[`sub:${subscription.id}`]}>
                {subscription.enabled ? '停用' : '启用'}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => editSubscription(subscription)}>编辑</button>
              <button class="icon-button danger" aria-label={`删除 ${subscription.title}`} title="删除订阅" onclick={() => (confirmTarget = { kind: 'subscription', id: subscription.id, name: subscription.title })}>×</button>
            </div>
          </article>
        {/each}
      </div>
    {/if}
  {/if}
</section>

{#if confirmTarget}
  <div class="confirm-overlay" role="presentation" onclick={(event) => event.target === event.currentTarget && (confirmTarget = null)}>
    <div class="card confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-title">
      <div class="confirm-icon">!</div>
      <h2 id="delete-title">{confirmTarget.kind === 'download' ? '删除下载任务？' : '删除 RSS 订阅？'}</h2>
      <p>“{confirmTarget.name}”将从管理列表中移除，此操作无法撤销。</p>
      {#if confirmTarget.kind === 'download'}
        <label class="switch-row danger-check"><input type="checkbox" bind:checked={deleteFiles} /><span>同时删除已经下载的文件</span></label>
      {/if}
      <div class="editor-actions">
        <button class="btn btn-ghost" onclick={() => (confirmTarget = null)}>取消</button>
        <button class="btn delete-button" onclick={confirmDelete} disabled={busy.delete}>{busy.delete ? '删除中…' : '确认删除'}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .download-heading { display:flex; align-items:flex-start; justify-content:space-between; gap:20px; }
  .download-heading .view-sub { margin-bottom:22px; }
  .engine-alert { display:flex; align-items:center; gap:13px; padding:12px 15px; margin-bottom:16px; border:1px solid rgba(208,59,59,.35); border-radius:11px; background:rgba(208,59,59,.08); font-size:12.5px; }
  .engine-alert b { color:#ff9d9d; flex:none; }
  .engine-alert span { color:var(--ink-2); }
  .download-tiles { grid-template-columns:repeat(4,minmax(0,1fr)); margin-bottom:16px; }
  .tile-note { color:var(--ink-3); font-size:11px; text-align:right; }
  .download-tabs { display:flex; align-items:center; gap:4px; border-bottom:1px solid var(--hairline); margin-bottom:16px; }
  .download-tabs button { appearance:none; border:0; border-bottom:2px solid transparent; background:transparent; color:var(--ink-3); padding:10px 14px 11px; font:600 13px inherit; cursor:pointer; }
  .download-tabs button:hover { color:var(--ink-2); }
  .download-tabs button.active { color:var(--ink); border-bottom-color:var(--accent); }
  .download-tabs button span { margin-left:5px; color:var(--ink-3); font:11px var(--mono); }
  .editor-card { padding:17px 18px; margin-bottom:16px; border-color:rgba(57,135,229,.3); }
  .editor-head { display:flex; justify-content:space-between; align-items:flex-start; gap:16px; margin-bottom:15px; }
  .editor-head b { display:block; color:var(--ink); font-size:14px; margin-bottom:3px; }
  .editor-head span { color:var(--ink-3); font-size:11.5px; }
  .form-row textarea { resize:vertical; min-height:68px; background:var(--surface-2); border:1px solid var(--hairline); border-radius:9px; padding:9px 12px; color:var(--ink); font:12px/1.55 var(--mono); outline:none; }
  .form-row textarea:focus, .form-row select:focus { border-color:var(--accent); }
  .form-row small { color:var(--ink-3); font-size:10.5px; }
  .editor-actions { display:flex; justify-content:flex-end; gap:8px; margin-top:15px; }
  .icon-button { width:29px; height:29px; display:grid; place-items:center; flex:none; padding:0; border:1px solid var(--hairline); border-radius:8px; background:var(--surface-2); color:var(--ink-3); font:18px/1 inherit; cursor:pointer; }
  .icon-button:hover { color:var(--ink); border-color:var(--hairline-2); }
  .icon-button.danger:hover { color:#ff9d9d; border-color:rgba(208,59,59,.5); background:rgba(208,59,59,.08); }
  .loading-ring { width:18px; height:18px; margin:0 auto 12px; border:2px solid var(--hairline-2); border-top-color:var(--accent); border-radius:50%; animation:spin .8s linear infinite; }
  .error-empty, .download-empty { display:flex; flex-direction:column; align-items:center; gap:8px; }
  .error-empty b, .download-empty b { color:var(--ink); font-size:14px; }
  .error-empty span, .download-empty span { max-width:540px; }
  .empty-download-icon { width:48px; height:48px; display:grid; place-items:center; margin-bottom:3px; border-radius:15px; background:var(--accent-dim); color:#9cc4f2; font-size:25px; }
  .empty-download-icon.rss { color:#d8b1f0; background:rgba(141,78,177,.15); }
  .task-list, .subscription-list { display:flex; flex-direction:column; gap:10px; }
  .download-task { display:grid; grid-template-columns:auto minmax(0,1fr) auto; align-items:center; gap:14px; padding:14px 15px; }
  .download-task.task-error { border-color:rgba(208,59,59,.4); }
  .task-icon { width:38px; height:38px; display:grid; place-items:center; border-radius:11px; background:var(--surface-2); border:1px solid var(--hairline); color:var(--ink-3); font-size:17px; }
  .task-icon.active { color:#9cc4f2; background:var(--accent-dim); border-color:rgba(57,135,229,.3); animation:soft-pulse 1.8s ease-in-out infinite; }
  .task-main { min-width:0; }
  .task-head { display:flex; align-items:center; gap:9px; margin-bottom:9px; }
  .task-head b { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--ink); font-size:13px; }
  .task-progress-row { display:flex; align-items:center; gap:10px; }
  .task-progress-row .meter { flex:1; }
  .task-progress-row > span { width:42px; text-align:right; color:var(--ink-2); font:11px var(--mono); }
  .task-meta { display:flex; flex-wrap:wrap; gap:6px 14px; margin-top:7px; color:var(--ink-3); font-size:10.5px; }
  .task-meta .hash { font-family:var(--mono); }
  .task-meta .ingested { color:#7fd97f; }
  .task-error-text { margin-top:7px; color:#ff9d9d; font:11px/1.45 var(--mono); word-break:break-word; }
  .task-actions, .subscription-actions { display:flex; align-items:center; justify-content:flex-end; gap:7px; }
  .subscription-form-grid { display:grid; grid-template-columns:1fr 180px; gap:12px; }
  .switch-row { display:flex; align-items:center; gap:8px; color:var(--ink-2); font-size:12px; cursor:pointer; }
  .switch-row input { accent-color:var(--accent); }
  .subscription-card { display:grid; grid-template-columns:auto minmax(0,1fr) auto; gap:14px; align-items:center; padding:14px 15px; }
  .subscription-card.disabled { opacity:.68; }
  .rss-mark { width:42px; height:42px; display:grid; place-items:center; border-radius:12px; background:rgba(141,78,177,.14); border:1px solid rgba(141,78,177,.28); color:#d8b1f0; font:700 10px var(--mono); letter-spacing:.04em; }
  .subscription-main { min-width:0; }
  .subscription-head { display:flex; align-items:center; flex-wrap:wrap; gap:8px; margin-bottom:5px; }
  .subscription-head b { color:var(--ink); font-size:13px; }
  .subscription-url { color:var(--ink-3); font:10.5px var(--mono); white-space:nowrap; overflow:hidden; text-overflow:ellipsis; margin-bottom:7px; }
  .filter-chips { display:flex; flex-wrap:wrap; gap:5px; }
  .filter-chips span { padding:2px 7px; border-radius:5px; background:var(--surface-2); border:1px solid var(--hairline); color:var(--ink-3); font-size:10px; }
  .confirm-overlay { position:fixed; inset:0; z-index:120; display:grid; place-items:center; padding:24px; background:rgba(0,0,0,.65); backdrop-filter:blur(7px); }
  .confirm-dialog { width:min(420px,100%); padding:23px; box-shadow:0 22px 80px rgba(0,0,0,.45); }
  .confirm-icon { width:38px; height:38px; display:grid; place-items:center; border-radius:11px; background:rgba(208,59,59,.12); color:#ff9d9d; font-weight:800; margin-bottom:13px; }
  .confirm-dialog h2 { color:var(--ink); font-size:17px; margin:0 0 8px; }
  .confirm-dialog p { color:var(--ink-2); font-size:12.5px; line-height:1.6; margin:0 0 15px; word-break:break-word; }
  .danger-check { padding:10px 12px; border-radius:8px; background:rgba(208,59,59,.08); color:#ffb1b1; }
  .delete-button { color:#fff; background:var(--critical); }
  .delete-button:hover:not(:disabled) { background:#e04c4c; }
  @keyframes spin { to { transform:rotate(360deg); } }
  @keyframes soft-pulse { 50% { box-shadow:0 0 0 4px rgba(57,135,229,.08); } }
  @media (max-width:1050px) {
    .download-tiles { grid-template-columns:repeat(2,minmax(0,1fr)); }
    .download-task, .subscription-card { grid-template-columns:auto minmax(0,1fr); }
    .task-actions, .subscription-actions { grid-column:2; justify-content:flex-start; flex-wrap:wrap; }
  }
  @media (max-width:720px) {
    .download-heading { align-items:stretch; flex-direction:column; }
    .download-heading .view-sub { margin-bottom:4px; }
    .download-tiles { grid-template-columns:repeat(2,1fr); }
    .subscription-form-grid { grid-template-columns:1fr; gap:0; }
    .download-task, .subscription-card { grid-template-columns:1fr; }
    .task-icon, .rss-mark { display:none; }
    .task-actions, .subscription-actions { grid-column:1; }
  }
</style>
