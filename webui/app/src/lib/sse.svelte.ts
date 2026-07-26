// 单一 EventSource 连 /api/v1/events，store 分发到各视图（docs/05 §6）。
// 断线自动重连：指数退避 1s → 2s → 4s … 上限 30s；顶栏胶囊显示连接态。

import type { AgentEnvelope, EventMsg } from './types';

export type ConnState = 'connecting' | 'open' | 'retrying';

const MAX_EVENTS_PER_TASK = 400;
const MAX_STEWARD_EVENTS = 300;

class SseStore {
  conn = $state<ConnState>('connecting');
  retryDelayMs = $state(1000);
  lastHeartbeat = $state(0);

  /** 扫描进度：library_id → 最近一条消息 */
  scanProgress = $state<Record<number, string>>({});
  /** 刮削任务状态：task_id → queued/running/done/needs_review/failed */
  scrapeStates = $state<Record<number, string>>({});
  /** 刮削 agent 事件流：task_id → 信封列表（按 seq） */
  scrapeEvents = $state<Record<number, AgentEnvelope[]>>({});
  /** 管家过程事件（当前对话轮，chat 请求返回后由视图清空） */
  stewardEvents = $state<AgentEnvelope[]>([]);

  #es: EventSource | null = null;
  #retryTimer: ReturnType<typeof setTimeout> | null = null;
  #listeners = new Set<(msg: EventMsg) => void>();

  /** 额外订阅（视图级副作用，如自动滚底）。返回取消函数。 */
  subscribe(fn: (msg: EventMsg) => void): () => void {
    this.#listeners.add(fn);
    return () => this.#listeners.delete(fn);
  }

  connect() {
    if (this.#es) return;
    this.#open();
  }

  #open() {
    this.conn = this.#es ? 'retrying' : 'connecting';
    this.#es?.close();
    const es = new EventSource('/api/v1/events');
    this.#es = es;

    es.onopen = () => {
      this.conn = 'open';
      this.retryDelayMs = 1000;
    };
    es.onerror = () => {
      es.close();
      if (this.#es !== es) return;
      this.#es = null;
      this.conn = 'retrying';
      this.#retryTimer && clearTimeout(this.#retryTimer);
      this.#retryTimer = setTimeout(() => this.#open(), this.retryDelayMs);
      this.retryDelayMs = Math.min(this.retryDelayMs * 2, 30_000);
    };
    es.onmessage = (e) => {
      let msg: EventMsg;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      this.#dispatch(msg);
    };
  }

  #dispatch(msg: EventMsg) {
    switch (msg.type) {
      case 'heartbeat':
        this.lastHeartbeat = msg.ts;
        break;
      case 'scan_progress':
        this.scanProgress[msg.library_id] = msg.message;
        break;
      case 'scrape_update':
        this.scrapeStates[msg.task_id] = msg.state;
        break;
      case 'scrape': {
        const list = (this.scrapeEvents[msg.task_id] ??= []);
        list.push(msg.agent);
        if (list.length > MAX_EVENTS_PER_TASK) list.splice(0, list.length - MAX_EVENTS_PER_TASK);
        break;
      }
      case 'steward':
        this.stewardEvents.push(msg.agent);
        if (this.stewardEvents.length > MAX_STEWARD_EVENTS) {
          this.stewardEvents.splice(0, this.stewardEvents.length - MAX_STEWARD_EVENTS);
        }
        break;
    }
    for (const fn of this.#listeners) fn(msg);
  }

  clearStewardEvents() {
    this.stewardEvents.length = 0;
  }

  /** 运行中的刮削任务 id（媒体库占位卡用） */
  get runningScrapeTasks(): number[] {
    return Object.entries(this.scrapeStates)
      .filter(([, s]) => s === 'running' || s === 'queued')
      .map(([id]) => Number(id));
  }
}

export const sse = new SseStore();
