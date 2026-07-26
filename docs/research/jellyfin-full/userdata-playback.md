# Jellyfin 用户数据与播放状态精读报告（对标 nipaserver）

来源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin`（GPL，以下只提炼逻辑与协议，不做代码翻译）。

---

## 1. UserItemData 字段全表与"已播放"判定

### 1.1 字段全表

存储实体 `MediaBrowser.Controller/Entities/UserItemData.cs`，DB 主键为 (UserId, ItemId, CustomDataKey)：

| 字段 | 类型 | 语义 |
|---|---|---|
| Key (CustomDataKey) | string | 用户数据键（同一条目可能因元数据变更留下多个 key，读取时按条目当前 key 顺序解析） |
| Rating | double? | 用户评分 0–10，越界抛错；`MinLikeValue = 6.5` |
| PlaybackPositionTicks | long | 续播位置，**1 tick = 100ns**（1s = 10,000,000 ticks；注释里的 "1 tick = 10000 ms" 是 Jellyfin 自己的文档笔误） |
| PlayCount | int | 播放次数 |
| IsFavorite | bool | 收藏 |
| LastPlayedDate | DateTime? | 最近播放时间 |
| Played | bool | 已看完标记 |
| AudioStreamIndex | int? | 记住的音轨选择 |
| SubtitleStreamIndex | int? | 记住的字幕选择 |
| Likes | bool?（派生，不落库为独立列） | `Rating >= 6.5` 为 true；写 Likes=true → Rating=10，false → Rating=1，null → 清空 |

对外 DTO（`UserItemDataDto`）额外派生两个文件夹聚合字段：`PlayedPercentage`（子项已播比例）与 `UnplayedItemCount`；Series/Season 级 `Played = playedCount >= totalCount`（见 `Folder.FillUserDataDtoValues`）。

### 1.2 "已播放"判定规则（`UserDataManager.UpdatePlayState`，核心算法）

配置常量（`ServerConfiguration`）：**MinResumePct = 5，MaxResumePct = 90，MinResumeDurationSeconds = 300**（有声书另有 Min/MaxAudiobookResume=5 分钟，可忽略）。

对每次上报的 position（无 position 时用总时长代入）：

1. `pct = position / runtime * 100`
2. **pct < 5%** → 视为刚开始，position 归零（不记录续播点），Played 不变
3. **pct > 90% 或 position >= runtime - 1 秒** → 判定看完：position 归零，`Played = true`，返回 playedToCompletion
4. 5% ≤ pct ≤ 90% 但 **总时长 < 300 秒** → 短视频不记续播点，直接 `Played = true`、position 归零
5. 否则 → 正常记录 `PlaybackPositionTicks = position`
6. **runtime 未知** → 只能假定播完：`Played = true`，position 归零

注意：该函数**只置 true 不置 false**——中途重看不会清掉已看完标记；只有显式"标记未播放"才清。

---

## 2. 进度上报协议（PlaystateController + SessionManager）

三个正式端点（POST，Body 为 JSON，均返回 204）：

### 2.1 `POST /Sessions/Playing`（开始播放）
`PlaybackStartInfo` 关键字段：`ItemId`、`MediaSourceId`、`PositionTicks`、`AudioStreamIndex`、`SubtitleStreamIndex`、`PlayMethod`(DirectPlay/DirectStream/Transcode)、`PlaySessionId`、`LiveStreamId`、`CanSeek`、`IsPaused`、`IsMuted`、`VolumeLevel`、`NowPlayingQueue`。

服务端动作（`SessionManager.OnPlaybackStart`）：
- **PlayCount++，LastPlayedDate = now**（即：播放次数在开始时就 +1，不是在结束时）
- 不支持续播位置的类型（如照片）直接 Played=true
- 更新会话 NowPlaying、启动服务端 1 秒一跳的自动进度计时器（仅内存推进 NowPlaying 位置，**isAutomated=true 的自动进度不写库**）

### 2.2 `POST /Sessions/Playing/Progress`（进度心跳，客户端约每 10 秒发一次，暂停/跳转/换轨时也发）
`PlaybackProgressInfo` 字段同上再加 `IsPaused`、`RepeatMode`、`PlaybackOrder`。

服务端动作（每次真实上报都写库）：
- 有 PositionTicks 时跑一遍 **§1.2 的 UpdatePlayState**（即中途跨过 90% 也会即时判 Played）
- 记录 Audio/SubtitleStreamIndex（受用户 RememberAudioSelections/RememberSubtitleSelections 偏好控制）
- 保存后若 Played=true，会向同条目的其他版本传播已播状态并清其续播点

### 2.3 `POST /Sessions/Playing/Stopped`（停止）
`PlaybackStopInfo`：`ItemId`、`MediaSourceId`、`PositionTicks`（最终位置）、`PlaySessionId`、`Failed`（播放失败标志）、`NextMediaType`。

服务端动作：
- `Failed=true` → 什么都不更新
- 有 PositionTicks → 跑 UpdatePlayState（决定是记续播点还是判看完）
- **无 PositionTicks → 假定播完：PlayCount++、Played=true、position=0**
- 负数 PositionTicks 是 400 错误
- 停止转码任务、关闭 LiveStream

另有 `POST /Sessions/Playing/Ping?playSessionId=`（保活转码）与手动标记：
- `POST /UserPlayedItems/{itemId}?datePlayed=` → MarkPlayed：Played=true、position=0、PlayCount=max(PlayCount,1)（传了 datePlayed 才 ++）、LastPlayedDate=datePlayed??now
- `DELETE /UserPlayedItems/{itemId}` → 清 Played、position

---

## 3. NextUp 与 Resume 的确切查询逻辑

### 3.1 NextUp（`GET /Shows/NextUp`，参数：seriesId?、parentId?、nextUpDateCutoff?、enableResumable=true、enableRewatching=false、startIndex/limit）

两步算法（`NextUpService` + `TVSeriesManager`）：

**Step 1 — 选出候选剧集（series 级）**：episodes JOIN user_data，按 series 分组取 `MAX(LastPlayedDate)`，过滤 `>= nextUpDateCutoff`，按该时间倒序、取 limit。即：**只有播过（有 LastPlayedDate）的剧才会进 NextUp**，最近看的剧排最前。

**Step 2 — 每部剧算下一集**：
1. **最后看完的一集**：该剧中 `Played=true` 且 `season_no != 0`（**排除第 0 季/特殊集**）的集，按 `(season_no DESC, episode_no DESC)` 取第一个 → 得到位置 (S, E)。注意用的是**集号最大**的已播集，不是最近播的。
2. **下一集**：该剧中 `Played 不为 true`、非虚拟条目（无文件的占位集）、`season_no != 0` 的集里，取满足 `season_no > S OR (season_no = S AND episode_no > E)` 的最小 `(season_no ASC, episode_no ASC)` → **天然跨季**（S1E12 看完 → S2E01）。若该剧从未有已播集（只靠 rewatching/cutoff 进来），则直接取全剧最小未播集（即第一集）。
3. **特殊集（第 0 季）**：仅当服务器配置"特殊集显示在季内"时参与。带 `AirsBeforeSeasonNumber/AirsAfterSeasonNumber` 元数据的特殊集与"最后看完集、下一集"一起按 AiredEpisodeOrder 排序，取最后看完集之后的第一个未播集——特殊集可插队成为 NextUp。
4. **enableResumable=false** 时：若下一集已有续播进度（position>0），从 NextUp 剔除（它会出现在 Resume/继续观看里，避免两个列表重复）。默认 true。
5. **enableRewatching**：另按 LastPlayedDate 找最近播的已播集，向后取下一**已播**集（重刷模式）。
6. 汇总所有剧的下一集，按"该剧最后观看时间倒序"排序，再 startIndex/limit 分页。

### 3.2 Resume / 继续观看（`GET /UserItems/Resume`）

- 过滤条件：该用户 user_data 中 **`PlaybackPositionTicks > 0`** 的条目（§1.2 保证了 <5% 和 >90% 的都已归零，所以这一个条件就够）
- `IsVirtualItem = false`
- 排序：**DatePlayed（LastPlayedDate）倒序**
- 若查询包含 Series 类型：一部剧可"resumable"的条件是 *有集在播* 或 *既有已播集又有未播集*（半途弃剧也算）
- 多版本（一集多文件）时只保留最近播的那个版本（nipaserver 的 file 维度对应此处）

---

## 4. 收藏 / 评分 API（UserLibraryController + ItemsController）

| 端点 | 语义 |
|---|---|
| `POST /UserFavoriteItems/{itemId}` | 收藏，`IsFavorite=true`，返回 UserItemDataDto |
| `DELETE /UserFavoriteItems/{itemId}` | 取消收藏 |
| `POST /UserItems/{itemId}/Rating?likes=true|false` | 喜欢/不喜欢（内部写 Rating=10/1） |
| `DELETE /UserItems/{itemId}/Rating` | 清除评分 |
| `GET /UserItems/{itemId}/UserData` | 读取用户数据 |
| `POST /UserItems/{itemId}/UserData` | 通用更新（UpdateUserItemDataDto，所有字段可选、只更新传入的：Rating/PlaybackPositionTicks/PlayCount/IsFavorite/Likes/Played/LastPlayedDate） |
| `/Items?filters=IsPlayed,IsUnplayed,IsFavorite,IsResumable,Likes,Dislikes` | 列表过滤 |

---

## 5. nipaserver watch_history 演进建议

当前表（`/Users/sakiko/Desktop/nipaserver/crates/nipa-server/migrations/0001_init.sql`）：`user_id/item_id/file_id/position_ms/duration_ms/updated_at`，PK (user_id, item_id)。本质是"续播点表"，缺整个用户状态维度。

### 5.1 增列清单（新 migration，如 0005_user_item_data.sql）

建议把表演进为 Jellyfin 的 UserItemData 等价物（可保留表名，也可改名 `user_item_data`）：

```sql
ALTER TABLE watch_history ADD COLUMN played INTEGER NOT NULL DEFAULT 0;      -- 看完标记
ALTER TABLE watch_history ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;  -- Playing 时 +1
ALTER TABLE watch_history ADD COLUMN last_played_at INTEGER;                 -- 排序 NextUp/Resume 都靠它
ALTER TABLE watch_history ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN user_rating REAL;                       -- 0..10，NULL=未评分
-- 可选（有多音轨/字幕记忆需求再加）：
ALTER TABLE watch_history ADD COLUMN audio_stream_idx INTEGER;
ALTER TABLE watch_history ADD COLUMN subtitle_stream_idx INTEGER;

