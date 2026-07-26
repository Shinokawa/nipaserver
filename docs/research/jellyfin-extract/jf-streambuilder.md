# Jellyfin 播放决策（StreamBuilder / PlaybackInfo）精读报告 — 供 nipa-stream 移植

精读文件（全部为绝对路径）：
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/Dlna/StreamBuilder.cs`（2479 行，核心判定）
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/Dlna/{DeviceProfile,DirectPlayProfile,TranscodingProfile,CodecProfile,ContainerProfile,SubtitleProfile,ProfileCondition,ProfileConditionValue,ProfileConditionType,ConditionProcessor,MediaOptions,StreamInfo}.cs`
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/Session/{TranscodeReason,PlayMethod}.cs`
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Model/MediaInfo/{PlaybackInfoResponse,PlaybackErrorCode}.cs`
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Controllers/MediaInfoController.cs`、`/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Helpers/MediaInfoHelper.cs`、`/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Models/MediaInfoDtos/PlaybackInfoDto.cs`
- 真实客户端 profile 样本：`/Users/sakiko/Desktop/nipaserver/reference/jellyfin/tests/Jellyfin.Model.Tests/Test Data/DeviceProfile-*.json`（Chrome/Safari/AndroidTV/WebOS 等 19 份，移植测试用例的金矿）

（Jellyfin 为 GPL，以下全部是逻辑/协议/参数提炼，非代码翻译。）

---

## 1. Direct Play / Direct Stream / Transcode 完整判定顺序

### 1.0 三种 PlayMethod 语义（enum 数值有意义，客户端会比较）
- `Transcode = 0`：重编码
- `DirectStream = 1`：remux 进兼容容器，流不重编码（视频 copy；音频可能转）
- `DirectPlay = 2`：文件原样发送（`/videos/{id}/stream?Static=true`）

### 1.1 API 层入口流程（MediaInfoController.GetPostedPlaybackInfo → MediaInfoHelper.SetDeviceSpecificData）

```
POST /Items/{itemId}/PlaybackInfo (body: PlaybackInfoDto)
  profile = body.DeviceProfile ?? 设备注册时上报的 capabilities.DeviceProfile
  info = 取 item 的所有 MediaSource（多版本文件），深拷贝，生成 PlaySessionId = 随机 GUID
  对每个 mediaSource:
    构造 MediaOptions {
      MaxBitrate = min(客户端请求的 maxStreamingBitrate, 服务器针对远程 IP 的限速),
      // 关键坑：Jellyfin 10.10 注释 "direct-stream http streaming is currently broken"
      // → 非强制时 options.EnableDirectStream 恒为 false，DirectStream 实际走
      //   GetVideoTranscodeProfile 的 rank 机制产生（见 1.4）
    }
    if !enableDirectPlay  → source.SupportsDirectPlay = false
    if !enableDirectStream || !allowVideoStreamCopy → source.SupportsDirectStream = false
    if !enableTranscoding → source.SupportsTranscoding = false
    streamInfo = 音频 ? GetOptimalAudioStream : GetOptimalVideoStream
    // 回写 MediaSource：
    source.SupportsDirectPlay  = (playMethod == DirectPlay)
    source.SupportsDirectStream = (playMethod == DirectPlay || DirectStream)
    if !SupportsDirectPlay && (SupportsTranscoding || SupportsDirectStream):
       playMethod = Transcode
       source.TranscodingUrl = streamInfo.ToUrl(...)   // 见第 3 节
    source.TranscodeReasons = streamInfo.TranscodeReasons
    source.DefaultAudioStreamIndex / DefaultSubtitleStreamIndex = 决策选中的流
    字幕流回写 DeliveryMethod / DeliveryUrl（External 时）
  多 MediaSource 排序（SortMediaSources）：
    被查询 item 自己的版本优先 → DirectPlay+本地文件 → DirectPlay/DirectStream →
    协议为 File → bitrate <= maxBitrate → 原始顺序
