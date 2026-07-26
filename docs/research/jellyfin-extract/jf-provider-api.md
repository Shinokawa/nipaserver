# Jellyfin Provider 框架 + TMDB Provider 精读报告（供 nipa-providers / agent tool 设计）

来源：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin`（GPL-2.0，以下全部为逻辑/参数/协议提炼，无代码翻译）。

关键文件索引：
- 接口层：`MediaBrowser.Controller/Providers/{IRemoteMetadataProvider.cs, ItemLookupInfo.cs, MetadataResult.cs, IRemoteImageProvider.cs, IExternalId.cs}`、`MediaBrowser.Model/Providers/{RemoteSearchResult.cs, RemoteImageInfo.cs}`
- TMDB 实现：`MediaBrowser.Providers/Plugins/Tmdb/{TmdbClientManager.cs, TmdbUtils.cs, TV/TmdbSeriesProvider.cs, TV/TmdbSeasonProvider.cs, TV/TmdbEpisodeProvider.cs, Movies/TmdbMovieProvider.cs, Configuration/PluginConfiguration.cs}`
- 编排层：`MediaBrowser.Providers/Manager/MetadataService.cs`（provider 循环与合并）、`Manager/ProviderManager.cs`（图片语言过滤）、`MediaBrowser.Model/Extensions/EnumerableExtensions.cs`（图片语言排序）

---

## 1. Provider 接口抽象

### 1.1 查询输入 ItemLookupInfo（所有 lookup 的基类）

字段清单（`ItemLookupInfo.cs`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| Name | string | 查询名（注意：调用方传的是**去扩展名的文件名**，provider 内部才做 ParseName+CleanName） |
| OriginalTitle | string | 原名 |
| Path | string | 文件路径（本地 provider 用） |
| MetadataLanguage | string | 如 `zh-CN`，逐级继承自库/项目设置 |
| MetadataCountryCode | string | ISO 3166-1，用于分级/发行日期选国 |
| ProviderIds | `Dictionary<string,string>`（key 大小写不敏感） | 外部 id 包，key 为 provider 名（"Tmdb"/"Imdb"/"Tvdb"…） |
| Year | int? | 年份 |
| IndexNumber / ParentIndexNumber | int? | 集号 / 季号 |
| PremiereDate | DateTime? | 首播日期（按日期匹配集时用） |
| IsAutomated | bool | 是否自动刮削（区分用户手动 Identify） |

子类差异（关键设计——**父级 id 单独开口袋**）：
- `SeriesInfo` / `MovieInfo`：无新增字段。
- `SeasonInfo`：+ `SeriesProviderIds`（父剧集的 id 包）、`SeriesDisplayOrder`。
- `EpisodeInfo`：+ `SeriesProviderIds`、`SeasonProviderIds`、`IndexNumberEnd`（跨集文件 S01E01-E03）、`IsMissingEpisode`、`SeriesDisplayOrder`。

### 1.2 两阶段接口（search 与 metadata 分离）

`IRemoteMetadataProvider<TItem, TLookup>` 两个方法：
- `GetSearchResults(lookup) -> Vec<RemoteSearchResult>`：候选列表，给用户 Identify UI 或消歧用；
- `GetMetadata(lookup) -> MetadataResult<TItem>`：完整刮削，内部自带"没 id 就先 search 取第一条"的逻辑。

`RemoteSearchResult` 字段（**这就是"候选"该有的信息密度**，直接对应 agent tool 的 search 返回）：Name、ProviderIds、ProductionYear、PremiereDate、IndexNumber/IndexNumberEnd/ParentIndexNumber（集搜索用）、ImageUrl（一张海报）、Overview、SearchProviderName。

### 1.3 输出 MetadataResult<T>

| 字段 | 说明 |
|---|---|
| HasMetadata | bool，false=本 provider 无结果（区别于报错） |
| Item | 实体本身（含 Name/OriginalTitle/Overview/PremiereDate/ProductionYear/CommunityRating/OfficialRating/Genres/Tags/Studios/ProviderIds/RemoteTrailers…） |
| People | `Vec<PersonInfo>`，与 Item 分离存（人物是独立表）；**null 与空列表语义不同**（null=未提供，空=明确清空） |
| RemoteImages | `(url, ImageType)` 列表（可选，多数场景图片走独立 IImageProvider） |
| ResultLanguage | 结果实际语言（用于判断回退） |
| Provider | 来源名 |
| QueriedById | 是否按 id 精确查得（true=可信度高；按名搜索命中则为 false）——**这个 bool 就是 Jellyfin 的"置信度"，对应你们的 confidence 字段** |

`PersonInfo`：Name、Role（角色名/职务）、Type（Actor/Director/Writer/Producer/GuestStar/Creator）、SortOrder（cast 排序）、ImageUrl（头像绝对 URL）、ProviderIds（人物自己的 TMDB id）。

图片单独接口 `IRemoteImageProvider`：`GetSupportedImages(item) -> [ImageType]` + `GetImages(item) -> Vec<RemoteImageInfo>`；`RemoteImageInfo` = { Url, ThumbnailUrl, Width, Height, Language, Type(Primary/Backdrop/Logo/Thumb/…), CommunityRating, VoteCount, ProviderName }。**元数据刮削与图片刮削是两条独立流水线**，nipa 里建议同样拆开：agent 只定"是谁"，图片下载是确定性后处理。

### 1.4 编排层的合并逻辑（MetadataService，可借鉴）

- 多 provider 按 Order 顺序跑，每个成功的结果 `MergeData` 进临时结果；已有字段不覆盖（除非 ReplaceAllMetadata），锁定字段（LockedFields）永不覆盖。
- **MergeNewData**：每个 provider 跑完后，把 Item 上新获得的 ProviderIds 用 `TryAdd` 回写进 lookup（**不覆盖已有 id**）——这是链式补全 id 的机制（TMDB 跑完带回 imdb/tvdb id，下一个 provider 就能用）。
- 用户手动 Identify 的结果应用（ApplySearchResult）：普通条目直接替换 lookup 的 ProviderIds/Name/Year；**Episode/Season 不支持直接 Identify，选中的其实是 Series 的结果**，所以写入 `SeriesProviderIds` 并清空自身 ProviderIds。

---

## 2. TMDB 调用姿势

### 2.1 search → details 流程（以 TmdbMovieProvider/TmdbSeriesProvider.GetMetadata 为准）

id 解析优先级（严格顺序）：
1. lookup 里已有 Tmdb id → 直接 details；
2. 有 Imdb id → `/find/{imdb_id}?external_source=imdb_id` 取第一条的 tmdb id；
3. 有 Tvdb id（剧集）→ `/find` external_source=tvdb_id；
4. 都没有 → 名字清洗后 `/search/movie` 或 `/search/tv`，**取第一条**（此时 QueriedById=false）。

名字清洗两步：先 ParseName（从文件名剥年份等，Jellyfin 用库内解析器，nipa 对应 anitomy/自实现），再 `CleanName`：把所有非词字符（正则 `[\W_]+`，但保留 `·`）替换为空格——TMDB 期望空格分词。搜索时 year 参数传 `info.Year ?? parsedName.Year ?? 0`（0=不过滤）。

搜索端点参数：
- movie: `/search/movie?query=&language=&year=&include_adult=`
- tv: `/search/tv?query=&language=&first_air_date_year=&include_adult=`

### 2.2 details 的 append_to_response

Jellyfin 用 TMDbLib 的 extraMethods，等价于原生 API 的 `append_to_response`，**一次请求拿全**：
- movie：`credits,releases,images,videos,keywords`（keywords 可配置关）
- tv series：`credits,images,external_ids,videos,content_ratings,episode_groups,keywords`
- tv season：`credits,images,external_ids,videos`
- tv episode：`credits,images,external_ids,videos`
- person：`tv_credits,movie_credits,images,external_ids`

注意：**season 的 external_ids 只有 tvdb id；episode 的 external_ids 有 imdb/tvdb/tvrage**；series 的 details 响应本身不含 imdb id，必须靠 append external_ids。

### 2.3 图片 URL 组装

- 先 `GET /configuration`（每客户端进程一次，惰性），拿 `images.secure_base_url` 与各类 size 列表（poster_sizes/backdrop_sizes/logo_sizes/profile_sizes/still_sizes）；
- URL = `{secure_base_url}{size}{file_path}`，如 `https://image.tmdb.org/t/p/w500/xxx.jpg`；
- 配置的 size 若不在服务器返回的合法列表里，回退到列表**最后一项**（即 `original`）；size 为空默认 `original`；
- 各类型默认对应：海报→PosterSize、背景→BackdropSize、logo→LogoSize、人物头像→ProfileSize、剧照 still→StillSize；
- **非 original 尺寸时不要存 width/height**（API 返回的是原图尺寸，缩放后不准）；
- 语义修正：**backdrop 若带语言代码（有文字）则归类为 Thumb 而非 Backdrop**（`ConvertToRemoteImageInfo`）——nipa 选背景图时同样应偏好无语言 backdrop。
- 图片语言代码 `xx` 表示无语言，等价空。

