// 后端 API 类型（对齐 crates/nipa-server/src/api.rs / api_library.rs
// 与 docs/03-agent接口契约.md §4）

export interface SystemInfo {
  name: string;
  version: string;
  platform: string;
  arch: string;
  headless: boolean;
  data_dir: string;
  database_ok: boolean;
  capabilities: {
    ffmpeg: boolean;
    dandanplay_l1: boolean;
    ai_scrape: boolean;
  };
}

export interface Library {
  id: number;
  name: string | null;
  path: string;
  kind: string | null;
  file_count: number;
}

export interface Item {
  id: number;
  kind: string; // series | movie | season | episode
  parent_id: number | null;
  title: string | null;
  original_title: string | null;
  year: number | null;
  season_no: number | null;
  episode_no: number | null;
  air_date: string | null;
  poster_path: string | null;
}

export interface ItemDetail extends Item {
  external_ids: [string, string][];
  children: Item[];
  files: { id: number; rel_path: string; size: number }[];
}

export interface PendingTask {
  task_id: number;
  file: string | null;
  result: Record<string, unknown> | null;
  confidence: string | null;
  evidence: string | null;
}

export interface ChatSession {
  id: number;
  title: string | null;
  updated_at: number;
}

export interface ChatHistoryRow {
  id: number;
  role: string; // user | steward | tool
  content: unknown; // tool 行为 JSON 对象，user/steward 行为字符串
  created_at: number;
}

export interface ToolEventSnap {
  tool: string;
  arguments: unknown;
  output_preview: string;
  success: boolean;
}

export interface ChatResponse {
  session_id: number;
  reply: string;
  tool_events: ToolEventSnap[];
}

// ===== agent 事件协议（契约 §4，SSE 与 transcript 共用） =====

export type AgentEvent =
  | { type: 'task_started'; model: string; max_rounds: number }
  | { type: 'round_started'; round: number; max_rounds: number }
  | { type: 'assistant_message'; text: string }
  | { type: 'tool_call_begin'; call_id: string; tool: string; arguments: unknown }
  | {
      type: 'tool_call_end';
      call_id: string;
      tool: string;
      success: boolean;
      output_preview: string;
      error?: string | null;
      duration_ms: number;
    }
  | {
      type: 'token_usage';
      last_input: number;
      last_output: number;
      total_input: number;
      total_output: number;
    }
  | { type: 'retrying'; attempt: number; max_attempts: number; message: string }
  | { type: 'warning'; message: string }
  | { type: 'task_completed'; result: unknown; rounds_used: number; duration_ms: number }
  | { type: 'task_failed'; reason: unknown; message: string; rounds_used: number }
  | { type: 'task_aborted'; reason: unknown };

export type AgentEnvelope = { task_id: string; seq: number; ts_ms: number } & AgentEvent;

// ===== SSE 载荷（nipa-core EventMsg） =====

export type EventMsg =
  | { type: 'heartbeat'; ts: number }
  | { type: 'scan_progress'; library_id: number; message: string }
  | { type: 'scrape_update'; task_id: number; state: string }
  | { type: 'scrape'; task_id: number; agent: AgentEnvelope }
  | { type: 'steward'; agent: AgentEnvelope };
