# Jellyfin 图片系统精读 → nipaserver 对标报告

（来源：/Users/sakiko/Desktop/nipaserver/reference/jellyfin，GPL 代码，本报告只提炼协议/逻辑，不做代码翻译）

## 1. ImageType 全枚举与各实体典型拥有的图

`MediaBrowser.Model/Entities/ImageType.cs`（枚举值即 API 路径里的字符串）：

| ImageType | 值 | 说明 | 是否允许多张 |
|---|---|---|---|
| Primary | 0 | 海报/封面（竖版 2:3；episode 为横版截图） | 否 |
| Art | 1 | ClearArt（透明修饰图） | 否 |
| Backdrop | 2 | 横版背景图（16:9） | **是（唯一允许多张的展示图）** |
| Banner | 3 | 横幅（超宽 1000x185） | 否 |
| Logo | 4 | 透明台标/片名 Logo | 否 |
| Thumb | 5 | 横版缩略图（landscape 16:9） | 否 |
| Disc | 6 | 光盘图 | 否 |
| Box | 7 | 盒装图 | 否 |
| Screenshot | 8 | 已废弃 | - |
| Menu | 9 | 菜单图 | 否 |
| Chapter | 10 | 章节缩略图 | 是 |
| BoxRear | 11 | 盒背图 | 否 |
| Profile | 12 | 用户头像 | 否 |

单张类型集中定义在 `MediaBrowser.Providers/Manager/ItemImageProvider.cs` 的 `_singularImages`（Primary/Art/Banner/Box/BoxRear/Disc/Logo/Menu/Thumb）；`BaseItem.AllowsMultipleImages` 只对 Backdrop 和 Chapter 返回 true。

各实体典型拥有的图（TMDB provider 的 `GetSupportedImages`，`MediaBrowser.Providers/Plugins/Tmdb/`）：
- **Series / Movie**：Primary + Backdrop + Logo + Thumb
- **Season**：仅 Primary（季海报）
- **Episode**：仅 Primary（横版剧照；无远程图时由 ffmpeg 从视频 10% 处截帧，见 `MediaBrowser.Providers/MediaInfo/VideoImageProvider.cs`）
- **Person**：Primary
- **BoxSet（合集）**：Primary + Backdrop + Logo + Thumb

每类图的默认下载策略（`MediaBrowser.Model/Configuration/TypeOptions.cs` 的 `DefaultImageOptions`）：Movie/Series 默认 Primary=1、Backdrop=1（MinWidth 1280）、Logo=1、Thumb=1，Art/Disc/Banner 默认 0（不下载）。`Limit`（每类最多几张）+ `MinWidth`（远程图最小宽度过滤）是 per-type 可配置的两个参数——这个"按类型限量+最小宽度"的模型很值得抄。

`RemoteImageInfo`（provider 返回的候选图元数据）字段：Url、ThumbnailUrl、Width/Height、CommunityRating、VoteCount、Language、Type、ProviderName——选图时按语言匹配和评分排序，backdrop 优先无语言版本（`listWithNoLangFirst`）。

## 2. 存储/缓存策略（原图 + 缩放缓存两层）

### 2.1 原图存哪里（ImageSaver.GetStandardSavePath）
两条路径：
- **saveLocally（媒体目录旁）**：如 `poster.jpg`、`backdrop.jpg`/`backdrop1.jpg`、`landscape.jpg`（Thumb）、`clearart.png`（Art）、`logo.png`；Season 特殊命名 `season01-poster.jpg`/`season-specials-poster.jpg`；Episode 的 Primary 存为 `<视频文件名>-thumb.jpg`。这是 Kodi/NFO 兼容约定。
- **内部 metadata 目录（默认路径）**：`{data}/metadata/library/{id前2位}/{id}/poster.jpg` 等（`BaseItem.GetInternalMetadataPath`：`Path.Join(basePath, "library", idString[..2], idString)`——两位前缀分桶避免单目录过多子目录）。

关键点：**数据库不存图片二进制，只存 ItemImageInfo{Path, Type, DateModified, Width, Height, BlurHash}**。Path 可以是本地路径也可以是 http URL（`IsLocalFile => !Path.StartsWith("http")`）——远程 URL 可作为"stub"先存着（`SaveImageStub`，多个候选 URL 用 `|` 连接），首次被请求时 `LibraryManager.ConvertImageToLocal` 才真正下载落地（惰性下载，失败则移除该图防止反复重试）。

### 2.2 什么时候刷新
`ItemImageProvider.RefreshImages`：
- 已有图的类型默认**不重复下载**（`ContainsImages` 检查：每个 singular 类型有图即跳过；backdrop 数量达到 limit 即跳过）；
- 只有 `ReplaceImages/ReplaceAllImages`（用户手动"替换图片"刷新）才重下；替换 backdrop 时先记旧图，新图下载成功后才删旧图（"只有新图到手才删旧图"防止刷新失败导致丢图）；
- 本地图（媒体目录里的 poster.jpg）优先于远程图，`MergeImages` 用文件 mtime 变化判断是否需要重新计算尺寸；
- 下载时对 404/403 跳过该 URL 继续下一张，其他非 2xx 直接放弃该 provider 剩余请求。