### 2.4 语言与 fallback（zh-CN 的处理，重点）

**文本语言（`language` 参数）**：`NormalizeLanguage`：
- 连字符后必须大写（`zh-cn` → `zh-CN`，TMDB API 特性）；
- `es-419` 按 countryCode 转 `es-AR`/`es-MX`；`xx-CH`（瑞士）去国码只留语言。
- 文本无 per-request fallback——TMDB 的 language 参数本身有服务端 fallback（缺翻译返回原文）。Episode provider 用启发式判断本地化是否生效：**overview 非空 ⇒ 认为返回了本地化数据**，并设置 ResultLanguage。nipa 建议更进一步：zh-CN 结果 overview/name 为空时，可再请求一次 `language=zh-TW` 或直接接受原文（Jellyfin 没做，是它的弱点；metashark 插件为此存在）。

**图片语言（`include_image_language` 参数）**：`GetImageLanguagesParam` 生成逗号串，固定模式：
```
"{norm(preferred)},null,en"     // preferred 非 en 时
"en,null"                        // preferred 为 en 时
```
即：请求 zh-CN 时传 `include_image_language=zh-CN,null,en`——**一次请求同时拿中文图、无语言图、英文图**，这就是缺 zh 时的图片兜底。

**图片排序/过滤（ProviderManager + OrderByLanguageDescending）**：
- 过滤：只留（无语言 ∥ 匹配 preferred ∥ en）；
- 排序打分：匹配 preferred=4 > en=3 > 无语言=2 > 其他=0，再按 CommunityRating（一位小数）降序、VoteCount 降序。
- `AdjustImageLanguage`：图片语言是 2 位码（zh）而请求是 5 位（zh-CN）且前缀匹配时，把图片语言提升为 5 位，保证排序命中第一档。

