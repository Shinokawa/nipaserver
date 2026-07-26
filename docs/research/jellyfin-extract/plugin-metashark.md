# MetaShark（jellyfin-plugin-metashark）精读报告 — 供 NipaServer search_douban 工具与聚合策略参考

**License 提示**：仓库根 `LICENSE` 为 **GPL-3.0**；内嵌的 `AnitomySharp/` 是 **MPL-2.0**（Anitomy 的 C# 移植）。本报告只提炼协议/参数/控制流，Rust 实现须独立编写，不可逐行翻译其 C# 代码。Anitomy 在 Rust 侧可用独立移植（如 crates.io 上的 anitomy 绑定或纯 Rust 端口），避开 GPL 传染。

关键文件（绝对路径）：
- 豆瓣抓取：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin-plugin-metashark/Jellyfin.Plugin.MetaShark/Api/DoubanApi.cs`
- 反爬 challenge：`.../Api/Http/DoubanSecHandler.cs`
- 文件名解析：`.../Core/NameParser.cs`
- 聚合/匹配核心：`.../Providers/BaseProvider.cs`、`MovieProvider.cs`、`SeriesProvider.cs`、`SeasonProvider.cs`、`EpisodeProvider.cs`
- TMDB 封装：`.../Api/TmdbApi.cs`；IMDB/OMDb 辅助：`.../Api/ImdbApi.cs`、`OmdbApi.cs`

---

## 1. 豆瓣数据获取方式

**结论：纯网页爬虫 + 一个公开 JSON suggest 接口。不用 frodo（豆瓣移动端 API）、无 apikey、无签名。** 全部靠 UA/Referer 伪装 + 可选用户 Cookie + 限速 + PoW 验证码求解。

### 端点清单（全部 GET）
| 用途 | URL | 解析方式 |
|---|---|---|
| 搜索（主） | `https://www.douban.com/search?cat=1002&q={urlencode(kw)}` | HTML，CSS 选择器 `div.result-list .result`，从 `div.title a` 的 `onclick` 里用正则 `sid: (\d+),` 抠出 subject id；类别取 `div.title>h3>span` 的 `[电影]`/`[电视剧]`；年份/原名从 `div.rating-info>span:last-child` 正则提取（`原名[:：](.+?)\s*?/`） |
| 搜索（防封备选） | `https://www.douban.com/j/search_suggest?q={kw}` | JSON，`cards[]`，过滤 `type=="movie"`，字段 sid/title/year。排序质量差、不分电影/剧，但访问代价低 |
| 条目详情 | `https://movie.douban.com/subject/{sid}/` | HTML。标题优先取 `<meta name="keywords">` 第一个逗号段，退回 `<title>` 去掉"(豆瓣)"；原名 = `h1>span:first-child` 文本去掉中文名后剩余；`#info` 区块整段取文本后用行正则抽：`导演: (.+?)\n`、`编剧:`、`主演:`、`类型:`、`制片国家/地区:`、`语言:`、`片长:`、`(上映日期|首播): `、`又名:`、`IMDb: (tt\d+)`、`官方网站:`；评分 `div.rating_self strong.rating_num`；简介 `div#link-report-intra>span.all`（去"©豆瓣"、压缩空白）；**是否剧集判据：页面存在 `div.episode_list` → 电视剧，否则电影** |
| 演职员 | `https://movie.douban.com/subject/{sid}/celebrities` | HTML，`div#celebrities>.list-wrapper`，只保留 h2 含"导演/演员"的分组；头像从 `div.avatar` 的 style `url(...)` 抠 |
| 剧照/背景图 | `https://movie.douban.com/subject/{sid}/photos?type=W&start=0&sortby=size&size=a&subtype=a` | HTML `.poster-col3>li`，`data-id` + img src 抠出 img host（`//(img\d+)\.`，默认 img2），再自行拼四档 URL：`https://{host}.doubanio.com/view/photo/{s|m|l|raw}/public/p{id}.jpg`；尺寸取 `div.prop` 文本 `WxH` |
| 人物 | `https://www.douban.com/personage/{id}/`（旧 7 位 celebrity id 先 GET `movie.douban.com/celebrity/{id}/` 不跟随重定向，从 Location 拿新 id） | HTML `ul.subject-property>li` 按 label 分发（性别/星座/出生日期/去世日期/生卒日期/出生地/职业/更多外文名/IMDb编号） |
| Cookie 有效性检测 | `https://www.douban.com/mine/` | 跳到 `accounts.douban.com`/`sec.douban.com` 或 URL 含 `/login` → 未登录 |

海报小图→大图：`Img.replace("s_ratio_poster","m")` / `"l"`。