### 2.3 缩放缓存（ImageProcessor）
`{cache}/images/resized-images/{md5前1位}/{md5}.{ext}`。缓存 key = 原图路径 + quality + **原图 mtime(ticks)** + 输出格式 + width/height/maxWidth/maxHeight/fillWidth/fillHeight + percentPlayed/unplayedCount/blur/backgroundColor/foregroundLayer + 版本号 `v=3`（逻辑变更时递增全量失效）拼串取 MD5。原图 mtime 进 key ⇒ 原图更新后旧缓存自然失效、无需主动清理。命中即直接回文件；并发编码用信号量限流（默认 CPU 核数）防内存爆。

## 3. ImageController 伺服参数（`Jellyfin.Api/Controllers/ImageController.cs`）

端点：`GET/HEAD /Items/{itemId}/Images/{imageType}[/{imageIndex}]`，query 参数：

| 参数 | 语义 |
|---|---|
| width / height | 固定宽/高（只给一个则按比例算另一个） |
| maxWidth / maxHeight | 上限（等比缩小到不超过） |
| fillWidth / fillHeight | 装进目标盒（`DrawingUtils.ResizeFill`：取 min(widthRatio,heightRatio) 等比缩小填满盒，**只缩不放大**，scaleRatio<1 时返回原尺寸） |
| quality | 0-100，默认 90 |
| format | 强制输出格式（Bmp/Gif/Jpg/Png/Webp/Svg）；不给则按 Accept 头协商：webp 优先 → 需透明则 png → jpg 兜底 |
| tag | 客户端带上 item 的 imageTag 则响应 `Cache-Control: public, max-age=31536000, immutable` + ETag；tag = MD5(item路径+图片mtime.Ticks) |
| percentPlayed / unplayedCount / blur / backgroundColor / foregroundLayer | 叠加层（播放进度条/未看数角标/模糊/底色/前景蒙层）——Jellyfin 特有，客户端现在多半自绘，可不抄 |

伺服逻辑要点：
- **默认参数短路**：若 quality>=90 且无任何缩放/叠加参数且格式已支持 → 直接回原文件不重编码（`ImageProcessingOptions.HasDefaultOptions`）；
- 响应带 Last-Modified / ETag / Age / `Vary: Accept`，支持 If-None-Match 与 If-Modified-Since 返回 304；
- 列表 API（BaseItemDto）里每个 item 带 `ImageTags: {Primary: tag, ...}`、`BackdropImageTags: [...]`、`ImageBlurHashes`——客户端由 tag 拼图片 URL 实现不可变缓存；
- 尺寸链：先 `Resize`（width/height/maxWidth/maxHeight）再 `ResizeFill`（fillWidth/fillHeight）。
- 另有 `GET /Items/{id}/Images`（列出图片元数据：type/index/width/height/size/blurhash/tag）与 POST/DELETE 上传删除端点。

## 4. 给 nipaserver 的设计（对照现状：items.poster_path/backdrop_path 存 Bangumi 直链 URL，`/api/v1/items` 直接吐 URL 给前端）

现状问题：a) 单列只能一种海报，episode 剧照/logo 无处放；b) 前端直连 api.bgm.tv——裸奔 referer/限流风险、离线不可用、每次全量重下；c) Bangumi 302 端点 URL 无 mtime/tag 概念，客户端缓存失效策略缺失。

### 必备（P0：多图建模 + 本地缓存 + 基本伺服）

**(1) item_images 表**（替代 poster_path/backdrop_path 两列，迁移时把现有两列搬进来）：

```sql
CREATE TABLE item_images (
  id INTEGER PRIMARY KEY,
  item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  image_type TEXT NOT NULL CHECK(image_type IN ('primary','backdrop','logo','thumb','banner')),
  idx INTEGER NOT NULL DEFAULT 0,          -- 仅 backdrop 允许 >0
  source_url TEXT,                          -- 远程原始 URL（stub，可先存 URL 后惰性下载）
  provider TEXT,                            -- bangumi|tmdb|local|extracted
  local_path TEXT,                          -- 下载落地后的相对路径；NULL = 尚未落地
  width INTEGER, height INTEGER,
  file_size INTEGER,
  modified_at INTEGER,                      -- 落地文件 mtime，参与 tag
  blurhash TEXT,                            -- 推荐项，可先留 NULL
  UNIQUE(item_id, image_type, idx)
);
```
Jellyfin 的两个约束抄过来：除 backdrop 外每类每 item 唯一一张；"URL stub → 首次请求时下载转本地，下载失败（404/403）删记录防重试风暴"。