### 2.5 缓存与请求约束

- 进程内 MemoryCache，**TTL 1 小时**，key 模式：`movie-{id}-{lang}`、`series-{id}-{lang}`、`season-{seriesId}-s{n}-{lang}`、`episode-{seriesId}-s{n}e{m}-{displayOrder}-{lang}`、`find-{source}-{extId}-{lang}`、`searchseries-{name}-{year}-{lang}`、`moviesearch-{name}-{year}-{lang}`、`person-{id}-{lang}`——**key 必带语言**；搜索空结果不缓存（details 空也不缓存）。
- 404 不作为异常（ThrowApiExceptions=false，返回 null）。
- nipa 建议：egress 层做同样的 `(endpoint, id, lang)` 键缓存但落 SQLite（跨进程/重启），TTL 可拉长到天级（agent 重试/多文件同番会反复查同一 series）；1 小时内存缓存作为一级。

### 2.6 其他可移植的映射细节

- **Crew 过滤**：只保留 Director（department=Directing+job=Director）、Producer（Production+Producer）、Writer（Writing + job∈{writer,screenplay,novel}），其余丢弃；cast 按 `order` 升序取前 N（默认 MaxCastMembers=15，MaxCrewMembers=15）。
- **分级**：movie 从 `releases.countries`、tv 从 `content_ratings.results` 里选：先 preferred country → US → 第一条；非 US 前缀国码（`DE-16`→存储时 `FSK-16`，US 不加前缀）。
- **预告片**：videos 里 site=YouTube 且 type∈{Trailer,Teaser}，Trailer 排前，URL=`https://www.youtube.com/watch?v={key}`。
- movie overview 的 `\n\n` 压成 `\n`。
- Series 附加：Networks+ProductionCompanies 合并为 studios；EpisodeRunTime 第一项为时长；status 字符串解析为 Continuing/Ended；BelongsToCollection → TmdbCollection id + CollectionName。
- ProviderIds 的 key 集合（`MetadataProvider` enum 名）：`Tmdb`、`Imdb`、`Tvdb`、`TmdbCollection`、`TvRage` 等——nipa 直接用小写 `tmdb/imdb/tvdb/bangumi/dandanplay` 即可，但**保持 dictionary-of-strings 形态**，这是全链路通用货币。

