# Jellyfin API 面精读报告：对标 nipaserver 的差距清单与补全方案

来源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Controllers/`（58 个 controller 全扫），重点精读 ItemsController / TvShowsController / UserLibraryController / SearchController / FilterController，以及查询语义的真正实现（`Jellyfin.Server.Implementations/Item/NextUpService.cs`、`BaseItemRepository.TranslateQuery.cs`、`Emby.Server.Implementations/Library/UserViewManager.cs`、`Library/Search/SqlSearchProvider.cs`、`Library/UserDataManager.cs`）。Jellyfin 为 GPL，本报告只提炼协议与查询语义，不含代码翻译。

对照物：`/Users/sakiko/Desktop/nipaserver/crates/nipa-server/migrations/0001_init.sql`（+0002/0003 增量）、`crates/nipa-server/src/api_library.rs`、`crates/nipa-server/src/api.rs`、`docs/01-开发文档.md` §8.1/§8.2。

---

## 1. Controller 全清单（58 个）

### 1.1 基础功能（nipaserver 必须有对应物）

| Controller | 一句话 | nipaserver 现状 |
|---|---|---|
| **ItemsController** | `/Items` 万能查询（过滤/排序/分页/搜索）+ `/UserItems/Resume`（继续观看）+ `/UserItems/{id}/UserData`（读写单条目用户数据） | `/api/v1/items` 仅 6 个参数，无 Resume、无 UserData |
| **UserLibraryController** | `/Items/{id}` 详情、`/Items/Latest`（最新添加）、收藏（`UserFavoriteItems`）、喜欢/评分、LocalTrailers/SpecialFeatures | 详情有；Latest/收藏/评分全缺 |
| **TvShowsController** | `/Shows/NextUp`（下一集）、`/Shows/Upcoming`、`/Shows/{id}/Seasons`、`/Shows/{id}/Episodes` | 全缺（详情页 children 部分覆盖 Seasons/Episodes） |
| **SearchController** | `/Search/Hints` 搜索提示 | 缺 |
| **FilterController** | `/Items/Filters`（legacy：Genres/Tags/OfficialRatings/Years）、`/Items/Filters2`（Genres+音轨/字幕语言） | 缺 |
| **UserViewsController** | `/UserViews`：用户可见的库列表（客户端首页入口） | `/api/v1/libraries` 语义近似，但兼容层必须做 Views 映射 |
| **PlaystateController** | `/Sessions/Playing`(+Progress/Ping/Stopped)、`/UserPlayedItems/{id}`（标记已看） | 有 watch_history 表但**无任何 API** |
| **UserController** | 登录（`/Users/AuthenticateByName`）、用户管理 | §8.4 已规划，未实现 |
| **ImageController** | `/Items/{id}/Images/{type}`（含缩放参数 fillWidth/quality/tag） | 目前海报是 Bangumi 外链，无本地伺服 |
| **VideosController** | `/Videos/{id}/stream` Direct Play | §8.2 已定范围（M2b/M3） |
| **MediaInfoController** | `/Items/{id}/PlaybackInfo`（DeviceProfile→播放决策） | §8.2 已定（v1 恒 Direct Play） |
| **SystemController** | `/System/Info`、`/System/Info/Public` | `/api/v1/system/info` 有；兼容层 Public 端点缺 |
| **SessionController** | 会话列表/`/Sessions/Capabilities`（客户端上报能力） | 兼容层可返回空数组即可 |
| **LibraryController** | 库管理、`/Items/{id}/Similar`、`/Library/Refresh`、媒体文件删除 | scan 有；Similar 可 P2 |
| **SubtitleController** | 外挂字幕列举/伺服 | NipaPlay 支持外挂字幕，P1（跟播放一起） |
| **BrandingController** | `/Branding/Configuration`（客户端启动会拉，返回空即可） | 缺（兼容层需要 stub） |
| **DisplayPreferencesController** | 客户端 UI 偏好存取（很多客户端启动即调，可返回默认值 stub） | 缺（兼容层 stub） |
| **QuickConnectController** | 扫码/配对登录 | 明确不做，但兼容层需返回 `Enabled: false` 而非 404 |

### 1.2 增强功能（可后置，P2）

MoviesController（电影推荐 Suggestions）、SuggestionsController、GenresController / StudiosController / PersonsController / YearsController（按流派/工作室/人物/年份聚合浏览——nipaserver schema 尚无这些维度）、CollectionController（手动合集）、PlaylistsController、TrickplayController（进度条预览图，Infuse/官方客户端体验项）、ItemUpdateController / ItemLookupController / ItemRefreshController（元数据手动编辑/识别——nipaserver 用自己的审批流替代）、RemoteImageController、MediaSegmentsController（片头片尾跳过，弹幕番剧场景价值高但依赖章节检测）、InstantMixController、LyricsController、DevicesController、ActivityLogController、ScheduledTasksController、ConfigurationController / DashboardController / StartupController（Jellyfin 自己的管理台）。

### 1.3 明确跳过（§8.2 已定：空实现/501）

LiveTvController、SyncPlayController、DlnaController 系（本仓已无独立 DLNA controller，但 UDP 发现同理跳过）、ChannelsController（插件频道）、PluginsController / PackageController、TimeSyncController（SyncPlay 附属）、UniversalAudioController / AudioController / ArtistsController / MusicGenresController（音乐栈）、HlsSegmentController / DynamicHlsController（M3 转码期再回头）、VideoAttachmentsController、EnvironmentController、BackupController、ClientLogController、ApiKeyController、LocalizationController、TrailersController（远程预告片）。

---

## 2. `/Items` 查询参数完整清单

`GET /Items`（ItemsController.GetItems，约 90 个参数）。按组归类：

**分页/结构**：`startIndex`、`limit`、`enableTotalRecordCount`（默认 true）、`parentId`（限定范围到某库/某文件夹）、`recursive`（在库上带 includeItemTypes 时默认自动 recursive=true——这是客户端"进库看海报墙"的标准请求形态）、`ids`、`excludeItemIds`、`adjacentTo`（返回兄弟节点）、`collapseBoxSetItems`。

**类型**：`includeItemTypes` / `excludeItemTypes`（BaseItemKind，逗号分隔：Movie,Series,Season,Episode,BoxSet…）、`mediaTypes`（Video/Audio…）、`isMovie` / `isSeries`（LiveTV 语义，普通库可忽略）、`isFolder`（经 filters）。

**排序**：`sortBy`（逗号分隔多键）+ `sortOrder`（Ascending/Descending，与 sortBy 一一对应）。ItemSortBy 全集：Default, AiredEpisodeOrder, Album, AlbumArtist, Artist, DateCreated, OfficialRating, DatePlayed, PremiereDate, StartDate, SortName, Name, Random, Runtime, CommunityRating, ProductionYear, PlayCount, CriticRating, IsFolder, IsUnplayed, IsPlayed, SeriesSortName, VideoBitRate, AirTime, Studio, IsFavoriteOrLiked, DateLastContentAdded, SeriesDatePlayed, ParentIndexNumber, IndexNumber。

**用户态过滤**：`filters`（枚举：IsFolder, IsNotFolder, IsUnplayed, IsPlayed, IsFavorite, IsResumable, Likes, Dislikes）、`isFavorite`、`isPlayed`。

**元数据过滤**：`genres`/`genreIds`、`tags`、`officialRatings`、`years`、`person`/`personIds`/`personTypes`、`studios`/`studioIds`、`minCommunityRating`、`minCriticRating`、`minPremiereDate`/`maxPremiereDate`、`minDateLastSaved`、`hasOverview`、`hasImdbId`/`hasTmdbId`/`hasTvdbId`、`nameStartsWith`/`nameStartsWithOrGreater`/`nameLessThan`（字母索引条）、`seriesStatus`（Continuing/Ended）、`indexNumber`/`parentIndexNumber`。

**流/文件属性过滤**：`hasSubtitles`、`audioLanguages`、`subtitleLanguages`、`isHd`/`is4K`/`is3D`、`videoTypes`、`minWidth/maxWidth/minHeight/maxHeight`、`isMissing`/`isUnaired`（虚拟缺集）、`locationTypes`/`excludeLocationTypes`（Virtual 过滤）。

**输出控制**：`fields`（ItemFields：Overview, Genres, DateCreated, MediaStreams, MediaSources, People, ProviderIds, Path, ParentId, PrimaryImageAspectRatio, SortName, Studios, Taglines, Chapters…）、`enableImages`/`imageTypeLimit`/`enableImageTypes`、`enableUserData`（是否附带 UserData 块）。

**搜索**：`searchTerm`——注意语义：先走 SearchProvider 拿 (id,score)，再用 ids 走常规查询、按 score 排序、内存分页（此时忽略 sortBy）。

**旧路由**：`GET /Users/{userId}/Items` 与新路由完全同参转发（NipaPlay 等旧客户端用这个路径，兼容层两个都要挂）。

### 常见客户端（NipaPlay/Infuse/Findroid）实际用到的子集

实现下面这些就覆盖 90% 请求：

```
parentId, recursive, includeItemTypes, excludeItemTypes,
sortBy=SortName|DateCreated|PremiereDate|ProductionYear|CommunityRating|Random|IndexNumber,
sortOrder, startIndex, limit, enableTotalRecordCount,
filters=IsUnplayed|IsPlayed|IsFavorite|IsResumable,
isFavorite, isPlayed, searchTerm, genres, years,
fields=Overview|Genres|DateCreated|MediaSources|ProviderIds|Path|PrimaryImageAspectRatio,
enableUserData, enableImages, imageTypeLimit, ids
```

未实现的参数**安全忽略即可**（Jellyfin 对未知参数也不报错），但 `startIndex/limit/TotalRecordCount` 必须真实——客户端靠它做无限滚动。响应形状固定为 `{ "Items": [...], "TotalRecordCount": n, "StartIndex": n }`。

---

## 3. 三大首页功能的查询语义

### 3.1 继续观看 Resume（`GET /UserItems/Resume`，旧 `/Users/{id}/Items/Resume`）

查询语义（ItemsController.GetResumeItems + TranslateQuery IsResumable 分支）：

- **候选**：该用户 user_data 中 `PlaybackPositionTicks > 0` 的条目（每个版本文件各自记进度）；
- **过滤**：`IsVirtualItem = false`、`CollapseBoxSetItems = false`、recursive 全库、可按 `parentId`/`mediaTypes`/`includeItemTypes` 收窄；`excludeActiveSessions=true` 时剔除正在播放的条目（含其所有版本）；
- **Series 也可 resumable**（当 includeItemTypes 含 Series）：条件是"有 in-progress 集"**或**"既有已看集又有未看集"（部分看过）；
- **排序**：`DatePlayed DESC`（最近播放在前）；
- **分页**：标准 startIndex/limit + TotalRecordCount。

**进度写入侧的三条阈值规则**（UserDataManager.UpdatePlayState，nipaserver 写 watch_history 时应照抄语义，默认值来自 ServerConfiguration）：
1. 进度 < **MinResumePct=5%** → 位置清 0（不进 Resume 列表）；
2. 进度 > **MaxResumePct=90%** 或距结尾 <1s → 位置清 0 且标记 `Played=true`（看完）；
3. 时长 < **MinResumeDurationSeconds=300s** 的短片 → 直接标记看完不留进度；
4. 不知道时长 → 视为看完。

对 nipaserver：`watch_history(position_ms, duration_ms)` 已够支撑查询（`position_ms > 0` 即 resumable），但缺 `played`、`play_count`、`last_played_at`、`is_favorite` 字段——见 §6。

### 3.2 下一集 NextUp（`GET /Shows/NextUp`）

参数：`userId, seriesId?, parentId?, startIndex, limit, nextUpDateCutoff?, enableResumable=true, enableRewatching=false, fields, enableUserData…`

两阶段算法（TVSeriesManager + NextUpService，可直接翻成两条 SQL）：

**阶段一：找"最近在看的剧"**（GetNextUpSeriesKeys）：
```
对该用户所有 episode 的 user_data 按 series 分组，
取每组 MAX(last_played_date)，过滤 >= nextUpDateCutoff，
按该日期 DESC 排序 → 得到 series 键列表（即"最近看过的剧优先"）。
```
若指定 `seriesId` 则只查这一部。

**阶段二：对每部剧求下一集**（GetNextUpEpisodesBatch）：
```
lastWatched = 该剧已看(Played=true)集中 (season_no, episode_no) 最大者
              （排除 season 0 特别篇）；
