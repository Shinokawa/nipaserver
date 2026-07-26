// REST 客户端（开发期经 Vite 代理到 127.0.0.1:11810）
// 批次 B 端点未上线时：404/网络错误由调用方 catch，区块优雅隐藏。

import type {
  ChatHistoryRow,
  ChatResponse,
  ChatSession,
  Item,
  ItemDetail,
  Library,
  NextUpItem,
  PendingTask,
  PlaybackInfo,
  ResumeItem,
  SearchGroups,
  SystemInfo,
} from './types';

const BASE = '/api/v1';

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(BASE + path, {
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `${res.status} ${res.statusText}`);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

/** 列表请求，附带 X-Total-Count 头（缺头时 total=null，视为“未知总数”） */
async function requestList<T>(path: string): Promise<{ rows: T[]; total: number | null }> {
  const res = await fetch(BASE + path);
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `${res.status} ${res.statusText}`);
  }
  const rows = (await res.json()) as T[];
  const h = res.headers.get('X-Total-Count');
  const total = h !== null && h !== '' && !Number.isNaN(Number(h)) ? Number(h) : null;
  return { rows: Array.isArray(rows) ? rows : [], total };
}

export interface ItemsParams {
  kind?: string;
  sort?: string;
  air_year?: number;
  year?: number;
  genre?: string;
  search?: string;
  is_played?: boolean;
  is_favorite?: boolean;
  limit?: number;
  offset?: number;
}

function qs(params: Record<string, unknown>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
  }
  const s = q.toString();
  return s ? '?' + s : '';
}

export const api = {
  systemInfo: () => request<SystemInfo>('/system/info'),

  libraries: () => request<Library[]>('/libraries'),
  createLibrary: (body: { name: string; path: string; kind?: string }) =>
    request<Library>('/libraries', { method: 'POST', body: JSON.stringify(body) }),
  scanLibrary: (id: number) =>
    request<{ library_id: number; hint: string }>(`/libraries/${id}/scan`, { method: 'POST' }),

  items: (params: ItemsParams = {}) =>
    request<Item[]>(`/items${qs(params as Record<string, unknown>)}`),
  /** 同 items，但带 X-Total-Count（无限滚动用） */
  itemsPaged: (params: ItemsParams = {}) =>
    requestList<Item>(`/items${qs(params as Record<string, unknown>)}`),
  item: (id: number) => request<ItemDetail>(`/items/${id}`),

  playbackInfo: (fileId: number) =>
    request<PlaybackInfo>('/playback/info', {
      method: 'POST',
      body: JSON.stringify({ file_id: fileId, device_profile: { client: 'web' } }),
    }),

  // ===== 批次 B：首页 sections =====
  resume: () => request<ResumeItem[]>('/items/resume'),
  nextUp: () => request<NextUpItem[]>('/shows/next-up'),
  /** 专用最新端点（未上线会 throw，调用方自行退化到 /items?sort=added_at） */
  latest: (limit = 16, libraryId?: number) =>
    request<Item[]>(`/items/latest${qs({ limit, library: libraryId })}`),

  // ===== 批次 B：搜索（兼容分组对象或平铺数组两种返回） =====
  search: async (q: string): Promise<SearchGroups> => {
    const raw = await request<unknown>(`/search?q=${encodeURIComponent(q)}`);
    const groups: SearchGroups = { series: [], movies: [], episodes: [], other: [] };
    const push = (it: Item) => {
      if (it.kind === 'series') groups.series.push(it);
      else if (it.kind === 'movie') groups.movies.push(it);
      else if (it.kind === 'episode') groups.episodes.push(it);
      else groups.other.push(it);
    };
    if (Array.isArray(raw)) {
      for (const it of raw as Item[]) push(it);
    } else if (raw && typeof raw === 'object') {
      const o = raw as Record<string, unknown>;
      for (const key of ['series', 'movies', 'episodes', 'items', 'results', 'other']) {
        const arr = o[key];
        if (Array.isArray(arr)) for (const it of arr as Item[]) push(it);
      }
    }
    return groups;
  },

  // ===== 批次 B：已看/收藏/进度 =====
  setPlayed: (id: number, played: boolean) =>
    request<unknown>(`/items/${id}/played`, { method: played ? 'POST' : 'DELETE' }),
  setFavorite: (id: number, fav: boolean) =>
    request<unknown>(`/items/${id}/favorite`, { method: fav ? 'POST' : 'DELETE' }),
  reportProgress: (
    itemId: number,
    fileId: number,
    positionMs: number,
    durationMs: number | null,
    event: 'start' | 'progress' | 'stop'
  ) =>
    request<unknown>('/playback/progress', {
      method: 'POST',
      body: JSON.stringify({
        item_id: itemId,
        file_id: fileId,
        position_ms: positionMs,
        duration_ms: durationMs,
        event,
      }),
    }),

  scrapePending: () => request<PendingTask[]>('/scrape/pending'),
  scrapeTest: (evidence: string) =>
    request<{ task_id: number; hint: string }>('/scrape/test', {
      method: 'POST',
      body: JSON.stringify({ evidence }),
    }),

  chat: (message: string, sessionId?: number | null) =>
    request<ChatResponse>('/chat', {
      method: 'POST',
      body: JSON.stringify({ message, session_id: sessionId ?? undefined }),
    }),
  chatSessions: () => request<ChatSession[]>('/chat/sessions'),
  chatHistory: (id: number) => request<ChatHistoryRow[]>(`/chat/sessions/${id}/messages`),
};

/**
 * 图片 URL 统一出口：批次 C 图片伺服端点上线后只需改这里。
 * 目前 poster/backdrop 直接用 item 上的外链路径；backdrop 缺席时回退海报。
 */
export function imageUrl(
  item: { id: number; poster_path?: string | null; backdrop_path?: string | null },
  type: 'poster' | 'backdrop' = 'poster',
  width?: number
): string | null {
  // 批次 C：走本地缓存伺服（后端下载失败会 302 回源，天然兜底）。
  // 无源 URL 时返回 null 让调用方渲染渐变占位。
  const source = type === 'backdrop' ? item.backdrop_path : item.poster_path;
  if (!source) return null;
  const imageType = type === 'backdrop' ? 'backdrop' : 'primary';
  const w = width ? `?width=${width}` : '';
  return `/api/v1/items/${item.id}/images/${imageType}${w}`;
}