---

## 3. Series / Season / Episode 三级刮削与 id 传递链

**核心结论：TMDB 只有 series 级有独立可搜索 id；season/episode 没有自己的搜索入口，完全靠 `(series_tmdb_id, season_number, episode_number)` 定位。**

传递链（`Season.GetLookupInfo` / `Episode.GetLookupInfo`）：
1. Series 刮削：search/find/名字搜索得 tmdb id → details → Item.ProviderIds 写入 {Tmdb, Imdb, Tvdb, TvRage}；
2. 构造 SeasonInfo 时：`season_lookup.SeriesProviderIds = series.ProviderIds`（整包拷贝，非单个 id），season 自身 IndexNumber=季号；
3. SeasonProvider：从 `SeriesProviderIds["Tmdb"]` + IndexNumber 调 `/tv/{id}/season/{n}`；**series id 或季号缺一即放弃**（不猜）。产出：季名（可配置是否采用）、Overview、AirDate、tvdb id、cast/crew；
4. 构造 EpisodeInfo 时：`SeriesProviderIds = series.ProviderIds`、`SeasonProviderIds = season.ProviderIds`、`SeriesDisplayOrder = series.DisplayOrder`；
5. EpisodeProvider：`/tv/{id}/season/{n}/episode/{m}`；季号缺省时**默认 1**；集号缺失放弃。episode 自身 ProviderIds 只写 {Imdb, Tvdb, TvRage}（来自 external_ids），**不写 tmdb**（因为 episode 的 tmdb 定位是三元组不是单 id）；
6. 反向：Episode/Season 的 Identify 结果按 Series 处理（写 SeriesProviderIds，见 §1.4）。

**Episode Groups（备用集序）**：series details append `episode_groups`；`SeriesDisplayOrder` ∈ {originalAirDate, absolute, dvd, digital, storyArc, production, tv} 时，找 series.episode_groups 中对应 type 的 group id → `/tv/episode_group/{id}` → 在 group 里按 `season.order==季号`、`episode.order==集号-1`（**group 内集序从 0 起**）找到条目，取其**真实 (season_number, episode_number)** 再走标准 episode 端点。这是对付"绝对集数命名的动画"的官方姿势，nipa 做动画场景强烈建议实现 absolute order 这条。

**跨集文件**（S01E01-E03，IndexNumberEnd）：逐集拉 details，Name/Overview 用 ` / ` 连接，日期/评分/ids 取第一集。

**季映射对 nipa 的启示**：Jellyfin 模型 = series 一个 id 包 + season/episode 用序号定位。Bangumi/弹弹play 每季独立 id，因此 nipa 的 series 实体应存**多源 id 映射表（每季一行：{season_number → bangumi_subject_id, dandan_anime_id}）**，而不是照抄 Jellyfin 的单 id 包（开发文档 §4.5 已识别此高危项，Jellyfin 框架本身没有答案）。

