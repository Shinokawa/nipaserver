# jellyfin-plugin-bangumi 精读报告（供 NipaServer AI 刮削工具层移植）

许可证事实：仓库根 `LICENSE` 为 **GPL-2.0**（`/Users/sakiko/Desktop/nipaserver/reference/jellyfin-plugin-bangumi/LICENSE`）。文件名解析依赖 NuGet 包 `AnitomySharp.NET6 0.5.1`（anitomy 的 C# 移植，上游 anitomy 系为 MPL-2.0）。本报告只提炼协议、参数、算法结构；下文引用的正则以"事实性参数清单"形式列出，Rust 实现请按语义重写、勿逐行照搬代码结构。

---

## 1) Bangumi API 调用清单

Base URL：`https://api.bgm.tv`（可配置），网站 `https://bgm.tv`。JSON 序列化统一 camelCase。

### 认证 / UA / 超时
- **UA 必须带自我标识**（bgm.tv API 官方要求）：插件发 `Jellyfin.Plugin.Bangumi/{version} (https://github.com/kookxiang/jellyfin-plugin-bangumi)`。NipaServer 应发类似 `NipaServer/{ver} (repo-url)`。
- 认证：可选 `Authorization: Bearer {access_token}`（OAuth2）。匿名可用绝大多数只读接口；带 token 才能看 NSFW 条目、读写收藏/进度。
- OAuth 端点（仅进度同步需要，AI 刮削可忽略）：`GET {web}/oauth/authorize?client_id&redirect_uri&response_type=code`、`POST {web}/oauth/access_token`、`POST {web}/oauth/token_status`。
- 请求超时默认 **5000ms**。

### 端点清单（刮削相关）
| 用途 | 方法/路径 | 关键参数 |
|---|---|---|
| 搜索(旧,默认) | `GET /search/subject/{keyword}?responseGroup=large&type=2` | keyword 需 URL-escape；发送前把 `" -"` 替换为 `" "`（连字符触发旧 API 的排除语义）；type=2 即 Anime。**404 时返回的是 HTML/异常 JSON**，插件按 JsonException 捕获视作空结果 |
| 搜索(新,v0) | `POST /v0/search/subjects` | body `{"keyword":"...","sort":null,"filter":{"type":[2],"nsfw":null}}`，响应 `{total,offset,limit,data:[Subject]}` |
| 条目详情 | `GET /v0/subjects/{id}` | 返回含 `infobox`（键值数组，含"别名"/"官方网站"/"播放结束"） |
| 条目图片 | `GET /v0/subjects/{id}/image?type=large` | **禁自动重定向**，读 301/302 的 Location；若等于 `https://lain.bgm.tv/img/no_icon_subject.png` 视为无图 |
| 分集列表 | `GET /v0/episodes?subject_id={id}&limit=50&type={0..6}&offset={n}` | 分页 total/limit/offset；type: 0=本篇 1=SP 2=OP 3=ED 4=Preview 5=Madness 6=Other |
| 分集列表(旧回退) | `GET /subject/{id}?responseGroup=large` 取 `eps` 数组 | v0 返回 total=0 时的回退（老音乐条目等），字段 `sort/disc/airdate/desc/name/name_cn` |
| 单集详情 | `GET /v0/episodes/{id}` | |
| 关联条目 | `GET /v0/subjects/{id}/subjects` | 每项含 `relation`（中文字符串："续集"/"前传"/"番外篇"/"总集篇"…）与 `type` |
| 角色 | `GET /v0/subjects/{id}/characters` | relation 排序：主角<配角<客串 |
| 制作人员 | `GET /v0/subjects/{id}/persons` | |
| 人物详情 | `GET /v0/persons/{id}` | |
| 人物搜索 | `POST /v0/search/persons` | body 同搜索 |
| 收藏/进度 | `GET/POST /v0/users/-/collections/{subject_id}`、`GET/PUT /v0/users/-/collections/-/episodes/{episode_id}`、`GET /v0/users/-/collections/{subject_id}/episodes?episode_type=` 、`GET /v0/me` | 均需 Bearer |
| 离线归档 | `https://raw.githubusercontent.com/bangumi/Archive/master/aux/latest.json` → 下载 zip dump | 插件用它做本地缓存优先查询（可选优化，NipaServer 初期可不做） |

