# Jellyfin 实体模型精读 → nipaserver 对标报告

来源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Controller/Entities/`（BaseItem.cs、Video.cs、TV/{Series,Season,Episode}.cs、Movies/{Movie,BoxSet}.cs、LinkedChild*.cs、PersonInfo.cs、Genre.cs、Studio.cs、ItemImageInfo.cs）与 `src/Jellyfin.Database/Jellyfin.Database.Implementations/Entities/`（BaseItemEntity.cs、People.cs、PeopleBaseItemMap.cs、ItemValue*.cs、UserData.cs、Chapter.cs、MediaStreamInfo.cs、LinkedChildEntity.cs、AncestorId.cs、BaseItemProvider.cs），以及 `MediaBrowser.Providers/TV/SeriesMetadataService.cs`（虚拟季逻辑）。**Jellyfin 为 GPL，以下只提炼字段语义与建模逻辑，不含任何代码翻译。**

对照对象：`/Users/sakiko/Desktop/nipaserver/crates/nipa-server/migrations/0001_init.sql`（+0003 的 `air_date`）与 `crates/nipa-server/src/api_library.rs`。

---

## 1. BaseItem 核心字段全表（内存模型 ∩ DB 列 BaseItemEntity）

nipaserver items 现有列：`id, library_id, kind, parent_id, title, original_title, year, season_no, episode_no, overview, rating, poster_path, backdrop_path, added_at, deleted_at, air_date`。

### 1a. 标识与结构（★=基础必备）

| Jellyfin 字段 | 语义 | nipaserver 现状 | 评级 |
|---|---|---|---|
| Id / ParentId | 主键 / 树父节点 | 有（id/parent_id） | ★已有 |
| Name / OriginalTitle | 标题 / 原名 | 有（title/original_title） | ★已有 |
| **SortName / ForcedSortName** | 排序名：自动生成（去冠词、集号补零成 "001 - 0004 - 名称"），ForcedSortName 为手工覆盖，DB 里两列都存 | **无**。当前 `ORDER BY title` 对日文/中文假名排序不可用 | ★必备 |
| Path / IsFolder / MediaType | 路径与类型判别 | media_files 承担 | 已有等价 |
| ProviderIds（DB: BaseItemProvider 表，ItemId+ProviderId+ProviderValue） | 外部 ID 字典 | item_ids 表等价，且多了 UNIQUE(provider,external_id) 去重约束（更好） | ★已有 |
| PresentationUniqueKey / SeriesPresentationUniqueKey | 多库同剧合并显示的分组键（基于 provider id + 语言 + 库） | 无；item_ids 的唯一约束已在写入侧合并，单库场景可不做 | 锦上添花 |
| AncestorId 表（ItemId, ParentItemId） | 祖先闭包表，用于"某 series 下所有 episode"一跳查询 | 无；nipa 树仅 3 层，`parent_id` 两跳 JOIN 或递归 CTE 足够 | 锦上添花 |
| DateCreated / DateModified / DateLastRefreshed / DateLastSaved | 入库/修改/刮削时间 | 仅 added_at | DateModified 推荐，其余远期 |
| IsVirtualItem | 虚拟条目（缺失集/占位季），LocationType==Virtual | **无** | 推荐（见 §2） |
| ExtraType / OwnerId | 花絮（Trailer/BehindTheScenes…）挂在 Owner 条目下 | 无 | 远期 |
| IsLocked / LockedFields | 锁定元数据不被刮削覆盖（逐字段锁） | 无；对 agentic 刮削很有价值：用户手工改过的字段 steward 不应覆写 | 推荐 |
| TopParentId / IsInMixedFolder / ChannelId / ExternalId… | 多库根/混合目录/LiveTV | 不适用 | 忽略 |

### 1b. 展示元数据

| Jellyfin 字段 | 语义 | nipaserver 现状 | 评级 |
|---|---|---|---|
| **Overview** | 简介 | 有 | ★已有 |
| **PremiereDate / EndDate** | 首播/完结日期（DateTime） | air_date（TEXT，仅首播） | ★end_date 必备（判断"已完结"） |
| ProductionYear | 年份（与 PremiereDate 独立存，电影常只有年份） | 有（year） | ★已有 |
| **RunTimeTicks** | 时长（100ns ticks；DB 同名列） | **无**（watch_history.duration_ms 是播放态不是元数据） | ★必备（列表页显示时长、进度条百分比） |
| **CommunityRating** | 社区评分 float（TMDB/Bangumi 分） | 有（rating） | ★已有 |
| CriticRating | 媒体评分（烂番茄类） | 无 | 锦上添花 |
| **OfficialRating** | 分级字符串（"TV-14"/"R"） | 无 | 家庭场景★必备，个人服务器→推荐 |
| CustomRating / InheritedParentalRatingValue(+Sub) | 自定义分级 / 数值化分级继承（家长控制过滤用） | 无 | 远期 |
| **Genres**（string[]，DB 冗余 CSV + ItemValue 规范化表） | 题材 | **无** | ★必备 |
| **Studios**（同上双写） | 制作公司 | **无** | ★必备（番剧场景=动画公司，高频筛选维度） |
| **Tags** | 自由标签 | 无 | 推荐 |
| Tagline | 宣传语（单条；DB 一列） | 无 | 锦上添花 |
| **People**（独立表，见 §3） | 演职员 | **无** | ★必备 |
| ProductionLocations / HomePageUrl / OriginalLanguage / PreferredMetadataLanguage、CountryCode | 产地/官网/原语言/刮削偏好 | 无 | 远期（OriginalLanguage 对"优先日文标题"策略有点用→锦上添花） |
| RemoteTrailers（MediaUrl 列表） | 外部预告片 URL | 无 | 远期 |
| ImageInfos（DB: BaseItemImageInfo：Path/Type/Width/Height/**BlurHash**；ImageType: Primary=0, Art, Backdrop, Banner, Logo, Thumb, Disc, Box, Screenshot…） | 每条目多图多类型 | poster_path/backdrop_path 两列 | 两列对海报墙够用；多图表→推荐（blurhash 占位是体验大项） |

### 1c. Video 层（Episode/Movie 继承）

Video3DFormat、IsoType、VideoType、PrimaryVersionId/LinkedAlternateVersions（多版本）、AdditionalParts（分段视频）、DefaultVideoStreamIndex、HasSubtitles、Height/Width、Container、Size、TotalBitrate。
→ nipaserver 把 ffprobe JSON 存 media_files 一列，**这个设计可以保留**（Jellyfin 规范化成 MediaStreamInfo 表是为了 SQL 侧按编解码筛选，个人服务器不需要）。多版本 = nipa 的 file_item 一对多，已覆盖。`HasSubtitles/Height/Width/Container` 建议在扫描时从 ffprobe 提升为 media_files 生成列或普通列，避免播放页解析 JSON。

### 1d. UserData（DB 独立表，per user × item）

`Played`（看完标记）、`PlayCount`、`PlaybackPositionTicks`、`IsFavorite`、`Rating`（用户打分）、`Likes`、`LastPlayedDate`、`AudioStreamIndex/SubtitleStreamIndex`（记住音轨字幕选择）。
→ nipa 的 watch_history 只有 position/duration。**缺 `played`、`play_count`、`is_favorite`、`last_played_at`，这四个是"继续观看/下一集/收藏"三大基础功能的地基，必备。** audio/subtitle stream index 推荐（播放器记忆）。

---

## 2. Series / Season / Episode 特有字段与关系

### Series
- `Status`（SeriesStatus: Continuing/Ended/Unreleased）——★必备，追番场景核心。
- `AirDays: DayOfWeek[]` + `AirTime: string`——放送星期几+时刻。对番剧日历功能是必备原料（nipa 有订阅功能，推荐）。
- `DisplayOrder`（"airdate"/"dvd"/"absolute"）——集排序方案，传给 provider 决定用哪套集号。远期。
- 关键约定：**Series 不直接存季数/集数统计，全部动态查询**。

### Season
- 只有 `IndexNumber`（=季号，nipa 的 season_no ✅）+ 三个冗余：SeriesId、SeriesName、SeriesPresentationUniqueKey（避免 JOIN）。
- 排序名 = 季号补零四位；**季 0 即 Specials**。
- **虚拟季（SeriesMetadataService 的逻辑，值得照抄思路）**：
  1. 扫描后收集所有 episode 的 ParentIndexNumber（先尝试从路径补全缺失的季号）；
  2. 对每个没有物理季文件夹的季号，**自动创建 Season 条目**（无 Path，LocationType=Virtual）；季号为 null 的集创建 "Season Unknown" 占位季——**"季永远存在"是硬约定**，客户端 UI 依赖它；
  3. 虚拟季 ID 按 `seriesId+季号+季名` 做确定性生成（重扫不产生重复）；
  4. 若虚拟季后来出现真实集，翻转 IsVirtualItem=false；
  5. 最后把每个 episode 的 SeasonId 对齐到对应季。
  → nipaserver 含义：扫平铺目录（番剧极常见 `Title/01.mkv` 无季文件夹）时，也要为 episode 生成 season 中间节点，并给 items 加 `is_virtual` 区分"目录真实存在"与"结构占位"。
- **缺失集**：provider 返回但本地无文件的集也建条目（IsVirtualItem=true），用于"缺第 7 集"展示。推荐（对下载/订阅联动特别有价值：缺集 → 触发 RSS 搜索）。

### Episode
- `IndexNumber`（集号）/ `ParentIndexNumber`（季号）——nipa 的 episode_no/season_no ✅。
- **`IndexNumberEnd`**：双集合并文件（S01E01E02）的结束集号。nipa 用 file_item.episode_range 表达"一文件多集"，语义更好（每集仍是独立条目），可不加此列——但要确认 UI/播放侧真按 range 切进度。
- **特殊集（SP）三字段**：`AirsBeforeSeasonNumber` / `AirsAfterSeasonNumber` / `AirsBeforeEpisodeNumber`。SP 物理上在季 0，但显示时按 `AiredSeasonNumber = AirsAfter ?? AirsBefore ?? ParentIndexNumber` 插入正片季的对应位置（配置项 DisplaySpecialsWithinSeasons 控制；季 0 本身按名称排序，正常季按 AiredEpisodeOrder 排序）。番剧 SP/OVA 极多，**推荐**尽早加这三列。
- 冗余列：SeasonId、SeriesId（DB 实列）+ SeriesName/SeasonName（冗余文本）。nipa 三层树用 parent_id 逐级即可，但**在 episode 行上冗余 `series_id` 一列**能让"继续观看→聚合到剧"与"按剧查全部集"免掉两跳 JOIN，推荐。
- 集直接躺在剧目录（无季文件夹）时：靠 ParentIndexNumber 匹配季号找 season 归属。
- 排序名 = `季号(3位) - 集号(4位) - 标题`。

---

## 3. People 与 Genre/Studio 关联建模

### People（三表设计）
- **People 表**：`Id, Name, PersonType`——全局去重的人物主档（同名即同人，Jellyfin 的取舍）。
- **PeopleBaseItemMap 关联表**：`ItemId, PeopleId, Role（角色名，如配音的角色）, SortOrder, ListOrder`。
- **PersonKind 枚举**（挂在关联上或 PersonType）：Unknown/Actor/Director/Composer/Writer/GuestStar/Producer/Creator/Author/Editor/Translator… 番剧需要的最小集：Actor（声优，Role=角色名）、Director（监督）、Writer（系构）、Composer（音乐）、Studio 走 Studios 不走 People。
- PersonInfo（刮削产物）还带 ImageUrl 与 ProviderIds → 人物头像与 Bangumi person id 应存在人物主档上。
- Person 同时也是可浏览实体（点声优→出演作品列表 = 反查关联表）。

### Genre / Studio / Tag（ItemValue 统一表）
- **ItemValue 表**：`ItemValueId, Type, Value, CleanValue`；**ItemValueMap**：`ItemId, ItemValueId`。
- Type 枚举：Artist=0, AlbumArtist=1, **Genre=2, Studios=3, Tags=4, InheritedTags=6**。
- `CleanValue` = 规范化小写形式，用于不区分大小写的去重与筛选；`Value` 保留显示原文。
- 同时 BaseItemEntity 上还冗余 CSV 文本列（Genres/Studios/Tags）加速 DTO 组装——SQLite 场景可以不学这一层冗余，直接 JOIN。
- Genre/Studio 也是可浏览实体（IItemByName.GetTaggedItems）：类型页→该类型全部作品。

**给 nipa 的建议：不必分三张表，一张 `item_values(type, value)` 统一 genre/studio/tag 是 Jellyfin 验证过的最省事结构。**

---

## 4. BoxSet（合集）与 Playlist（播放列表）

两者都是 Folder 子类，**成员都用 LinkedChild 机制**，不是 parent_id：
- **LinkedChildEntity 表**：`ParentId（合集/列表 id）, ChildId（成员 item id）, ChildType（Manual=0 手工加入 / Shortcut=1 .lnk 文件）, SortOrder`。
- 与 parent_id 树的区别：一个 item 可属多个合集、不影响其在库中的原位置、删除合集不删成员——**多对多 + 排序**。
- **BoxSet**：DisplayOrder 默认 "PremiereDate"（合集内按首映日期排）；LibraryFolderIds 限定可见库；电影经 TmdbCollectionName 自动归入合集（Movie.cs 上的字段）；Series 实现 ISupportsBoxSetGrouping（剧也能进合集）。按用户过滤成员可见性（FilterLinkedChildrenPerUser）。
- **Playlist**：额外 `OwnerUserId`（属主）、`OpenAccess`（公开）、`Shares`（按用户的读/写授权列表）、`PlaylistMediaType`；成员靠 SortOrder 手工排序，支持任意媒体类型混排。

nipa 落法：合集直接给 items.kind 加 'boxset'（复用海报/简介/评分列）+ 一张 `collection_items` 关联表；播放列表因带 owner/权限语义，单独 `playlists` 表更干净。

---

## 5. nipaserver 落地清单

### 5.1 items 表增列（ALTER）

**必备（P0）**
```sql
ALTER TABLE items ADD COLUMN sort_title TEXT;        -- 生成规则：SP/集补零、去前导冠词；写入时由刮削器算好
ALTER TABLE items ADD COLUMN end_date TEXT;          -- 完结日期，与 air_date 同为 ISO8601 文本
ALTER TABLE items ADD COLUMN runtime_ms INTEGER;     -- 元数据时长（ffprobe 或 provider）
ALTER TABLE items ADD COLUMN status TEXT;            -- series 专用: 'continuing'|'ended'|'unreleased'
ALTER TABLE items ADD COLUMN official_rating TEXT;   -- 分级字符串
CREATE INDEX idx_items_sort ON items(library_id, kind, sort_title);
```

**推荐（P1）**
```sql
ALTER TABLE items ADD COLUMN tagline TEXT;
ALTER TABLE items ADD COLUMN is_virtual INTEGER NOT NULL DEFAULT 0;  -- 虚拟季/缺失集
ALTER TABLE items ADD COLUMN series_id INTEGER REFERENCES items(id); -- episode 上冗余，免两跳 JOIN
ALTER TABLE items ADD COLUMN updated_at INTEGER;                     -- DateModified 等价
ALTER TABLE items ADD COLUMN locked_fields JSON;                     -- 用户手改字段名数组，steward 刮削跳过
ALTER TABLE items ADD COLUMN airs_before_season INTEGER;             -- SP 插排三件套
ALTER TABLE items ADD COLUMN airs_after_season INTEGER;
ALTER TABLE items ADD COLUMN airs_before_episode INTEGER;
```

**远期（P2）**：critic_rating REAL、display_order TEXT（series）、original_language TEXT、air_days/air_time（或并入 series 的 JSON 元数据列）、extra_type + owner_id（花絮）。

### 5.2 新表

**必备（P0）**
```sql
CREATE TABLE people (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  image_path TEXT,            -- 头像（Bangumi person 图 URL，同海报直链策略）
  UNIQUE(name)                -- Jellyfin 同款取舍；如需精确区分同名，改为按 provider id 合并
);
CREATE TABLE person_ids (     -- 与 item_ids 对称
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  provider TEXT NOT NULL, external_id TEXT NOT NULL,
  UNIQUE(provider, external_id)
);
CREATE TABLE item_people (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,         -- 'actor'|'director'|'writer'|'composer'|'guest'|…（PersonKind 子集）
  role TEXT,                  -- 声优的角色名
  sort_order INTEGER,
  PRIMARY KEY (item_id, person_id, kind)
);
CREATE INDEX idx_item_people_person ON item_people(person_id);