---

## 4. nipa-providers Rust 设计建议

### 4.1 trait 设计（服务于 agent tool，而非照抄 provider 编排）

nipa 不需要 Jellyfin 的多 provider 合并编排（那是给"无脑刮削器"设计的，你们由 LLM 做裁决），需要的是**确定性 client 层 + 薄 tool 适配层**：

```rust
// nipa-providers：确定性 client 层（可被 tool 与"传统兜底"共用）
pub struct ProviderIds(pub BTreeMap<String, String>); // key: "tmdb"|"imdb"|"tvdb"|"bangumi"|"dandanplay"

pub struct SearchQuery {
    pub name: String,
    pub year: Option<u16>,
    pub language: String,          // "zh-CN"
    pub country: Option<String>,   // "CN"
    pub ids: ProviderIds,          // 有 id 时走 find/details 短路，模仿 Jellyfin 优先级
}

pub struct SearchCandidate {       // ≈ RemoteSearchResult
    pub id: String,
    pub name: String,
    pub original_name: Option<String>,
    pub year: Option<u16>,
    pub media_type: MediaType,     // tv | movie
    pub overview_snippet: Option<String>, // 截断后的
    pub ids: ProviderIds,          // find 场景会带 imdb/tvdb
    pub popularity_rank: u32,      // 结果序号，供 LLM 参考"这是第几名"
}

#[async_trait]
pub trait MetadataSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchCandidate>>;
    async fn details(&self, id: &str, lang: &str) -> Result<FullMetadata>;   // 内含 append_to_response 全量
}

// TV 层级用独立 trait，参数就是 Jellyfin 的三元组：
#[async_trait]
pub trait TvHierarchySource: MetadataSource {
    async fn season(&self, series_id: &str, season: u16, lang: &str) -> Result<SeasonMetadata>;
    async fn episode(&self, series_id: &str, season: u16, ep: u32,
                     order: EpisodeOrder, lang: &str) -> Result<EpisodeMetadata>; // order: Default|Absolute|Dvd...
}
```

要点：
- `FullMetadata`（≈ MetadataResult）：`{ item: ItemMeta, people: Vec<PersonMeta>, images: Vec<ImageRef>, ids: ProviderIds, result_language: String, queried_by_id: bool }`；people/images 独立于 item；
- 图片不下载、只产 `ImageRef { url, kind, language, vote_avg, vote_count, width, height }`，排序用 §2.4 的打分函数（可直接实现为 `fn score(lang, preferred) -> u8`：匹配=4/en=3/无语言=2/其他=0，再按评分、票数）；
- 缓存放 client 内部（`(method, id, lang)` 键），tool 层无感知；
- 错误分层：`NotFound`（→ 正常空结果，模仿 ThrowApiExceptions=false）vs `Transient`（429/5xx，egress 重试）vs `Fatal`。

### 4.2 agent tool 的 JSON 输出粒度（token 预算权衡）

原则：**search 给消歧所需的最小字段集，details 给 submit_result 所需的字段集，图片/人物一律不进 LLM 上下文**（那是 high-confidence 入库后的确定性后处理）。

`search_tmdb` 返回（每条 ~40-60 tokens，最多给 5-8 条）：
```json
{ "results": [
  { "id": 95479, "type": "tv", "name": "咒术回战", "original_name": "呪術廻戦",
    "year": 2020, "overview": "前 80 字截断…" }
], "total": 23 }
```
- 必须有：id/type/name/original_name/year——Jellyfin 的 RemoteSearchResult 证明这几项就够人类消歧，LLM 同理；
- overview 截 60-100 字（全文 300+ 字 × 8 条就是 1.5K tokens，纯浪费）；
- **不要**：poster 路径、popularity 数值、genre_ids、vote 数据（消歧用不上）；
- total 保留：让模型知道"还有 15 条没显示，我的搜索词可能太泛"。

