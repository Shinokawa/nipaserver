//! 弹弹play match 接口请求/响应类型（对齐调研文档 §1 的 swagger 定义）
//! 与缓存语义三分类（[`MatchOutcome`] / [`classify`]）。

use serde::{Deserialize, Serialize};

/// 匹配模式（`MatchRequest.matchMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchMode {
    /// hash + 文件名（§4.1 默认策略）。
    #[serde(rename = "hashAndFileName")]
    HashAndFileName,
    #[serde(rename = "fileNameOnly")]
    FileNameOnly,
    #[serde(rename = "hashOnly")]
    HashOnly,
}

/// `POST /api/v2/match` 请求体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchRequest {
    /// 视频文件名，**不含文件夹路径和扩展名**。
    pub file_name: String,
    /// 文件前 16MB 的 MD5（hex，32 位，不区分大小写；见 [`crate::dandan_hash_bytes`]）。
    pub file_hash: String,
    /// 文件总长度（Byte）。
    pub file_size: i64,
    /// [可选] 视频时长（秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_duration: Option<i32>,
    pub match_mode: MatchMode,
}

impl MatchRequest {
    /// 以 §4.1 默认策略（`matchMode=hashAndFileName`）构造请求。
    ///
    /// `file_name` 需为**不含路径与扩展名**的裸文件名（如 `"[Sub] Title - 01"`）。
    pub fn new(file_name: impl Into<String>, file_hash: impl Into<String>, file_size: i64) -> Self {
        Self {
            file_name: file_name.into(),
            file_hash: file_hash.into(),
            file_size,
            video_duration: None,
            match_mode: MatchMode::HashAndFileName,
        }
    }
}

/// 作品类型（`AnimeType` 枚举；未知新值容错为 [`AnimeType::Unknown`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimeType {
    TvSeries,
    TvSpecial,
    Ova,
    Movie,
    MusicVideo,
    Web,
    Other,
    JpMovie,
    JpDrama,
    /// 对接 TMDB 的一般电视剧条目（§4.5 反查 TMDB 锚点的线索）。
    TmdbTv,
    TmdbMovie,
    /// `unknown` 或任何未识别的新枚举值。
    #[serde(other)]
    Unknown,
}

/// 单条匹配结果（`MatchResultV2`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResultV2 {
    /// 弹幕库 ID（节目编号，后续取弹幕用它）。
    pub episode_id: i64,
    /// 作品 ID。
    pub anime_id: i64,
    #[serde(default)]
    pub anime_title: Option<String>,
    #[serde(default)]
    pub episode_title: Option<String>,
    #[serde(rename = "type", default = "AnimeType::unknown")]
    pub anime_type: AnimeType,
    #[serde(default)]
    pub type_description: Option<String>,
    /// 弹幕偏移秒数（负数表示应提前）。
    #[serde(default)]
    pub shift: f64,
    /// 作品海报地址。
    #[serde(default)]
    pub image_url: Option<String>,
}

impl AnimeType {
    fn unknown() -> Self {
        AnimeType::Unknown
    }
}

/// `POST /api/v2/match` 响应（`MatchResponseV2`，含 `ResponseBase` 字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResponse {
    // --- ResponseBase ---
    #[serde(default)]
    pub error_code: i32,
    pub success: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    // --- MatchResponseV2 ---
    /// 是否已精确关联到某个弹幕库（true 时 matches 仅 1 项，客户端应自动选用）。
    #[serde(default)]
    pub is_matched: bool,
    #[serde(default)]
    pub matches: Vec<MatchResultV2>,
}

/// `POST /api/v2/match/batch` 单项结果（`BatchMatchResponseItem`）。
///
/// batch 只回精确命中：未命中项 `success=false`、`match_result=None`；
/// 结果与请求一一对应（顺序一致）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub success: bool,
    #[serde(default)]
    pub file_hash: Option<String>,
    #[serde(default)]
    pub match_result: Option<MatchResultV2>,
}

/// `POST /api/v2/match/batch` 请求体包装（`BatchMatchRequest`），crate 内部使用。
#[derive(Debug, Serialize)]
pub(crate) struct BatchMatchRequestBody<'a> {
    pub requests: &'a [MatchRequest],
}

/// `POST /api/v2/match/batch` 响应包装（`BatchMatchResponse`），crate 内部使用。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchMatchResponse {
    #[serde(default)]
    pub error_code: i32,
    pub success: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub results: Vec<BatchItem>,
}