### 请求头 / Cookie
- UA 固定：`Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/93.0.4577.63 Safari/537.36 Edg/93.0.961.44`
- 默认头：`Origin: https://movie.douban.com`、`Referer: https://movie.douban.com/`；suggest 接口改为 `www.douban.com`。
- Cookie：不做登录流程，由用户从浏览器复制整串 cookie 粘贴到配置；按 `;` 拆 `k=v`，全部落在 domain `.douban.com` path `/`。配置变更时把旧 cookie 全部过期后重加。
- **图片下载必须带 `Referer: https://www.douban.com/`**（doubanio.com 有防盗链），插件为此专门做了 `/plugin/metashark/proxy/image?url=` 代理端点转发给 Jellyfin 客户端。NipaServer 同样需要一个图片代理/落盘下载时带 Referer。
- TLS 证书校验被关闭（`ServerCertificateCustomValidationCallback => true`），并开 gzip/deflate 解压。

### 反爬对策（数值可直接抄）
- **限速（进程内令牌限流）**：
  - 默认（未开防封）：1 次 / 200ms；
  - 防封模式・无 cookie（游客）：复合限制 10 次/分钟 **且** 1 次/5s（注释：超了 5 分钟后封 IP）；
  - 防封模式・有 cookie（已登录）：20 次/分钟 且 1 次/3s（超了触发机器人检验）。
  - 封 IP 约 **6 小时**恢复（README）。
- **内存缓存**：搜索结果 5 分钟；详情/演职员/图片/人物 30 分钟；只在结果非空时缓存（搜索）。TMDB 侧统一 1 小时，季接口失败也缓存 30s 防抖。
- **sec.douban.com PoW 验证**（`DoubanSecHandler`，一个响应拦截层）：请求被重定向到 `sec.douban.com` 且页面含 `name="cha"`/`name="tok"` 表单时 → 取 `tok`、`cha`、`difficulty`（默认 4），暴力找 nonce 使 `sha512_hex(cha + nonce)` 前缀有 difficulty 个 `0`，然后 `POST https://sec.douban.com{form action，默认 /c}`，form 体 `tok/cha/sol`，带原请求为 Referer；成功后 cookie 会被写入 jar，**重试原请求一次**。
- **降低搜索页访问**：防封模式下有年份时先走 `j/search_suggest`（代价低），命中年份就不打搜索页。

### 失败模式（可写进工具错误分支）
- 搜索页 body 含 `"sec.douban.com"` → 触发风控/IP 被封；
- 搜索有响应但解析出 0 条 → 大量出现时说明触发爬虫风控；
- 详情页取不到 `#content` → 防爬或页面结构变更；
- 结果含"尚未播出"的条目直接跳过；
- 豆瓣页的 IMDb id 可能是旧的：先打 OMDb（`https://www.omdbapi.com/?i={tt}&apikey=...`）换新 id，或 GET `https://www.imdb.com/title/{tt}/` 不跟随重定向从 Location 取新 tt。

---

## 2. 豆瓣 × TMDB 聚合策略

**总原则：豆瓣是中文元数据主源，TMDB 是"结构与补全"源。** 配置项：`EnableTmdb`（总开关）、`EnableTmdbMatch`（豆瓣失败后回退）、`EnableTmdbSearch`（搜索结果并列展示，默认关）、`EnableTmdbBackdrop/Logo/Collection/OfficialRating`。

### 主源选择
- 自动匹配顺序：先 `GuessByDouban`；豆瓣找不到才回退 TMDB（记录 metaSource=Tmdb）。元数据来源用 `{Douban|Tmdb}_{id}` 复合 ProviderId 记住，刷新时不换源。
- 文件名可强制指定：`[douban-12345]`/`[doubanid-12345]`、`[tmdb-12345]`/`[tmdbid-12345]`。

### 豆瓣命中后仍要串 TMDB 的链路（关键）
1. 豆瓣详情抠到 IMDb id → OMDb 校验换新 → `TMDB /find?external_source=imdb_id` 得 tmdbId（电影取 movie_results[0]；剧集依次 tv_results / tv_episode.show_id / tv_season.show_id）；
2. 没有 IMDb 时用 `豆瓣中文名 + 年份` 搜 TMDB 兜底；
3. 拿到 tmdbId 后补：**电影系列 Collection 名、官方分级（按国家码，us 无前缀、其他 `{cc}-` 前缀、de→FSK）、背景图备选、Logo（豆瓣没有 logo，只能 TMDB）**。

### 字段归属（豆瓣主源时）
- 豆瓣提供：中文标题、原名、评分（CommunityRating）、简介、年份、类型（`/` 拆分）、首映日（"上映日期/首播"第一个日期，去括号地区）、导演/演员（含中文角色名，导演最多 5 个、演员上限 15）、海报、剧照背景图（筛 `width∈[1280,4096] 且 w>1.3h` 的横图）。
- TMDB 提供：剧集（Episode）全部数据、季数据兜底、Collection、分级、logo、backdrop 备选、外部 id（imdb/tvdb/tvrage）。
- TMDB 主源时才用 TMDB 的 overview/tagline/studios/genres 等全套。