CREATE INDEX idx_wh_resume ON watch_history(user_id, position_ms) WHERE position_ms > 0;
CREATE INDEX idx_wh_played ON watch_history(user_id, played);
CREATE INDEX idx_wh_last_played ON watch_history(user_id, last_played_at);
```

要点：
- **PK 保持 (user_id, item_id) 即可**——Jellyfin 的 CustomDataKey 多 key 机制是历史包袱，nipaserver 有稳定 item id，不需要。file_id 列保留，语义为"最近播的是哪个文件版本"（对应 Jellyfin 多版本 resume 选版本逻辑）。
- 收藏/评分对 series 也要能写（不只是 episode），(user_id, item_id) 天然支持，item 指向 series 行即可。
- series/season 级的 Played 与 UnplayedItemCount **不落库，查询时聚合**（Jellyfin 也是这样，见 §1.1）。

### 5.2 played 判定常量（服务端统一实现，客户端只报裸 position）

```rust
const MIN_RESUME_PCT: f64 = 5.0;        // <5% 不记进度
const MAX_RESUME_PCT: f64 = 90.0;       // >90% 判看完
const MIN_RESUME_DURATION_SEC: u64 = 300; // 总长<5min 播了就算看完
// 以及：position >= duration - 1s 也判看完；duration 未知 → 直接判看完
```

判定后：看完 → `position_ms=0, played=1`；<5% → `position_ms=0`（played 不动）；否则存 position。**played 只置 1 不置 0**，清除走显式接口。

### 5.3 播放上报端点设计（贴 nipaserver 现有 /api/v1 风格，合并 Jellyfin 三端点）

```
POST /api/v1/playback/playing   { item_id, file_id?, position_ms? }
  → play_count+=1, last_played_at=now, upsert 行

