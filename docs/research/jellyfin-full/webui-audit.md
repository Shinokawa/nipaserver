五个调研方向全部完成。以下为完整审计报告（按要求只读调研、不落盘，直接输出）。

---

# jellyfin-web UI 功能面审计报告 —— 对标 nipaserver WebUI

> 2026-07-26。源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin-web`（v12.0.0，GPL——本报告仅提炼功能清单与交互逻辑，无代码翻译）。
> 对照物：`/Users/sakiko/Desktop/nipaserver/webui/app/src/`（四视图 + ItemDetailModal + AgentTimeline）、`crates/nipa-server/src/api_library.rs` / `api.rs`、migration 0005、`docs/07-jellyfin对标实施计划.md`。
> 架构注：jellyfin-web 是 Modern(React)/Legacy(JS) 双实现并存，`layoutManager.modern` 分流；**详情页与首页至今仍是 legacy controller，Modern 也复用它**——说明这两页逻辑最重、最值得精读。

## 1. 页面/路由全清单（含 nipaserver 适用性）

路由为 hash 路由（`#/home`），定义在 `src/RootAppRouter.tsx` + 各 app 的 `routes/`。

### 1.1 用户端主路由

| 路由 | 页面 | 一句话 | 适用性 |
|---|---|---|---|
| `/home?tab=0\|1` | 首页 | sections 聚合（详见 §2）+ 收藏 tab | **必备**（继续观看/最新入库 sections） |
| `/movies?topParentId=&tab=` | 电影库 | tabs: Movies/Suggestions/Favorites/Collections/Genres | **必备**（Movies 主网格）；Suggestions 推荐、Collections 合集 P2 |
| `/tv?topParentId=&tab=` | 剧集库 | tabs: Shows/Suggestions/Upcoming/Genres/Networks/Episodes | **必备**（Shows 主网格）；Upcoming 对追番场景**推荐**（nipa 有"按季度浏览"设计可替代） |
| `/details?id=` | 通用详情页 | 所有类型共用一个 controller（§3） | **必备**（P0 核心） |
| `/list?type=&parentId=` | 万能列表页 | query 驱动的下钻列表（genre 下钻、nextup 全列表等） | **推荐**（genre/标签下钻的低成本实现方式） |
| `/search?query=` | 搜索页 | 防抖搜索 + 分组结果行（§4.7） | **必备** |
| `/video` | 视频播放器 OSD | §5 | **必备**（M3） |
| `/queue` | 播放队列页 | 全屏队列 | 跳过（v1 无队列概念） |
| `/lyrics` | 歌词页 | 音乐 | **跳过**（音乐域） |
| `/music` `/musicvideos` `/playlists` | 音乐族 | — | **跳过** |
| `/livetv` | 直播电视（6 tabs） | — | **跳过** |
| `/books` `/homevideos` `/boxsets` `/mixed` | 书/照片/合集/混合库 | — | 跳过（合集 P2 可再议） |
| `/userprofile` `/mypreferencesmenu` + display/home/playback/subtitles/controls 五页 | 用户偏好族 | 显示语言/首页 section 配置/播放偏好/字幕外观/操控 | **推荐**（v1 精简为单页设置即可；首页 section 配置与字幕外观是其中最有价值的两项） |
| `/login` `/selectserver` `/addserver` `/forgotpassword*` | 会话族 | — | login **必备**（§8.4 auth 落地时）；多服务器选择跳过 |
| `/quickconnect` | 扫码配对 | — | 跳过（兼容层返回 `Enabled:false` 即可） |

### 1.2 管理端（`/dashboard/*`，分组一行一条）

服务器概览与活动日志 / 常规与品牌 / 用户管理（资料/库访问/家长控制/密码）/ 库管理（增删改扫描/显示/元数据/NFO）/ 播放（转码/续播阈值/串流限速/Trickplay）/ LiveTV+DVR / 插件 / 计划任务 / 日志查看 / 设备 / 网络与 API Key 与备份 / `/metadata` 元数据管理器 / `/wizard` 六步初始化向导。

**nipaserver 立场**：管理面已由 SettingsView + ConsoleView + 管家承担，Jellyfin 的表单式管理台**整体跳过**——这正是产品差异点（元数据编辑走管家审批而非 `/metadata` 表单堆砌）。值得抄的只有：库管理的"扫描进度可见"（已有 SSE）、日志查看页（P2）、向导式首启（P2，headless 初始化目前走配置文件）。

