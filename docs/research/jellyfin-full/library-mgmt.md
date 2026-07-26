# Jellyfin 库管理/刷新机制精读 → nipaserver 对标报告

来源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin`（只提炼逻辑与协议，不做代码翻译）
对标对象：`/Users/sakiko/Desktop/nipaserver/crates/nipa-server/`（migrations/0001-0004、src/api_library.rs、scan.rs、ingest.rs、steward/tools.rs）

---

## 1. 库类型与解析规则（Emby.Naming + Library/Resolvers）

### 1.1 库类型（CollectionType，Jellyfin.Data/Enums/CollectionType.cs）
用户可建库类型（0-12）：`unknown / movies / tvshows / music / musicvideos / trailers / homevideos / boxsets / books / photos / livetv / playlists / folders`。101+ 是虚拟视图（tvlatest、movieresume 等），不是物理库。关键点：

- **库类型驱动 resolver 分派**。每个库（"VirtualFolder"= CollectionFolder）带一个 `CollectionType?`，扫描时传给 resolver 链（ResolverPriority 排序：Series 第二、Movie 第四）。`MovieResolver` 只在 `movies/homevideos/musicvideos/tvshows/photos/null` 下工作；`collectionType == null`（mixed/folders 库）时走"猜测模式"——目录下有 `tvshow.nfo`/`season.nfo` 判为剧集树，否则按电影解析。
- **同一文件在不同库类型下产出不同实体**：`tvshows` 下的散文件解析为 Episode（`ResolveVideos<Episode>`），`movies` 下解析为 Movie，`homevideos` 下解析为普通 Video（不 parseName，即不从文件名剥年份）。

### 1.2 各类解析规则要点

**电影（MovieResolver + Emby.Naming/Video）**
- 年份括号：`CleanDateTimeParser`，正则本质是 `名称 (19xx|20xx)`，允许 `.`/`_`/`-`/`[]`/`()` 分隔，产出 `(Name, Year)`；再经 `CleanStringParser` 剥离画质/编码等 token（`1080p|x264|bluray|...`，NamingOptions.cs:154）。
- **多分段（stack）**：`FileStackRule`：`filename + (cd|dvd|part|pt|disc|disk) + 数字或a-d` → 合并为一个条目的 `AdditionalParts`。
- **多版本（multi-version）**：`VideoListResolver.GetVideosGroupedByVersion`——同目录、同年份、且文件名清理后等于目录名（`片名 (2020)/片名 (2020) - 1080p.mkv` 与 `- 4K.mkv`）→ 主版本 + `LocalAlternateVersions`。tvshows 库下改按"同季同集号"分组版本。
- **多碟目录**：子目录含 `VIDEO_TS`/`BDMV` → 整个目录判为 DVD/BluRay 类型的一部电影。
- **文件夹内单文件**：`movies` 库中"每片一目录"时条目名取目录名而非文件名。

**剧集（TV Resolvers + Emby.Naming/TV）**
- Series 判定（SeriesResolver）：目录 + 库类型 tvshows → 直接是 Series；mixed 库则要求（a）含 `tvshow.nfo`，或（b）子目录能被解析为 Season 目录，或（c）子文件能解析出集号。
- Season 目录（SeasonPathParser）：三类模式——`Season 1`（多语言关键字：season/staffel/saison/시즌/シーズン/сезон/temporada…）、`S01` 前缀、纯数字目录（仅 tvshows 库开启 `supportNumericSeasonFolders`）。`Specials`/`Season 0` → season_no=0。非 season 目录但含视频 → 仍算季（Path 置空）。
- Episode（EpisodePathParser，NamingOptions.EpisodeExpressions 约 20 条正则，按序命中）：`SxxExx`、`1x01`、`S01xE01`、日期型 `2020-01-01`、`EP01`/`E01`、纯数字 `101`（季+集连写）、anime 弹幕组风格 `[Group] Title - 01 [1080p]`（NamingOptions.cs:379、489 两条专门的方括号规则）、`Episode 01`。**多集文件**：`MultipleEpisodeExpressions` 解析 `S01E01-E02` → `endingepnumber`。
- 无 Season 目录时的容错：集文件直接在 Series 目录下且解析不出季号但有集号 → **默认 season 1**（EpisodeResolver.cs:81）。
- 路径内嵌 provider id：目录/文件名中 `[tmdbid-123]` `[tvdbid-123]` `[imdbid-tt123]` `[anidbid-]` `[anilistid-]` 直接捕获为外部 ID（对 anime 特别有用，nipaserver 可捕 `[bgmid-]`）。

**extras 目录/后缀（NamingOptions.VideoExtraRules）**
- 目录名（大小写不敏感）：`trailers / backdrops / theme-music / behind the scenes / deleted scenes / interviews / scenes / samples / shorts / featurettes / extras / extra / other / clips` → 内容归为 owner 条目的 extra，**不产生独立海报墙条目**。
- 文件名/后缀：`trailer`、`sample`、`theme`，及 `-trailer/-sample/-scene/-clip/-interview/-behindthescenes/-deleted/-featurette/-short/-extra/-other` 后缀。
- 全局忽略（IgnorePatterns.cs）：`**/sample.*`、`**/metadata/**`、`**/extrafanart/**`、`**/.actors/**`、`@eaDir`（群晖）等 glob；另有 `.ignore` 文件规则（DotIgnoreIgnoreRule）——目录里放 `.ignore` 即整树跳过。

---

## 2. 实时监控与定时扫描编排（Emby.Server.Implementations/IO + LibraryManager）

**三条触发路径汇合到同一验证入口 `ValidateChildren`：**

1. **定时扫描**：`RefreshMediaLibraryTask`（默认 12h IntervalTrigger）→ `LibraryManager.ValidateMediaLibraryInternal`。`POST /Library/Refresh` 也只是 `CancelIfRunningAndQueue<RefreshMediaLibraryTask>()`——**API 与定时任务共用一个可取消的单例任务**，天然去重。流程：置 `IsScanRunning=true` → **停 watcher** → 校验顶层库目录（删掉磁盘上已消失的 CollectionFolder 对应 DB 行）→ 递归 `ValidateChildren`（0-96% 进度）→ PostScanTasks（96-100%）→ 重启 watcher。
2. **实时监控 LibraryMonitor**：每个开了 `EnableRealtimeMonitor`（LibraryOptions 按库配置）的库路径起一个 FileSystemWatcher（IncludeSubdirectories，64KB 缓冲，监听 Create/Delete/Rename/Change）。父路径已被监听则不重复挂（`ContainsParentFolder`）。关键防抖设计：
   - **FileRefresher 聚合器**：每个变更路径进一个 refresher，带定时器（`LibraryMonitorDelay` 默认 60s，每来新事件就重置）。同路径→重启计时；子路径→并入现有；父路径→接管；**兄弟路径→上卷到共同父目录**。到期后从 DB `FindByPath` 逐级向上找到受影响条目，调 `item.ChangedExternally()`（= 入队一次该条目的 refresh+validate）。
   - **自写忽略**：服务器自己写文件（存 nfo/图片）前 `ReportFileSystemChangeBeginning(path)` 加入 tempIgnored，完成后延迟 45s 移除——防止自己触发自己。
   - watcher 出错（如网络盘断开）→ 释放该 watcher，不拖垮进程。
3. **外部通知**：`POST /Library/Media/Updated`（body: 路径+UpdateType）、`/Library/Series|Movies/Added|Updated`（tvdbId/tmdbId）——给 Sonarr/Radarr 用的入库回调，落点同样是 ReportFileSystemChanged。

**互斥规则**：扫描期间 watcher 停；库结构变更 API（增删路径/改名）也先 `_libraryMonitor.Stop()`，操作完成后要么触发全量扫描、要么延迟 1s 再 `Start()`（防止目录移动的事件风暴）。

---

## 3. 刷新元数据的粒度与 API（ItemRefreshController + ProviderManager）

**端点**：`POST /Items/{itemId}/Refresh`，参数即粒度矩阵：
- `metadataRefreshMode` / `imageRefreshMode`：`None | ValidationOnly | Default(补缺失) | FullRefresh(重新请求 provider)`——元数据与图片**独立控制**。
- `replaceAllMetadata` / `replaceAllImages`：只在 FullRefresh 下生效。false = 重刮但保留已有字段/已有图；true = 全覆盖（且 `RemoveOldMetadata=true` 清掉旧的）。
- 语义组合：**"补缺失" = Default + replace=false；"全部替换" = FullRefresh + replaceAll=true**。
- 返回 **204 立即返回**，实际执行走 `ProviderManager.QueueRefresh(itemId, options, RefreshPriority.High)`——进程内 PriorityQueue + 单消费循环（懒启动）。`GetRefreshQueue()` 暴露"正在刷新中"的 item 集合，`GET /Library/VirtualFolders` 借此返回每个库的 RefreshProgress。
- **递归**：刷新的 item 若是 Folder，消费时自动 `ValidateChildren(递归)`——即"刷新一个 Series"隐含重扫其目录并逐集刷新；对整库（CollectionFolder）则遍历物理目录逐个刷。用户手动刷新（IsAutomated=false）会绕过"已刷新过就跳过"的节流。
- 整库入口：`POST /Library/Refresh`（全库扫描）；被锁定字段（LockedFields，ItemUpdateController 设置）在刷新时不被 provider 覆盖。

---

## 4. 条目删除/合并/识别修正 API

**删除（LibraryController）**
- `DELETE /Items/{itemId}`、`DELETE /Items?ids=a,b,c`。权限：用户须有 content-deletion 权限。`DeleteOptions{ DeleteFileLocation=true }`——**Jellyfin 的删除是删源文件**（先 ReportFileSystemChangeBeginning 防 watcher 回环）。
- 删除多版本主条目时（LibraryManager.DeleteItem, :452-521）：把文件已不存在的 alternate 一并清理；把第一个存活 alternate **提升为新主版本**，转移 LinkedChildren 并重路由 playlist/collection 引用——防"幽灵条目"。
- nipaserver 注意：软删除（`deleted_at`）是你已有的更安全的默认；Jellyfin 没有软删除，靠扫描时发现文件消失直接删行。

**合并/多版本（VideosController）**
- `POST /Videos/MergeVersions?ids=a,b`：≥2 个视频合并为一条（自动选主：非3D、分辨率最高者），其余变 alternate。
- `DELETE /Videos/{itemId}/AlternateSources`：拆开合并。

**识别修正（ItemLookupController = "Identify" 功能）**，这是 Jellyfin 手动改错识别的标准三步协议：
1. `GET /Items/{itemId}/ExternalIdInfos`：该条目支持哪些外部 ID（供编辑表单）。
2. `POST /Items/RemoteSearch/{Movie|Series|BoxSet|...}`，body=`{ SearchInfo: { Name, Year, ProviderIds }, ItemId, IncludeDisabledProviders }` → 返回候选列表（RemoteSearchResult：Name/Year/ProviderIds/ImageUrl/Overview）。
3. `POST /Items/RemoteSearch/Apply/{itemId}?replaceAllImages=true`，body=选中的 RemoteSearchResult → **直接把 ProviderIds 写到条目上，再以 FullRefresh + ReplaceAllMetadata + RemoveOldMetadata 同步重刮**（此端点是同步等待完成的，与 /Refresh 的异步入队不同）。
- 手工编辑：`POST /Items/{itemId}`（整个 BaseItemDto 覆盖写，配 LockedFields 防止后续刷新冲掉）；`GET /Items/{itemId}/MetadataEditor` 返回编辑器所需的枚举（可用 countries/languages/external id infos/content type options）。

**库配置（LibraryStructureController，/Library/VirtualFolders）**
- `GET`（列库+RefreshProgress）/ `POST ?name=&collectionType=&paths=&refreshLibrary=`（建库）/ `DELETE`（删库）/ `POST /Name`（改名）/ `POST|DELETE /Paths`（一库多路径）/ `POST /LibraryOptions`（改选项）。所有变更操作统一模式：**停 watcher → 改 → refreshLibrary?全量扫描:延迟1s重启watcher**。

---

## 5. 给 nipaserver 的差距清单与实施建议

### 差距总表

| Jellyfin 能力 | nipaserver 现状 | 差距级别 |
|---|---|---|
| 库类型驱动解析（movies/tvshows/mixed） | `libraries.kind` 仅存储不使用（api_library.rs:56 注释自认） | 高 |
| 命名解析（SxxExx/Season 目录/年份/多集文件） | 无本地解析，全部丢给 L1 弹弹play/L2 AI | 中（agentic 路线下是特色而非缺陷，但可做"零成本前置"） |
| extras/sample/忽略规则 | walk_library 无忽略规则 | 高（sample.mkv、SPs、menu 会进刮削队列烧 token） |
| 实时监控 watcher | 无（scan.rs TODO M1.x） | 高 |
| 定时扫描 | 无 | 中 |
| /items/{id}/refresh（粒度化重刮） | 无；只有 steward 内部 requeue_scrape 工具 | 高 |
| 手动识别修正（RemoteSearch 三步） | steward 对话式 requeue_scrape(hint)/confirm_pending；无结构化 API；confirm_pending 尚未真正入库（tools.rs:344 TODO） | 高 |
| 条目删除 API | 无（有 deleted_at 列） | 中 |
| 多版本合并/拆分 | schema 已支持（file_item 多对多），无 API | 低 |
| 整库刷新 + 扫描互斥/进度查询 | trigger_scan 可重复 spawn，无互斥；进度仅 SSE | 中 |
| 库的增删改（改路径/删库/改名） | 只有 create | 中 |

### 5.1 让 `libraries.kind` 用起来（anime/movie/tv）

kind 不需要改 schema，改三处消费点：

- **scan.rs 的 evidence 组装**：把 kind 注入 AI 的 user_message（"这是动画库/电影库"），并决定 L1 是否启用——`kind='movie'` 时跳过弹弹play（弹弹play 是番剧库，电影命中率低），直接 L2；`kind='anime'` 保持 L1→L2。
- **前置结构解析（Jellyfin 的思路，AI 前的零成本层）**：按 kind 选解析策略——
  - `anime`：目录名剥 `[字幕组]`、捕获 `- 01` 集号（对应 NamingOptions.cs:379/489 两条方括号规则的语义）、识别 `SPs/Specials/Menu/CDs/Scans` 子目录为 extras；同目录同集号不同 `[1080p]/[720p]` 标签 → 预标记多版本候选。
  - `tv`：Season 目录关键字 + SxxExx/1x01/多集 `S01E01-02`（写入 file_item.episode_range 你已有的列）。
  - `movie`：`名称 (年份)` 提取 + stack（`cd1/cd2`）合并。
  - 解析结果不直接入库，而是作为 evidence 的结构化字段（`parsed: {series_guess, season, episode, year, version_tag}`）交给 L2 校验——这是 agentic 架构下"Jellyfin 基础功能都要有"的正确姿势：**解析器提供先验，AI 裁决**，同时高置信度命中（Season 目录 + SxxExx）可直接走快速通道不进 AI 队列，省 token。
- **忽略规则（直接抄语义，necessity 最高）**：walk_library 增加：glob 忽略 `sample.*`、`**/metadata/**`、`@eaDir`、`.ignore` 文件跳过整树；目录名命中 `extras/trailers/menu/SPs/CDs/Scans...`（按 kind 扩充 anime 特有集）→ 文件标 `status='ignored'` 或挂到 owner 条目的 extras（可加 `media_files.extra_type TEXT` 列）。

### 5.2 watcher（notify crate）接入点

照抄 LibraryMonitor + FileRefresher 的两层结构，Rust 侧对应物：

- `notify::recommended_watcher` 每库一个（或 `RecursiveMode::Recursive` 单 watcher 多路径），仅当 `libraries.options` JSON 里 `realtime_monitor=true`（对应 Jellyfin 的按库开关；NAS/SMB 上 notify 不可靠，必须可关）。
- **防抖聚合器**（关键，必须有）：`HashMap<PathBuf, Instant>` + tokio 定时器实现 FileRefresher 语义——事件进来后 60s 静默期（新事件重置计时），路径合并规则照抄：同路径重置/子并父/兄弟上卷到父目录。到期后不做全库扫描，而是**对该子树跑 scan_library 的单目录版**（把 scan.rs 主循环抽出 `scan_paths(db, library_id, rel_paths)`，复用现有 L0 指纹跳过/移动检测/L1/L2 管线——你的 L0 `(size,dandan_hash)` 移动检测天然适配 rename 事件）。
- **自写忽略集**：nipaserver 将来写 nfo/下载海报到库目录时，先注册 temp-ignore 路径（AppState 里一个 `DashSet<PathBuf>`，完成后延迟移除），否则 watcher 会自触发。
- **互斥**：AppState 加 `scan_running: HashMap<i64, AtomicBool>`（顺手修掉 trigger_scan 可重复 spawn 的问题）；全量扫描开始时暂停该库 watcher 事件消费（丢弃或缓冲），结束后恢复——照抄 ValidateMediaLibraryInternal 的 Stop/Start 括号。
- **兜底定时扫描**：tokio 定时任务每 12h（可配）对所有库跑增量 scan_library——notify 丢事件是常态（尤其网络盘），Jellyfin 的双保险结构值得保留。

### 5.3 `/items/{id}/refresh` 端点设计（requeue_scrape 的 API 化）

```
POST /api/v1/items/{id}/refresh
body: {
  "mode": "missing" | "full",        // 对应 Default vs FullRefresh+ReplaceAll
  "images": "keep" | "refresh",      // 海报是 Bangumi 直链，refresh=重取 poster_path
  "recursive": true,                  // series → 其下 season/episode 全部重刮
  "hint": "用户提示，可选"            // nipaserver 特色：直通 AI 的重刮线索
}
→ 202 { "queued_tasks": [task_id, ...] }
```

实现路径（全部复用现有件）：
1. 按 id 查 items，recursive 时沿 `parent_id` 收集子树 episode/movie；经 `file_item` 找到关联 `media_files`。
2. 对每个 file 找到其最新 scrape_task 的 evidence，按 RequeueScrape（steward/tools.rs:268-303）的逻辑：`state='queued'` + `scrape.enqueue(ScrapeRequest{...})`，hint 拼进 user_message。**把 tools.rs 里这段抽成 `pub async fn requeue_file(db, scrape, file_id, hint)` 供 API 与 steward 工具共用**，避免两份逻辑漂移。
3. `mode="full"` 时 ingest_result 需要区分：missing=只填 NULL 字段（`COALESCE(new, old)` 反过来），full=覆盖 title/overview/rating/poster_path 等（当前 ingest.rs 的 upsert 行为需确认，加一个 `replace: bool` 参数贯穿）。
4. 队列可见性：`GET /api/v1/scrape/queue`（state IN ('queued','running')），对应 Jellyfin GetRefreshQueue → 前端能显示"刷新中"角标。
5. 整库版：`POST /api/v1/libraries/{id}/refresh`（区别于 /scan：scan=走文件系统 diff，refresh=已知文件全部重刮）。

### 5.4 手动修正条目端点（correct_pending API 化 + 已入库重识别）

对齐 Jellyfin 的 Identify 三步，落到 nipaserver 的实体上：

**A. pending 任务修正（对应现 steward confirm/requeue）**
```
GET  /api/v1/scrape/pending                       # 已有
POST /api/v1/scrape/tasks/{task_id}/confirm       # confirm_pending 的 API 化
POST /api/v1/scrape/tasks/{task_id}/reject        # state='queued' + hint（= requeue_scrape）
POST /api/v1/scrape/tasks/{task_id}/apply         # body: 手选的识别结果（见 C 的候选搜索）
```
前提：**先补上 tools.rs:344 的 TODO**——confirm 必须真正走 ingest_result 写 items/file_item，否则 confirm API 和 steward 工具都只是翻状态。confirm 端点与 ConfirmPending 工具共用同一个 service 函数。

**B. 已入库条目的重新识别（Jellyfin RemoteSearch/Apply 的对应物）**
```
POST /api/v1/items/search-candidates
body: { "query": "苍穹的法芙娜", "kind": "series", "provider": "bangumi" }
→ [ { provider, external_id, title, year, poster_url, overview } ]   # 走 providers 的只读元数据工具（已有 bangumi 客户端）
POST /api/v1/items/{id}/identify
body: { "provider": "bangumi", "external_id": "1546", "apply_to_children": true }
```
identify 的实现语义（照抄 ApplySearchCriteria 的顺序）：
1. 写 `item_ids`（UNIQUE(provider,external_id) 冲突 = 撞上已有条目 → 走 §4.5 合并：file_item 迁移到既有 item，旧 item 软删）。
2. 以新 external_id 为准同步拉取元数据覆盖 items 行（title/overview/air_date/poster_path，复用 ingest.rs 的 backfill_bangumi_poster）。
3. `apply_to_children`：series 重定位后其下 episode 按 episode_no 重新对齐新作品的集列表；对不上的置 needs_review。
4. **顺手写 scrape_corrections**（schema 已建好但未使用）：记 `(dir_path, item_id)`，让同目录后续新文件（追番周更场景）跳过 AI 直接挂靠——这是 Jellyfin 没有、但 nipaserver schema 已预留的优势能力。

**C. 删除与合并（低成本补齐）**
```
DELETE /api/v1/items/{id}?delete_files=false      # 默认软删（UPDATE deleted_at），delete_files=true 才动磁盘（admin only）
POST   /api/v1/items/merge   body: {"ids":[a,b]}  # item_ids/file_item/watch_history 迁到主条目（保留 external_ids 并集），其余软删
```
合并主条目选择可照抄 Jellyfin：文件版本多者/分辨率高者优先；watch_history 主键 (user_id,item_id) 迁移时 ON CONFLICT 取 position 较大者。

### 5.5 建议实施顺序

1. **忽略规则 + kind 前置解析进 evidence**（直接省 token、减少 needs_review，改动最小）。
2. **confirm_pending 真正入库**（阻塞所有修正功能的地基）。
3. **/items/{id}/refresh + /scrape/tasks/{id}/{confirm,reject}**（requeue/confirm 抽 service 层，API 与 steward 工具共用）。
4. **identify + merge + delete**（管理闭环）。
5. **notify watcher + 12h 定时增量**（scan_paths 子树扫描抽取是前置）。
6. 库管理补齐：`PATCH/DELETE /api/v1/libraries/{id}`、扫描互斥标志、`GET /api/v1/libraries` 返回 refresh 进度。

