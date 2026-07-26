-- M4 下载与订阅：librqbit Session 是事实源，本表只增强投影信息。
ALTER TABLE torrents ADD COLUMN session_id INTEGER;
ALTER TABLE torrents ADD COLUMN progress_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE torrents ADD COLUMN total_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE torrents ADD COLUMN uploaded_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE torrents ADD COLUMN error TEXT;

-- 完成入库是可恢复状态机，不依赖一次性 torrent-complete 事件。
CREATE TABLE torrent_ingests (
  info_hash TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending','done')),
  last_error TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (info_hash, manifest_hash)
);

-- RSS 条目持久化去重；重启后不会因 last_check 丢失而重复添加。
CREATE TABLE subscription_entries (
  subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
  entry_key TEXT NOT NULL,
  title TEXT NOT NULL,
  source_url TEXT NOT NULL,
  info_hash TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (subscription_id, entry_key)
);

ALTER TABLE subscriptions ADD COLUMN last_error TEXT;
CREATE INDEX idx_subscription_entries_hash ON subscription_entries(info_hash);