## 2. 首页 sections 体系（`src/components/homesections/`）

**调度模型**：10 个可配置槽位（`homesection0-9` 存 userSettings），每槽位一个下拉选 section 类型；默认顺序 `我的媒体, 继续观看, 继续收听, 继续阅读, livetv, Next Up, 最新添加, none×3`。各 section 先渲染骨架挂 `fetchData`，由 itemsContainer 懒取数，**空结果整节隐藏**；`data-monitor="videoplayback,markplayed"` 驱动播放/标记后自动刷新。

| Section | 数据来源 | 关键参数 | 卡片形态 |
|---|---|---|---|
| 我的媒体 | `GET /UserViews`（复用，不再请求） | — | 横版 backdrop 卡，库名居中 |
| **继续观看** | `GET /UserItems/Resume` | limit 12、`mediaTypes:['Video']`、`fields:[PrimaryImageAspectRatio]`，服务端按最近播放排序 | 横版缩略图，**底部进度条**（`UserData.PlayedPercentage` ∈ (0,100)），showTitle+showParentTitle+showYear |
| **Next Up** | `GET /Shows/NextUp` | limit 24、`enableResumable:false`（去重关键）、`nextUpDateCutoff=今天−365天`（可配）、`enableRewatching`（默认关） | 横版缩略图，无进度条 |
| **最新添加** | 对每个库各调一次 `GET /Items/Latest?parentId=库id` | limit 16（音乐 30）、受 `LatestItemsExcludes`（用户可按库排除）与 `HidePlayedInLatest`（服务端过滤已看）控制 | 电影/剧集=竖版海报+年份，未看集数角标 |
| 继续收听/阅读、LiveTV On Now、正在录制 | 同构 | — | 跳过（音乐/书/LiveTV 域） |

**Resume 与 NextUp 去重**：唯一手段是 NextUp 请求带 `enableResumable:false`（服务端剔除已有进度的剧），前端零交叉比对——nipaserver 照抄该语义即可（docs/07 批次 B 的 next-up 查询需实现此标志位语义）。

**对 nipaserver 的映射**：`我的媒体`＝库入口卡（可选）；`继续观看`+`Next Up`+`按库最新添加`三件套是 P0；用户槽位配置 v1 可硬编码顺序，P2 再开放。

## 3. 详情页解剖（重点）

单一 controller（`src/apps/legacy/controllers/itemDetails/index.js`，2197 行）服务所有类型。**关键取数模式：`GET /Items/{id}` 单条目查询服务端默认返回全量 DTO（含 MediaStreams/People/Chapters），不需要 Fields 参数**——nipaserver 的 `/items/{id}` 应遵循同样契约（一次拿全，子区块另发请求）。

### 3.1 通用布局（自上而下）

1. **头部 banner backdrop**（40vh、cover、800ms 淡入；三级回退：自身 Backdrop → 父级 Backdrop → Primary 兜底）→ 需要 `item_images` 的 backdrop 行 + 图片伺服端点；
2. **Logo**（右上角 25vw，Episode 继承剧集 logo；移动端隐藏）→ `item_images.logo`，P1；
3. **海报**（左浮、跨 banner 与内容区、宽 25vw；形状由 `PrimaryImageAspectRatio` 决定）；
4. **标题行**：按类型 1-3 行——Episode 为 `剧名(链接)` / `季名(链接) - 3. 集名`；`OriginalTitle≠Name` 时追加原名行（nipa 已有 original_title）；
5. **meta 信息行**（顺序固定）：Series 年份区间（`Continuing`→"2019 至今"，否则 `年份-EndDate年`）· 年份 · 时长（"2h 15m"）· 官方分级徽章 · 社区评分★ · 烂番茄 CriticRating（≥60 新鲜/<60 烂）· **"结束于 hh:mm"**（`RunTimeTicks − PlaybackPositionTicks` 换算时钟、每 60s 自刷新）→ 字段全部在 nipa 0005 schema 内（rating/official_rating/runtime_ms/series_status/end_date），CriticRating 缺列（P2）；
6. **版本/音轨/字幕下拉**（4 个 select，多版本文件时切换联动 resume 位置）→ nipa 有 file_item 多版本关系，M3 播放时需要；
7. **Tagline**（`Taglines[0]`）+ **Overview**（markdown 渲染 + 6/12 行折叠 + ShowMore）→ tagline/overview 列已有；
8. **播放按钮组**：`btnPlay`（`data-action=resume`，有断点时 title 变 Resume）+ **`btnReplay` 仅在有断点时出现**（从头播放）+ Trailer + 已看勾（emby-playstatebutton）+ 收藏心（emby-ratingbutton）+ ⋮ 更多菜单。已看/收藏按钮**订阅 WebSocket UserDataChanged 实时同步**（nipa 用 SSE 等价）；
9. **导演/编剧/工作室/类型链接行**（固定序 Author/Creator/Director/Writer/Studio/Genre，单复数自动，链到 person/genre 下钻）→ people/item_values 表已有，缺 API；
10. **Tags** 链接行、**外部链接**（`ExternalUrls[]{Name,Url}` 由**服务端**从 ProviderIds 生成——nipa 目前前端拼 URL，建议照此移到服务端）。