关键参考文件（含行号）：
- 刷新参数矩阵：`Jellyfin.Api/Controllers/ItemRefreshController.cs:62-92`
- 识别修正协议：`Jellyfin.Api/Controllers/ItemLookupController.cs:244-281`
- 库配置端点：`Jellyfin.Api/Controllers/LibraryStructureController.cs`
- watcher/防抖：`Emby.Server.Implementations/IO/LibraryMonitor.cs:388-430`、`FileRefresher.cs:63-128`
- 扫描编排：`Emby.Server.Implementations/Library/LibraryManager.cs:1354-1459`
- 刷新队列：`MediaBrowser.Providers/Manager/ProviderManager.cs:1116-1214`
- 命名规则总表：`Emby.Naming/Common/NamingOptions.cs`（episode 正则 :320-489，extras :495-706）
- Season 解析：`Emby.Naming/TV/SeasonPathParser.cs:13-30`
- 多版本删除提升：`Emby.Server.Implementations/Library/LibraryManager.cs:452-521`
- nipaserver 接入点：`crates/nipa-server/src/scan.rs:30`（scan_library 抽 scan_paths）、`src/steward/tools.rs:268-303`（requeue 抽 service）、`src/steward/tools.rs:344`（confirm 入库 TODO）、`src/ingest.rs:17`（ingest_result 加 replace 语义）