### 搜索匹配规则（两边一致的择优阶梯，值得进 agent prompt）
- TMDB 电影：名称完全相等（Title 或 OriginalTitle）优先 → 否则取第一个（作者注释：BT 资源多为英文名，中日韩泰印法影片不适合做相似度阈值过滤，故不设阈值）。
- TMDB 剧集：名称+首播年同时匹配 → 仅年份 → 仅名称 → 第一个。
- 豆瓣：有年份时"类别+年份"匹配，**匹配不到就返回 None 让下游接手（不硬选第一个）**；无年份时取该类别第一个。代码里留有被注释掉的 JaroWinkler>0.8 相似度方案（被放弃）。
- 图片语言优先级：默认语言 > 无语言 > en；若首选语言无图且备选是日语，把第一张日语图 Language 置空提权。TMDB 语言码规范化：`zh-cn → zh-CN`（连字符后必须大写）；image language 参数附加 `null` 和 `en`。

---

## 3. 文件名解析：自研 NameParser 包装 AnitomySharp

结构：`NameParser.Parse(fileName, isEpisode)` = **AnitomySharp（动漫向 tokenizer）为主 + Jellyfin 原生 Emby.Naming 解析器兜底**，输出 `ParseNameResult{ Name, ChineseName, Year, ParentIndexNumber(季), IndexNumber(集), AnimeType, IsAnime }`。

控制流（可直接移植为 Rust 的 pipeline）：
1. 预处理：`第 X 集/话/期` 中间空格去掉（否则 Anitomy 切错）。
2. Anitomy 解析，按 element 分发：AnimeTitle→标题；AnimeSeason→季；EpisodeNumber→先试年份正则 `[12][890]\d\d`（Anitomy 会把年份误判为集号），否则为集号；AnimeType（SP/OVA/PV…）；AnimeYear→年。
3. **中英混合标题拆分**：标题按首个空格/`.` 切两段，前段含中文且后段不含中文且后段不以 `-～~` 开头 → ChineseName=前段、Name=后段（如 `V字仇杀队.V.for.Vendetta`）。
4. 标题清洗：去尾部 ` S\d{1,2}`、去 `[...]``(...)``【...】`、`.`→空格。
5. 修补链（依序，仅缺失时触发）：动漫 `[SXX]`/`.SXX.` 抠季号 → 非动漫且无年份时用 Jellyfin 默认电影解析器（**先删掉分辨率 `\d{3,4}x\d{3,4}` 防止当年份**）→ 集号缺失依次试 Anitomy volume、纯数字标题、中文集号 `第([0-9零一二三四五六七八九]+)(集|章|话|話|期)`（含汉字数字转换）、`ep(\d{1,2})`。
6. `IsAnime` 启发式（借鉴 nas-tools）：`【xx】【`、` - 12 `、`S01`/`EP01-EP12` 等序列格式、`[xx][`、`[.+].*?[.+]` 任一命中。
7. 特典/花絮目录：父目录名 `SP/SPs/SPECIALS/含"特典"` → special（季=0）；`EXTRA/MENU(S)/PV/PV&CM/CM/BONUS/含OPED/NCED/花絮` → extra（直接跳过刮削或只改名）；AnimeType 非 SP/OVA/TV 也视为 extra。
8. 季号目录猜测（`GuessSeasonNumberByDirectoryName`）：中文 `第X季/部` → SxxSeason 标准解析 → 动漫罗马数字/序数词映射（`I|1st→1, II|2nd→2, III|3rd→3, IIII|4th→4`）。
9. 电影取名来源：影片在独立文件夹且文件夹名包含 info.Name 时用**文件夹名**，否则用文件名（不含扩展名）。

---

## 4. 电影 vs 剧集识别路径差异

**电影（MovieProvider）**：文件名 → NameParser（ChineseName 优先作搜索词）→ 豆瓣搜索过滤 `Category=="电影"` → 详情 + 演职员 → IMDb→OMDb→TMDB 串联补全（Collection/分级）。extras 在电影目录内直接返回空元数据忽略。