### 3.2 类型差异

- **Series**：季卡片区（`GET /Shows/{id}/Seasons`，竖版海报 + **未看集数角标** `UnplayedItemCount`/看完✓）+ **Next Up 区块**（`GET /Shows/NextUp?SeriesId=`）+ 演职员 + 相似推荐。**Series 页没有集列表，只有季**。桌面网格换行、仅移动端横滚。
- **Season**：**列表式集列表**（`GET /Shows/{seriesId}/Episodes?seasonId=&Fields=...,Overview`——注意集列表要显式带 Overview）。每行：500px 缩略图（回退到剧集海报）+ 缩略图上悬浮 ▶ + **底部进度条** + 已看✓角标 + "3. 集名" + 简介 + 时长/首播日期 + 右侧按钮组（ⓘ/✓/♥/⋮）。这就是 nipa 详情页集列表的目标形态（docs/05 §4.2 的规划与此一致，另加弹幕徽章）。
- **Episode**：主图为宽幅剧照（aspect ratio 驱动自动变横版卡）+ 剧集/季两级链接 + **"本季其他集"横滚**（同季全集、自动滚动定位到当前集，≥2 集才显示）。meta 行不显示年份与分级，显示首播日期。
- **Movie**：附加部分（`PartCount>1` → `/Videos/{id}/AdditionalParts`）+ 特典（`SpecialFeatureCount>0`）+ **所属合集**（`GET /Items/{id}/Collections`）→ nipa 均 P2。

### 3.3 通用子区块

- **演职员横滚**：数据就在 `item.People[]`（无额外请求），竖版人像卡 + 角色名（"饰 XX"，Role 与 Type 的去重显示逻辑很细）。GuestStar 单独一节。→ nipa `people`/`item_people` 表已备好，缺：detail API 输出 people、person 头像伺服、person 点击下钻（可用 `/list?person=` 式查询）。
- **相似推荐**：`GET /Items/{id}/Similar?limit=12`（服务端按 genre/tag 重合度算）→ nipa P1，可用 item_values 重合度 SQL 实现，或走管家做"AI 推荐"差异化。
- **章节 Scenes**：`Chapters[]{Name,StartPositionTicks,ImageTag}`，**没有章节缩略图整个区块不显示**；点击从该位置播放 → 依赖 ffprobe 章节 + 截图生成，M3 后 P2。
- **媒体信息弹窗**（itemMediaInfo）：MediaSource（容器/Path 仅管理员/Size）→ MediaStream 全字段（编码/分辨率/码率/声道 ChannelLayout/语言/位深/HDR-DoVi 全套/Default/Forced/External），带复制按钮 → nipa 已存 ffprobe JSON，只差前端渲染（P1，低成本高感知）。

## 4. 库浏览页