### 限流/缓存/错误
- **插件没有任何 429/退避处理**——只靠一层进程内 MemoryCache 压请求量：key=完整 URL，仅 GET 缓存，**绝对过期 7 天 + 滑动 6 小时**，容量上限 256MB；非 GET 请求会先删除同 URL 缓存。NipaServer 建议：同样按 URL 做 TTL 缓存（moka/自建 sqlx 表），并补 429 退避（bgm.tv 未公开明确速率，社区经验保守值 1-2 req/s 即安全）。
- 错误响应体为 `{"title":"...","description":"..."}`，解析失败则报原始文本。
- 支持配置 HTTP 代理和忽略 SSL 错误（国内网络环境需求，NipaServer 可用 env var 透传 reqwest proxy）。

关键数据模型（Rust struct 对照）：
- Subject: `id, type, name(原名), name_cn, summary, date("YYYY-MM-DD"), images{large..}, eps(集数), rating{score}, tags[{name,count}], nsfw, platform("TV"/"剧场版"/"OVA"/"Web"...), infobox`。`ProductionYear = date[..4]`。热门标签 = tags 按 count 降序取 `max(8, len/25)` 个。
- Episode: `id, subject_id, type, name, name_cn, sort(条目内展示序号,double 可小数), ep(本篇内序号,double), airdate, disc, desc, duration`。**匹配用 `sort`，不用 `ep`**。

## 2) 文件名解析

插件有三个解析器（配置选一）：`Basic`（默认，纯正则）、`AnitomySharp`、`Torrent`（自研正则+Anitomy 兜底，最强）。

### Basic 解析器正则（BasicEpisodeParser.cs）
**先删噪声**（依次替换为空，`IgnoreCase`）：
```
[\[\(][0-9A-F]{8}[\]\)]        # CRC32
S\d{2,}                        # S01 等季标
yuv[4|2|0]{3}p(10|8)?          # yuv420p10
\d{3,4}p                       # 1080p
\d{3,4}x\d{3,4}                # 1920x1080
(Hi)?10p
(8|10)bit
(x|h)(264|265)
\d{2,}FPS
\[\d{2}(0[1-9]|1[0-2])(0[1-9]|1[0-9]|2[0-9]|3[0-1])]   # [YYMMDD]
(?<=[^P])V\d+                  # V2 版本号（避开 PV）
```
**再按序提取集号**（第一个命中即用，捕获组允许小数如 `13.5`）：
```
\[([\d\.]{2,})\]
- ?([\d\.]{2,})
EP?([\d\.]{2,})      (i)
第(\d+)巻
\[([\d\.]{2,})
#([\d\.]{2,})
(\d{2,})
\[([\d\.]+)\]
^(\d+(\.\d+)?)\.[a-zA-Z]+$    # 纯数字文件名 01.mkv
```
**类型识别正则**（在删噪后的名字上判定，命中即定类型）：
```
OP:  (NC)?OP([^a-zA-Z]|$)
ED:  (NC)?ED([^a-zA-Z]|$)
SP:  (SPs?|Specials?|OVA|OAD)([^a-zA-Z]|$)
PV:  [^\w]PV([^a-zA-Z]|$)
```
SP 判定还会检查**父目录名**是否命中 SP 正则（仅当父目录是 Season 级子目录而非系列根目录时，避免 `XXX (TV+OVA)` 这种根目录名误判）。

### Torrent 解析器正则（Utils/FileNameParser.cs，处理中文命名最全）
**季号预处理**（删干扰）：`\d+\s*-\s*\d+`（01-12 合集）、`\b\d{1,2}-Bit\b`、`\bAT-X\b`（防罗马数字 X 误判）。
**季号提取**（顺序匹配，命名捕获 seasonNumber）：
```
第?([零一二三四五六七八九十\d]+)[季部期]
Season\s*(\d+)
\bS(\d+)\b
\b(\d+)(st|nd|rd|th)(\s*Season)?\b
\bSeason\s*(One|...|Ten)\b
\b(I|II|...|XX)\b                      # 罗马数字
([Ⅰ-Ⅻⅰ-ⅻ]+)        # Unicode 罗马数字Ⅰ-Ⅻ
\b(\d{1,2})\b                          # 纯数字，最后兜底
```
**集号提取**（顺序匹配，均支持 `(\.\d)` 小数）：
```
第([零一二三四五六七八九十\d]+(\.\d)?)[话話集]
(?<=S\d+)E(\d+(\.\d)?)                 # S01E01
[\[【(（](\d+(\.\d)?)[\]】)）]          # 括号集号
\b(E|Ep|Episode\s*)(\d+(\.\d)?)\b
\s-\s(\d+(\.\d)?)\b                    # "Title - 01"
\b(?<=S\d+|Season\s*\d+\b.*?)(\d+(\.\d)?)\b
(\d+(\.\d)?)\.\w+$                     # xxx01.mkv
\b(\d+(\.\d)?)\b                       # 兜底
```
配套：中文数字转换器（支持到万位，"十一"→11、"一百零二"→102）、罗马/英文数字映射表。**拆分顺序重要**：剧集文件名先摘除集号再匹配季号，季目录名直接匹配季号。