**剧集（三层各自独立，路径完全不同）**：
- **Series**：同电影流程但过滤 `Category=="电视剧"`；豆瓣的剧名带"第X季"后缀要剥掉（`\s第[0-9一二三四五六七八九十]+?季$|\sSeason \d+$|结尾孤立数字`）作为剧名。
- **Season（豆瓣特色难点）**：**豆瓣把每一季当独立条目（独立 sid），TMDB 把季挂在剧下** ——聚合的核心错位。猜季 sid 的顺序：① 季目录名 `[douban-xxx]` 属性；② 用 TMDB 的 `season.air_date.year` 反查豆瓣"剧名+该年份"（还会校验豆瓣条目名里的"第X季"和季号一致，防多季同年错配）；③ 直接搜"剧名+N"或"剧名 第N季"（第一季用裸剧名）精确匹配。豆瓣拿不到季时回退 TMDB 季数据。虚拟季（无季文件夹）季号默认为 1。
- **Episode：完全只用 TMDB**（豆瓣没有分集数据）。流程：特典/extra 处理 → `FixParseInfo` 用 Anitomy 重解文件名修正季/集号（处理 SXXEPXX、虚拟季、季目录猜测）→ 需要 `seriesTmdbId + season + episode` 三元组 → 支持 TMDB **Episode Groups**（Jellyfin 的 displayOrder：originalAirDate/absolute/dvd/digital/storyArc/production/tv），组内 season=Order、episode=Order+1 再映射回真实 SxxExx；否则直接取 `GetSeason` 缓存里的第 N 集。缺三元组时仍返回 `HasMetadata=true` 只带文件名解析出的季/集号（保底不阻塞入库）。

---

## 5. 给 NipaServer 的建议

### search_douban tool（可选 feature，风险最低姿势）
1. **默认走 `j/search_suggest` JSON 接口**，只有需要区分电影/剧或 suggest 无结果时才打搜索页 HTML —— 这是 MetaShark 防封模式的经验反转：suggest 代价低得多。详情页无 JSON 替代，必须 HTML。
2. **限速取保守档**：无 cookie 时 ≤10 次/分钟且相邻 ≥5s；建议在 Rust 侧用 `governor` 之类做进程级限流，工具层强制串行。豆瓣封 IP ~6 小时，代价高。
3. **缓存必须做**（sqlx 落库比内存缓存更适合 server）：搜索 ≥5 分钟、详情 ≥30 分钟；agent 刮削场景同一条目会被多轮对话反复查，建议详情缓存拉长到天级 + 永久保存已确认的 sid→metadata 映射。
4. **Cookie 作为可选配置**直接透传整串浏览器 cookie（按 `;` 拆到 `.douban.com`），不实现登录流程；提供 `GET /mine/` 探活判断失效。
5. **sec.douban.com PoW 可以先不实现**：限速足够保守时很少触发；实现的话就是 sha512(cha+nonce) 前缀 difficulty 个 0 的暴力求解 + POST tok/cha/sol，Rust 里 trivial。第一版遇到 sec 重定向直接报"触发风控，稍后重试"即可。
6. **HTML 解析用 `scraper` crate**，选择器与正则清单照第 1 节表格移植；重点鲁棒性：`#content` 缺失即报结构变更错误；标题从 meta keywords 取（比 h1 稳）。
7. **图片必须带 Referer 下载**；NipaServer 建议刮削时直接落盘（server 端下载），不需要 MetaShark 那种给客户端用的代理端点。
8. 工具返回结构建议对齐 `DoubanSubject`：`sid, name, original_name, year, rating, category(电影/电视剧), genres[], intro, screen_date, imdb_id, country, language, actors, directors, poster_url`——其中 `imdb_id` 是打通 TMDB 的关键字段，务必返回给 agent。
9. TMDB 侧注意：插件里硬编码了公共 apikey（`4219e299c89411838049ab0dab19ebd5`）和 OMDb key（`2c9d9507`）——**不要抄**，NipaServer 用户自配 key；TMDB 语言码 `zh-CN` 连字符后大写；支持自定义 api host（国内可换代理域名）+ 可配 http/socks 代理。

### 值得写进 agent system prompt 的聚合规则
- 「中文标题/简介/评分/演员中文名 → 优先豆瓣；集结构（季/集列表、每集标题简介、air date）→ 只信 TMDB；logo/collection/分级 → 只有 TMDB 有。」
- 「豆瓣→TMDB 打通首选 IMDb id（豆瓣详情页有）：`search_tmdb` 支持 external id 查找的话优先用它，其次中文名+年份搜索。」
- 「豆瓣每一季是独立条目：剧集季的豆瓣数据要用 `剧名 第N季` 或 TMDB 该季首播年份去搜；剧名末尾的『第X季/数字』要剥掉再当剧名。」
- 「有年份时必须年份匹配，匹配不到宁可返回未找到，不要选第一个；无年份才允许取首个结果。名称完全相等 > 年份匹配 > 第一个。」
- 「文件名含 `[douban-123]`/`[tmdb-456]` 属性时无条件采用。」
- 「动漫文件名用 anitomy 类解析；父目录为 SP/特典→season 0，EXTRA/PV/CM/NCED/花絮→跳过刮削。」
- 「BT 资源常见『中文名.English.Name.2020...』格式：取中文段搜豆瓣、英文段搜 TMDB。」