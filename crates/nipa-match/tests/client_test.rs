//! DandanClient 集成测试（wiremock 模拟弹弹play 开放平台）。

use std::time::Duration;

use nipa_match::{
    AnimeType, DandanAuth, DandanClient, MatchError, MatchOutcome, MatchRequest, classify,
    compute_signature,
};
use wiremock::matchers::{body_json_string, header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn client(server: &MockServer, auth: DandanAuth) -> DandanClient {
    DandanClient::builder(auth)
        .base_url(server.uri())
        .min_interval(Duration::ZERO)
        .build()
        .unwrap()
}

fn credentials() -> DandanAuth {
    DandanAuth::Credentials {
        app_id: "testAppId".into(),
        app_secret: "testAppSecret".into(),
    }
}

fn sample_request() -> MatchRequest {
    MatchRequest::new(
        "[Sub] Title - 01",
        "658d05841b9476ccc7420b3f0bb21c3b",
        123_456_789,
    )
}

fn match_result_json(episode_id: i64) -> serde_json::Value {
    serde_json::json!({
        "episodeId": episode_id,
        "animeId": 700,
        "animeTitle": "作品标题",
        "episodeTitle": format!("第{episode_id}话"),
        "type": "tvseries",
        "typeDescription": "TV动画",
        "shift": 0.0,
        "imageUrl": "https://img.example/poster.jpg"
    })
}

fn ok_json(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

// ---------- 认证头 ----------

#[tokio::test]
async fn credentials_mode_sends_appid_and_secret_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .and(header("X-AppId", "testAppId"))
        .and(header("X-AppSecret", "testAppSecret"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true, "isMatched": false, "matches": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    c.match_file(sample_request()).await.unwrap();
    // expect(1) 在 server drop 时校验：头不匹配则 mock 未命中、请求 404 已然失败。
}

#[tokio::test]
async fn signature_mode_sends_signature_headers_with_valid_signature() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .and(header("X-AppId", "sigAppId"))
        .and(header_exists("X-Timestamp"))
        .and(header_exists("X-Signature"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true, "isMatched": false, "matches": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(
        &server,
        DandanAuth::Signature {
            app_id: "sigAppId".into(),
            app_secret: "sigSecret".into(),
        },
    );
    c.match_file(sample_request()).await.unwrap();

    // 进一步校验签名值：用请求头里的 Timestamp 重算应完全一致。
    let requests: Vec<Request> = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let ts: i64 = req.headers["X-Timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let sig = req.headers["X-Signature"].to_str().unwrap();
    assert_eq!(
        sig,
        compute_signature("sigAppId", ts, "/api/v2/match", "sigSecret")
    );
    // 无凭证模式独有头不应出现。
    assert!(!req.headers.contains_key("X-AppSecret"));
}

// ---------- match 三分类 ----------

#[tokio::test]
async fn match_exact_hit_classifies_as_exact() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true, "errorMessage": null,
            "isMatched": true,
            "matches": [match_result_json(10001)]
        })))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let resp = c.match_file(sample_request()).await.unwrap();
    assert!(resp.is_matched);
    match classify(resp) {
        MatchOutcome::Exact(r) => {
            assert_eq!(r.episode_id, 10001);
            assert_eq!(r.anime_id, 700);
            assert_eq!(r.anime_type, AnimeType::TvSeries);
            assert_eq!(r.anime_title.as_deref(), Some("作品标题"));
        }
        other => panic!("应为 Exact，实为 {other:?}"),
    }
}

#[tokio::test]
async fn match_fuzzy_hit_classifies_as_candidates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true,
            "isMatched": false,
            "matches": [match_result_json(1), match_result_json(2)]
        })))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let outcome = classify(c.match_file(sample_request()).await.unwrap());
    match outcome {
        MatchOutcome::Candidates(list) => {
            assert_eq!(list.len(), 2);
            assert_eq!(list[0].episode_id, 1);
            assert_eq!(list[1].episode_id, 2);
        }
        other => panic!("应为 Candidates，实为 {other:?}"),
    }
}

#[tokio::test]
async fn match_no_result_classifies_as_no_match() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true,
            "isMatched": false, "matches": []
        })))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let outcome = classify(c.match_file(sample_request()).await.unwrap());
    assert_eq!(outcome, MatchOutcome::NoMatch);
}

// ---------- 请求体格式 ----------

#[tokio::test]
async fn match_request_body_is_camel_case_hash_and_filename() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .and(body_json_string(
            r#"{"fileName":"[Sub] Title - 01","fileHash":"658d05841b9476ccc7420b3f0bb21c3b","fileSize":123456789,"matchMode":"hashAndFileName"}"#,
        ))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true, "isMatched": false, "matches": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    c.match_file(sample_request()).await.unwrap();
}

// ---------- batch ----------

