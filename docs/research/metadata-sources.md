# 媒体刮削元数据源 API 调研

## 1. TMDB API v3

**API Key 申请**：注册 TMDB 账号 → 账号设置 → API 页面申请，填写应用信息后即时/快速发放，个人非商用免费。发放两种凭证：v3 API Key（query 参数 `?api_key=`）和 "API Read Access Token"（v4 格式 JWT，作为 `Authorization: Bearer <token>` header，v3/v4 通用）。**官方推荐使用 Bearer token**——v3 key 走 URL 会留在日志/代理记录中。

**关键 endpoint**（base：`https://api.themoviedb.org/3`）：
- `GET /search/movie?query=...&language=zh-CN&year=...&page=...`
- `GET /search/tv?query=...&language=zh-CN&first_air_date_year=...`
- `GET /movie/{movie_id}`、`GET /tv/{tv_id}`（详情，支持 `append_to_response=credits,images,external_ids` 一次拉取）
- `GET /tv/{tv_id}/season/{season_number}`（返回整季所有 episode）、`GET /tv/{tv_id}/season/{n}/episode/{m}`
- `GET /movie/{id}/images`、`GET /tv/{id}/images`
- `GET /configuration`：返回 `images.secure_base_url`（`https://image.tmdb.org/t/p/`）、`poster_sizes`/`backdrop_sizes` 等；图片 URL = `secure_base_url + size + file_path`，如 `https://image.tmdb.org/t/p/w500/xxx.jpg`。官方建议缓存 configuration，数天检查一次即可（实践中 base_url 几乎不变，可硬编码 + 定期刷新）。

**中文支持**：`language=zh-CN` 对元数据（标题/简介）支持良好，中文缺失时可用 `language=zh-CN` + fallback 或 `include_adult`/`append_to_response` 组合。**图片注意坑**：图片过滤应使用 `include_image_language=zh,null`（裸 ISO 639-1 代码 `zh`，而非 `zh-CN`——Kodi 社区实测 `zh-cn` 过滤取不到中文海报）；部分中文海报只标了 `zh-TW`，需要时可 `include_image_language=zh,null` 再自行按 `iso_639_1` 排序。

**限流**：旧的 40 req/10s 限制 2019 年底已取消；目前仅 CDN 层约 **50 req/s、每 IP 20 并发连接**（api 与 image.tmdb.org 相同），按 IP 计。自建服务端刮削建议自行节流（如 20-40 req/s 以内）并缓存。

**使用条款（对开源/商用）**：
- 非商用免费，但**必须署名**：在应用 About/Credits 中使用 TMDB 官方 logo + 声明数据来自 TMDB（"This product uses the TMDB API but is not endorsed or certified by TMDB"），链接指向 themoviedb.org；名称只能写 "TMDb" 或 "The Movie Database"。
- 商用（收费、广告、付费功能）需商业授权：2026 年 1 月官方口径为 $149/月订阅，或联系商务。开源免费分发项目按非商用处理（Jellyfin/Kodi 等即此模式，通常内置项目级 key）。
- 缓存限制：只允许"为提供服务所需的合理期限"内缓存元数据/图片；授权终止须删除全部缓存内容。禁止用 TMDB 内容训练 AI/ML。

## 2. Bangumi API（api.bgm.tv）

文档：https://bangumi.github.io/api/ ；OpenAPI 规范在 https://github.com/bangumi/api （open-api/v0.yaml）。

**关键 endpoint**（base：`https://api.bgm.tv`）：
- `POST /v0/search/subjects?limit=&offset=`（新版搜索，标注"实验性"）：body 为 JSON `{"keyword": "...", "sort": "match|heat|rank|score", "filter": {"type": [2], "air_date": [">=2020-07-01","<2020-10-01"], "tag": [...], "nsfw": false}}`。type：1=书籍 2=动画 3=音乐 4=游戏 6=三次元。`air_date` 过滤可用于按年份消歧。另有旧版 `GET /search/subject/{keywords}?type=2`（legacy，仍可用但不推荐）。
- `GET /v0/subjects/{subject_id}`：条目详情，含 `name`（原名）、`name_cn`（中文名）、`summary`、`date`、`rating`、`tags`、`infobox`（staff 等）、`images`（large/common/medium/small/grid，域名 lain.bgm.tv）。
- `GET /v0/subjects/{subject_id}/image?type=large`：302 跳转到封面图。
- `GET /v0/episodes?subject_id=&type=&limit=&offset=`：章节列表；episode type：0=本篇 1=SP 2=OP 3=ED 4=预告 5=MAD 6=其他。每话含 `sort`（全局话数）、`ep`、`name`、`name_cn`、`airdate`、`desc`。
- `GET /v0/subjects/{id}/persons`、`/characters`、`/subjects`（关联条目，可用于识别续季关系）。