POST /api/v1/playback/progress  { item_id, file_id?, position_ms, duration_ms?, paused? }
  → 跑 5.2 判定后 upsert；客户端约 10s 一次 + 暂停/跳转时立即发
  → duration_ms 优先取 ffprobe（media_files.ffprobe），客户端报的做兜底

POST /api/v1/playback/stopped   { item_id, file_id?, position_ms?, failed? }
  → failed=true 忽略；无 position 视为播完(played=1, play_count+=1)；有则跑 5.2

POST   /api/v1/items/{id}/played     → played=1, position_ms=0, play_count=max(1,..)
DELETE /api/v1/items/{id}/played     → played=0, position_ms=0
POST   /api/v1/items/{id}/favorite   / DELETE 同路径
PUT    /api/v1/items/{id}/rating     { rating: 0..10 | null }
GET    /api/v1/items/{id}/userdata   → 单条目用户数据（列表接口建议直接 LEFT JOIN 内嵌 user_data 对象）
```

简化取舍：nipaserver 无转码会话/多用户投屏，可不做 PlaySessionId/服务端 1s 自动进度计时器；Jellyfin 的自动进度本来就不落库，跳过无损。

### 5.4 Resume 查询 SQL 思路

```sql
-- 继续观看（电影+单集混排；§3.2 语义）
SELECT i.*, wh.position_ms, wh.duration_ms
FROM watch_history wh
JOIN items i ON i.id = wh.item_id AND i.deleted_at IS NULL
WHERE wh.user_id = ?
  AND wh.position_ms > 0
  AND i.kind IN ('movie','episode')
