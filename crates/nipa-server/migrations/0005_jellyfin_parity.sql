-- Jellyfin 对标批次 A：数据地基（docs/07-jellyfin对标实施计划.md）
-- 依据 research/jellyfin-full/entity-model.md 的"必备"级清单

-- items 增列
ALTER TABLE items ADD COLUMN sort_name TEXT;          -- 排序名（刮削写入；兜底 title）
ALTER TABLE items ADD COLUMN end_date TEXT;           -- 完结日期（series）
ALTER TABLE items ADD COLUMN runtime_ms INTEGER;      -- 元数据时长（ffprobe/provider）
ALTER TABLE items ADD COLUMN official_rating TEXT;    -- 分级（TV-14 等）
ALTER TABLE items ADD COLUMN series_status TEXT;      -- Continuing|Ended（series 行）
ALTER TABLE items ADD COLUMN is_virtual INTEGER NOT NULL DEFAULT 0;  -- 虚拟季/缺失集
ALTER TABLE items ADD COLUMN series_id INTEGER REFERENCES items(id) ON DELETE CASCADE; -- episode 冗余
ALTER TABLE items ADD COLUMN tagline TEXT;
ALTER TABLE items ADD COLUMN date_modified INTEGER;
CREATE INDEX idx_items_sort_name ON items(sort_name);
CREATE INDEX idx_items_series ON items(series_id);

-- genre/studio/tag 统一表（Jellyfin ItemValue 结构）
CREATE TABLE item_values (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('genre','studio','tag')),
  value TEXT NOT NULL,
  UNIQUE(kind, value)
);
CREATE TABLE item_value_map (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  value_id INTEGER NOT NULL REFERENCES item_values(id) ON DELETE CASCADE,
  PRIMARY KEY (item_id, value_id)
);
CREATE INDEX idx_ivm_value ON item_value_map(value_id);

-- 演职员（声优 role=角色名）
CREATE TABLE people (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'actor',   -- actor|director|writer|composer|other
  image_url TEXT,
  provider_ids JSON,                    -- {"bangumi_person": 123, "tmdb_person": 456}
  UNIQUE(name, kind)
);
CREATE TABLE item_people (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  role TEXT,                            -- 配音角色名/职位描述
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (item_id, person_id, role)
);
CREATE INDEX idx_item_people_person ON item_people(person_id);

-- 多图类型（poster_path/backdrop_path 保留为快捷列）
CREATE TABLE item_images (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  image_type TEXT NOT NULL CHECK(image_type IN ('primary','backdrop','thumb','logo','banner')),
  url TEXT,                             -- 远程源
  local_path TEXT,                      -- data/images/ 下的缓存相对路径
  width INTEGER, height INTEGER,
  blurhash TEXT,
  PRIMARY KEY (item_id, image_type)
);

-- watch_history 用户态增列（Jellyfin UserItemData 对齐）
ALTER TABLE watch_history ADD COLUMN played INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN last_played_at INTEGER;
ALTER TABLE watch_history ADD COLUMN audio_stream_index INTEGER;
ALTER TABLE watch_history ADD COLUMN subtitle_stream_index INTEGER;
CREATE INDEX idx_watch_resume ON watch_history(user_id, last_played_at DESC);
