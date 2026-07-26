<script lang="ts">
  import { onMount } from 'svelte';
  import { nav, type View } from './lib/nav.svelte';
  import { sse } from './lib/sse.svelte';
  import { api } from './lib/api';
  import type { SystemInfo } from './lib/types';
  import LibraryView from './views/LibraryView.svelte';
  import StewardView from './views/StewardView.svelte';
  import ConsoleView from './views/ConsoleView.svelte';
  import SettingsView from './views/SettingsView.svelte';

  let sysInfo = $state<SystemInfo | null>(null);
  let pendingCount = $state(0);

  onMount(() => {
    sse.connect();
    api.systemInfo().then((i) => (sysInfo = i)).catch(() => {});
    refreshPending();
    const t = setInterval(refreshPending, 30_000);
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'j') {
        e.preventDefault();
        nav.go('steward');
      }
    };
    window.addEventListener('keydown', onKey);
    return () => {
      clearInterval(t);
      window.removeEventListener('keydown', onKey);
    };
  });

  function refreshPending() {
    api.scrapePending().then((p) => (pendingCount = p.length)).catch(() => {});
  }

  const navItems: { v: View; label: string }[] = [
    { v: 'library', label: '媒体库' },
    { v: 'steward', label: '管家' },
    { v: 'console', label: 'Agent 控制台' },
  ];

  const runningScans = $derived(Object.keys(sse.scanProgress).length);
  const runningTasks = $derived(
    Object.values(sse.scrapeStates).filter((s) => s === 'running' || s === 'queued').length
  );
</script>

<div class="app">
  <!-- ============ 侧边栏 ============ -->
  <nav class="sidebar">
    <div class="logo">
      <div class="logo-mark">n</div>
      <div><b>nipaserver</b><span>agentic media server</span></div>
    </div>
    {#each navItems as item (item.v)}
      <div
        class="nav-item"
        class:active={nav.view === item.v}
        role="button"
        tabindex="0"
        onclick={() => nav.go(item.v)}
        onkeydown={(e) => e.key === 'Enter' && nav.go(item.v)}
      >
        {#if item.v === 'library'}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="4" width="7" height="16" rx="1.5"/><rect x="14" y="4" width="7" height="9" rx="1.5"/><rect x="14" y="17" width="7" height="3" rx="1"/></svg>
        {:else if item.v === 'steward'}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3a7 7 0 017 7v1.5a3.5 3.5 0 01-3.5 3.5h-7A3.5 3.5 0 015 11.5V10a7 7 0 017-7z"/><path d="M8 21c1-1.5 2.5-2 4-2s3 .5 4 2"/><circle cx="9.5" cy="10" r="1" fill="currentColor" stroke="none"/><circle cx="14.5" cy="10" r="1" fill="currentColor" stroke="none"/></svg>
        {:else}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3.2"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9L17 7M7 17l-2.1 2.1"/></svg>
        {/if}
        {item.label}
        {#if item.v === 'console' && pendingCount > 0}
          <span class="n-badge">{pendingCount}</span>
        {/if}
      </div>
    {/each}
    <div class="nav-sec">系统</div>
    <div
      class="nav-item"
      class:active={nav.view === 'settings'}
      role="button"
      tabindex="0"
      onclick={() => nav.go('settings')}
      onkeydown={(e) => e.key === 'Enter' && nav.go('settings')}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 11-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 11-4 0v-.09a1.65 1.65 0 00-1-1.51 1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 11-2.83-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 110-4h.09a1.65 1.65 0 001.51-1 1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 112.83-2.83l.06.06a1.65 1.65 0 001.82.33h0a1.65 1.65 0 001-1.51V3a2 2 0 114 0v.09a1.65 1.65 0 001 1.51h0a1.65 1.65 0 001.82-.33l.06-.06a2 2 0 112.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82v0a1.65 1.65 0 001.51 1H21a2 2 0 110 4h-.09a1.65 1.65 0 00-1.51 1z"/></svg>
      设置
    </div>
    <div class="nav-foot">
      <span class="dot" class:off={sysInfo !== null && !sysInfo.database_ok}></span>
      {#if sysInfo}v{sysInfo.version} · {sysInfo.platform}/{sysInfo.arch}{:else}连接中…{/if}
    </div>
  </nav>

  <div class="main">
    <!-- ============ 顶栏 ============ -->
    <div class="topbar">
      <div class="search">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>
        搜索条目、文件、设置…
        <kbd>⌘K</kbd>
      </div>
      <div style="flex:1"></div>
      {#if sse.conn === 'open'}
        {#if runningScans > 0 || runningTasks > 0}
          <div class="scan-pill">
            <span class="pulse"></span>
            {#if runningScans > 0}扫描中 · {runningScans} 个库{/if}
            {#if runningScans > 0 && runningTasks > 0}&nbsp;·&nbsp;{/if}
            {#if runningTasks > 0}AI 队列 {runningTasks}{/if}
          </div>
        {:else}
          <div class="scan-pill ok"><span class="pulse"></span>已连接</div>
        {/if}
      {:else}
        <div class="scan-pill warn"><span class="pulse"></span>重连中…</div>
      {/if}
      <div class="bell">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 8a6 6 0 10-12 0c0 7-3 9-3 9h18s-3-2-3-9M13.7 21a2 2 0 01-3.4 0"/></svg>
        {#if pendingCount > 0}<span class="b-dot"></span>{/if}
      </div>
      <div class="avatar">S</div>
    </div>

    {#if nav.view === 'library'}
      <LibraryView />
    {:else if nav.view === 'steward'}
      <StewardView />
    {:else if nav.view === 'console'}
      <ConsoleView onPendingChange={(n) => (pendingCount = n)} />
    {:else}
      <SettingsView {sysInfo} />
    {/if}
  </div>
</div>

{#if nav.view !== 'steward'}
  <button class="steward-fab" onclick={() => nav.go('steward')}>
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3a7 7 0 017 7v1.5a3.5 3.5 0 01-3.5 3.5h-7A3.5 3.5 0 015 11.5V10a7 7 0 017-7z"/><path d="M8 21c1-1.5 2.5-2 4-2s3 .5 4 2"/></svg>
    召唤管家
    <kbd>⌘J</kbd>
  </button>
{/if}