```

### 1.2 视频判定主流程（BuildVideoItem 伪代码）

```
fn build_video_item(source, options):
  选字幕流: options.SubtitleStreamIndex ?? 按 Score 最高者（同分时偏好可 External 直出的格式）
  选音频流: options.AudioStreamIndex ?? source.DefaultAudioStreamIndex
  候选音频集 candidateAudioStreams:
    - 用户显式指定 index → 只有该流，不做重选
    - 无偏好 → 全部音频流（若默认流 IsDefault，则限定在 IsDefault 流中）
    - 有语言偏好 → 限同语言；再叠加"偏好默认轨" → 同语言中的默认轨，否则所有默认轨

  bitrateLimitExceeded = source.Bitrate(未知时按 40Mbps) > maxBitrate（远程源不限）
  eligibleDP = EnableDirectPlay && (ForceDirectPlay || !bitrateLimitExceeded)
  eligibleDS = EnableDirectStream && (ForceDirectStream || !bitrateLimitExceeded)
  if source 是 DVD/BluRay 文件夹 → eligibleDP = false（强制 remux/转码）
  reasons = bitrateLimitExceeded ? ContainerBitrateExceedsLimit : 0

  if eligibleDP || eligibleDS:
    (profile, method, audioIdx, r) = get_video_direct_play_profile(...)   // 1.3
    reasons |= r
    if method == DirectPlay:
       容器归一化(见下), SubProtocol=http, 记录选中音轨与其原生 codec
    if method == DirectStream:
       容器归一化, 视频 codec 原样, 音频 codec 取 DirectPlayProfile 声明的列表
    字幕: GetSubtitleProfile(..., method) → 决定 Encode/Embed/External/Hls/Drop

  if method 不是 DirectPlay/DirectStream:
    (tcProfile, method2) = get_video_transcode_profile(...)   // 1.4，rank 机制
    if found:
       套用 TranscodingProfile（container/protocol/segment 参数）
       build_stream_video_item(...)     // 1.5，填 codec/码率/分辨率上限
       playMethod = Transcode（若 rank.Video==1 则实为 DirectStream——视频可 copy）
       字幕重新按 Transcode 决策
       if reasons 含视频类原因或超码率 → 应用 TranscodingProfile.Conditions 限制
  多 MediaSource 时排序取第一（DirectPlay本地文件 > DP/DS > File 协议 > 码率接近 maxBitrate）
```

容器归一化 `NormalizeMediaSourceFormatIntoSingleContainer`：ffprobe 的容器名可能是逗号列表（如 `mov,mp4,m4a,3gp`），取其中第一个被 DirectPlayProfile 支持的格式作为最终容器（决定 stream URL 的扩展名）。

### 1.3 GetVideoDirectPlayProfile（DirectPlay/DirectStream 打分核心）

```
if ForceDirectPlay → 直接 DirectPlay；if ForceDirectStream → 直接 DirectStream

预计算三组"档位失败原因"（与具体 DirectPlayProfile 无关）：
  containerReasons  = ContainerProfiles 中匹配当前容器的条件不满足项 → 映射 TranscodeReason
  videoCodecReasons = CodecProfiles(type=Video, codec/container 匹配, ApplyConditions 全满足)
                      的 Conditions 中不满足项 → 映射 TranscodeReason
  audioMatches[流]  = 每个候选音轨的 CodecProfiles(type=VideoAudio) 失败原因；
                      外挂音轨额外加 AudioIsExternal
  subtitleReasons   = 字幕按 DirectPlay 决策后若 method 不属于 {Drop, External, Embed}
                      → SubtitleCodecNotSupported

