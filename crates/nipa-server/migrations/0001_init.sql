-- NipaServer 初始 schema（开发文档 §9，0.2 全面修订版）
-- SQLite WAL 模式；外键在连接层启用（PRAGMA foreign_keys=ON，见 db.rs）。

-- 用户与会话
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL CHECK(role IN ('admin','guest')),
  password_hash TEXT NOT NULL            -- argon2id
);

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER,
  expires_at INTEGER
);

-- 媒体库
CREATE TABLE libraries (
  id INTEGER PRIMARY KEY,
  name TEXT,
  path TEXT NOT NULL,
  kind TEXT,
  options JSON
);

-- 物理文件
CREATE TABLE media_files (
  id INTEGER PRIMARY KEY,
  library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL,     -- 一律 '/' 分隔的规范化形式；非 UTF-8 路径 lossy 存储 + raw_path BLOB 保留原始字节
  raw_path BLOB,
  size INTEGER,
  mtime INTEGER,
  fingerprint TEXT,           -- sha256(size|mtime)[:16]，仅变更检测（是否需重算 hash）
  dandan_hash TEXT,           -- 前 16MB MD5；L0 缓存主键=(size, dandan_hash)
  ffprobe JSON,
  status TEXT,                -- pending|matched|ai_matched|needs_review|failed|ignored
  UNIQUE(library_id, rel_path)
);
CREATE INDEX idx_media_files_hash ON media_files(size, dandan_hash);

-- 逻辑条目（海报墙实体）
CREATE TABLE items (
  id INTEGER PRIMARY KEY,
  library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,  -- series 级冗余，中间节点归属明确
  kind TEXT CHECK(kind IN ('series','season','episode','movie')),
  parent_id INTEGER REFERENCES items(id) ON DELETE CASCADE,
  title TEXT,
  original_title TEXT,
  year INTEGER,
  season_no INTEGER,
  episode_no INTEGER,
  overview TEXT,
  rating REAL,
  poster_path TEXT,
  backdrop_path TEXT,
  added_at INTEGER,
  deleted_at INTEGER          -- 软删除：文件消失先标记，宽限期后清理（防 NAS 掉线误删）
);
CREATE INDEX idx_items_parent ON items(parent_id);
CREATE INDEX idx_items_library ON items(library_id, kind);

CREATE TABLE item_ids (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,     -- tmdb|bangumi|dandanplay|imdb
  external_id TEXT NOT NULL,
  UNIQUE(provider, external_id),   -- 合并去重的关键约束（§4.5）
  UNIQUE(item_id, provider)
);

-- 文件↔条目：支持合集文件（一文件多集）与多版本（一集多文件）
CREATE TABLE file_item (
  file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  episode_range TEXT,         -- 合集文件时如 "1-2"；常规为 NULL
  PRIMARY KEY (file_id, item_id)
);

-- 刮削
CREATE TABLE scrape_tasks (
  id INTEGER PRIMARY KEY,
  file_id INTEGER REFERENCES media_files(id) ON DELETE CASCADE,
  tier TEXT,
  state TEXT,                 -- queued|running|done|needs_review|failed
  result JSON,
  confidence TEXT,
  transcript JSON,            -- 保留策略：needs_review 与最近 N 条全量，其余仅摘要（防 SQLite 虚胖）
  model TEXT,
  tokens_in INTEGER,
  tokens_out INTEGER,
  created_at INTEGER
);

CREATE TABLE scrape_corrections (
  id INTEGER PRIMARY KEY,
  dir_path TEXT,
  pattern TEXT,
  item_id INTEGER REFERENCES items(id) ON DELETE CASCADE
);

-- provider 响应缓存
CREATE TABLE api_cache (
  cache_key TEXT PRIMARY KEY,
  provider TEXT,
  response JSON,
  expires_at INTEGER
);
CREATE INDEX idx_api_cache_expiry ON api_cache(expires_at);

-- 播放（0.2：加 user 与 file 维度——多版本文件时长不同，进度必须绑定文件）
CREATE TABLE watch_history (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  file_id INTEGER REFERENCES media_files(id) ON DELETE SET NULL,
  position_ms INTEGER,
  duration_ms INTEGER,
  updated_at INTEGER,
  PRIMARY KEY (user_id, item_id)
);

-- 下载/订阅（torrents 仅为 librqbit session 的投影缓存，§7.1）
CREATE TABLE torrents (
  id INTEGER PRIMARY KEY,
  info_hash TEXT UNIQUE,
  name TEXT,
  state TEXT,
  save_path TEXT,
  added_at INTEGER
);

CREATE TABLE subscriptions (
  id INTEGER PRIMARY KEY,
  rss_url TEXT,
  title TEXT,
  filters JSON,
  enabled INTEGER,
  last_check INTEGER
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value JSON                  -- api_key：OS keychain 优先，退化为文件加密
);
