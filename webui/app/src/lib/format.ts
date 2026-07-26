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

/** 片长 "2h 15m" / "45m" 格式（Jellyfin meta 行同款） */
export function fmtRuntime(ms: number | null | undefined): string {
  if (!ms || ms <= 0) return '';
  const totalMin = Math.round(ms / 60_000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`;
  return `${m}m`;
}

/** 系列年份区间："2019 至今"（Continuing）/ "2019 - 2021"（Ended）/ "2019" */
export function fmtYearSpan(
  year: number | null | undefined,
  seriesStatus: string | null | undefined,
  endDate: string | null | undefined
): string {
  if (!year) return '';
  const status = (seriesStatus ?? '').toLowerCase();
  if (status === 'continuing') return `${year} 至今`;
  const endYear = endDate ? Number(endDate.slice(0, 4)) : null;
  if (endYear && !Number.isNaN(endYear) && endYear !== year) return `${year} - ${endYear}`;
  return String(year);
}

/** 剩余分钟（继续播放按钮："继续 · 剩余 X 分钟"） */
export function remainingMinutes(
  runtimeMs: number | null | undefined,
  positionMs: number | null | undefined
): number | null {
  if (!runtimeMs || !positionMs || positionMs <= 0 || positionMs >= runtimeMs) return null;
  return Math.max(1, Math.round((runtimeMs - positionMs) / 60_000));
}

/** 进度百分比 0-100（clamp；无效输入返回 0） */
export function progressPct(
  positionMs: number | null | undefined,
  durationMs: number | null | undefined
): number {
  if (!positionMs || !durationMs || durationMs <= 0) return 0;
  return Math.min(100, Math.max(0, (positionMs / durationMs) * 100));
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
