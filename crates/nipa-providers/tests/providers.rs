//! wiremock 集成测试：TMDB/Bangumi 客户端主要方法、工具端到端、缓存命中。

use std::sync::Arc;

use nipa_agent::Tool;
use nipa_providers::bangumi::BangumiClient;
use nipa_providers::tmdb::TmdbClient;
use nipa_providers::tools::build_tools;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn tmdb_client(server: &MockServer) -> Arc<TmdbClient> {
    TmdbClient::new("test-token", Some(server.uri())).expect("token 非空")
}

fn bgm_client(server: &MockServer) -> Arc<BangumiClient> {
    BangumiClient::new(Some(server.uri()), None).expect("默认 UA 非空")
}

fn find_tool(tools: &[Arc<dyn Tool>], name: &str) -> Arc<dyn Tool> {
    tools
        .iter()
        .find(|t| t.name() == name)
        .unwrap_or_else(|| panic!("缺少工具 {name}"))
        .clone()
}

// ===== 构造函数 =====

#[test]
fn tmdb_new_rejects_empty_token() {
    assert!(TmdbClient::new("", None).is_err());
    assert!(TmdbClient::new("   ", None).is_err());
}

#[test]
fn bangumi_new_rejects_empty_user_agent() {
    assert!(BangumiClient::new(None, Some("  ".into())).is_err());
}

#[test]
fn build_tools_without_tmdb_only_returns_bangumi_tools() {
    let bgm = BangumiClient::new(None, None).unwrap();
    let tools = build_tools(None, bgm);
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names, ["search_bangumi", "get_bangumi_subject"]);
}

#[test]
fn build_tools_with_tmdb_returns_all_five() {
    let bgm = BangumiClient::new(None, None).unwrap();
    let tmdb = TmdbClient::new("t", None).unwrap();
    let tools = build_tools(Some(tmdb), bgm);
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(
        names,
        [
            "search_tmdb",
            "get_tmdb_detail",
            "get_tmdb_season",
            "search_bangumi",
            "get_bangumi_subject",
        ]
    );
}

// ===== TMDB 客户端 =====

