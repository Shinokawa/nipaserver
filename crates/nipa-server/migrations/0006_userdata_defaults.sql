-- Jellyfin 对标批次 B 前置（docs/07 §批次B）
-- auth 未实现（§8.4），播放上报固定 user_id=1——先落一个默认用户满足外键。
INSERT INTO users (id, name, role, password_hash)
SELECT 1, 'default', 'admin', ''
WHERE NOT EXISTS (SELECT 1 FROM users WHERE id = 1);

-- 0005 之前入库的 episode 行回填 series_id 冗余列（next-up/resume 免两跳 JOIN）
UPDATE items SET series_id = (
  SELECT s.parent_id FROM items s WHERE s.id = items.parent_id
) WHERE kind = 'episode' AND series_id IS NULL;