#[tokio::test]
async fn batch_splits_35_requests_into_two_calls_and_concatenates_in_order() {
    let server = MockServer::start().await;

    // 按请求体内容动态应答：requests 数量 ≤32，未命中项 success=false。
    Mock::given(method("POST"))
        .and(path("/api/v2/match/batch"))
        .respond_with(|req: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let requests = body["requests"].as_array().expect("包装体应含 requests");
            assert!(requests.len() <= 32, "单批不得超过 32 个");
            let results: Vec<serde_json::Value> = requests
                .iter()
                .map(|r| {
                    let hash = r["fileHash"].as_str().unwrap();
                    // 约定：hash 以偶数序号结尾的命中。
                    let idx: i64 = hash.trim_start_matches("hash").parse().unwrap();
                    if idx % 2 == 0 {
                        serde_json::json!({
                            "success": true,
                            "fileHash": hash,
                            "matchResult": match_result_json(idx)
                        })
                    } else {
                        serde_json::json!({
                            "success": false,
                            "fileHash": hash,
                            "matchResult": null
                        })
                    }
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errorCode": 0, "success": true, "errorMessage": null,
                "results": results
            }))
        })
        .expect(2) // 35 个请求 → 32 + 3 两批
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let reqs: Vec<MatchRequest> = (0..35)
        .map(|i| MatchRequest::new(format!("file{i}"), format!("hash{i}"), 1000 + i))
        .collect();
    let items = c.match_batch(reqs).await.unwrap();

    // 一一对应：数量、顺序均与请求一致。
    assert_eq!(items.len(), 35);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item.file_hash.as_deref(), Some(format!("hash{i}").as_str()));
        if i % 2 == 0 {
            assert!(item.success);
            assert_eq!(item.match_result.as_ref().unwrap().episode_id, i as i64);
        } else {
            assert!(!item.success);
            assert!(item.match_result.is_none());
        }
    }
}

#[tokio::test]
async fn batch_empty_input_makes_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match/batch"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let items = c.match_batch(vec![]).await.unwrap();
    assert!(items.is_empty());
}

// ---------- 错误处理 ----------

#[tokio::test]
async fn forbidden_surfaces_x_error_message_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(
            ResponseTemplate::new(403).insert_header("X-Error-Message", "Invalid Signature"),
        )
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let err = c.match_file(sample_request()).await.unwrap_err();
    match err {
        MatchError::AuthRejected { reason } => assert_eq!(reason, "Invalid Signature"),
        other => panic!("应为 AuthRejected，实为 {other:?}"),
    }
}

#[tokio::test]
async fn forbidden_without_header_still_diagnosable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let err = c.match_file(sample_request()).await.unwrap_err();
    match err {
        MatchError::AuthRejected { reason } => assert_eq!(reason, "(无 X-Error-Message 头)"),
        other => panic!("应为 AuthRejected，实为 {other:?}"),
    }
}

#[tokio::test]
async fn business_error_maps_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 777,
            "success": false,
            "errorMessage": "参数错误",
            "isMatched": false,
            "matches": []
        })))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let err = c.match_file(sample_request()).await.unwrap_err();
    match err {
        MatchError::Api {
            error_code,
            message,
        } => {
            assert_eq!(error_code, 777);
            assert_eq!(message, "参数错误");
        }
        other => panic!("应为 Api，实为 {other:?}"),
    }
}

#[tokio::test]
async fn batch_business_error_maps_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match/batch"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 888,
            "success": false,
            "errorMessage": "请求过多",
            "results": []
        })))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let err = c.match_batch(vec![sample_request()]).await.unwrap_err();
    match err {
        MatchError::Api {
            error_code,
            message,
        } => {
            assert_eq!(error_code, 888);
            assert_eq!(message, "请求过多");
        }
        other => panic!("应为 Api，实为 {other:?}"),
    }
}

#[tokio::test]
async fn server_error_maps_to_upstream_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal"))
        .mount(&server)
        .await;

    let c = client(&server, credentials());
    let err = c.match_file(sample_request()).await.unwrap_err();
    match err {
        MatchError::UpstreamStatus { status, message } => {
            assert_eq!(status, 500);
            assert_eq!(message, "internal");
        }
        other => panic!("应为 UpstreamStatus，实为 {other:?}"),
    }
}

// ---------- 节流 ----------

#[tokio::test]
async fn min_interval_throttles_consecutive_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/match"))
        .respond_with(ok_json(serde_json::json!({
            "errorCode": 0, "success": true, "isMatched": false, "matches": []
        })))
        .mount(&server)
        .await;

    let c = DandanClient::builder(credentials())
        .base_url(server.uri())
        .min_interval(Duration::from_millis(80))
        .build()
        .unwrap();
    let start = std::time::Instant::now();
    for _ in 0..3 {
        c.match_file(sample_request()).await.unwrap();
    }
    // 第 2、3 次各至少间隔 80ms。
    assert!(
        start.elapsed() >= Duration::from_millis(160),
        "3 次请求应至少耗时 160ms，实际 {:?}",
        start.elapsed()
    );
}