**认证**：公开数据（搜索/条目/章节）**无需认证**；需要用户相关或 NSFW 内容时用 Access Token（在 https://next.bgm.tv/demo/access-token 生成，或走 OAuth），header `Authorization: Bearer <token>`。

**User-Agent 硬性要求**：必须设置包含开发者 ID + 应用名的 UA，开源项目须附项目主页，分发的应用须附版本号，例如 `yourname/nipaserver/0.1.0 (https://github.com/yourname/nipaserver)`。**默认 UA（reqwest 等库的默认值）可能被直接封禁**；禁止使用 `Bangumi/1.0`、`database` 这类 UA。

**限流**：无公开的具体数字，官方要求合理使用；实践中建议 1-2 req/s 以内并缓存。注意条目图片仅封面，没有 TMDB 那样的分季海报/剧照/背景图；分季在 Bangumi 中是独立条目（每季一个 subject id）。

## 3. 豆瓣

**现状**：豆瓣官方开放平台 API（api.douban.com v2）多年前已停止对外发放 key，**没有官方公开 API**。社区方案：

- **移动端 frodo API**：`https://frodo.douban.com/api/v2/movie/{id}?apiKey=054022eaeae0b00e0fc068c0c0a2102a`（从豆瓣官方 App 逆向提取的 apiKey，社区流传若干个）。Libitum/jellyfin-plugin-douban 走这条路线，key 可在插件配置中替换。风险：key 随时可能被轮换/封禁，部分接口还要求签名（HMAC）与特定 UA。
- **网页解析**：caryyu/jellyfin-plugin-opendouban 本身不含 apiKey，依赖独立部署的 douban-openapi-server（Python 爬虫）解析豆瓣网页；cxfksword/douban-api-rs 是 Rust 版等价物（Docker 镜像 ghcr.io/cxfksword/douban-api-rs），支持可选 `DOUBAN_COOKIE`（填登录 cookie 解决部分需登录才能搜索的条目）。Xzonn/JellyfinPluginDouban 则把网页解析内置在插件里，免额外容器。
- **风险**：豆瓣反爬激进——IP 限流、验证码、需登录 cookie，Libitum 插件曾被整体封禁（issue #29）；网页结构变更即失效；法律/ToS 上属于未授权抓取。**结论：豆瓣只适合作为可选、用户自担风险的数据源**，建议架构上做成可插拔 provider，并强制低速率 + 长缓存。

## 4. AniDB / AniList / TVDB 简评

- **AniDB**：数据最全的动画数据库之一，但 API 极不友好。UDP API：≤0.5 packet/s（长期 ≤1/4s），响应截断在 1400 字节，需注册 client；HTTP API（httpapi.anidb.net 的 anime dump）要求重度本地缓存，同一天重复请求同一数据即可能被 ban（ban 约 24h 衰减）。适合离线导入其 anime-titles dump 做标题匹配，不适合在线实时刮削。**不推荐作为主数据源**。
- **AniList**：GraphQL API（`https://graphql.anilist.co`），无需 key（公开数据匿名可查，用户数据走 OAuth2）。限流：正常 90 req/min，当前降级为 **30 req/min**（429 + `Retry-After`）。有中文别名但主要是英/罗马音/日文标题，`title.native` 可拿日文原名辅助 Bangumi 搜索。可用性好，适合做辅助源。
- **TVDB**：v4 API（`https://api4.thetvdb.com/v4`，JWT Bearer，key 在 dashboard 创建）。商业模式：协商授权（公司年收入 <$50k 免费但需署名，$50k-250k 收 $1000/年……）或"用户订阅"模式（用户各自花 $12/年买订阅拿 PIN 配合项目 key）。开源项目有折扣但**仍需申请谈判**，条款可随时变更。对个人开源项目来说门槛高于 TMDB，**优先级低**；TMDB 数据对剧集/动画已足够。