next = 未看、非虚拟、(season_no, episode_no) > lastWatched 位置的
       最小 (season_no, episode_no) 集；
若这部剧一集都没看过 → next = 第一集（无位置约束的最小集）。
```
细节：
- `enableResumable=false` 时，若 next 已有播放进度（在 Resume 里了）则跳过该剧，避免 Resume 与 NextUp 重复展示；默认 true（新版 Jellyfin 把有进度的下一集也放进 NextUp）；
- `enableRewatching=true` 时额外计算"已看集里的下一集"（重刷场景），可不实现；
- 特别篇若带 `AirsBeforeSeasonNumber/AirsAfterSeasonNumber` 会插入排序参与"下一集"判定——nipaserver 无此字段，v1 直接排除 season 0 即可；
- **最终排序**：按各剧 lastWatched 的 last_played_date DESC（不是按剧名）；
- 多版本文件：优先延续用户上次播放的版本名——nipaserver 的 file_item 多版本可后置。

### 3.3 最新添加 Latest（`GET /Items/Latest`，旧 `/Users/{id}/Items/Latest`）

参数：`parentId?, includeItemTypes, isPlayed?, limit=20, groupItems=true, fields…`。**注意：返回裸数组 `[BaseItemDto]`，不是 QueryResult 包裹**（唯一的例外，客户端按此解析）。

语义（UserViewManager.GetLatestItems）：
- 基础查询：非虚拟条目，`ORDER BY DateCreated DESC, SortName DESC, ProductionYear DESC`，`IsFolder=false`（当未指定类型时排除 folder 与 Person/Genre 等 by-name 实体），取 `limit*2` 条再折叠；
- `isPlayed` 未传时看用户偏好 `HidePlayedInLatest`（默认 true → 隐藏已看）；
- **groupItems=true 的折叠**：episode 折到其 series（"LatestItemsIndexContainer"），同剧多集只出一张卡（卡是 series，`ChildCount` = 折叠的集数）；电影不折叠。TV 库场景等价 SQL：
```
按 series 分组取 MAX(episode.added_at) 排序 series，
每组返回 series 卡 + child_count。
```

对 nipaserver：items.added_at 即 DateCreated。TV 折叠 = `GROUP BY 顶层 series 祖先`。

---

## 4. 搜索（`GET /Search/Hints`）行为

新架构分两层：

**Provider 层**（SearchManager）：external provider（插件，可无视）失败/为空时回落 internal `SqlSearchProvider`。provider 只返回 `(itemId, score)` 列表；条目本体再经常规查询加载（顺带套用户可见性过滤），最后按 score 降序排列、内存分页。

**SqlSearchProvider 打分规则**（值得照抄）：
- 搜索词与 `CleanName`（预归一化：小写+去变音符号）比较：**完全相等=100，前缀=80，词首前缀（`contains "词 "`）=75，包含=50**；
- `OriginalTitle` 走 case-insensitive LIKE `%term%` 兜底（命中算 50 档）——**对番剧场景关键：original_title 存日文原名，中文/日文都能搜到**；
- 排除虚拟条目；tie-break 按 id 保证确定性；默认 limit 100。

**响应形状**：`{ "SearchHints": [ {Id, Name, Type, MediaType, ProductionYear, IndexNumber, ParentIndexNumber, Series(剧名), RunTimeTicks, PrimaryImageTag, ...} ], "TotalRecordCount": n }`。参数含 `includeItemTypes/excludeItemTypes/mediaTypes/parentId/limit/startIndex` 及 `includePeople/Genres/Studios/Artists/Media` 五个开关（nipaserver 无人物/流派实体，前四个开关忽略、恒返回媒体即可）。

另外注意：**`/Items?searchTerm=` 也走同一 provider 管线**——很多客户端（含 NipaPlay 的搜索页）直接用 `/Items?searchTerm=xxx&includeItemTypes=Series,Movie` 而不用 `/Search/Hints`。所以 nipaserver 只要在 items 查询里把 searchTerm 做好，两个端点可以共用一套打分逻辑。

对 nipaserver 实现建议：SQLite `LIKE`（title/original_title，NOCASE 对 ASCII 即可，中日文 LIKE 本身无大小写问题）+ 上述四档打分的 CASE WHEN，即可等价复刻；库大了再上 FTS5。CleanName 等价物 = 入库时存一列 `clean_title`（小写化+全半角归一）。

---

## 5. FilterController（筛选条数据源）

- `GET /Items/Filters`（legacy，**NipaPlay 用的是这个**）：给定 parentId+includeItemTypes，返回 `{ Genres: [string], Tags: [string], OfficialRatings: [string], Years: [int] }`——即当前范围内实际存在的可筛选值（DISTINCT 聚合）；
- `GET /Items/Filters2`：返回 `{ Genres: [{Name, Id}], AudioLanguages, SubtitleLanguages }`（语言来自 MediaStream 聚合，series/season 查询会 join 到 episode 的流）。

对 nipaserver：v1 返回 `Years`（`SELECT DISTINCT year`）+ 空 Genres/Tags 即合法；补 genres 表后填充。

---

## 6. 差距清单与补全方案

### 6.1 Schema 差距（先于 API）

| 缺口 | 建议 |
|---|---|
| **无用户级条目状态**（played/favorite/play_count/last_played_at）；watch_history 只有进度 | 方案 A（推荐）：扩 watch_history 为通用 user_item_data：`ALTER TABLE watch_history ADD COLUMN played INTEGER DEFAULT 0; ADD COLUMN play_count INTEGER DEFAULT 0; ADD COLUMN is_favorite INTEGER DEFAULT 0; ADD COLUMN last_played_at INTEGER;`（updated_at 已可当 DatePlayed 用，但独立 last_played_at 语义更准）。收藏可以打在任意 kind 的 item 上 |
| 无 genres/tags | `item_genres(item_id, genre TEXT)` 简表即可（agentic 刮削本就产出 Bangumi tags）；People/Studio 对番剧优先级低，P2 |
| 无 clean_title/sort_title | items 加 `clean_title TEXT` 列 + 索引，入库时归一化，搜索与 nameStartsWith 都靠它 |
| 无 runtime 冗余 | episode/movie 从 ffprobe JSON 提 `runtime_ms` 冗余列，Resume 百分比、Runtime 排序、DTO RunTimeTicks 都要 |
| overview/rating/backdrop_path 建了列但 API 不吐 | ItemRow 补齐这三个字段（详情页刚需） |
| watch_history 主键 (user_id, item_id) | 够用；多版本进度按 file_id 区分的需求先不做（Jellyfin 也是 per-version user data，一致） |

### 6.2 `/api/v1` 端点补全清单（按优先级）

**P0（"Jellyfin 基础功能"的最小闭环）**

1. `GET /api/v1/items` 扩参：`search=`（四档打分排序）、`sort=` 扩为 `title|air_date|added_at|year|rating|random|episode`、`order=asc|desc`、`filter=unplayed|played|favorite|resumable`（需登录态 user_id）、`genre=`、`year=`（现 air_year 保留）、**响应加 `total` 包裹**：`{items:[], total, offset}`——现在返回裸数组，客户端没法做分页 UI。列表行补 `overview/rating/backdrop_path/runtime_ms/user_data{position_ms, played, is_favorite}`（enableUserData 语义，可用 `with=user_data` 开关）。
2. `POST /api/v1/playback/progress`（§8.1 已规划未实现）：body `{item_id, file_id?, position_ms, duration_ms}`；服务端套 5%/90%/300s 三规则落 watch_history（清零/标记 played/play_count+1）。
3. `GET /api/v1/resume?limit=`：`watch_history WHERE position_ms>0 ORDER BY updated_at DESC` join items。
4. `GET /api/v1/nextup?limit=&series_id=`：按 §3.2 两条 SQL（最近看过的剧 → 每剧下一未看集）。SQLite 一条 CTE 也能写完。
5. `GET /api/v1/latest?limit=&kind=`：`ORDER BY added_at DESC`，series 折叠（TV 库按 series 分组取最新集时间）+ child_count。
6. `POST /api/v1/items/{id}/played` + `DELETE`：标记已看/未看；**对 series/season 调用时级联到所有 episode**（Jellyfin 语义，客户端"整季标记已看"靠它）。
7. `GET /api/v1/items/{id}/images/{type}`（§8.1 已规划）：poster/backdrop 本地缓存伺服（现在 Bangumi 外链——兼容层没有本地图片就没法给 Jellyfin 客户端出图，且外链有防盗链风险）。刮削时落盘一份。

**P1**

8. `POST /api/v1/items/{id}/favorite` + `DELETE`；items 列表支持 filter=favorite。
9. `GET /api/v1/search?q=&kind=&limit=`（或并入 items?search=，二选一，兼容层 SearchHints 从这投影）。
10. `GET /api/v1/filters?library=`：`{years:[], genres:[]}`。
11. `GET /api/v1/items/{id}/children?sort=`：把详情页里内嵌的 children 独立成可分页端点（对应 Shows/Seasons/Episodes；一季几百集的老番详情页会太肥）。
12. `GET /api/v1/upcoming`：`air_date >= 今天-1d` 的 episode 升序——追番场景比 Jellyfin 原版更有用，air_date 单列已支持。

**P2**：similar（同 genres/tags 交集打分）、adjacentTo、collections、trickplay、media segments（片头跳过）。

### 6.3 与 §8.2 Jellyfin 兼容层的映射

§8.2 表格已列认证/浏览/图片/播放/进度，本次精读补充映射细节与遗漏项：

| Jellyfin 端点 | 投影自 nipa | 备注 |
|---|---|---|
| `GET /Users/{id}/Views` 与 `GET /UserViews` | libraries | 每库一个 CollectionFolder DTO，`CollectionType: "tvshows"/"movies"`（由 library.kind 映射，anime→tvshows） |
| `GET /Items` **和** `GET /Users/{id}/Items`（两个路由都要挂） | items 扩参版 | 参数子集见 §2；`ParentId=库id` → library 过滤；`ParentId=series` → children；BaseItemKind 映射 Series/Season/Episode/Movie ↔ kind；id 需在 GUID 与 i64 间做稳定映射（如 i64 零填充成 GUID） |
| `GET /UserItems/Resume` 与 `/Users/{id}/Items/Resume` | /resume | 响应 QueryResult 包裹 |
| `GET /Shows/NextUp` | /nextup | |
| `GET /Items/Latest` 与 `/Users/{id}/Items/Latest` | /latest | **裸数组，无 QueryResult 包裹** |
| `GET /Shows/{id}/Seasons`、`/Shows/{id}/Episodes` | /items/{id}/children | Episodes 支持 seasonId/season 参数 |
| `GET /Search/Hints` | /search | |
| `GET /Items/Filters` | /filters | NipaPlay 筛选条用 legacy 版 |
| `POST /UserPlayedItems/{id}`、`DELETE`（旧 `/Users/{id}/PlayedItems/{id}`） | /items/{id}/played | |
| `POST /UserFavoriteItems/{id}`、`DELETE` | /items/{id}/favorite | |
| `GET /UserItems/{id}/UserData` | watch_history 读 | UserData DTO：`{PlaybackPositionTicks, PlayCount, IsFavorite, Played, LastPlayedDate}`；注意 ticks = ms × 10000 |
| `POST /Sessions/Playing` /Progress /Stopped（+`/Sessions/Playing/Ping` 返回 204 即可） | /playback/progress | Progress body 取 `ItemId, PositionTicks`；Stopped 时同样落一次 |
| `GET /Branding/Configuration`、`GET /QuickConnect/Enabled`、`GET /DisplayPreferences/*`、`GET /Sessions` | stub | 返回空对象/false/默认值——很多客户端启动序列会调，404 会导致报错弹窗 |
| Item DTO 关键字段 | | `Id, Name, OriginalTitle, Type, ParentId, SeriesId/SeriesName, SeasonId, IndexNumber(episode_no), ParentIndexNumber(season_no), ProductionYear, PremiereDate(air_date), Overview, CommunityRating(rating), RunTimeTicks, ImageTags:{Primary: tag}, BackdropImageTags, UserData:{...}, ProviderIds:{Bangumi, DanDanPlay,...}, MediaType:"Video", IsFolder(series/season=true), ChildCount` |

### 6.4 一个跨切面提醒

现 `list_items` 里 `ORDER BY {sort}` 用 format! 拼接——目前 sort 值是白名单匹配所以安全，但扩参数时（order、filter 组合）务必保持"枚举→静态 SQL 片段"模式，不要把任何用户输入进 format!。分页也建议同时返回 `total`（`COUNT(*) OVER()` 或独立 COUNT 查询，对齐 enableTotalRecordCount 语义）。

---

## 7. 实施顺序建议（一句话版）

先补 schema（watch_history 四列 + items 的 clean_title/runtime_ms + item_genres），再按 P0 顺序落 `/api/v1`（progress → resume/nextup/latest → played → images → items 扩参），最后兼容层纯投影——届时兼容层每个端点都只是"改 URL + 改 DTO 形状"，不含新查询逻辑，这正是 §8.2"兼容层是投影层"的设计意图。