`get_tmdb_detail` 返回（一条 ~150-250 tokens）：
```json
{ "id": 95479, "type": "tv", "name": "咒术回战", "original_name": "呪術廻戦",
  "year": 2020, "first_air_date": "2020-10-03", "status": "Returning",
  "genres": ["动画","动作冒险"], "origin_country": ["JP"], "original_language": "ja",
  "external_ids": { "imdb": "tt12343534", "tvdb": 377367 },
  "seasons": [ { "season_number": 0, "episode_count": 5, "name": "特别篇" },
               { "season_number": 1, "episode_count": 24, "air_date": "2020-10-03" },
               { "season_number": 2, "episode_count": 23, "air_date": "2023-07-06" } ] }
```
- **seasons 数组（季号+集数+首播日期）是 TV 消歧的决胜信息**——模型拿文件的"第 35 集"对 season episode_count 累加即可推断真实季集（这正是 episode groups 想解决的问题的 LLM 版）；每季只给 3-4 个字段，别给季 overview；
- external_ids 必带（跨源关联、写库都靠它）；
- **不要**：cast/crew（最贵且对识别无用）、images 列表、production_companies、keywords、videos。这些在 submit_result 置信度过闸后由 nipa-providers 用同一个缓存好的 details 响应（append_to_response 已拿全）做确定性提取入库——**同一次 HTTP 响应，喂 LLM 的是裁剪视图，入库的是全量**，这是省 token 的关键架构：tool handler 裁剪，client 缓存全量。
- 若需要集级确认（对齐特定一集的标题/日期），单独做薄参数 `get_tmdb_detail(series_id, season, episode)` 返回 `{name, air_date, episode_number, season_number}` 四个字段即可。

### 4.3 坑与教训（从 Jellyfin 代码里读出的）

1. **language 参数大小写**：`zh-cn` 会拿不到中文数据，必须规范成 `zh-CN`（TMDB API 对 region 段大小写敏感，Jellyfin 专门写了注释引官方论坛帖）。
2. **include_image_language 里要带字符串 `"null"`**，不是省略——否则无语言图片（多数高质量 backdrop）全丢。
3. 图片有语言的 backdrop 是"带标题文字图"，当 Thumb 用，别当背景。
4. 搜索前清洗名字（非词字符→空格），否则 `[SubGroup]Title.S01E01` 这类直接搜必挂；但 nipa 的 agent 场景中这一步交给 LLM/anitomy，client 不要重复清洗破坏模型给的精确 query。
5. episode 缓存 key 必须含 displayOrder，season/episode 的定位受 episode group 影响。
6. 空搜索结果不缓存 → 但 nipa 有 LLM 重试，建议对空结果做**短 TTL 负缓存**（如 10 分钟）防 agent 循环打同一查询。
7. season 默认号：episode 查询缺季号时 Jellyfin 默认 season=1——对动画（常只有一季或用特别篇 season 0）是合理默认，但要在 tool 描述里告诉模型"season 0 = 特别篇/OVA"。
8. QueriedById 语义值得保留：按 id 查=高置信，按名搜第一条=需要复核，映射到你们 confidence 的 high/medium 边界。
9. Jellyfin 的 `MergeNewData` 用 TryAdd（不覆盖已有 id）——nipa 合并多源 id 时同样**先到先得 + 用户改正可覆盖**，防止后跑的低质 provider 污染 id。
10. license：Jellyfin 主仓 GPL-2.0、jellyfin-plugin-bangumi GPL-2.0、jellyfin-plugin-metashark GPL-3.0——本报告仅含参数/协议/流程事实（API 端点、参数格式、排序权重、TTL 数值），这些不受版权保护；实现时независимо编写 Rust 代码，勿对照 C# 逐行翻译。另注意 TmdbUtils 里的 API key 是 Jellyfin 项目自己的，**不可复用**，nipa 须自行申请 TMDB API key。