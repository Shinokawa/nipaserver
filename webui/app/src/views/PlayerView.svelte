<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type HlsInstance from 'hls.js';
  import { api } from '../lib/api';
  import { nav } from '../lib/nav.svelte';
  import type { Item, ItemDetail, PlaybackMediaSource } from '../lib/types';

  let { itemId, fileId }: { itemId: number; fileId: number } = $props();

  let video: HTMLVideoElement;
  let shell: HTMLElement;
  let detail = $state<ItemDetail | null>(null);
  let source = $state<PlaybackMediaSource | null>(null);
  let error = $state('');
  let loading = $state(true);
  let playing = $state(false);
  let current = $state(0);
  let duration = $state(0);
  let buffered = $state(0);
  let volume = $state(Number(sessionStorage.getItem('nipa.volume') ?? '1'));
  let muted = $state(false);
  let speed = $state(Number(sessionStorage.getItem('nipa.speed') ?? '1'));
  let fit = $state<'contain' | 'cover' | 'fill'>('contain');
  let loop = $state(false);
  let osdVisible = $state(true);
  let showRemaining = $state(false);
  let previous = $state<Item | null>(null);
  let next = $state<Item | null>(null);
  let hls: HlsInstance | null = null;
  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  let clickTimer: ReturnType<typeof setTimeout> | null = null;
  let started = false;
  let lastReportAt = 0;
  let resumeApplied = false;

  const method = $derived(
    source?.supports_direct_play ? 'Direct Play' : source?.supports_direct_stream ? 'Remux' : 'Transcode'
  );
  const remaining = $derived(Math.max(0, duration - current));
  const upNextVisible = $derived(!!next && duration > 0 && remaining <= 30 && remaining > 0);

  onMount(() => {
    shell.focus();
    void load();
    return () => {
      if (hideTimer) clearTimeout(hideTimer);
      if (clickTimer) clearTimeout(clickTimer);
      report('stop', true);
      hls?.destroy();
    };
  });

  async function load() {
    try {
      detail = await api.item(itemId);
      await loadSiblings();
      const info = await api.playbackInfo(fileId);
      source = info.media_sources[0] ?? null;
      const url = source?.direct_url ?? source?.transcode_url;
      if (!url) throw new Error(info.error_code ?? '服务器没有返回可播放地址');
      await tick();
      await setupMedia(url, !!source?.transcode_url);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  async function setupMedia(url: string, isHls: boolean) {
    video.volume = clamp(volume, 0, 1);
    video.playbackRate = speed;
    if (isHls && !video.canPlayType('application/vnd.apple.mpegurl')) {
      const { default: Hls } = await import('hls.js');
      if (!Hls.isSupported()) {
        error = '当前浏览器不支持 HLS/MSE 播放';
        loading = false;
        return;
      }
      hls = new Hls({ enableWorker: true, backBufferLength: 60 });
      hls.on(Hls.Events.ERROR, (_event, data) => {
        if (!data.fatal) return;
        error = `HLS 播放失败：${data.details}`;
        loading = false;
      });
      hls.loadSource(url);
      hls.attachMedia(video);
    } else {
      video.src = url;
    }
    video.load();
  }

  async function loadSiblings() {
    if (!detail || detail.kind !== 'episode' || detail.parent_id == null) return;
    try {
      const parent = await api.item(detail.parent_id);
      const episodes = parent.children
        .filter((item) => item.kind === 'episode')
        .sort((a, b) => (a.episode_no ?? 0) - (b.episode_no ?? 0));
      const index = episodes.findIndex((item) => item.id === itemId);
      previous = index > 0 ? episodes[index - 1] : null;
      next = index >= 0 && index + 1 < episodes.length ? episodes[index + 1] : null;
    } catch {
      previous = null;
      next = null;
    }
  }

  async function goSibling(item: Item | null) {
    if (!item) return;
    try {
      report('stop');
      const target = await api.item(item.id);
      const file = target.files[0];
      if (!file) throw new Error('下一集还没有可播放文件');
      nav.goPlayer(target.id, file.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function metadataReady() {
    loading = false;
    duration = Number.isFinite(video.duration) ? video.duration : 0;
    if (!resumeApplied) {
      const saved = (detail?.user_data?.position_ms ?? 0) / 1000;
      if (saved > 5 && (!duration || saved < duration - 10)) video.currentTime = saved;
      resumeApplied = true;
    }
    void video.play().catch(() => {});
  }

  function timeUpdate() {
    current = video.currentTime || 0;
    duration = Number.isFinite(video.duration) ? video.duration : duration;
    updateBuffered();
    if (playing && current - lastReportAt >= 10) {
      lastReportAt = current;
      report('progress');
    }
  }

  function updateBuffered() {
    if (!video.buffered.length || !duration) return (buffered = 0);
    buffered = clamp((video.buffered.end(video.buffered.length - 1) / duration) * 100, 0, 100);
  }

  function onPlay() {
    playing = true;
    if (!started) {
      started = true;
      report('start');
    }
    scheduleHide();
  }

  function onPause() {
    playing = false;
    osdVisible = true;
    report('progress');
  }

  function report(event: 'start' | 'progress' | 'stop', keepalive = false) {
    if (!video || (event !== 'start' && !started)) return;
    const body = {
      item_id: itemId,
      file_id: fileId,
      position_ms: Math.round((video.currentTime || 0) * 1000),
      duration_ms: Number.isFinite(video.duration) ? Math.round(video.duration * 1000) : null,
      event,
    };
    if (keepalive) {
      void fetch('/api/v1/playback/progress', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        keepalive: true,
      });
    } else {
      void api
        .reportProgress(itemId, fileId, body.position_ms, body.duration_ms, event)
        .catch(() => {});
    }
  }

  function togglePlay() {
    if (video.paused) void video.play();
    else video.pause();
  }

  function seekBy(seconds: number) {
    video.currentTime = clamp(video.currentTime + seconds, 0, duration || Infinity);
    showOsd();
  }

  function seekTo(value: number) {
    video.currentTime = value;
    current = value;
  }

  function setVolume(value: number) {
    volume = clamp(value, 0, 1);
    video.volume = volume;
    muted = volume === 0;
    video.muted = muted;
    sessionStorage.setItem('nipa.volume', String(volume));
  }

  function toggleMute() {
    muted = !muted;
    video.muted = muted;
  }

  function setSpeed(value: number) {
    speed = value;
    video.playbackRate = value;
    sessionStorage.setItem('nipa.speed', String(value));
  }

  function cycleFit() {
    fit = fit === 'contain' ? 'cover' : fit === 'cover' ? 'fill' : 'contain';
  }

  async function toggleFullscreen() {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await shell.requestFullscreen();
  }

  async function togglePip() {
    if (!document.pictureInPictureEnabled) return;
    if (document.pictureInPictureElement) await document.exitPictureInPicture();
    else await video.requestPictureInPicture();
  }

  function showOsd() {
    osdVisible = true;
    scheduleHide();
  }

  function scheduleHide() {
    if (hideTimer) clearTimeout(hideTimer);
    if (!playing) return;
    hideTimer = setTimeout(() => (osdVisible = false), 3000);
  }

  function surfaceClick(event: MouseEvent) {
    if ((event.target as HTMLElement).closest('.player-osd, .up-next')) return;
    if (clickTimer) clearTimeout(clickTimer);
    clickTimer = setTimeout(togglePlay, 220);
  }

  function surfaceDoubleClick(event: MouseEvent) {
    if ((event.target as HTMLElement).closest('.player-osd, .up-next')) return;
    if (clickTimer) clearTimeout(clickTimer);
    void toggleFullscreen();
  }

  function handleWheel(event: WheelEvent) {
    if (!event.ctrlKey && !event.metaKey) setVolume(volume + (event.deltaY < 0 ? 0.05 : -0.05));
  }

  function handleKey(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName)) return;
    const key = event.key.toLowerCase();
    if (key === ' ' || key === 'k') togglePlay();
    else if (key === 'j' || key === 'arrowleft') seekBy(-10);
    else if (key === 'l' || key === 'arrowright') seekBy(30);
    else if (key === 'arrowup') setVolume(volume + 0.05);
    else if (key === 'arrowdown') setVolume(volume - 0.05);
    else if (key === 'f') void toggleFullscreen();
    else if (key === 'm') toggleMute();
    else if (key === 'escape' && !document.fullscreenElement) nav.goItem(itemId);
    else if (event.shiftKey && key === 'p') void goSibling(previous);
    else if (event.shiftKey && key === 'n') void goSibling(next);
    else if (event.shiftKey && key === ',') setSpeed(Math.max(0.5, speed - 0.25));
    else if (event.shiftKey && key === '.') setSpeed(Math.min(4, speed + 0.25));
    else if ((key === ',' || key === '.') && video.paused) seekBy(key === ',' ? -1 / 30 : 1 / 30);
    else if (/^[0-9]$/.test(key) && duration) seekTo((Number(key) / 10) * duration);
    else return;
    event.preventDefault();
    showOsd();
  }

  function fmtTime(seconds: number): string {
    if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
    const total = Math.floor(seconds);
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}` : `${m}:${String(s).padStart(2, '0')}`;
  }

  function endClock(): string {
    const end = new Date(Date.now() + (remaining / Math.max(speed, 0.1)) * 1000);
    return end.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
  }
</script>

<!-- Composite media widget: its child controls own semantics; the surface handles video shortcuts. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
  class="player-shell"
  class:osd-hidden={!osdVisible}
  bind:this={shell}
  role="application"
  aria-label="视频播放器"
  tabindex="-1"
  onpointermove={showOsd}
  onwheel={handleWheel}
  onclick={surfaceClick}
  ondblclick={surfaceDoubleClick}
  onkeydown={handleKey}
>
  <video
    bind:this={video}
    style="object-fit:{fit}"
    playsinline
    {loop}
    onloadedmetadata={metadataReady}
    ondurationchange={() => (duration = Number.isFinite(video.duration) ? video.duration : 0)}
    ontimeupdate={timeUpdate}
    onprogress={updateBuffered}
    onplay={onPlay}
    onpause={onPause}
    onended={() => { report('stop'); void goSibling(next); }}
  ></video>

  {#if loading}<div class="player-loading"><span></span>正在准备播放…</div>{/if}
  {#if error}
    <div class="player-error">
      <b>无法播放</b><p>{error}</p>
      <button onclick={() => nav.goItem(itemId)}>返回详情</button>
    </div>
  {/if}

  <div class="player-top">
    <button class="icon-btn back" title="返回详情" onclick={(e) => { e.stopPropagation(); nav.goItem(itemId); }}>←</button>
    <div class="player-title">
      <b>{detail?.title ?? source?.name ?? '正在加载'}</b>
      {#if detail?.kind === 'episode'}<span>S{detail.season_no ?? 1} · E{detail.episode_no ?? '?'}</span>{/if}
    </div>
    {#if source}
      <span class="method" class:transcoding={!source.supports_direct_play}>{method}</span>
    {/if}
  </div>

  {#if upNextVisible && next}
    <button class="up-next" onclick={(e) => { e.stopPropagation(); void goSibling(next); }}>
      <span>即将播放下一集 · {Math.ceil(remaining)}s</span>
      <b>{next.episode_no != null ? `${next.episode_no}. ` : ''}{next.title ?? '下一集'}</b>
      <i>立即播放 →</i>
    </button>
  {/if}

  <div class="player-osd">
    <div class="timeline">
      <div class="buffer" style="width:{buffered}%"></div>
      <input
        aria-label="播放进度"
        type="range"
        min="0"
        max={duration || 0.1}
        step="0.1"
        value={current}
        oninput={(e) => seekTo(Number(e.currentTarget.value))}
      />
    </div>
    <div class="time-row">
      <span>{fmtTime(current)}</span>
      <button onclick={() => (showRemaining = !showRemaining)}>
        {showRemaining ? `-${fmtTime(remaining)}` : fmtTime(duration)}
      </button>
      <span class="ends">结束于 {endClock()}</span>
    </div>
    <div class="controls">
      <div class="control-group">
        <button class="icon-btn" disabled={!previous} title="上一集 Shift+P" onclick={() => void goSibling(previous)}>◀│</button>
        <button class="icon-btn" title="快退 10 秒 J/←" onclick={() => seekBy(-10)}>↶<small>10</small></button>
        <button class="play-btn" title="播放/暂停 Space" onclick={togglePlay}>{playing ? 'Ⅱ' : '▶'}</button>
        <button class="icon-btn" title="快进 30 秒 L/→" onclick={() => seekBy(30)}>↷<small>30</small></button>
        <button class="icon-btn" disabled={!next} title="下一集 Shift+N" onclick={() => void goSibling(next)}>│▶</button>
      </div>
      <div class="control-group grow">
        <button class="icon-btn" title="静音 M" onclick={toggleMute}>{muted || volume === 0 ? '🔇' : volume < 0.5 ? '🔉' : '🔊'}</button>
        <input class="volume" aria-label="音量" type="range" min="0" max="1" step="0.01" value={muted ? 0 : volume} oninput={(e) => setVolume(Number(e.currentTarget.value))} />
      </div>
      <div class="control-group right">
        <select aria-label="播放速度" value={speed} onchange={(e) => setSpeed(Number(e.currentTarget.value))}>
          {#each [0.5,0.75,1,1.25,1.5,1.75,2,2.5,3,3.5,4] as rate}
            <option value={rate}>{rate}×</option>
          {/each}
        </select>
        <button class="text-btn" class:on={loop} title="循环播放" onclick={() => (loop = !loop)}>循环</button>
        <button class="text-btn" title="画面适配" onclick={cycleFit}>{fit === 'contain' ? '适应' : fit === 'cover' ? '填充' : '拉伸'}</button>
        <button class="icon-btn" disabled={!document.pictureInPictureEnabled} title="画中画" onclick={() => void togglePip()}>▣</button>
        <button class="icon-btn" title="全屏 F" onclick={() => void toggleFullscreen()}>⛶</button>
      </div>
    </div>
    {#if source?.transcode_reasons.length}
      <div class="transcode-info">{source.video_codec ?? 'video'} · {source.audio_codec ?? 'audio'} → HLS · {source.transcode_reasons.join(' · ')}</div>
    {/if}
  </div>
</section>

<style>
  .player-shell { position: fixed; inset: 0; z-index: 100; background: #000; overflow: hidden; color: #fff; cursor: default; }
  video { width: 100%; height: 100%; display: block; background: #000; }
  .player-shell::after { content: ''; position: absolute; inset: 0; pointer-events: none; background: linear-gradient(180deg, rgba(0,0,0,.5), transparent 22%, transparent 62%, rgba(0,0,0,.78)); opacity: 1; transition: opacity .25s; }
  .player-top { position: absolute; z-index: 2; top: 0; left: 0; right: 0; display: flex; align-items: center; gap: 14px; padding: 22px 28px; transition: opacity .25s; }
  .player-title { display: flex; flex-direction: column; min-width: 0; text-shadow: 0 2px 12px #000; }
  .player-title b { font-size: 17px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .player-title span { font-size: 12px; color: rgba(255,255,255,.65); }
  .method { margin-left: auto; padding: 4px 10px; border: 1px solid rgba(80,210,110,.45); color: #9aeea9; background: rgba(0,0,0,.45); border-radius: 999px; font: 11px var(--mono); }
  .method.transcoding { border-color: rgba(250,178,25,.45); color: #ffd479; }
  .player-osd { position: absolute; z-index: 3; left: 24px; right: 24px; bottom: 18px; transition: opacity .25s, transform .25s; }
  .timeline { height: 18px; position: relative; display: flex; align-items: center; }
  .timeline::before, .buffer { content: ''; position: absolute; left: 0; right: 0; height: 4px; border-radius: 2px; background: rgba(255,255,255,.2); }
  .buffer { right: auto; background: rgba(255,255,255,.42); }
  .timeline input { position: relative; z-index: 1; width: 100%; height: 18px; accent-color: #fff; cursor: pointer; opacity: .92; }
  .time-row { display: flex; align-items: center; gap: 10px; margin: 1px 2px 10px; color: rgba(255,255,255,.75); font: 11px var(--mono); }
  .time-row button { color: inherit; border: 0; background: none; font: inherit; cursor: pointer; }
  .ends { margin-left: 4px; color: rgba(255,255,255,.48); }
  .controls { display: flex; align-items: center; gap: 18px; }
  .control-group { display: flex; align-items: center; gap: 5px; }
  .control-group.grow { flex: 1; min-width: 100px; }
  .control-group.right { justify-content: flex-end; }
  button, select { font-family: inherit; }
  .icon-btn, .text-btn, .play-btn { border: 0; color: #fff; background: transparent; cursor: pointer; display: grid; place-items: center; border-radius: 8px; height: 38px; min-width: 38px; padding: 0 8px; font-weight: 650; }
  .icon-btn:hover, .text-btn:hover { background: rgba(255,255,255,.13); }
  .icon-btn:disabled { opacity: .25; cursor: default; }
  .icon-btn small { font-size: 8px; margin-left: -2px; }
  .play-btn { border-radius: 50%; background: #fff; color: #080808; width: 44px; height: 44px; font-size: 17px; box-shadow: 0 3px 18px rgba(0,0,0,.5); }
  .text-btn { font-size: 12px; }
  .text-btn.on { color: #8abdf6; background: rgba(57,135,229,.2); }
  .volume { width: min(110px, 10vw); accent-color: #fff; }
  select { border: 1px solid rgba(255,255,255,.18); background: rgba(0,0,0,.5); color: #fff; border-radius: 8px; padding: 7px 8px; }
  .transcode-info { margin: 8px 4px 0; color: rgba(255,255,255,.47); font: 10px var(--mono); text-align: right; }
  .player-loading, .player-error { position: absolute; z-index: 5; inset: 0; display: grid; place-content: center; justify-items: center; gap: 12px; background: #080808; }
  .player-loading span { width: 34px; height: 34px; border: 3px solid rgba(255,255,255,.18); border-top-color: #fff; border-radius: 50%; animation: spin .8s linear infinite; }
  .player-error b { font-size: 20px; }
  .player-error p { color: rgba(255,255,255,.62); max-width: 540px; text-align: center; }
  .player-error button { padding: 8px 15px; border: 1px solid rgba(255,255,255,.2); border-radius: 8px; background: rgba(255,255,255,.08); color: #fff; cursor: pointer; }
  .up-next { position: absolute; z-index: 4; right: 28px; bottom: 126px; width: 300px; padding: 14px 16px; text-align: left; color: #fff; border: 1px solid rgba(255,255,255,.16); border-radius: 12px; background: rgba(20,20,20,.88); backdrop-filter: blur(14px); box-shadow: 0 14px 40px rgba(0,0,0,.5); cursor: pointer; }
  .up-next span, .up-next i { display: block; color: rgba(255,255,255,.58); font-size: 11px; font-style: normal; }
  .up-next b { display: block; margin: 4px 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .up-next i { color: #8abdf6; }
  .osd-hidden { cursor: none; }
  .osd-hidden::after, .osd-hidden .player-top, .osd-hidden .player-osd { opacity: 0; pointer-events: none; }
  .osd-hidden .player-osd { transform: translateY(10px); }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (max-width: 720px) {
    .player-top { padding: 14px; }
    .player-osd { left: 10px; right: 10px; bottom: 8px; }
    .volume, .ends, .control-group.right .text-btn { display: none; }
    .controls { gap: 6px; }
    .control-group { gap: 1px; }
    .icon-btn { min-width: 34px; padding: 0 5px; }
    .up-next { right: 12px; bottom: 112px; width: min(300px, calc(100vw - 24px)); }
  }
</style>
