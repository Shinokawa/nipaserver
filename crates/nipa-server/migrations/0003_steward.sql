-- 首播时间一等公民（docs/06-管家设计.md §2.1）：episode 行=放送日，series 行=首播日
ALTER TABLE items ADD COLUMN air_date TEXT;
CREATE INDEX idx_items_air_date ON items(air_date);

-- 重识别需要原始证据（管家 requeue_scrape 带 hint 重入队）
ALTER TABLE scrape_tasks ADD COLUMN evidence TEXT;
