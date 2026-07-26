# Jellyfin 全面对标实施计划

> 2026-07-26。依据：`research/jellyfin-full/` 五份结构精读 + jellyfin-web UI 审计（进行中）。
> 原则：**基础功能全对标，形态按 nipa 的 agentic 理念重构**（元数据编辑走管家/审批而非表单堆砌）。

## 批次 A：数据地基（schema 0005）

**items 增列**（entity-model 报告"必备"级）：
`sort_name`（排序名，中文转拼音首字母/日文罗马音由刮削写入，兜底 title）、`end_date`、`runtime_ms`（元数据时长，来自 ffprobe/provider）、`official_rating`、`series_status`（Continuing/Ended，series 行）、`is_virtual`（虚拟季/缺失集占位）、`series_id`（episode 行冗余，免两跳 JOIN）、`tagline`、`date_modified`。

**新表**：
- `item_values(id, type, value)` + `item_value_map(item_id, value_id)`——genre/studio/tag 统一表（Jellyfin ItemValue 验证过的结构）；
- `people(id, name, kind, image_url, provider_ids)` + `item_people(item_id, person_id, role, sort_order)`——声优（Role=角色名）/监督/系构；
- `item_images(item_id, image_type, url, local_path, width, height, blurhash)`——多图类型（Primary/Backdrop/Thumb/Logo），poster_path/backdrop_path 保留为快捷列；
- `watch_history` 增列：`played`、`play_count`、`is_favorite`、`last_played_at`、`audio_stream_index`、`subtitle_stream_index`。

**submit_result schema 扩展**：overview、genres[]、studios[]、people[]（agent 从 provider 详情一并带回——agentic 刮削的优势：一次任务拿全）。

## 批次 B：用户数据与三大首页查询

- **played 判定**（照抄语义）：MinResumePct=5 / MaxResumePct=90 / MinResumeDuration=300s；只置 true 不置 false；
- `POST /api/v1/playback/progress`（start/progress/stop 三合一简化版，PlayMethod 字段预留）；
- `POST/DELETE /api/v1/items/{id}/played`、`/favorite`；
- `GET /api/v1/items/resume`（position>0，DatePlayed DESC）；
- `GET /api/v1/shows/next-up`（两阶段：最近在看的剧 → 每剧下一集）；
- `GET /api/v1/items/latest`（按库分组）；
- `GET /api/v1/search`（标题/原名 LIKE + 拼音前缀，score 排序）；
- `/api/v1/items` 参数扩展：`search`、`genre`、`year`、`is_played`、`is_favorite`、`sort=sort_name|premiere|rating|random`、`total_count` 响应头。

## 批次 C：图片本地缓存

- `data/images/{item_id}/{type}.jpg` 下载缓存（首次请求时拉取 + 后台预取队列）；
- `GET /api/v1/items/{id}/images/{type}?width=`（image crate 缩放，缓存缩放结果）；
- blurhash 生成（blurhash crate）→ item_images.blurhash → WebUI 占位。

## 批次 D：WebUI 对标（等 jellyfin-web 审计定稿后细化）

- **详情页升级为完整页面**（路由 #/item/{id}）：backdrop 区 + 元信息行（年份/时长/分级/评分/类型标签）+ 简介 + 播放按钮组 + 季选择 Tab + 集列表（缩略图/简介/进度条/已看勾）+ 演职员横滚 + 外部链接；
- 首页 sections 化：继续观看/Next Up/各库最新添加；
- 库浏览：筛选器行（类型/年份/已看状态）+ 排序切换 + 字母索引 + 无限滚动；
- 条目上下文菜单：标记已看/收藏/重新识别（走管家）/查看识别过程；
- 搜索页（⌘K 已有壳）。

## 批次 E：Jellyfin 兼容层（§8.2 范围，M5 提前起步）

- `/Users/AuthenticateByName`、`/System/Info/Public`、`/UserViews`、`/Items`（参数子集映射）、`/Users/{id}/Items`（旧路由）、`/Shows/{id}/Seasons|Episodes`、`/Items/{id}/Images/{type}`、`/Videos/{id}/stream`（Direct Play）、Playstate 三端点、Branding/DisplayPreferences/QuickConnect stub；
- 响应形状 `{Items, TotalRecordCount, StartIndex}`；未知参数安全忽略。

## 执行顺序

A → B（依赖 A 的列）→ C 与 D 并行 → E。刮削侧同步：worker prompt 与 submit_result 扩展（A 的一部分）、入库 ingest 写新表。