CREATE TABLE item_values (    -- genre/studio/tag 三合一（ItemValue 思路）
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  type TEXT NOT NULL,         -- 'genre'|'studio'|'tag'
  value TEXT NOT NULL,        -- 显示原文
  clean_value TEXT NOT NULL,  -- 规范化小写，筛选用
  PRIMARY KEY (item_id, type, clean_value)
);
CREATE INDEX idx_item_values_lookup ON item_values(type, clean_value);
```

**必备（P0）——watch_history 补齐用户数据**
```sql
ALTER TABLE watch_history ADD COLUMN played INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE watch_history ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0;
-- last_played_at 可复用 updated_at；如需区分"改收藏"与"播放"再拆列
```
（Jellyfin 的 Played 判定惯例：播放进度超过约 90% 即标记看完并清零 position——写在播放上报 handler 里。）

**推荐（P1）**
```sql
-- 多图多类型 + blurhash 占位
CREATE TABLE item_images (
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  type TEXT NOT NULL,         -- 'primary'|'backdrop'|'logo'|'thumb'|'banner'
  path TEXT NOT NULL,         -- URL 或本地缓存路径
  width INTEGER, height INTEGER, blurhash TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (item_id, type, sort_order)
);
-- 迁移期 poster_path/backdrop_path 保留为视图/兼容读

