# 弹弹play (dandanplay) 开放平台 API 调研报告

API 基础地址：`https://api.dandanplay.net`。官方文档：https://doc.dandanplay.com/open/ （旧 GitHub 仓库 kaedei/dandanplay-libraryindex 已迁移至此）。在线 Swagger 调试工具与 `swagger/v2/swagger.json`（OpenAPI 3.0）可直接获取，本报告接口细节均来自该 swagger 定义原文。

## 1. 文件识别接口 `POST /api/v2/match`

用途：打开视频文件时，用文件名 + Hash + 文件长度查找对应节目。服务器首先用 Hash 精确搜寻，命中则返回"精确关联"（`isMatched=true`，matches 列表仅 1 项，客户端应自动选用、无需用户选择）；Hash 未命中则退化为按文件名模糊搜索，返回候选列表（越靠前越可能）。

请求体（JSON，`MatchRequest`）：
| 字段 | 类型 | 说明 |
|---|---|---|
| `fileName` | string | 视频文件名，**不含文件夹路径和扩展名**，特殊字符需转义 |
| `fileHash` | string | **文件前 16MB（16×1024×1024 字节）数据的 32 位 MD5**，不区分大小写。文件不足 16MB 时对全文件计算 |
| `fileSize` | int64 | 文件总长度，单位 Byte |
| `videoDuration` | int32 | [可选] 视频时长（秒），默认 0 |
| `matchMode` | enum | [可选] `hashAndFileName` / `fileNameOnly` / `hashOnly` |

官方提供测试视频（https://kaedei.lanzouo.com/itjCq30geele），其前 16MB MD5 为 `658d05841b9476ccc7420b3f0bb21c3b`，可用于校验 hash 实现。

响应（`MatchResponseV2`，继承 `ResponseBase`：`errorCode`(int, 0=无错)、`success`(bool)、`errorMessage`、`errorDetail`）：
- `isMatched` (bool)：是否已精确关联到某个弹幕库
- `matches`：`MatchResultV2` 数组，每项含：
  - `episodeId` (int64)：弹幕库 ID（即"节目编号"，唯一标识某番剧某一集，后续取弹幕用它）
  - `animeId` (int64)：作品 ID
  - `animeTitle` (string)：作品标题
  - `episodeTitle` (string)：剧集标题
  - `type` (enum `AnimeType`)：`tvseries`/`tvspecial`/`ova`/`movie`/`musicvideo`/`web`/`other`/`jpmovie`/`jpdrama`/`unknown`/`tmdbtv`/`tmdbmovie`
  - `typeDescription` (string)
  - `shift` (double)：弹幕偏移秒数（负数表示应提前）
  - `imageUrl` (string)：作品海报地址

另有 `POST /api/v2/match/batch`（`BatchMatchRequest`，requests 数组最多 32 个、不可重复），只返回"精确关联"结果，未命中项的 `success=false`，结果与请求一一对应。识别失败时可用 `GET /api/v2/search` 系列接口手动搜索匹配。

## 2. AppId/AppSecret 与签名认证（现已强制）

**是，现在必须。** 开放平台已启用应用强制认证机制：所有请求都要携带认证头，否则返回 403（`X-Error-Message: Missing Authentication Headers` 等）。

申请方式：访问 弹弹play 开发者中心 DevCenter（doc.dandanplay.com/open/ 页面顶部链接）注册账号 → 完善开发者资料并通过邮件验证 → 在【应用管理】页面创建应用并提交审核（公开/私有项目均可申请）。审核通过后获得 **1 个 AppId 和 2 个 AppSecret**。凭证滥用会被立即停用。也可邮件联系 kaedei@dandanplay.net。

两种认证模式（任选其一，所有请求都需带）：
- **凭证模式**（适合服务器端应用）：请求头 `X-AppId` + `X-AppSecret`。
- **签名验证模式**（官方推荐，客户端应用强烈建议）：请求头 `X-AppId`、`X-Timestamp`、`X-Signature`。

签名算法（原文）：
```
X-Signature = base64( sha256( AppId + Timestamp + Path + AppSecret ) )
```
- 四段按顺序直接字符串拼接，区分大小写；SHA-256 结果取**原始字节**做 Base64（不是 hex 字符串）。
- `Timestamp`：UTC Unix 时间戳，单位秒；与服务器时间偏差过大会 403（`Invalid Timestamp`），需保证设备时间同步。
- `Path`：以 `/` 开头的 API 路径，**不含协议、域名和 `?` 查询参数**。例：访问 `https://api.dandanplay.net/api/v2/comment/123450001?withRelated=true`，Path 为 `/api/v2/comment/123450001`。建议全小写，不做 URL 编码。

错误处理：业务错误以 200 + `success:false, errorCode, errorMessage` 返回；401 = 调用受限接口缺少用户 JWT；403 = 认证头缺失/时间戳无效/AppId 或 AppSecret 无效/签名不匹配（具体原因在响应头 `X-Error-Message`：`Missing Authentication Headers` / `Invalid Timestamp` / `Invalid AppId` / `Invalid Signature` / `Invalid AppSecret`）。IP 被封时所有页面均 403。

安全建议（官方）：客户端尽量不硬编码 AppSecret；开源项目可自建服务器转发，或代码中用占位符、CI 构建时从 secret 注入。

接口权限分两类：**公开接口**（文件识别、搜索、获取弹幕等，仅需应用认证）与**受限接口**（关注、播放历史、发送弹幕等，需用户 JWT `Authorization: Bearer`）。新应用默认可访问全部公开接口；受限接口目前暂不对第三方开放，开放后可申请。