#[tokio::test]
async fn tmdb_search_tv_sends_bearer_language_and_year() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .and(header("authorization", "Bearer test-token"))
        .and(query_param("query", "葬送的芙莉莲"))
        .and(query_param("language", "zh-CN"))
        .and(query_param("first_air_date_year", "2023"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 209867, "name": "葬送的芙莉莲" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let resp = client.search_tv("葬送的芙莉莲", Some(2023)).await.unwrap();
    assert_eq!(resp["results"][0]["id"], 209867);
}

#[tokio::test]
async fn tmdb_search_movie_uses_year_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .and(query_param("query", "铃芽之旅"))
        .and(query_param("language", "zh-CN"))
        .and(query_param("year", "2022"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "id": 916224, "title": "铃芽之旅" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let resp = client.search_movie("铃芽之旅", Some(2022)).await.unwrap();
    assert_eq!(resp["results"][0]["id"], 916224);
}

#[tokio::test]
async fn tmdb_tv_detail_appends_external_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/209867"))
        .and(query_param("language", "zh-CN"))
        .and(query_param("append_to_response", "external_ids"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 209867,
            "name": "葬送的芙莉莲",
            "external_ids": { "imdb_id": "tt22248376", "tvdb_id": 424536 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let resp = client.tv_detail(209867).await.unwrap();
    assert_eq!(resp["external_ids"]["imdb_id"], "tt22248376");
}

#[tokio::test]
async fn tmdb_movie_detail_hits_movie_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/movie/916224"))
        .and(query_param("append_to_response", "external_ids"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 916224,
            "title": "铃芽之旅",
            "external_ids": { "imdb_id": "tt14169960" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let resp = client.movie_detail(916224).await.unwrap();
    assert_eq!(resp["title"], "铃芽之旅");
}

#[tokio::test]
async fn tmdb_season_episodes_hits_season_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/209867/season/1"))
        .and(query_param("language", "zh-CN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "episodes": [
                { "episode_number": 1, "name": "旅の終わり", "air_date": "2023-09-29" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let resp = client.season_episodes(209867, 1).await.unwrap();
    assert_eq!(resp["episodes"][0]["episode_number"], 1);
}

#[tokio::test]
async fn tmdb_upstream_404_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/1"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "status_message": "The resource you requested could not be found."
        })))
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    assert!(client.tv_detail(1).await.is_err());
}

// ===== Bangumi 客户端 =====

#[tokio::test]
async fn bangumi_search_sends_ua_and_type_filter() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/search/subjects"))
        .and(header(
            "user-agent",
            "AimesSoft/nipaserver/0.1 (https://github.com/AimesSoft/nipaserver)",
        ))
        .and(body_partial_json(json!({
            "keyword": "葬送的芙莉莲",
            "filter": { "type": [2] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": 400602, "name": "葬送のフリーレン" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = bgm_client(&server);
    let resp = client.search_subjects("葬送的芙莉莲", None).await.unwrap();
    assert_eq!(resp["data"][0]["id"], 400602);
}

#[tokio::test]
async fn bangumi_search_includes_air_date_filter() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/search/subjects"))
        .and(body_partial_json(json!({
            "filter": { "type": [2], "air_date": [">=2023-01-01", "<2024-01-01"] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;

    let client = bgm_client(&server);
    client
        .search_subjects(
            "芙莉莲",
            Some(vec![">=2023-01-01".into(), "<2024-01-01".into()]),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn bangumi_custom_user_agent_is_sent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/subjects/400602"))
        .and(header("user-agent", "custom/ua/1.0 (https://example.com)"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 400602 })))
        .expect(1)
        .mount(&server)
        .await;

    let client = BangumiClient::new(
        Some(server.uri()),
        Some("custom/ua/1.0 (https://example.com)".into()),
    )
    .unwrap();
    client.subject_detail(400602).await.unwrap();
}

#[tokio::test]
async fn bangumi_subject_episodes_queries_type_0() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/episodes"))
        .and(query_param("subject_id", "400602"))
        .and(query_param("type", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "sort": 1.0, "ep": 1.0, "name": "旅の終わり" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = bgm_client(&server);
    let resp = client.subject_episodes(400602).await.unwrap();
    assert_eq!(resp["data"][0]["ep"], 1.0);
}

// ===== 缓存命中（第二次调用不发请求，expect(1) 验证）=====

#[tokio::test]
async fn tmdb_detail_second_call_hits_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/209867"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 209867 })))
        .expect(1) // 第二次调用若打上游，drop 时 verify 失败
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    let a = client.tv_detail(209867).await.unwrap();
    let b = client.tv_detail(209867).await.unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn bangumi_search_second_call_hits_cache() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/search/subjects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;

    let client = bgm_client(&server);
    client.search_subjects("同一关键词", None).await.unwrap();
    client.search_subjects("同一关键词", None).await.unwrap();
}

#[tokio::test]
async fn tmdb_cache_key_distinguishes_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .expect(2) // 不同 year 不能共用缓存
        .mount(&server)
        .await;

    let client = tmdb_client(&server);
    client.search_tv("q", Some(2023)).await.unwrap();
    client.search_tv("q", Some(2024)).await.unwrap();
}

// ===== 工具端到端（mock 上游 → tool.call → 验证精简 JSON 形状）=====

#[tokio::test]
async fn search_tmdb_tool_end_to_end_slims_output() {
    let server = MockServer::start().await;
    let long_overview = "字".repeat(300);
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 1,
            "total_results": 7,
            "results": (0..7).map(|i| json!({
                "id": 1000 + i,
                "name": format!("剧集{i}"),
                "original_name": format!("Show {i}"),
                "first_air_date": "2023-09-29",
                "overview": long_overview,
                "popularity": 12.3,
                "vote_average": 8.9
            })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "id": 2000,
                "title": "电影",
                "original_title": "Movie",
                "release_date": "2022-11-11",
                "overview": "短简介"
            }]
        })))
        .mount(&server)
        .await;

    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));
    let tool = find_tool(&tools, "search_tmdb");
    let out = tool
        .call(json!({ "query": "剧集", "year": 2023 }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();

    let results = v["results"].as_array().unwrap();
    // tv 截到 5 条 + movie 1 条。
    assert_eq!(results.len(), 6);
    let first = &results[0];
    assert_eq!(first["media_type"], "tv");
    assert_eq!(first["id"], 1000);
    assert_eq!(first["name"], "剧集0");
    assert_eq!(first["original_name"], "Show 0");
    assert_eq!(first["year"], 2023);
    // overview 截 120 字 + 省略号。
    assert_eq!(first["overview"].as_str().unwrap().chars().count(), 121);
    // 精简形状：不透传上游无关字段。
    assert!(first.get("popularity").is_none());
    assert!(first.get("vote_average").is_none());
    assert_eq!(results[5]["media_type"], "movie");
    assert_eq!(results[5]["year"], 2022);
}

#[tokio::test]
async fn search_tmdb_tool_media_type_tv_skips_movie_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .expect(1)
        .mount(&server)
        .await;
    // 不 mock /search/movie：若被调用，未匹配请求会 404 → 测试报错。

    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));
    let tool = find_tool(&tools, "search_tmdb");
    let out = tool
        .call(json!({ "query": "q", "media_type": "tv" }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(v["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_tmdb_detail_tool_shapes_external_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/209867"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 209867,
            "name": "葬送的芙莉莲",
            "original_name": "葬送のフリーレン",
            "first_air_date": "2023-09-29",
            "overview": "长".repeat(500),
            "number_of_seasons": 1,
            "external_ids": { "imdb_id": "tt22248376", "tvdb_id": 424536, "wikidata_id": "Q117467698" }
        })))
        .mount(&server)
        .await;

    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));
    let tool = find_tool(&tools, "get_tmdb_detail");
    let out = tool
        .call(json!({ "id": 209867, "media_type": "tv" }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();

    assert_eq!(v["id"], 209867);
    assert_eq!(v["name"], "葬送的芙莉莲");
    assert_eq!(v["original_name"], "葬送のフリーレン");
    assert_eq!(v["year"], 2023);
    assert_eq!(v["overview"].as_str().unwrap().chars().count(), 201);
    assert_eq!(v["external_ids"]["imdb"], "tt22248376");
    assert_eq!(v["external_ids"]["tvdb"], 424536);
    assert!(v["external_ids"].get("wikidata_id").is_none());
    assert_eq!(v["number_of_seasons"], 1);
}

#[tokio::test]
async fn get_tmdb_season_tool_caps_episodes_at_40() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/1/season/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "episodes": (1..=50).map(|i| json!({
                "episode_number": i,
                "name": format!("第{i}话"),
                "air_date": "2023-01-01",
                "runtime": 24
            })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;

    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));
    let tool = find_tool(&tools, "get_tmdb_season");
    let out = tool
        .call(json!({ "series_id": 1, "season": 1 }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();

    assert_eq!(v["episode_count"], 50);
    let eps = v["episodes"].as_array().unwrap();
    assert_eq!(eps.len(), 40);
    assert_eq!(eps[0]["episode"], 1);
    assert_eq!(eps[0]["name"], "第1话");
    assert_eq!(eps[0]["air_date"], "2023-01-01");
    assert!(eps[0].get("runtime").is_none());
}

#[tokio::test]
async fn search_bangumi_tool_end_to_end_slims_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/search/subjects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 6,
            "data": (0..6).map(|i| json!({
                "id": 400600 + i,
                "name": format!("作品{i}"),
                "name_cn": format!("中文名{i}"),
                "date": "2023-09-29",
                "summary": "梗".repeat(200),
                "rank": 12,
                "images": { "large": "https://lain.bgm.tv/x.jpg" }
            })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;

    let tools = build_tools(None, bgm_client(&server));
    let tool = find_tool(&tools, "search_bangumi");
    let out = tool.call(json!({ "keyword": "作品" })).await.unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();

    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 5); // 最多 5 条
    let first = &results[0];
    assert_eq!(first["id"], 400600);
    assert_eq!(first["name"], "作品0");
    assert_eq!(first["name_cn"], "中文名0");
    assert_eq!(first["air_date"], "2023-09-29");
    assert_eq!(first["summary"].as_str().unwrap().chars().count(), 121);
    assert!(first.get("images").is_none());
    assert!(first.get("rank").is_none());
}