## 5. jellyfin-plugin-bangumi 匹配思路（参考）

kookxiang/jellyfin-plugin-bangumi 的做法：
1. **文件名解析**：内置一组针对国内字幕组命名习惯的正则（`番剧名 - 01`、`[番剧名][01]`、剔除 `[1080P][10bit][h264]`、CRC32 hash 等噪声）提取集数；另提供可选的 **AnitomySharp**（Anitomy 的 C# 移植）解析器，用于复杂命名下的标题/集数/SP 识别。Rust 侧对应物是 `anitomy-rs` / `anitomy` crate（Anitomy FFI 绑定）或纯 Rust 的 `anitomy-rust`。
2. **搜索匹配**：用解析出的标题调 Bangumi 搜索（type=2 动画），结合**播出年份/日期过滤**（`air_date` filter）消歧；新版引入 **Levenshtein 距离模糊匹配**提高非标准命名命中率。文件名多为罗马音时 bgm 条目可能搜不到，社区建议先经 TMDB/AniDB 拿到日文原名再搜。
3. **强制 ID 绑定**：支持在文件名/路径中嵌入 `[bangumi-<id>]`（或 nfo 中预置 bangumiid），有 ID 时跳过搜索直接按 ID 取数据——这是解决匹配错误的兜底手段，值得照抄。
4. **季/集映射**：Bangumi 每季是独立 subject，插件按目录层级（系列目录/季目录）分别匹配 subject，episode 用 `/v0/episodes?subject_id=` 的 `sort`/`ep` 与解析出的集数对齐；SP 归入 type=1 的章节。已知痛点：合集（1-12Fin+SP）命名、SP 小数集号（SP0.89）仍难精确匹配。

**对 nipaserver 的建议**：TMDB（电影/剧集主源，zh-CN）+ Bangumi（动画主源，天然中文）双 provider，anitomy 系 crate 做动画文件名解析，豆瓣做可选自担风险 provider；所有源强制本地缓存与节流，UA 按 Bangumi 规范设置。

## Sources
https://developer.themoviedb.org/docs/getting-started
https://developer.themoviedb.org/reference/authentication
https://developer.themoviedb.org/docs/authentication-application
https://developer.themoviedb.org/docs/rate-limiting
https://developer.themoviedb.org/docs/image-languages
https://www.themoviedb.org/api-terms-of-use
https://www.themoviedb.org/talk/681a1956bbbf46d7f66404f9
https://www.themoviedb.org/talk/654330c83e01ea00c6900d74
https://forum.kodi.tv/showthread.php?tid=329639
https://bangumi.github.io/api/
https://github.com/bangumi/api
https://raw.githubusercontent.com/bangumi/api/master/open-api/v0.yaml
https://github.com/bangumi/api/blob/master/docs-raw/user%20agent.md
https://github.com/caryyu/jellyfin-plugin-opendouban
https://github.com/Libitum/jellyfin-plugin-douban
https://github.com/Libitum/jellyfin-plugin-douban/issues/29
https://github.com/cxfksword/douban-api-rs
https://github.com/Xzonn/JellyfinPluginDouban
https://wiki.anidb.net/UDP_API_Definition
https://wiki.anidb.net/API
https://docs.shokoanime.com/shoko-server/understanding-anidb-bans
https://docs.anilist.co/guide/rate-limiting
https://github.com/thetvdb/v4-api
https://support.thetvdb.com/kb/faq.php?id=62
https://support.thetvdb.com/kb/faq.php?id=81
https://github.com/kookxiang/jellyfin-plugin-bangumi
https://github.com/kookxiang/jellyfin-plugin-bangumi/issues/53
https://github.com/kookxiang/jellyfin-plugin-bangumi/issues/33
https://kk.sb/2023/use-jellyfin-to-organize-anime-library.htm