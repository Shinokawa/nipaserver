//! 五个只读 agent 工具（开发文档 §4.2 工具集，契约 §3 `Tool` trait）。
//!
//! 安全论证（开发文档 §4.2 prompt 安全）：工具全为只读查询，写入只经
//! `submit_result` 置信度闸门——字幕/文件名等不可信输入即使注入指令，
//! 能触发的也只是元数据检索。
//!
//! 错误约定：上游 4xx/5xx/超时/解析失败一律 `ToolError::RespondToModel`
//! （回喂模型让它换关键词/换数据源）；仅配置缺失用 `Fatal`。
//! 返回 JSON 刻意精简（截断长文本、丢弃无关字段）以省 token。

use std::sync::Arc;

use nipa_agent::{BoxFuture, Tool, ToolError, ToolOutput};
use serde_json::{Value, json};

use crate::bangumi::BangumiClient;
use crate::error::ProviderError;
use crate::tmdb::{MediaType, TmdbClient};

/// 搜索结果最多返回条数（每类）。
const MAX_SEARCH_HITS: usize = 5;
/// 章节/剧集列表最多返回条数。
const MAX_EPISODES: usize = 40;

/// 组装工具集：TMDB 无 key（`tmdb` 为 `None`）时只返回 Bangumi 工具
/// （无 key 降级路径，开发文档 §4.3）。
pub fn build_tools(tmdb: Option<Arc<TmdbClient>>, bgm: Arc<BangumiClient>) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    if let Some(tmdb) = tmdb {
        tools.push(Arc::new(SearchTmdb {
            client: tmdb.clone(),
        }));
        tools.push(Arc::new(GetTmdbDetail {
            client: tmdb.clone(),
        }));
        tools.push(Arc::new(GetTmdbSeason { client: tmdb }));
    }
    tools.push(Arc::new(SearchBangumi {
        client: bgm.clone(),
    }));
    tools.push(Arc::new(GetBangumiSubject { client: bgm }));
    tools
}

// ===== 共用小工具 =====

fn provider_err(e: ProviderError) -> ToolError {
    match e {
        ProviderError::MissingConfig(m) | ProviderError::Init(m) => ToolError::Fatal(m),
        other => ToolError::RespondToModel(other.to_string()),
    }
}

/// 按字符数截断（避免截断 UTF-8 多字节字符），超限追加省略号。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 从 "2020-07-01" 形式的日期取年份。
fn year_of(date: Option<&str>) -> Value {
    date.and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<i32>().ok())
        .map(Value::from)
        .unwrap_or(Value::Null)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(ToolError::RespondToModel(format!(
            "参数 `{key}` 缺失或为空，需为非空字符串"
        ))),
    }
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, ToolError> {
    args.get(key).and_then(Value::as_i64).ok_or_else(|| {
        ToolError::RespondToModel(format!("参数 `{key}` 缺失或不是整数"))
    })
}