- **视图切换 6 模式**：`Banner/List/Poster(默认)/PosterCard/Thumb/ThumbCard`，按库持久化（`{libId}-{mode}-view`）。Modern 版简化为 Grid/List + ImageType + ShowTitle/ShowYear/CardLayout 开关组合——**nipa 建议学 Modern 模型**（海报/海报带字/列表三档足够）。
- **筛选器**（filterdialog，折叠面板）：
  - 状态：`IsPlayed/IsUnPlayed/IsResumable/IsFavorite`（Filters 参数）；
  - 剧集专属：Specials(`ParentIndexNumber=0`)、缺集/未播出（`IsMissing/IsUnaired` 联动三态）、SeriesStatus(`Continuing/Ended/Unreleased`)；
  - 特性：HasSubtitles/HasTrailer/HasSpecialFeature/HasThemeSong/HasThemeVideo；
  - 视频类型：BD/DVD/HD/4K/SD/3D（`IsHD/Is4K/Is3D/VideoTypes`，HD/SD 互斥）；
  - **动态筛选**：`GET /Items/Filters?parentId=` 返回该库实有的 Genres/OfficialRatings/Tags/Years 供勾选（与当前已选 union）→ nipa 需要对应的 facet 端点（一条 `SELECT DISTINCT` 级查询）；
  - 有筛选生效时按钮上挂红点指示。
- **排序全清单**（皆为多键 tie-break 串）：电影 = Name(`SortName,ProductionYear`)/Random/CommunityRating/CriticRating/DateCreated/DatePlayed/OfficialRating/PlayCount/PremiereDate/Runtime；剧集 = Name/Random/CommunityRating/DateCreated/**DateLastContentAdded（最新有新集）**/SeriesDatePlayed/OfficialRating/PremiereDate。方向 Asc/Desc 单选。→ nipa 现只有 3 种排序；`DateLastContentAdded` 对追番很关键（剧集卡应按"最近有新集"排）。
- **分页**：传统上/下页按钮（页首尾各一条），`StartIndex/Limit` + `TotalRecordCount`，**默认 100 条/页**（userSettings.libraryPageSize，0=不分页），翻页回滚顶部。**不是无限滚动**——nipa 的 docs/05 规划无限滚动，实现上更现代，保留即可，但后端必须给 `total_count`。
- **字母索引条**：右侧竖排 `#A-Z`，点击=**发起 `NameStartsWith=X` 新查询**（`#`→`NameLessThan=A`），再点取消；排序不含 SortName 时自动隐藏 → nipa 需要 sort_name 列参与（0005 已有列）+ `name_starts_with` 参数；中文场景需要刮削写拼音首字母（docs/07 已规划）。
- **搜索页**：500ms 防抖 + URL `?query=` 同步；空查询显示随机建议（收藏优先 20 条纯文字）；**主查询一次 `limit:800` 拿混合类型结果、客户端按 Type 分桶**成 Movies/Shows/Episodes/People/Studios… 行（硬编码顺序），另并行 People/Studios 等专项请求；无"最近搜索"历史。库内搜索无结果时给"全局重搜"链接。→ nipa 的 ⌘K 搜索：一个 `GET /api/v1/search` 返回按 kind 分组即可等价。

## 5. 播放器页（OSD）——按 direct play / 转码依赖标注

`playback/video/index.js`（2066 行）+ htmlVideoPlayer 插件。

**Direct Play（原生 `<video>`）就能做——M3 前即可全部落地**：

- 播放/暂停、进度条（含缓冲区显示）、左右时间（右侧点击切"剩余时间"）、**"结束于 hh:mm"**（算入倍速）；
- 快退 10s/快进 30s（用户可配 5-30s）、上一集/下一集、音量条+静音+滚轮调音量；
- **播放速度** 11 档（0.5-4x，`video.playbackRate`，sessionStorage 跨集保持）、宽高比三档（CSS object-fit）、循环模式；
- 全屏（双击）/画中画/AirPlay(仅 Safari)；单击=播放暂停、双击=全屏、3s 无操作隐藏 OSD、移动端自动横屏锁定；
- **外挂字幕切换**（External 字幕客户端直切不断流；ASS/SSA 用 libass-wasm 渲染并从服务端拉附件字体）、字幕偏移调整（仅外挂）、字幕外观设置（仅客户端渲染字幕生效）；
- **下一集倒计时 UpNext**（剩余 30-40s 时弹"即将播放下一集"卡，纯元数据）；
- 音轨切换（浏览器多音轨支持差，Chrome 基本不行→实际常需转码，标记为"部分"）；
- 逐帧 `,`/`.`（暂停时）、快捷键全套（见 §6）。

**依赖服务端能力（M3 转码/预生成之后）**：