#[tokio::test]
async fn get_bangumi_subject_tool_with_episodes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/subjects/400602"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 400602,
            "name": "葬送のフリーレン",
            "name_cn": "葬送的芙莉莲",
            "date": "2023-09-29",
            "summary": "简介",
            "total_episodes": 28,
            "infobox": [{ "key": "话数", "value": "28" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v0/episodes"))
        .and(query_param("subject_id", "400602"))
        .and(query_param("type", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": (1..=50).map(|i| json!({
                "sort": i, "ep": i,
                "name": format!("第{i}話"),
                "name_cn": format!("第{i}话"),
                "airdate": "2023-09-29",
                "desc": "很长的介绍"
            })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;

    let tools = build_tools(None, bgm_client(&server));
    let tool = find_tool(&tools, "get_bangumi_subject");
    let out = tool
        .call(json!({ "id": 400602, "with_episodes": true }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();

    assert_eq!(v["id"], 400602);
    assert_eq!(v["name_cn"], "葬送的芙莉莲");
    assert_eq!(v["air_date"], "2023-09-29");
    assert_eq!(v["total_episodes"], 28);
    assert!(v.get("infobox").is_none());
    let eps = v["episodes"].as_array().unwrap();
    assert_eq!(eps.len(), 40); // 最多 40 条
    assert_eq!(eps[0]["sort"], 1);
    assert_eq!(eps[0]["ep"], 1);
    assert_eq!(eps[0]["name_cn"], "第1话");
    assert!(eps[0].get("desc").is_none());
}

#[tokio::test]
async fn get_bangumi_subject_tool_without_episodes_skips_episodes_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v0/subjects/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 1, "name": "n" })))
        .expect(1)
        .mount(&server)
        .await;
    // 不 mock /v0/episodes：若被调用则 404 → 出错。

    let tools = build_tools(None, bgm_client(&server));
    let tool = find_tool(&tools, "get_bangumi_subject");
    let out = tool.call(json!({ "id": 1 })).await.unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert!(v.get("episodes").is_none());
}