/// match 结果的缓存语义三分类（§4.1；本 crate 只做分类，不做存储）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum MatchOutcome {
    /// 精确命中（`isMatched=true`）→ 上层直接入库。
    Exact(MatchResultV2),
    /// 未精确命中但有候选（按可能性降序）→ 作为 L2 的输入线索。
    Candidates(Vec<MatchResultV2>),
    /// 无任何结果 → 上层按"未匹配"缓存 3 天（沿用 NipaPlay 策略）。
    NoMatch,
}

/// 把成功的 match 响应归入三分类。
///
/// - `isMatched=true` 且有结果 → [`MatchOutcome::Exact`]（取首项；服务器约定仅 1 项）；
/// - 未精确命中但 matches 非空 → [`MatchOutcome::Candidates`]；
/// - matches 为空（含 `isMatched=true` 却无结果的异常防御）→ [`MatchOutcome::NoMatch`]。
pub fn classify(resp: MatchResponse) -> MatchOutcome {
    let mut matches = resp.matches;
    if resp.is_matched && !matches.is_empty() {
        MatchOutcome::Exact(matches.swap_remove(0))
    } else if matches.is_empty() {
        MatchOutcome::NoMatch
    } else {
        MatchOutcome::Candidates(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_json(episode_id: i64) -> serde_json::Value {
        serde_json::json!({
            "episodeId": episode_id,
            "animeId": 789,
            "animeTitle": "某作品",
            "episodeTitle": "第1话",
            "type": "tvseries",
            "typeDescription": "TV动画",
            "shift": -1.5,
            "imageUrl": "https://img.example/1.jpg"
        })
    }

    fn parse(v: serde_json::Value) -> MatchResponse {
        serde_json::from_value(v).expect("MatchResponse 解析失败")
    }

    #[test]
    fn classify_exact() {
        let resp = parse(serde_json::json!({
            "errorCode": 0, "success": true, "errorMessage": null,
            "isMatched": true,
            "matches": [result_json(101)]
        }));
        match classify(resp) {
            MatchOutcome::Exact(r) => {
                assert_eq!(r.episode_id, 101);
                assert_eq!(r.anime_type, AnimeType::TvSeries);
                assert_eq!(r.shift, -1.5);
            }
            other => panic!("应为 Exact，实为 {other:?}"),
        }
    }

    #[test]
    fn classify_candidates_preserves_order() {
        let resp = parse(serde_json::json!({
            "errorCode": 0, "success": true,
            "isMatched": false,
            "matches": [result_json(1), result_json(2), result_json(3)]
        }));
        match classify(resp) {
            MatchOutcome::Candidates(list) => {
                let ids: Vec<i64> = list.iter().map(|r| r.episode_id).collect();
                assert_eq!(ids, vec![1, 2, 3]);
            }
            other => panic!("应为 Candidates，实为 {other:?}"),
        }
    }

    #[test]
    fn classify_no_match() {
        let resp = parse(serde_json::json!({
            "errorCode": 0, "success": true,
            "isMatched": false,
            "matches": []
        }));
        assert_eq!(classify(resp), MatchOutcome::NoMatch);
    }

    #[test]
    fn classify_matched_but_empty_is_defensive_no_match() {
        let resp = parse(serde_json::json!({
            "errorCode": 0, "success": true,
            "isMatched": true,
            "matches": []
        }));
        assert_eq!(classify(resp), MatchOutcome::NoMatch);
    }

    #[test]
    fn unknown_anime_type_is_tolerated() {
        let r: MatchResultV2 = serde_json::from_value(serde_json::json!({
            "episodeId": 1, "animeId": 2, "type": "somethingNew"
        }))
        .unwrap();
        assert_eq!(r.anime_type, AnimeType::Unknown);
    }

    #[test]
    fn tmdb_types_deserialize() {
        for (s, expected) in [
            ("tmdbtv", AnimeType::TmdbTv),
            ("tmdbmovie", AnimeType::TmdbMovie),
            ("jpdrama", AnimeType::JpDrama),
            ("unknown", AnimeType::Unknown),
        ] {
            let r: MatchResultV2 = serde_json::from_value(serde_json::json!({
                "episodeId": 1, "animeId": 2, "type": s
            }))
            .unwrap();
            assert_eq!(r.anime_type, expected, "type={s}");
        }
    }

    #[test]
    fn match_request_serializes_camel_case_without_none_duration() {
        let req = MatchRequest::new("Some File - 01", "658d05841b9476ccc7420b3f0bb21c3b", 42);
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "fileName": "Some File - 01",
                "fileHash": "658d05841b9476ccc7420b3f0bb21c3b",
                "fileSize": 42,
                "matchMode": "hashAndFileName"
            })
        );
    }
}