Rust 实现提示：可用 `sha2` + `base64` crate 计算签名，`md-5` crate 计算文件前 16MB 的 MD5，`reqwest` 发请求（注意 comment 接口 302 跳转，reqwest 默认跟随重定向即可）。

## 3. 弹幕获取接口 `GET /api/v2/comment/{episodeId}`

- 路径参数 `episodeId` (int64)：弹幕库编号（match 接口返回）。
- 查询参数：
  - `from` (int64, 默认 0)：起始弹幕编号，忽略之前的弹幕（增量获取）
  - `withRelated` (bool, 默认 false，**推荐 true**)：同时返回该弹幕库关联的所有第三方网站（B站等）弹幕，即整合后弹幕
  - `chConvert` (int, 默认 0)：简繁转换，0 不转换 / 1 转简体 / 2 转繁体
- 行为：返回 **302** 跳转到弹幕加速服务（Location 头），最终得到 `CommentResponseV2`：`count` (int) + `comments` 数组，每条为 `{ cid: int64 弹幕ID, p: string, m: string 弹幕内容 }`。
- `p` 字段格式：`出现时间,模式,颜色,用户ID`（逗号分隔）：时间为秒（两位小数）；模式 1=普通滚动、4=底部、5=顶部；颜色为 32 位整数 R×256×256+G×256+B；用户 ID 为字符串（通常为数字）。
- 注意：签名 Path 是 `/api/v2/comment/{episodeId}`（含具体数字 ID，不含 query）。
- 相关接口：`POST /api/v2/comment/{episodeId}/app` 供第三方应用发送弹幕（各应用弹幕存于独立私有弹幕库，互不干扰）；`GET /api/v2/bangumi/{bangumiId}/comments` 按番剧获取。

## 4. 使用约定与商用限制（官方原文要点）

基础要求：按需调用、避免高频；缓存返回数据（官方建议：一般数据缓存 2-6 小时，热门更新番当天 0.5-1 小时，非当季 12-24 小时，老番 2-7 天）；不得用于违法违规。

禁止事项：
- 禁止发送大量无意义/恶意弹幕；禁止大量提交无意义数据；
- **禁止规模化抓取，"批量下载弹幕"、"下载数据库"类行为被明令禁止**（调用应结合用户实际操作）；
- **禁止将 API 用于商业目的，除非获得弹弹play明确授权**。

关于商用的具体规定（重要）：
- 未经授权，禁止利用返回数据做任何形式的商业活动，包括向第三方收费或提供增值服务；需商业合作请联系官方。
- 若应用以弹弹play为主要弹幕来源：**弹幕功能必须对全部用户免费开放，不能作为付费功能点**；付费应用不能把弹幕功能作为重要卖点宣传（功能介绍中提及可以）；不能发布以弹幕为主要特色并收费的应用（如付费的《XXX弹幕播放器》《弹幕下载器》）。**免费应用不受此限制**。

速率限制：当前不限制调用频率，但对搜索、获取弹幕等高消耗接口有异常检测机制，远超正常范围会被自动限制访问（高频需求可申请白名单）；违反约定可不经通知直接停用应用。**自 2026 年 6 月 25 日起已启用应用分层与额度管理机制**，不同项目按实际情况获得对应接入层级和支持策略（细节见 DevCenter 公告）。

## 5. Hash 匹配库覆盖范围与局限

覆盖范围：
- 数据库以**日本动画（新番/老番、TV/剧场版/OVA/特别篇/WEB）为绝对主体**，这是弹弹play 2013 年起作为本地弹幕播放器积累的核心数据；`AnimeType` 枚举还包括 `jpmovie`（日本电影）、`jpdrama`（日剧）、`musicvideo`，以及近年新增的 `tmdbtv`/`tmdbmovie`（对接 TMDB 的一般电视剧/电影条目），说明库已扩展到部分真人影视，但弹幕/关联密度远不如动漫。
- 弹幕来源包括弹弹play官方弹幕 + 第三方网站关联弹幕（B站等，`withRelated=true` 获取）+ 开放平台应用发送的弹幕。

局限：
- **Hash 匹配依赖社区人工关联**：只有当某个具体文件版本（同一字幕组同一压制）曾被弹弹play用户播放并关联过，其前 16MB MD5 才在库中。冷门番、冷门字幕组版本、自压/重封装（remux 会改变前 16MB 字节）的文件 hash 通常无记录，只能落到文件名模糊匹配，需要用户手动确认。
- 文件名匹配对命名不规范的文件（无番名、纯数字、中文乱命名）效果差；官方流程建议此时用 `/api/v2/search` 手动搜索兜底，并**本地持久化 文件↔episodeId 的关联**避免重复识别。
- 一个 episodeId 可关联多个视频文件，但一个文件只能关联一个 episodeId；同一动画不同季度算不同"番剧"（animeId 不同）。
- 非动漫内容（欧美剧、综艺、纪录片等）覆盖很弱，基本无 hash 记录且弹幕稀少；日剧/电影有一定覆盖但同样有限。
- `shift` 字段提示部分关联弹幕存在时间轴偏移，客户端需按其值调整弹幕出现时间。


## Sources
https://doc.dandanplay.com/open/
https://api.dandanplay.net/swagger/v2/swagger.json
https://doc.dandanplay.com/open/changelog.html
https://github.com/kaedei/dandanplay-libraryindex/blob/master/api/OpenPlatform.md
https://github.com/warmstarts/dandanplay-potplayer-cli