// ===== 工具错误路径 =====

#[tokio::test]
async fn tool_upstream_error_becomes_respond_to_model() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tv/999"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "status_message": "not found"
        })))
        .mount(&server)
        .await;

    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));
    let tool = find_tool(&tools, "get_tmdb_detail");
    let err = tool
        .call(json!({ "id": 999, "media_type": "tv" }))
        .await
        .unwrap_err();
    match err {
        nipa_agent::ToolError::RespondToModel(msg) => assert!(msg.contains("404")),
        other => panic!("应为 RespondToModel，实际 {other:?}"),
    }
}

#[tokio::test]
async fn tool_bad_arguments_become_respond_to_model() {
    let server = MockServer::start().await;
    let tools = build_tools(Some(tmdb_client(&server)), bgm_client(&server));

    // query 缺失。
    let search = find_tool(&tools, "search_tmdb");
    assert!(matches!(
        search.call(json!({})).await.unwrap_err(),
        nipa_agent::ToolError::RespondToModel(_)
    ));
    // media_type 非法。
    assert!(matches!(
        search
            .call(json!({ "query": "q", "media_type": "anime" }))
            .await
            .unwrap_err(),
        nipa_agent::ToolError::RespondToModel(_)
    ));
    // id 不是整数。
    let detail = find_tool(&tools, "get_tmdb_detail");
    assert!(matches!(
        detail
            .call(json!({ "id": "abc", "media_type": "tv" }))
            .await
            .unwrap_err(),
        nipa_agent::ToolError::RespondToModel(_)
    ));
}