fn opt_year(args: &Value) -> Result<Option<i32>, ToolError> {
    match args.get("year") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(|y| Some(y as i32))
            .ok_or_else(|| ToolError::RespondToModel("参数 `year` 需为整数".into())),
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

// ===== search_tmdb =====

struct SearchTmdb {
    client: Arc<TmdbClient>,
}

/// 把 /search/tv 或 /search/movie 的一条 result 精简为工具输出行。
fn slim_tmdb_hit(r: &Value, media_type: MediaType) -> Value {
    let (name_key, orig_key, date_key) = match media_type {
        MediaType::Tv => ("name", "original_name", "first_air_date"),
        MediaType::Movie => ("title", "original_title", "release_date"),
    };
    json!({
        "id": r.get("id").cloned().unwrap_or(Value::Null),
        "media_type": media_type.as_str(),
        "name": str_field(r, name_key),
        "original_name": str_field(r, orig_key),
        "year": year_of(str_field(r, date_key)),
        "overview": str_field(r, "overview").map(|s| truncate_chars(s, 120)),
    })
}

impl Tool for SearchTmdb {
    fn name(&self) -> &str {
        "search_tmdb"
    }

    fn description(&self) -> &str {
        "搜索 TMDB 剧集/电影（中文优先）。可选按年份过滤消歧。返回每类最多 5 条候选。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "作品名关键词" },
                "year": { "type": "integer", "description": "首播/上映年份，可选" },
                "media_type": {
                    "type": "string",
                    "enum": ["tv", "movie", "both"],
                    "description": "搜索类型，默认 both"
                }
            },
            "required": ["query"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let query = arg_str(&arguments, "query")?.to_string();
            let year = opt_year(&arguments)?;
            let media_type = match arguments.get("media_type").and_then(Value::as_str) {
                None | Some("both") => None,
                Some("tv") => Some(MediaType::Tv),
                Some("movie") => Some(MediaType::Movie),
                Some(other) => {
                    return Err(ToolError::RespondToModel(format!(
                        "media_type `{other}` 无效，需为 tv|movie|both"
                    )));
                }
            };

            let mut hits = Vec::new();
            if media_type != Some(MediaType::Movie) {
                let resp = self
                    .client
                    .search_tv(&query, year)
                    .await
                    .map_err(provider_err)?;
                if let Some(results) = resp.get("results").and_then(Value::as_array) {
                    hits.extend(
                        results
                            .iter()
                            .take(MAX_SEARCH_HITS)
                            .map(|r| slim_tmdb_hit(r, MediaType::Tv)),
                    );
                }
            }
            if media_type != Some(MediaType::Tv) {
                let resp = self
                    .client
                    .search_movie(&query, year)
                    .await
                    .map_err(provider_err)?;
                if let Some(results) = resp.get("results").and_then(Value::as_array) {
                    hits.extend(
                        results
                            .iter()
                            .take(MAX_SEARCH_HITS)
                            .map(|r| slim_tmdb_hit(r, MediaType::Movie)),
                    );
                }
            }
            Ok(ToolOutput::json(&json!({ "results": hits })))
        })
    }
}

// ===== get_tmdb_detail =====

struct GetTmdbDetail {
    client: Arc<TmdbClient>,
}

impl Tool for GetTmdbDetail {
    fn name(&self) -> &str {
        "get_tmdb_detail"
    }

    fn description(&self) -> &str {
        "获取 TMDB 条目详情（含 IMDB/TVDB 外部 ID；剧集含季数）。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "TMDB 条目 id" },
                "media_type": { "type": "string", "enum": ["tv", "movie"] }
            },
            "required": ["id", "media_type"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let id = arg_i64(&arguments, "id")?;
            let media_type = match arg_str(&arguments, "media_type")? {
                "tv" => MediaType::Tv,
                "movie" => MediaType::Movie,
                other => {
                    return Err(ToolError::RespondToModel(format!(
                        "media_type `{other}` 无效，需为 tv|movie"
                    )));
                }
            };

            let d = self
                .client
                .detail(media_type, id)
                .await
                .map_err(provider_err)?;

            let (name_key, orig_key, date_key) = match media_type {
                MediaType::Tv => ("name", "original_name", "first_air_date"),
                MediaType::Movie => ("title", "original_title", "release_date"),
            };
            let ext = d.get("external_ids").cloned().unwrap_or(Value::Null);
            let mut out = json!({
                "id": d.get("id").cloned().unwrap_or(Value::Null),
                "name": str_field(&d, name_key),
                "original_name": str_field(&d, orig_key),
                "year": year_of(str_field(&d, date_key)),
                "overview": str_field(&d, "overview").map(|s| truncate_chars(s, 200)),
                "external_ids": {
                    "imdb": ext.get("imdb_id").cloned().unwrap_or(Value::Null),
                    "tvdb": ext.get("tvdb_id").cloned().unwrap_or(Value::Null),
                },
            });
            if let Some(n) = d.get("number_of_seasons").and_then(Value::as_i64) {
                out["number_of_seasons"] = json!(n);
            }
            Ok(ToolOutput::json(&out))
        })
    }
}

// ===== get_tmdb_season =====

struct GetTmdbSeason {
    client: Arc<TmdbClient>,
}

impl Tool for GetTmdbSeason {
    fn name(&self) -> &str {
        "get_tmdb_season"
    }

