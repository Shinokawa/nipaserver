// 轻量 toast（右下角浮出，3.2s 自动消失）

export interface Toast {
  id: number;
  msg: string;
  kind: 'info' | 'good' | 'warn' | 'crit';
}

let nextId = 1;

class ToastStore {
  list = $state<Toast[]>([]);

  show(msg: string, kind: Toast['kind'] = 'info') {
    const id = nextId++;
    this.list.push({ id, msg, kind });
    setTimeout(() => {
      const i = this.list.findIndex((t) => t.id === id);
      if (i >= 0) this.list.splice(i, 1);
    }, 3200);
  }
}

export const toast = new ToastStore();