ORDER BY wh.last_played_at DESC
LIMIT ?;
```

（因为写入端已把 <5% 和 >90% 的 position 归零，一个 `position_ms > 0` 即等价 Jellyfin 的 IsResumable。）

### 5.5 NextUp 查询 SQL 思路（适配 items 树：episode.parent→season, season.parent→series）

```sql
-- Step 1：候选剧（按最近观看倒序）。ep→series 通过两层 parent_id 上溯：
WITH ep AS (
  SELECT e.id AS episode_id, s.parent_id AS series_id,
         e.season_no, e.episode_no
  FROM items e JOIN items s ON s.id = e.parent_id      -- e=episode, s=season
  WHERE e.kind = 'episode' AND e.deleted_at IS NULL
),
recent_series AS (
  SELECT ep.series_id, MAX(wh.last_played_at) AS last_watched
  FROM watch_history wh JOIN ep ON ep.episode_id = wh.item_id
  WHERE wh.user_id = ?1 AND wh.last_played_at IS NOT NULL
  GROUP BY ep.series_id
  ORDER BY last_watched DESC LIMIT ?2
),
-- Step 2a：每剧"最后看完的位置"（排除第0季；取集号最大而非时间最近）
last_pos AS (
  SELECT ep.series_id, ep.season_no, ep.episode_no,
         ROW_NUMBER() OVER (PARTITION BY ep.series_id
                            ORDER BY ep.season_no DESC, ep.episode_no DESC) rn
  FROM ep JOIN watch_history wh
       ON wh.item_id = ep.episode_id AND wh.user_id = ?1 AND wh.played = 1
  WHERE ep.season_no != 0
)
-- Step 2b：位置之后第一个未播集（跨季由 (season_no, episode_no) 字典序保证）
SELECT rs.series_id, rs.last_watched, next_ep.*
FROM recent_series rs
LEFT JOIN last_pos lp ON lp.series_id = rs.series_id AND lp.rn = 1
JOIN LATERAL？-- SQLite 无 LATERAL，用相关子查询取 min：
  (SELECT ep2.episode_id FROM ep ep2
   LEFT JOIN watch_history w2 ON w2.item_id = ep2.episode_id AND w2.user_id = ?1
   WHERE ep2.series_id = rs.series_id
     AND ep2.season_no != 0
     AND COALESCE(w2.played, 0) = 0
     AND (lp.series_id IS NULL
          OR ep2.season_no > lp.season_no
          OR (ep2.season_no = lp.season_no AND ep2.episode_no > lp.episode_no))
   ORDER BY ep2.season_no, ep2.episode_no LIMIT 1) AS next_ep
ORDER BY rs.last_watched DESC;
```

实现时在 Rust 里分两条查询更清晰（Jellyfin 也是先取 series keys 再批量算每剧下一集）：注意三条 Jellyfin 语义要保留——**排除 season_no=0**、**排除无文件的占位集**（nipaserver 用 `EXISTS (SELECT 1 FROM file_item WHERE item_id=ep.id)` 代替 IsVirtualItem）、以及可选的 `enableResumable=false` 时剔除 `position_ms>0` 的下一集。特殊集插队（AirsBefore/AfterSeasonNumber）依赖 items 没有的元数据列，属低优先级，可作为 backlog（需先给 items 增加 airs_before_season/airs_after_season 两列）。

### 5.6 差距清单总结（按优先级）

| # | 差距 | 补法 | 优先级 |
|---|---|---|---|
| 1 | watch_history 无 played/play_count/last_played_at/favorite/rating | §5.1 migration | P0 |
| 2 | 无任何播放上报端点 | §5.3 三个 /playback/* 端点 + §5.2 常量 | P0 |
| 3 | 无 Resume（继续观看）列表 | §5.4 SQL，挂 `/api/v1/items?filter=resumable` 或独立 `/api/v1/resume` | P0 |
| 4 | 无 NextUp | §5.5 两步查询，`/api/v1/nextup` | P1 |
| 5 | 无手动 标记已看/未看、收藏、评分 API | §5.3 后四组端点 | P1 |
| 6 | items 列表/详情不带用户数据 | list/get_item LEFT JOIN watch_history，series 级聚合 unplayed_count/played_pct（§1.1 折叠规则） | P1 |
| 7 | series 级"整剧标记已看" | 递归对全部 episode 执行 MarkPlayed（Jellyfin Folder.MarkPlayed 语义） | P2 |
| 8 | 特殊集(S0)插队 NextUp、音轨/字幕记忆、多版本 played 传播 | 备忘 backlog；file_id 列已为多版本留好位 | P2 |