    fn description(&self) -> &str {
        "获取 TMDB 剧集某一季的分集列表（集号/标题/播出日期，最多 40 条）。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "series_id": { "type": "integer", "description": "TMDB 剧集 id" },
                "season": { "type": "integer", "description": "季号（0 为特别篇季）" }
            },
            "required": ["series_id", "season"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let series_id = arg_i64(&arguments, "series_id")?;
            let season = arg_i64(&arguments, "season")? as i32;

            let d = self
                .client
                .season_episodes(series_id, season)
                .await
                .map_err(provider_err)?;

            let episodes: Vec<Value> = d
                .get("episodes")
                .and_then(Value::as_array)
                .map(|eps| {
                    eps.iter()
                        .take(MAX_EPISODES)
                        .map(|e| {
                            json!({
                                "episode": e.get("episode_number").cloned().unwrap_or(Value::Null),
                                "name": str_field(e, "name"),
                                "air_date": str_field(e, "air_date"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let episode_count = d
                .get("episodes")
                .and_then(Value::as_array)
                .map(|e| e.len())
                .unwrap_or(0);

            Ok(ToolOutput::json(&json!({
                "episode_count": episode_count,
                "episodes": episodes,
            })))
        })
    }
}

// ===== search_bangumi =====

struct SearchBangumi {
    client: Arc<BangumiClient>,
}

impl Tool for SearchBangumi {
    fn name(&self) -> &str {
        "search_bangumi"
    }

    fn description(&self) -> &str {
        "搜索 Bangumi 动画条目（天然中文数据）。返回最多 5 条候选。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string", "description": "作品名关键词" }
            },
            "required": ["keyword"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let keyword = arg_str(&arguments, "keyword")?.to_string();
            let resp = self
                .client
                .search_subjects(&keyword, None)
                .await
                .map_err(provider_err)?;

            let hits: Vec<Value> = resp
                .get("data")
                .and_then(Value::as_array)
                .map(|data| {
                    data.iter()
                        .take(MAX_SEARCH_HITS)
                        .map(|s| {
                            json!({
                                "id": s.get("id").cloned().unwrap_or(Value::Null),
                                "name": str_field(s, "name"),
                                "name_cn": str_field(s, "name_cn"),
                                "air_date": str_field(s, "date"),
                                "summary": str_field(s, "summary")
                                    .map(|t| truncate_chars(t, 120)),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(ToolOutput::json(&json!({ "results": hits })))
        })
    }
}

// ===== get_bangumi_subject =====

struct GetBangumiSubject {
    client: Arc<BangumiClient>,
}

impl Tool for GetBangumiSubject {
    fn name(&self) -> &str {
        "get_bangumi_subject"
    }

    fn description(&self) -> &str {
        "获取 Bangumi 条目详情；with_episodes=true 时附带本篇章节列表（sort/ep 用于对齐集数）。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Bangumi subject id" },
                "with_episodes": {
                    "type": "boolean",
                    "description": "是否附带本篇章节列表，默认 false"
                }
            },
            "required": ["id"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let id = arg_i64(&arguments, "id")?;
            let with_episodes = arguments
                .get("with_episodes")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let d = self.client.subject_detail(id).await.map_err(provider_err)?;
            let mut out = json!({
                "id": d.get("id").cloned().unwrap_or(Value::Null),
                "name": str_field(&d, "name"),
                "name_cn": str_field(&d, "name_cn"),
                "air_date": str_field(&d, "date"),
                "summary": str_field(&d, "summary").map(|s| truncate_chars(s, 200)),
                "total_episodes": d.get("total_episodes").cloned().unwrap_or(Value::Null),
            });

            if with_episodes {
                let eps = self
                    .client
                    .subject_episodes(id)
                    .await
                    .map_err(provider_err)?;
                let episodes: Vec<Value> = eps
                    .get("data")
                    .and_then(Value::as_array)
                    .map(|data| {
                        data.iter()
                            .take(MAX_EPISODES)
                            .map(|e| {
                                json!({
                                    "sort": e.get("sort").cloned().unwrap_or(Value::Null),
                                    "ep": e.get("ep").cloned().unwrap_or(Value::Null),
                                    "name": str_field(e, "name"),
                                    "name_cn": str_field(e, "name_cn"),
                                    "airdate": str_field(e, "airdate"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out["episodes"] = json!(episodes);
            }

            Ok(ToolOutput::json(&out))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "春らんまん、季節は繰り返す";
        let t = truncate_chars(s, 5);
        assert_eq!(t, "春らんまん…");
        assert_eq!(truncate_chars("short", 120), "short");
    }

    #[test]
    fn year_of_parses_prefix() {
        assert_eq!(year_of(Some("2020-07-01")), json!(2020));
        assert_eq!(year_of(Some("")), Value::Null);
        assert_eq!(year_of(None), Value::Null);
    }
}