对每个 DirectPlayProfile(type=Video)，按声明顺序编号：
  r = 0
  容器不匹配 profile.Container       → r |= ContainerNotSupported
  视频 codec 不在 profile.VideoCodec → r |= VideoCodecNotSupported
  candidateAudioStreams 中找第一个 codec 在 profile.AudioCodec 的流:
     找不到 → r |= AudioCodecNotSupported；找到 → r |= audioMatches[该流]
  failure = r | containerReasons | subtitleReasons
  if 没有 VideoCodecNotSupported → failure |= videoCodecReasons   // 只有codec匹配才检查档位
  if 没有 AudioCodecNotSupported → failure |= 该音轨档位原因

  // DirectStream 可容忍的原因集合（换容器 remux + 音频转码即可解决）：
  DirectStreamReasons = 所有 Audio* 原因 | ContainerNotSupported | VideoCodecTagNotSupported
  dsFailure = failure & !DirectStreamReasons

  method = failure==0 && eligibleDP && source.SupportsDirectPlay   → DirectPlay
         : dsFailure==0 && eligibleDS && source.SupportsDirectStream → DirectStream
         : None

排序：method 降序（DirectPlay=2 优先）→ 失败类别排名降序 → 声明顺序升序
  排名序列（GetRank，越晚失败越好）：[VideoCodecNotSupported, 视频档位类, AudioCodecNotSupported, 音频档位类, 容器类]
取第一个 method != None 的；全失败时返回失败原因（优先取容器受支持的 profile 的原因；
全 0 时置 DirectPlayError）
```

### 1.4 GetVideoTranscodeProfile（转码档选择 + "假转码真 DirectStream"）

```
候选 = TranscodingProfiles.filter(Type==Video && Context==options.Context)
对每个候选打 rank (video, audio)，均为 1..3，越小越好：
  video: 源视频 codec ∈ profile.VideoCodec && AllowVideoStreamCopy
         && 视频档位条件全过 → 1；codec 匹配但档位有失败 → 2；否则 3
  audio: 对 profile.AudioCodec 列表逐个 codec 查 VideoAudio 档位条件
         （用该 codec 替换源 codec 评估）：无失败且与源 codec 相同 → 1（可 copy）；
         无失败但需转 → 2；有失败 → 3。取最小。
         // 关键设计：6ch flac 源、客户端支持 2ch flac + 6ch aac 时，
         // 选转 6ch aac 而不是降混 2ch flac
按 rank 排序取第一。若 rank.video == 1 → PlayMethod = DirectStream（视频 -c copy 的 remux），
否则 Transcode。
```

### 1.5 BuildStreamVideoItem（填输出参数，Transcode/DirectStream 共用）

- 输出视频 codec 列表 = profile.VideoCodec 列表（空则用源 codec）；HLS 时强制过滤到 `["h264","hevc","vp9","av1"]`；源 codec 不在列表 → 加 `VideoCodecNotSupported`。
- HLS 音频白名单：ts 容器 `["aac","ac3","eac3","mp3"]`；fmp4 容器 `["aac","ac3","eac3","mp3","alac","flac","opus","dts","truehd"]`。
- 音轨重选：候选中第一个 codec 受支持且 VideoAudio 条件全过、声道不超 TranscodingMaxAudioChannels、且没超码率 → 直接 copy 该音轨。
- 然后遍历 CodecProfiles，把满足 ApplyConditions 的 Conditions 转成输出上限（ApplyTranscodingConditions）：`VideoBitrate/VideoLevel/MaxWidth/MaxHeight/MaxFramerate/AudioBitrate/AudioSampleRate/audiochannels/profile...`，Equals 直接设值，LessThanEqual 取 min（GreaterThanEqual 无法表达，跳过）。
- 码率预算：`videoBitrate = clamp(maxBitrate - audioBitrate, 64_000, ...)`。
- 音频码率默认表（GetDefaultAudioBitrate）：aac/mp3/ac3/eac3 → mono 128k、stereo 384k、≥6ch 640k；flac/alac → 768k/1536k/3584k；其他 192k。总码率→音频码率上限阶梯（GetMaxAudioBitrateForTotalBitrate）：≤640k→128k，≤2M→384k，≤3M→448k，≤4M→640k，≤5M→768k，≤10M→1536k，≤15M→2304k，≤20M→3584k，否则 7168k。单声道且源 <64k 时编码上限 64k（webm 编码器 bug 规避）。

### 1.6 音频（纯音乐）判定（GetOptimalAudioStream）简表

```
DirectPlay: 存在 DirectPlayProfile(type=Audio) 容器和 codec 均匹配
  // mkv/webm 坑：mkv 别名 webm，但设备只报 webm 时不许直放 mkv 音频（issue #13344）
  且 Audio 类 CodecProfile 条件全过、码率不超 → DirectPlay
