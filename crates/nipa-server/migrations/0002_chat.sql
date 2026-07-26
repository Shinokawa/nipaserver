-- 管家对话（docs/06-管家设计.md §3）
CREATE TABLE chat_sessions (
  id INTEGER PRIMARY KEY,
  title TEXT,                 -- 首条用户消息截断生成
  summary TEXT,               -- 第 2 层记忆：滚动摘要（决定与承诺）
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE chat_messages (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('user','steward','tool')),
  content TEXT NOT NULL,      -- user/steward: 文本；tool: JSON {tool, arguments, output_preview, success}
  -- 压缩后从上下文移除但 DB 永存（UI 全史回看）；in_context 标记当前窗口成员
  in_context INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_chat_messages_session ON chat_messages(session_id, id);