### AnitomySharp 使用方式
- 把**文件名**（不是全路径）喂给 `AnitomySharp.Parse()`，得到元素列表，按 Category 取值：`AnimeTitle`（搜索关键词）、`EpisodeNumber`、`EpisodeNumberAlt`（跨季全局集号，如 `12 (48)` 里的 48）、`AnimeSeason`、`AnimeYear`、`AnimeType`（TV/OVA/SP/OP/ED/PV/CM…可多个）、`ReleaseGroup`。
- 标题/季号从**路径逐级**尝试（文件名可能只有集号，优先从父目录取标题）；季号也是父目录优先（文件名中易与集号混淆）。
- AnimeType → Bangumi 类型映射按优先级（先命中先用）：OP{NCOP,OP,OPENING} → ED{NCED,ED,ENDING} → Preview{PREVIEW,CM,SPOT,PV,Teaser,TRAILER,YOKOKU,予告} → Madness{MV} → Other{MENU,INTERVIEW,EVENT,TOKUTEN,LOGO,IV} → Special{OAD,OAV,ONA,OVA,番外編,總集編,DRAMA} → Other{特典,映像特典,SPECIAL,SP…} → Normal{TV,GEKIJOUBAN,MOVIE}。注意 SPECIAL 类词与其它特典词常混写，故排在 Special 之后。
- 电影检测辅助：文件名无集号时，若时长>10min 且体积>100MB，视为剧场版"第1集"（Bangumi 剧场版条目通常只有 1 集）。

### 特典/杂项目录排除（Torrent 解析器默认正则，可整套采纳）
- SP 目录名：`(\b|_)(SPs?|Specials?|OVA|OAD)(\b|_)` 与 `特典`；SP 文件名：`(\b|_)(SPs?\d*|Specials?|OVA\d*|OAD\d*)(\b|_)`、`特典`。
- 杂项目录名：`(\b|_)(PVs?|Previews?|Scans?|menus?|Fonts?|Extras?|CDs?|bonus|Music|Subs?|Subtitles?|其他|漫画|特别漫画|特典CD)(\b|_)`、`NCOP|NCED`；杂项文件名：`(\b|_)(WEB予告|次回予告|NCOP\d*|OP\d*|NCED\d*|ED\d*|menu\d*\w*|PV\d*|Preview\d*|CM集?\d*|IV\d*)(\b|_)`、`NCOP|NCED|ノンテロップ\s*OP|ノンテロップ\s*ED|メニュー画面\s*\d+`。杂项文件直接跳过刮削。系列根目录名**不参与**目录名匹配（`XXX S1+S2+OVA` 会误报）。另有白名单正则可豁免。

## 3) 匹配流程（series/subject 级）

1. **强制绑定优先**：目录名内嵌属性 `[bangumi=123]` 或 `[bangumi-123]`（实现：在名字里找 `[` + `bangumi` + `=`或`-` + 数字 + `]`，大小写不敏感）；以及目录下 `bangumi.ini`（键值行：`ID=`、`Offset=`、`Skip=`、`CorrectIndex=`、`Report=`）。命中即跳过搜索。
2. **搜索关键词构造**：默认先用库内名称（可配置原名优先），失败换 OriginalTitle 再试一次；可选先用 Anitomy 提取的 AnimeTitle。旧 API 关键词把 `" -"` 换成 `" "`。
3. **年份过滤（软过滤）**：`ProductionYear == null || ProductionYear == info.Year` —— 注意保留"无年份"的候选，不是硬过滤。
4. **排序取第一**：旧 API 结果 ≤1 条直接用；多条时默认 **Levenshtein**：`min(dist(keyword, name_cn), dist(keyword, name))` 升序。可选 FuzzScore 模式：只取前 5 条候选逐个拉 `/v0/subjects/{id}` 拿 infobox **别名**，对 `name_cn / name / 各别名` 做 Fuzz.Ratio（0-100 相似度）取最大值，降序排列后接回剩余候选。
5. 用选中 id 调 `/v0/subjects/{id}` 拉全量详情填充元数据。

## 4) 季/集映射策略

