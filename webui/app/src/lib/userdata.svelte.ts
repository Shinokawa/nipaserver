// 已看/收藏共享状态：乐观更新 + API 调用 + 失败回滚。
// 覆盖层（overrides）叠在 item.user_data 之上，卡片与详情页读同一份状态。

import { api } from './api';
import { toast } from './toast.svelte';
import type { Item, UserData } from './types';

class UserDataStore {
  #overrides = $state<Record<number, UserData>>({});

  /** 合并视图：override > item.user_data */
  get(item: Pick<Item, 'id' | 'user_data'>): UserData {
    return { ...(item.user_data ?? {}), ...(this.#overrides[item.id] ?? {}) };
  }

  isPlayed(item: Pick<Item, 'id' | 'user_data'>): boolean {
    return !!this.get(item).played;
  }
  isFavorite(item: Pick<Item, 'id' | 'user_data'>): boolean {
    return !!this.get(item).is_favorite;
  }
  positionMs(item: Pick<Item, 'id' | 'user_data'>): number {
    return this.get(item).position_ms ?? 0;
  }

  async togglePlayed(item: Pick<Item, 'id' | 'user_data'>) {
    const prev = this.isPlayed(item);
    const next = !prev;
    this.#overrides[item.id] = { ...this.#overrides[item.id], played: next };
    try {
      await api.setPlayed(item.id, next);
    } catch (e) {
      this.#overrides[item.id] = { ...this.#overrides[item.id], played: prev };
      toast.show(`标记失败：${shortErr(e)}`, 'crit');
    }
  }

  async toggleFavorite(item: Pick<Item, 'id' | 'user_data'>) {
    const prev = this.isFavorite(item);
    const next = !prev;
    this.#overrides[item.id] = { ...this.#overrides[item.id], is_favorite: next };
    try {
      await api.setFavorite(item.id, next);
    } catch (e) {
      this.#overrides[item.id] = { ...this.#overrides[item.id], is_favorite: prev };
      toast.show(`收藏失败：${shortErr(e)}`, 'crit');
    }
  }
}

function shortErr(e: unknown): string {
  const s = String(e instanceof Error ? e.message : e);
  return s.length > 60 ? s.slice(0, 60) + '…' : s;
}

export const userdata = new UserDataStore();