-- 合集：items 加 kind 'boxset'（需重建 CHECK 约束）+ 关联表
CREATE TABLE collection_items (
  collection_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  sort_order INTEGER,
  PRIMARY KEY (collection_id, item_id)
);

CREATE TABLE playlists (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL, is_public INTEGER NOT NULL DEFAULT 0, created_at INTEGER
);
CREATE TABLE playlist_items (
  playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  sort_order INTEGER NOT NULL,
  PRIMARY KEY (playlist_id, sort_order)
);
```

**远期（P2）**：`chapters(item_id, idx, start_ms, name, image_path)`；media_streams 规范化表（当前 ffprobe JSON 足够，只建议把 `duration_ms/width/height/video_codec/audio_codec/has_subtitles` 提升为 media_files 实列）；trickplay；播放列表共享授权表。

### 5.3 API 侧对应改动（api_library.rs）

1. **list_items**：sort 增加 `sort_title`（默认值从 added_at 改为可选）、`rating`；filter 增加 `genre=`, `studio=`, `tag=`, `person=`, `status=`, `favorite=`（JOIN item_values / item_people / watch_history）。
2. **get_item**：detail 返回 `people`（按 kind 分组）、`genres/studios/tags`、`runtime_ms`、`status`、`user_data`（position/played/favorite）；children 排序建议 SP 场景下按 `airs_*` 计算的有效季集号排（先简单按 season_no,episode_no，加列后再插排）。
3. 新端点（Jellyfin 对应能力）：`GET /api/v1/persons/{id}`（人物页+出演列表）、`GET /api/v1/genres` `GET /api/v1/studios`（聚合+计数）、`GET /api/v1/resume`（继续观看：watch_history 未 played 且 position>0，episode 聚合到 series）、`GET /api/v1/nextup`（每剧最后看完集的下一集）、collections/playlists 的 CRUD、`POST /api/v1/items/{id}/favorite`、`POST /api/v1/items/{id}/played`。
4. **扫描器**：平铺目录必须生成 season 中间节点（虚拟季，is_virtual=1，确定性 ID 防重扫重复）；"季永远存在"作为不变量，客户端才好写。
5. **steward 刮削**：写元数据前检查 `locked_fields`，用户手改过的字段跳过——这是 Jellyfin IsLocked/LockedFields 机制里最值得搬的部分。

### 5.4 nipa 已优于 Jellyfin 之处（不必改）
- `file_item` 多对多 + `episode_range` 同时覆盖 Jellyfin 的多版本（PrimaryVersionId/AlternateVersions）与双集文件（IndexNumberEnd），且更规整。
- `item_ids` 带全局唯一约束，合并去重在写入侧硬保证；Jellyfin 的 BaseItemProvider 无此约束。
- ffprobe 存 JSON 免去 MediaStreamInfo 100+ 列的维护成本，SQLite 个人场景合理。
