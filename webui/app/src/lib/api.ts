// REST 客户端（开发期经 Vite 代理到 127.0.0.1:11810）

import type {
  ChatHistoryRow,
  ChatResponse,
  ChatSession,
  Item,
  ItemDetail,
  Library,
  PendingTask,
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
  return res.json() as Promise<T>;
}

export const api = {
  systemInfo: () => request<SystemInfo>('/system/info'),

  libraries: () => request<Library[]>('/libraries'),
  createLibrary: (body: { name: string; path: string; kind?: string }) =>
    request<Library>('/libraries', { method: 'POST', body: JSON.stringify(body) }),
  scanLibrary: (id: number) =>
    request<{ library_id: number; hint: string }>(`/libraries/${id}/scan`, { method: 'POST' }),

  items: (params: {
    kind?: string;
    sort?: string;
    air_year?: number;
    limit?: number;
    offset?: number;
  } = {}) => {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) q.set(k, String(v));
    }
    const qs = q.toString();
    return request<Item[]>(`/items${qs ? '?' + qs : ''}`);
  },
  item: (id: number) => request<ItemDetail>(`/items/${id}`),

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