- **画质/码率 14 档切换**：需 `SupportsTranscoding` + PlaybackInfo 重协商 + HLS 重开流——M3；
- **进度条 Trickplay 缩略图预览**：需服务端预生成雪碧图 + 元数据——P2；降级为章节缩略图 → 需章节图提取；再降级纯时间气泡（direct play 可用）；
- **章节刻度/上下章跳转**：需 Chapters 数据（ffprobe 可出，属"探测层"而非转码，nipa-stream 已有 probe 能力，成本低）；
- 内嵌字幕在转码流上的切换、烧录字幕：M3；
- **跳过片头/片尾**（media segments）：需服务端片头检测（Jellyfin 靠插件）——nipa 可做成管家/agent 特色能力，P2；
- 播放统计面板的 TranscodingInfo/TranscodeReasons：M3。

## 6. 通用交互

- **右键/更多菜单**（itemContextMenu，按条件出现）：播放/从此播放全部/入队 ｜ 进多选/加合集/加播放列表/下载/复制流地址/**删除**（CanDelete）｜ **编辑元数据**（管理员）/编辑图片/**编辑字幕（含在线字幕搜索下载）**/**识别 Identify**（RemoteSearch 搜索→Apply 覆盖）/**媒体信息**/**刷新元数据**（三档：扫描新增/补缺失/全量替换，可选替换图片）。→ nipa 映射：编辑/识别/刷新统一改走**管家对话或重刮任务**（agentic 替代表单），菜单项为"重新识别（交给管家）/查看识别过程/标记已看/收藏/删除"。
- **多选**：卡片长按 550ms 或右键"选择"进入；批量操作 = 全选/加合集/加播放列表/删除（二次确认）/合并版本/**标记已看/未看**/刷新元数据。→ nipa P1（审批队列已有 J/K/Enter 键盘流，库页多选可后置）。
- **快捷键三层**：全局层（TV 方向键/媒体键）；**播放页层**：Space/K 播放暂停、J/← 快退、L/→ 快进、↑↓ 音量、F 全屏、M 静音、**0-9 跳转 0-90%**、Shift+P/N 上下集、PageUp/Down 章节、Home/End 首尾、Shift+,/. 倍速、G/H 字幕偏移、,/. 逐帧；列表页层：**输入字母数字直接跳转到条目**（alphanumericshortcuts）。
- **卡片交互**：点击图片=进详情（`data-action=link`）；桌面 hover 出**主播放 FAB（resume）+ 已看勾 + 收藏心 + ⋮**；移动端常驻右下 ▶/⋮；已看/收藏按钮独立组件、点击即调 API 并经 WebSocket 同步所有可见实例。
- 操作后刷新：菜单返回 `{updated,deleted}` → 容器 `notifyRefreshNeeded`；nipa 用 SSE 事件等价（已有 scrape_update 刷新模式）。

## 7. 差距清单（Jellyfin 有 & nipaserver 没有）

后端现状基线：`api_library.rs` 仅 5 端点（libraries×2/scan/items/items/{id}/pending），`api.rs` 仅 system/chat/events 族；**0005 schema 数据地基已建但零 API 使用**；ingest 只写 poster_path。

### P0 基础必备（对应 docs/07 批次 B/C/D，可直接排任务）