### 目录层级 → subject
核心思想：**Bangumi 一季 = 一个 subject**，没有 TMDB 式 season 概念，映射靠"续集/前传"关系图。
- Season 目录 id 来源优先级：`bangumi.ini` > 目录名 `[bangumi=]` 属性 > 已存 provider id > （若是第 1 季）直接继承 Series 的 subject id > 猜测。
- **猜测续季（SearchNextSubject）**：从上一季 subject 取 `/subjects/{id}/subjects` 中 relation=="续集" 的候选做 BFS（默认最多 2 层），**跳过 platform=="剧场版"/"OVA" 或 genre 含 OVA/剧场版 的节点**（它们插在正季之间），取第一个 TV 类候选。找上一季（SearchPreviousSubject）同理用 "前传"，且若同层存在非 OVA/Movie 候选则剔除 OVA/Movie。
- 名称猜测兜底：搜 `"{系列名} 第{一二三...}季"` 和 `"{系列名} Season {n}"`，排除与父系列相同的 id，再按年份软过滤。
- **全系列 id 收集（GetAllAnimeSeriesSubjectIds）**：BFS 关系图，"续集/前传"入队继续扩展，"番外篇/总集篇"只收录不扩展，仅 type==Anime，上限 1024 次请求。用于校验"目录名搜索出的 subject 是否真属于该系列"。

### episode 对齐（sort 语义）
- 匹配键是 **`sort`**（站点显示序号，double，SP 可为 0.5/13.5 等小数），不是 `ep`。
- **分页猜测算法**：limit=50；若 total>50 且目标集号不在首页 `[min(sort), max(sort)]` 内，猜 `offset = min(int(episodeNumber), total) - 20`，请求后若该页无 `int(sort)==int(episodeNumber)`，按该页 sort 范围与目标比较 `offset ±= 50` 迭代；用 visited-offset 集合防死循环；400（offset 越界）时回退首个结果。
- 已存 episode id 的**校验**：`episode.subject_id == 推定的 subjectId && |sort - 期望集号| < 0.1` 才信任；SP/OP/ED/PV 类型跳过校验（TrustExistedBangumiId 配置可整体跳过）。
- 匹配：`episodeList.OrderBy(type).First(sort == episodeIndex)`（多类型同号时**本篇优先**）。指定类型找不到 → 回退全类型再找。列表只有 1 集且是本篇 → 直接返回（剧场版场景）。
- 集号再猜测：若现有集号 > 列表最大 sort，认为库内序号错误，改用文件名解析出的序号（本地 Offset 参与换算：`实际集号 = 文件名集号 - Offset`；特典不应用 Offset）。
- **SP 处理**：SP → season number 恒为 0；episode.sort 为小数时设 `AirsBeforeEpisodeNumber = ceil(sort)`（插入正篇间的排序提示）；SP 标题为空时回退用其所属 subject 名。OVA 独立成条目时其分集 type 可能是 0（本篇），按 SP 查不到时需检查 subject platform==OVA 后改按 Normal 重查。
- **连续编号跨季**（本地 1-50 连排但 Bangumi 分两季）：确认当前 subject 无"前传"（即第一季）后，逐个取"续集"，累计 `seasonEpisodeCount`（注意每季 sort 可能重排从 1 起，也可能延续编号，用"本季第一集 sort 是否为 1"区分并归一化），直到目标集号落入区间，再用 `episodeIndex - 前几季总数` 或 anitomy 的 alt 集号匹配；命中后把 Order 改回本地连续序号避免多个"第 1 集"。
- **分割放送**（split-cour）：目标集号 < 当前 subject 最小 sort → 去上一季找（季号偏移 -1）；> 最大 sort → 去下一季找（偏移 +1）。

### 已知失败模式（插件自己注释承认的坑）
- 目录名 `XXX (TV + OVA)` 类混合包导致类型误判。
- 续集先出、前传后出时"第一季判定"失效。
- 多季目录搜索改 id 后需二次刷新才生效；季号重复时无法生成虚拟季。
- 非原名/中文搜索旧 API 不保证正确（旧 API 对罗马音支持差）。
- CM01 可能匹配到 PV01 的元数据（同类关键词冲突）。
- 电影 10min/100MB 启发式会把大体积特典误判为第 1 集。

## 5) NipaServer 移植建议

### AI agent 接管后可丢弃的部分
- Levenshtein/FuzzScore 排序、双关键词重试、年份软过滤、"第X季"中文搜索词构造、续集 BFS 猜季 —— 这些全是**为"无智能的确定性代码"设计的启发式**。LLM 拿到搜索结果列表（含 name/name_cn/date/platform/summary）后自行判断即可，无需移植。
- MemoryCache 之外的 Archive 离线 dump、OAuth/进度同步、Jellyfin 虚拟季逻辑：不需要。
- 三套解析器的"选择器"架构不需要：Rust 端做一个合并版解析函数即可。