DirectStream: 容器不匹配但 codec 匹配（remux）→ 容器取 TranscodingContainer ?? "ts"，
  HLS 时 codec 需在上面 HLS 白名单内
Transcode: 取第一个 Type==Audio && Context 匹配且 ffmpeg 能编码目标 codec 的 TranscodingProfile,
  码率 = min(请求码率, MusicStreamingTranscodingBitrate(默认128k), 档位限制)
```

### 1.7 字幕决策（GetSubtitleProfile 优先级）

内嵌字幕、非转码或非 HLS 转码时：① 找 Embed 且格式相同 → ② Embed 且文本格式可转换。外挂或需提取时：③ External/Hls profile，格式相同直接给；不同时需源为文本字幕且可转换（`IsInfiniteStream` 直播源不给外挂）。都失败 → 图形字幕 `Encode`（烧录），文本字幕默认也可 Drop。DirectPlay 合法的 method 只有 `{Drop, External, Embed}`，否则整个 DirectPlay 失败并触发转码烧录。nipa 最小实现：外挂 srt/ass → External（`/videos/{id}/{source}/subtitles/{index}/stream.vtt` 之类），内嵌 PGS → 烧录。

### 1.8 TranscodeReason 全部枚举值（bit flags，序列化为逗号分隔字符串）

| 名称 | bit | 名称 | bit |
|---|---|---|---|
| ContainerNotSupported | 1<<0 | AudioChannelsNotSupported | 1<<14 |
| VideoCodecNotSupported | 1<<1 | AudioProfileNotSupported | 1<<15 |
| AudioCodecNotSupported | 1<<2 | AudioSampleRateNotSupported | 1<<16 |
| SubtitleCodecNotSupported | 1<<3 | AudioBitDepthNotSupported | 1<<17 |
| AudioIsExternal | 1<<4 | ContainerBitrateExceedsLimit | 1<<18 |
| SecondaryAudioNotSupported | 1<<5 | VideoBitrateNotSupported | 1<<19 |
| VideoProfileNotSupported | 1<<6 | AudioBitrateNotSupported | 1<<20 |
| VideoLevelNotSupported | 1<<7 | UnknownVideoStreamInfo | 1<<21 |
| VideoResolutionNotSupported | 1<<8 | UnknownAudioStreamInfo | 1<<22 |
| VideoBitDepthNotSupported | 1<<9 | DirectPlayError | 1<<23 |
| VideoFramerateNotSupported | 1<<10 | VideoRangeTypeNotSupported | 1<<24 |
| RefFramesNotSupported | 1<<11 | VideoCodecTagNotSupported | 1<<25 |
| AnamorphicVideoNotSupported | 1<<12 | StreamCountExceedsLimit | 1<<26 |
| InterlacedVideoNotSupported | 1<<13 | VideoRotationNotSupported | 1<<27 |

组合别名：`DirectStreamReasons = 全部音频原因 | ContainerNotSupported | VideoCodecTagNotSupported`——这是"remux 能否解决"的判据，务必照抄。

---

## 2. DeviceProfile 结构与客户端上报 JSON

### 2.1 完整结构（PascalCase JSON）

```jsonc
{
  "Name": "...", "Id": null,
  "MaxStreamingBitrate": 120000000,      // 转码流总码率上限，默认 8M
  "MaxStaticBitrate": 100000000,         // DirectPlay 上限，默认 8M
  "MusicStreamingTranscodingBitrate": 384000,
  "MaxStaticMusicBitrate": 8000000,
  "DirectPlayProfiles": [                // 能"原样播"什么
    { "Container": "mp4,m4v", "Type": "Video",
      "VideoCodec": "h264,hevc,vp9,av1", "AudioCodec": "aac,mp3,opus,flac" },
    { "Container": "mp3", "Type": "Audio" }   // codec 省略 = 全部支持
  ],
  "TranscodingProfiles": [               // 不能直放时转成什么（按声明顺序取第一可用）
    { "Container": "mp4", "Type": "Video", "VideoCodec": "av1,hevc,h264",
      "AudioCodec": "aac,mp2,opus,flac", "Context": "Streaming",
      "Protocol": "hls",                 // "http" | "hls"
      "MaxAudioChannels": "2", "MinSegments": 2, "SegmentLength": 0,
      "CopyTimestamps": false, "EnableAudioVbrEncoding": true }
  ],
  "ContainerProfiles": [],               // 容器级附加条件（少用）
  "CodecProfiles": [                     // codec 档位限制
    { "Type": "Video",                   // "Video" | "VideoAudio" | "Audio"
      "Codec": "h264", "Container": null, "SubContainer": null,
      "ApplyConditions": [],             // 前置条件（全过才检查 Conditions）
      "Conditions": [
        { "Condition": "LessThanEqual",  // Equals|NotEquals|LessThanEqual|GreaterThanEqual|EqualsAny
          "Property": "VideoLevel",      // 见 ProfileConditionValue 枚举
          "Value": "52",
          "IsRequired": false } ] }      // 值未知(null)时: IsRequired=true 判失败
  ],
  "SubtitleProfiles": [
    { "Format": "vtt", "Method": "External" },   // Encode|Embed|External|Hls|Drop
    { "Format": "ass", "Method": "Encode" } ]
}
```

要点：
- 所有 Container/Codec 字符串是**逗号分隔列表**；空/缺省 = 匹配一切；前缀 `-` 表示**取反列表**（如 `"-flv"` = 除 flv 外都行）。这是 ContainerHelper 的核心语义，必须实现。
- `ProfileConditionValue` 全集：AudioChannels, AudioBitrate, AudioProfile, Width, Height, Has64BitOffsets, PacketLength, VideoBitDepth, VideoBitrate, VideoFramerate, VideoLevel, VideoProfile, VideoTimestamp, IsAnamorphic, RefFrames, NumAudioStreams, NumVideoStreams, IsSecondaryAudio, VideoCodecTag, IsAvc, IsInterlaced, AudioSampleRate, AudioBitDepth, VideoRangeType, NumStreams, VideoRotation。
- `EqualsAny` 的 Value 用 `|` 分隔（如 `"high|main|baseline"`）。
- 条件求值：当前值为 null → 返回 `!IsRequired`；数值比较按 int/float/double；VideoRangeType 是枚举字符串（SDR/HDR10/HLG/DOVI...）。

### 2.2 nipa POST /playback/info 建议接受的最小子集

请求体（对应 Jellyfin PlaybackInfoDto，砍掉 LiveStream/UserId 相关）：

```jsonc
{
  "MediaSourceId": null,
  "MaxStreamingBitrate": 20000000,
  "StartTimeTicks": 0,               // 1 tick = 100ns，保持兼容
  "AudioStreamIndex": null,
  "SubtitleStreamIndex": null,
  "MaxAudioChannels": 2,
  "EnableDirectPlay": true, "EnableDirectStream": true, "EnableTranscoding": true,
  "AllowVideoStreamCopy": true, "AllowAudioStreamCopy": true,
  "AlwaysBurnInSubtitleWhenTranscoding": false,
  "DeviceProfile": {
    // 最小子集：4 个数组 + 2 个码率
    "MaxStreamingBitrate": ..., "MaxStaticBitrate": ...,
    "DirectPlayProfiles": [...],
    "TranscodingProfiles": [...],       // 只需支持 Container/Type/VideoCodec/AudioCodec/Protocol/Context/MaxAudioChannels
    "CodecProfiles": [...],             // 只需支持 Conditions + ApplyConditions，属性集可先做
                                        // VideoLevel/VideoProfile/VideoRangeType/Width/Height/
                                        // VideoBitDepth/VideoFramerate/IsAnamorphic/IsInterlaced/
                                        // AudioChannels/IsSecondaryAudio，其余属性未知按"通过"处理
    "SubtitleProfiles": [...]
  }
}
```

可以不做：ContainerProfiles（实际 profile 里几乎总是 `[]`）、Xml 属性、DLNA 专有字段（EnableAlbumArtInDidl 等，客户端会发但直接忽略即可——**反序列化务必容忍未知字段**）。DeviceProfile 为 null 时用服务器内置的保守默认 profile（如 h264/aac/mp4+hls）。

---

## 3. PlaybackInfo 响应结构与播放 URL 形态

### 3.1 响应（PlaybackInfoResponse）

```jsonc
{
  "MediaSources": [ MediaSourceInfo, ... ],
  "PlaySessionId": "32位hex",
  "ErrorCode": null    // "NotAllowed" | "NoCompatibleStream" | "RateLimitExceeded"
}
```

### 3.2 MediaSourceInfo 客户端实际消费的字段（最小集）

```jsonc
{
  "Id": "32位hex（Jellyfin 用 item guid N 格式）",
  "Protocol": "File", "Type": "Default",
  "Container": "mkv",              // 已归一化为单一容器
  "Path": "...", "Size": 123, "Name": "...",
  "RunTimeTicks": 72000000000,
  "Bitrate": 8000000,
  "SupportsDirectPlay": true,      // ← 决策结果写回这三个布尔
  "SupportsDirectStream": true,
  "SupportsTranscoding": true,
  "TranscodingUrl": null,          // 不能 DirectPlay 时必填，客户端直接拿来播
  "TranscodingContainer": "ts",
  "TranscodingSubProtocol": "hls", // "http" | "hls"
  "TranscodeReasons": "VideoCodecNotSupported, ContainerBitrateExceedsLimit",
  "DefaultAudioStreamIndex": 1, "DefaultSubtitleStreamIndex": 2,
  "MediaStreams": [                // ffprobe 结果，含每条流：
    { "Type": "Video|Audio|Subtitle", "Index": 0, "Codec": "hevc",
      "Profile": "Main 10", "Level": 120, "Width":3840,"Height":2160,
      "BitRate":..., "BitDepth":10, "VideoRangeType":"HDR10",
      "Channels":6, "SampleRate":48000, "Language":"jpn", "IsDefault":true,
      "IsExternal":false, "DeliveryMethod":"External", "DeliveryUrl":"/..." } ],
  "MediaAttachments": [ { "Index":0, "DeliveryUrl":"/Videos/{item}/{source}/Attachments/0" } ]
}
```

客户端行为契约（重要）：`SupportsDirectPlay==true` → 客户端自己拼 `/Videos/{itemId}/stream.{container}?Static=true&MediaSourceId=...&Tag=...&api_key=...`；否则用服务器给的 `TranscodingUrl`。

### 3.3 播放 URL 形态（StreamInfo.ToUrl 生成规则）

- DirectPlay/DirectStream(http)：`/videos/{itemId}/stream.{container}?Static=true&MediaSourceId=..&DeviceId=..&Tag={etag}&ApiKey=..&AudioStreamIndex=..`
- 转码 HLS：`/videos/{itemId}/master.m3u8?MediaSourceId=..&VideoCodec=h264,hevc&AudioCodec=aac,mp3&VideoBitrate=..&AudioBitrate=..&AudioSampleRate=..&MaxWidth=..&MaxHeight=..&MaxFramerate=..&SegmentContainer=ts|mp4&SegmentLength=..&MinSegments=..&AudioStreamIndex=..&SubtitleStreamIndex=..&SubtitleMethod=Encode&PlaySessionId=..&ApiKey=..&TranscodeReasons=..&{codec}-level=..&{codec}-profile=..`（StreamOptions 以 `h264-level=51` 这类 qualifier 键附加）
- 转码 http（音频常用）：`/audio/{itemId}/stream.{container}?...&StartTimeTicks=..`
- HLS 层级端点（DynamicHlsController）：`master.m3u8`（多码率主表）→ `main.m3u8`（媒体播放列表）→ `hls1/{playlistId}/{segmentId}.{container}`（段）。nipa 简化：master 只出一个 variant；PlaySessionId 即转码 session 键。

nipa 建议直接产完整 URL：`/playback/info` 响应里对每个源给 `direct_url`（带签名）或 `transcode_url`（`/stream/hls/{session}/master.m3u8`），避免让客户端拼 query（Jellyfin 客户端拼 URL 是历史包袱）。

---

## 4. nipa-stream 的 Rust 判定器设计建议

```rust
// ---------- 输入侧 ----------
#[derive(Deserialize)]                       // serde(rename_all="PascalCase") + deny 未知字段关闭
pub struct DeviceProfile {
    pub max_streaming_bitrate: Option<u64>,  // 默认 8_000_000
    pub max_static_bitrate: Option<u64>,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub codec_profiles: Vec<CodecProfile>,
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

pub struct DirectPlayProfile {
    pub kind: ProfileType,                   // Video | Audio
    pub container: CodecList,                // 见下
    pub video_codec: CodecList,
    pub audio_codec: CodecList,
}

/// Jellyfin 的逗号列表 + "-"取反语义，做成独立类型并重点单测
pub struct CodecList { items: Vec<String>, negated: bool }
impl CodecList {
    /// 空列表匹配一切；输入本身也可能是逗号列表（ffprobe 容器名），任一命中即命中
    pub fn matches(&self, input: &str) -> bool { ... }
}

pub struct CodecProfile {
    pub kind: CodecType,                     // Video | VideoAudio | Audio
    pub codec: CodecList, pub container: CodecList,
    pub apply_conditions: Vec<Condition>,    // 全满足才启用 conditions
    pub conditions: Vec<Condition>,
}
pub struct Condition {
    pub op: CondOp,                          // Equals/NotEquals/Lte/Gte/EqualsAny
    pub property: CondProperty,              // 枚举，先实现 12 个常用项
    pub value: String,                       // EqualsAny 用 '|' 分隔
    pub is_required: bool,                   // 值未知时 required→失败
}

bitflags::bitflags! {                        // 与 Jellyfin bit 位一一对应，序列化为逗号名字列表
    pub struct TranscodeReason: u32 { const CONTAINER_NOT_SUPPORTED = 1<<0; /* ...同上表 */ }
}
impl TranscodeReason {
    pub const DIRECT_STREAM_TOLERABLE: Self = /* 音频全部 | 容器 | codec tag */;
}

// 媒体侧：来自 ffprobe（nipa 已有 sidecar），等价 MediaSourceInfo
pub struct MediaSource { pub id: String, pub container: String, pub bitrate: Option<u64>,
    pub runtime_ticks: Option<i64>, pub video: Option<VideoStreamInfo>,
    pub audios: Vec<AudioStreamInfo>, pub subtitles: Vec<SubtitleStreamInfo>, ... }

pub struct PlayRequest {                     // 等价 MediaOptions
    pub max_bitrate: Option<u64>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub max_audio_channels: Option<u32>,
    pub enable_direct_play: bool, pub enable_transcoding: bool,
    pub allow_video_stream_copy: bool, pub allow_audio_stream_copy: bool,
    pub burn_in_subtitle: bool,
}

// ---------- 输出侧 ----------
pub enum PlayMethod { Transcode, DirectStream, DirectPlay }   // 保持这个次序便于 Ord 排序

pub struct PlayDecision {                    // 等价 StreamInfo 精简版
    pub method: PlayMethod,
    pub reasons: TranscodeReason,
    pub container: String,                   // 输出容器（DirectPlay=归一化源容器）
    pub protocol: StreamProtocol,            // Http | Hls
    pub video_codec: Option<String>,         // Transcode 目标；DirectStream 时 = 源 codec(copy)
    pub audio_codec: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle: SubtitleDecision,          // { index, method: Encode/External/Embed/Drop, format }
    pub video_bitrate: Option<u64>,          // 转码预算: clamp(max - audio, 64k, ..)
    pub audio_bitrate: Option<u64>,
    pub max_width: Option<u32>, pub max_height: Option<u32>,
    pub max_framerate: Option<f32>,
    pub audio_channels: Option<u32>,
}

// ---------- 决策函数 ----------
/// 纯函数、无 IO，方便对着 Jellyfin 的 19 份 DeviceProfile-*.json +
/// MediaSourceInfo-*.json 测试数据写 golden test
pub fn decide_video(profile: &DeviceProfile, source: &MediaSource,
                    req: &PlayRequest) -> PlayDecision;
pub fn decide_audio(profile: &DeviceProfile, source: &MediaSource,
                    req: &PlayRequest) -> PlayDecision;

// 内部分层（对应 1.3/1.4/1.5）：
fn try_direct(profile, source, req, candidates: &[AudioStreamInfo])
    -> Result<(usize /*dp profile idx*/, PlayMethod, Option<i32>), TranscodeReason>;
fn pick_transcode(profile, source, req)
    -> Option<(usize /*tc profile idx*/, PlayMethod /*DirectStream if video copyable*/)>;
fn eval_conditions(conds: &[Condition], ctx: &StreamCtx) -> TranscodeReason; // 不满足项聚合
fn apply_output_limits(decision: &mut PlayDecision, conds: &[Condition]);    // Equals 设值/Lte 取 min
```

HTTP 层（axum handler）：`POST /playback/info` → 组装 `PlaybackInfoResponse { media_sources, play_session_id }`，每个 source 回填 `supports_direct_play/direct_stream/transcoding + transcode_reasons + direct_url/transcode_url`。`PlayDecision` 同时是 HLS session 的构造参数（喂给 nipa-stream 的 ffmpeg 参数生成器：`-c:v copy` 当 DirectStream、否则 videotoolbox/libx264 + decision 的码率/分辨率上限）。

### 移植取舍与坑（按重要度）

1. **必须照抄的语义**：CodecList 的空=全部/`-`取反/逗号列表双向匹配；`DirectStreamReasons` 掩码；条件 null 值→`!is_required`；DirectPlayProfile 按声明顺序优先；TranscodingProfile 按声明顺序取第一可用。
2. **rank 机制别省**：转码档选择的 (video,audio) rank 决定了"视频 copy + 音频转码"这条最常见路径（mkv/hevc/dts → hls/hevc copy/aac）。这就是 Jellyfin 里 DirectStream 的真正来源——不是 1.3 里那条（10.10 里 http direct-stream 被禁用）。nipa 可以只实现 rank 路径，1.3 只判 DirectPlay。
3. **HLS 白名单硬编码**：视频 `h264,hevc,vp9,av1`；音频 ts=`aac,ac3,eac3,mp3`、fmp4 多 `alac,flac,opus,dts,truehd`。客户端 profile 说支持也要过滤。
4. **码率未知按 40Mbps**、远程源不限码率、`ContainerBitrateExceedsLimit` 是最高频转码原因之一——bitrate 判定放在最前面。
5. **mkv/webm 别名坑**（Jellyfin issue #13344）：设备只声明 webm 时不要直放 mkv。
6. **多版本排序**：被请求 item 自身的 MediaSource 永远排第一（续播状态挂在它上面），宁可转码也不换版本。
7. 简化空间：ContainerProfiles、NumStreams/PacketLength/Has64BitOffsets/VideoTimestamp 等冷门条件属性、LiveStream 全链路、DLNA 字段都可以不实现；条件属性遇到未实现的一律按"满足"处理并打日志。
8. **测试资产**：`tests/Jellyfin.Model.Tests/Dlna/StreamBuilderTests.cs` + `Test Data/` 下的 profile/source JSON 是现成的兼容性用例库（JSON 数据本身描述客户端能力，直接拿来做 nipa 的 fixture 不构成代码翻译；测试逻辑要自己写）。