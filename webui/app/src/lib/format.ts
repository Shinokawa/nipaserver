// 展示格式化工具

/** 海报占位渐变：art-g1..g7 按 item.id 取模分配（mockup 类名） */
export function artClass(id: number): string {
  return `art-g${(Math.abs(id) % 7) + 1}`;
}

export function fmtSize(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(1) + ' GB';
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + ' MB';
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KB';
  return bytes + ' B';
}

export function fmtDuration(ms: number): string {
  if (ms >= 1000) return (ms / 1000).toFixed(1) + 's';
  return ms + 'ms';
}

export function fmtTime(unixSec: number): string {
  const d = new Date(unixSec * 1000);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  const hm = d.toTimeString().slice(0, 5);
  if (sameDay) return hm;
  return `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

/** 参数摘要：单行 JSON，超长截断 */
export function fmtArgs(args: unknown, max = 120): string {
  let s: string;
  try {
    s = typeof args === 'string' ? args : JSON.stringify(args);
  } catch {
    s = String(args);
  }
  if (!s) return '';
  return s.length > max ? s.slice(0, max) + '…' : s;
}

/** 极简 markdown：转义 HTML 后支持 **bold**、`code` 与换行 */
export function renderMarkdown(text: string): string {
  const esc = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return esc
    .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
    .replace(/`([^`]+)`/g, '<code style="font-family:var(--mono);font-size:12px">$1</code>')
    .replace(/\n/g, '<br>');
}

export function confidenceBadge(conf: string | null | undefined): string {
  switch (conf) {
    case 'high':
      return 'good';
    case 'medium':
      return 'warn';
    case 'low':
      return 'serious';
    default:
      return '';
  }
}