### 建议移植（作为 AI 的工具/预处理，注意 GPL：按参数事实重写）
- **UA 标识 + Bearer 可选 + URL 级 TTL 缓存**（7d/6h 参数可沿用）+ 429 退避。这是调用姿势的核心。
- **`[bangumi=id]` / `bangumi.ini` 强制绑定**：极便宜且用户刚需，应在进 AI 之前短路。建议 NipaServer 统一支持 `[bangumi-123]`、`[bangumiid-123]` 与 nfo/ini。
- **文件名解析**：优先用 Rust 生态的 anitomy 移植（如 `anitomy` 系 crate，MPL-2.0 与闭源/任意许可共存无碍），再补上文 Torrent 解析器的中文正则（第X话/話/集、中文数字、Unicode 罗马数字、括号集号）。正则本身是事实性模式集合，重写实现即可。删噪正则集（CRC32/分辨率/码率/日期戳/V2）建议全量采纳。
- **分集分页猜 offset 算法**：limit=50 + `offset = ep - 20` + 区间迭代，直接照逻辑重写；或者更简单：一次性翻页拉全量（大多数条目 <200 集）存库，AI 面对完整列表更稳。**推荐后者**——NipaServer 有 sqlx，可把 subject 的 episodes 全量缓存成表，彻底绕开 offset 猜测。
- **sort 语义**：入库时 episode 匹配一律用 `sort`（f64），SP 小数号保留原值展示，季号 SP=0。

### 工具 schema 建议

```
tool search_bangumi
参数:
  keyword: string          # 已由 agent 清理过的标题
  type: int = 2            # 1书籍/2动画/3音乐/4游戏/6三次元
  use_v0: bool = true      # v0 POST 搜索；false 走旧 GET（旧接口召回中文别名更好，可让 agent 自选或服务端并发双查合并）
返回: [{
  id, name, name_cn, date, platform,      # platform 供 agent 区分 TV/剧场版/OVA
  eps_count, score, rank?, summary_brief, # summary 截断 ~200 字省 token
  image_small
}]  # 建议最多 10 条，保持原始顺序，排序判断交给 agent
```

```
tool get_bangumi_subject
参数:
  id: int
  include_episodes: bool = true
  include_persons: bool = false     # 角色/staff 单独开关，省 token
返回: {
  id, name, name_cn, date, end_date?, platform, eps_count,
  rating: {score, total}, nsfw,
  tags: [string],                    # 按 count 降序取 max(8, n/25) 个（沿用插件参数）
  genres: [string],
  summary,
  aliases: [string],                 # 从 infobox["别名"] 提取——agent 确认匹配的关键依据
  official_website?,
  images: {large, medium},
  relations: [{id, relation, name, type}],   # "续集/前传/番外篇/总集篇"原文保留，agent 用它做季映射
  episodes: [{id, type, sort, ep, name, name_cn, airdate, duration}]
}
```

补充建议：
- 再加一个轻量 `get_bangumi_episodes(subject_id, type?)`（对应 `/v0/episodes`），当 agent 只需要对齐集号时不必拉全 subject。
- relations 是 agent 做"目录第 N 季 → subject"推理的关键素材，务必返回；配合返回每个候选的 `date`，agent 能自己完成年份过滤和续季链推导，等价替代插件的整套 BFS。
- 服务端应把 `date[..4]` 预提取成 `year` 字段，并对 `name/name_cn` 做 HTML entity 解码（插件用 WebUtility.HtmlDecode，Bangumi 数据里确有 `&amp;` 类脏数据）。
- infobox 解析注意：值可能是字符串或 `[{k?,v}]` 数组（别名多值），需展平。

关键源码路径备查：
- API：`Jellyfin.Plugin.Bangumi/BangumiApi.cs`、`BangumiApi.Jellyfin.cs`、`BangumiApi.Cache.cs`
- 解析：`Parser/BasicParser/BasicEpisodeParser.cs`、`Parser/TorrentParser/TorrentEpisodeParser.cs`、`Utils/FileNameParser.cs`、`Parser/AnitomyParser/AnitomyEpisodeTypeMapping.cs`
- 匹配：`Providers/SeriesProvider.cs`、`Providers/SeasonProvider.cs`、`Providers/EpisodeProvider.cs`、`Model/Subject.cs`（排序）、`Extensions.cs`（GetAttributeValue）
- 流程文档（作者手绘 mermaid）：`docs/剧集获取逻辑.md`、`docs/集数获取逻辑.md`