| # | 差距项 | 前端任务 | 需要的后端 API | 现状 |
|---|---|---|---|---|
| 1 | **详情页完整化**（Modal→独立页 `#/item/{id}`）：backdrop+海报+meta 行（年份/时长/分级/评分/系列状态区间）+tagline+overview 折叠+类型/工作室链接行+外部链接 | ItemDetailModal 重写为路由页 | `/items/{id}` 扩展输出：overview/tagline/rating/official_rating/runtime_ms/series_status/end_date/genres/studios/tags/people/images | **列全有（0005），API 一个不输出** |
| 2 | **Season/集列表升级**：缩略图+简介+进度条+已看勾+单集菜单 | 集列表组件 | children 输出 overview/runtime/图片 + user_data（played/position） | children 仅 10 基础列 |
| 3 | **已看/收藏标记** | 卡片 hover 按钮 + 详情页按钮 + 上下文菜单 | `POST/DELETE /items/{id}/played`、`/favorite`（Jellyfin 语义：Series/Season 标记递归子项） | 列已有，**端点缺** |
| 4 | **播放进度上报 + 继续观看** | 首页 Resume 横滚（海报底 2px 进度条，docs/05 已设计） | `POST /playback/progress`（三合一）+ `GET /items/resume`；played 判定 5%/90%/300s 常量照抄 | watch_history 表就绪，端点缺 |
| 5 | **Next Up** | 首页 section + Series 详情页区块 | `GET /shows/next-up`（含 `enableResumable:false` 去重语义 + 365 天 cutoff） | 缺 |
| 6 | **按库最新添加** | 首页按库分 section（每库一横滚） | `GET /items/latest?library=`（或复用 /items）+ 可选"隐藏已看" | 现有 /items 可凑合，建议专用端点 |
| 7 | **搜索** | ⌘K 面板 + 按 kind 分组结果 | `GET /search`（title/original_title LIKE + 拼音，分组返回） | 缺 |
| 8 | **筛选与排序扩展** | 库页筛选行（类型/年份/已看/收藏）+ 排序菜单 | `/items` 加 `genre/year/is_played/is_favorite/search` 参数 + `sort=sort_name/premiere/rating/random` + **`total_count`**（分页依赖） | 现 6 参数、3 排序、无总数 |
| 9 | **图片本地伺服** | 卡片/详情页图片走本端点 + blurhash 占位 | `GET /items/{id}/images/{type}?width=`（批次 C）；backdrop 三级回退逻辑放服务端或前端皆可 | 现在是 Bangumi 外链热链，必须改 |
| 10 | **刮削回填**：agent submit_result 扩展 overview/genres/people/多图 → ingest 写 0005 新表 | — | scrape.rs/ingest.rs 扩展（批次 A 尾巴） | ingest 只写 poster_path |

### P1 推荐（P0 后一个迭代）

- **演职员横滚**（详情页 People 卡，数据随 detail 输出即可；person 点击→`/items?person_id=` 下钻列表）；
- **媒体信息弹窗**（ffprobe JSON 已在库，纯前端渲染：编码/分辨率/码率/声道/语言/HDR）；
- **相似推荐**（`GET /items/{id}/similar`：item_values genre/tag 重合度 SQL；或走管家做 AI 推荐差异化）；
- **字母索引条**（`name_starts_with` 参数 + sort_name 拼音写入——刮削侧配合）；
- **动态筛选 facets**（`GET /items/filters?library=`：DISTINCT genres/years/tags）；
- **上下文菜单完整化**：重新识别（发管家/重刮任务）、删除（CanDelete + 二次确认 + 软删除已有）、复制文件路径；
- **播放器 OSD direct-play 全集**（§5 第一组 + 快捷键层，M3 拉流端点就绪后一次做齐）；外挂字幕加载与外观设置；UpNext 下一集倒计时；
- **`DateLastContentAdded` 排序**（剧集按最新有新集排——追番场景，需 ingest 维护 series 的该时间戳）；
- 详情页"结束于 hh:mm"、Overview 折叠、Series 未看集数角标（`UnplayedItemCount` 需按 series 聚合查询）。

### P2 远期

- 多选批量操作（批量标记/删除）；列表页字母数字快跳；视图切换（列表/海报卡多模式持久化）；
- 章节区块 + 章节缩略图 + 进度条章节刻度（ffprobe 章节，M3 后）；Trickplay 进度条预览（预生成雪碧图管线）；跳过片头/片尾（可做成 agent 检测特色）；
- 合集（BoxSet）、特典/附加部分、电影 Suggestions 推荐 tab、Upcoming 未播出虚拟集（`is_virtual` 列已备）；
- 画质/码率切换、音轨转码切换（M3 转码矩阵）；
- 首页 section 用户可配置（10 槽位模型）、用户偏好页族、多用户/家长控制；Jellyfin 兼容层（批次 E，api-surface.md 已列参数子集）。

### 明确不对标

LiveTV/DVR、音乐/歌词/InstantMix、书籍/照片库、SyncPlay、QuickConnect、Chromecast、插件市场、表单式元数据编辑器与管理台（由管家 + ConsoleView 审批流替代——nipa 的产品叙事核心）。

---

**执行建议**：本报告 P0 表与 docs/07 批次 B/C/D 完全咬合，可按"后端 B（端点 8 个）→ C（图片）→ 前端 D（详情页 → 首页 sections → 库页筛选排序 → 搜索）"排期；每项的字段需求、查询语义（played 判定常量、NextUp 去重、Latest 按库分组）、交互细节（Resume/Replay 双按钮、集列表行构成、hover 按钮组）均已在 §2-§6 给到实现精度。