**(2) 本地目录结构**（照抄 Jellyfin 内部 metadata 布局，避免媒体目录写入）：
```
{data}/metadata/items/{id % 100 或 id 前缀}/{item_id}/poster.jpg
                                              backdrop.jpg / backdrop1.jpg
                                              logo.png / thumb.jpg
{cache}/images/resized/{hash[0]}/{hash}.webp   -- 缩放缓存
```

**(3) 图片端点**：`GET /api/v1/items/{id}/images/{type}[/{idx}]`，P0 参数只做 `maxWidth/maxHeight/quality/format` 四个（覆盖海报墙 90% 需求），语义照 Jellyfin：不带参数且原图可用 → 直接回原文件。列表 API 的 poster_path 字段改为吐本服务的 `/api/v1/items/{id}/images/primary?tag=xxx`（或前端约定拼 URL），tag = hash(local_path + mtime)，响应带 `Cache-Control: immutable` + ETag/304。

**(4) 下载策略**：ingest 时（现在 `ingest.rs` 的 `backfill_bangumi_poster` 写 URL 的位置）改为写 item_images 的 source_url stub；后台任务或首次请求时下载落地。Bangumi 的 `/v0/subjects/{id}/image?type=large` 是 302，下载时跟随重定向、以最终 Content-Type 定扩展名（Jellyfin 对 octet-stream 会回退从 URL 猜 mime，这个坑要防）。**已有图不重下**（按 (item_id,type,idx) 存在且 local_path 非空即跳过），只有显式"刷新图片"操作才替换，且新图到手才删旧图。

**(5) 缩放实现**：Rust 侧用 `image` crate（jpeg/png 解码 + `resize` / `thumbnail`）+ `webp` 或直接 `image` 的 webp 编码即可；缓存文件名照 Jellyfin：所有参数+原图 mtime+版本号拼串取 hash，一级子目录按 hash 首字符分桶。并发限流用 `tokio::sync::Semaphore`（CPU 核数）。**不建议纯透传**：透传解决不了离线/限流，且 Bangumi 原图 ~1MB，海报墙一屏几十张必须有缩略图。

### 推荐（P1）
- **format 协商**：按 Accept 头选 webp/jpg（Jellyfin 顺序：webp → 需透明 png → jpg），响应 `Vary: Accept`；
- **blurhash**：落地时计算（xComp/yComp ≈ sqrt(16*w/h) 取整+1、上限 9，`blurhash` crate 有现成实现），列表 API 随 item 吐出，前端加载占位体验质变；
- **fillWidth/fillHeight**：装盒缩放（只缩不放），Jellyfin 客户端海报墙用它；
- **TMDB 图源**：series/movie 的 backdrop/logo Bangumi 基本没有，需要 TMDB `/tv/{id}/images`（item_ids 已存 tmdb id）；选图逻辑抄 RemoteImageInfo 排序：语言匹配(zh) > vote_average，backdrop 优先无文字（`iso_639_1 == null`）；MinWidth=1280 过滤小图；
- **episode 剧照**：TMDB still（`/tv/{id}/season/{n}/episode/{m}/images`）优先，缺失时 ffmpeg 从 10% 时长处截帧（Jellyfin `VideoImageProvider` 的做法，nipaserver 反正要依赖 ffmpeg 转码）；
- **`GET /api/v1/items/{id}/images` 元数据列表端点**（type/idx/width/height/size/blurhash/tag），管理前端选图用。

### 远期（P2）
- 用户上传/替换图片（POST/DELETE 端点 + base64 body）；
- 每类图 Limit/MinWidth 可配置（TypeOptions 模型）；
- percentPlayed/unplayedCount/blur 服务器端叠加层——建议永远不做，前端 CSS 自绘即可；
- Chapter 缩略图、Person 头像、合集（BoxSet）collage 封面；
- 本地媒体目录图片扫描（poster.jpg/fanart.jpg Kodi 约定）作为最高优先级图源——若目标用户有现存刮削库则提升到 P1。

## 相关文件
- 枚举：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/Entities/ImageType.cs
- 刷新/下载：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Providers/Manager/ItemImageProvider.cs
- 落地命名：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Providers/Manager/ImageSaver.cs
- 伺服：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Controllers/ImageController.cs
- 缩放缓存：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/src/Jellyfin.Drawing/ImageProcessor.cs、MediaBrowser.Model/Drawing/DrawingUtils.cs
- 默认限量：/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/Configuration/TypeOptions.cs
- nipaserver 现状：/Users/sakiko/Desktop/nipaserver/crates/nipa-server/migrations/0001_init.sql（items 表 L45-63）、/Users/sakiko/Desktop/nipaserver/crates/nipa-server/src/ingest.rs（L80-141 海报回填）、/Users/sakiko/Desktop/nipaserver/crates/nipa-server/src/api_library.rs