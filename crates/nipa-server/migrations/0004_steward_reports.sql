-- 管家巡检报告（docs/06-管家设计.md：主动唤醒产出的 feed）
CREATE TABLE steward_reports (
  id INTEGER PRIMARY KEY,
  session_id INTEGER REFERENCES chat_sessions(id) ON DELETE SET NULL,
  report TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_steward_reports_created ON steward_reports(created_at DESC);
