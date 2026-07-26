//! 批次 A 收尾验证：ingest_result 写入 0005 新字段
//! （overview/sort_name/runtime_ms/air_date/genres/studios/people/series_id）。

#[path = "../src/ingest.rs"]
mod ingest;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

async fn setup_db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    sqlx::migrate!("./migrations").run(&pool).await.expect("migrations");
    pool
}

async fn seed_library(db: &SqlitePool) -> i64 {
    sqlx::query_scalar("INSERT INTO libraries (name, path) VALUES ('t', '/tmp/t') RETURNING id")
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn ingest_writes_metadata_and_series_id() {
    let db = setup_db().await;
    let lib = seed_library(&db).await;
    let result = serde_json::json!({
        "media_type": "tv_episode",
        "title": "葬送的芙莉莲",
        "original_title": "葬送のフリーレン",
        "season": 1,
        "episode": 5,
        "air_date": "2023-10-27",
        "runtime_minutes": 24,
        "overview": "勇者一行的魔法使芙莉莲的后日谈。",
        "genres": ["奇幻", "冒险"],
        "studios": ["MADHOUSE"],
        "people": [
            {"name": "种崎敦美", "kind": "actor", "role": "芙莉莲"},
            {"name": "斋藤圭一郎", "kind": "director"}
        ],
        "ids": {"bangumi": 400602},
        "confidence": "high",
        "reasoning": "test"
    });

    let ep_id = ingest::ingest_result(&db, lib, None, &result).await.unwrap();

    // episode 行：series_id 回填 + air_date/runtime_ms/date_modified
    let (kind, series_id, air_date, runtime_ms, date_modified): (
        String,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    ) = sqlx::query_as(
        "SELECT kind, series_id, air_date, runtime_ms, date_modified FROM items WHERE id = ?",
    )
    .bind(ep_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(kind, "episode");
    assert_eq!(air_date.as_deref(), Some("2023-10-27"));
    assert_eq!(runtime_ms, Some(24 * 60_000));
    assert!(date_modified.is_some());
    let series_id = series_id.expect("series_id 冗余列应已回填");

    // series 行：overview + sort_name（title 兜底）
    let (overview, sort_name): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT overview, sort_name FROM items WHERE id = ?")
            .bind(series_id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(overview.as_deref(), Some("勇者一行的魔法使芙莉莲的后日谈。"));
    assert_eq!(sort_name.as_deref(), Some("葬送的芙莉莲"));

    // genres/studios → item_values + item_value_map
    let values: Vec<(String, String)> = sqlx::query_as(
        "SELECT v.kind, v.value FROM item_value_map m
         JOIN item_values v ON v.id = m.value_id
         WHERE m.item_id = ? ORDER BY v.kind, v.value",
    )
    .bind(series_id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        values,
        vec![
            ("genre".into(), "冒险".into()),
            ("genre".into(), "奇幻".into()),
            ("studio".into(), "MADHOUSE".into()),
        ]
    );

    // people → people + item_people（sort_order 保序）
    let people: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT p.name, p.kind, ip.role, ip.sort_order
         FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = ? ORDER BY ip.sort_order",
    )
    .bind(series_id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(people.len(), 2);
    assert_eq!(people[0], ("种崎敦美".into(), "actor".into(), Some("芙莉莲".into()), 0));
    assert_eq!(people[1].0, "斋藤圭一郎");
    assert_eq!(people[1].1, "director");
    assert_eq!(people[1].2, None);

    // 幂等：同结论再灌一次不产生重复 genres/people
    let ep2 = ingest::ingest_result(&db, lib, None, &result).await.unwrap();
    assert_eq!(ep2, ep_id);
    let n_values: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM item_value_map WHERE item_id = ?")
            .bind(series_id)
            .fetch_one(&db)
            .await
            .unwrap();
    let n_people: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_people WHERE item_id = ?")
        .bind(series_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(n_values, 3);
    assert_eq!(n_people, 2);
}

#[tokio::test]
async fn ingest_movie_metadata_on_single_node() {
    let db = setup_db().await;
    let lib = seed_library(&db).await;
    let result = serde_json::json!({
        "media_type": "movie",
        "title": "你的名字。",
        "year": 2016,
        "runtime_minutes": 106,
        "overview": "少年少女交换身体的故事。",
        "genres": ["爱情"],
        "confidence": "high",
        "reasoning": "test"
    });
    let id = ingest::ingest_result(&db, lib, None, &result).await.unwrap();
    let (overview, sort_name, runtime_ms): (Option<String>, Option<String>, Option<i64>) =
        sqlx::query_as("SELECT overview, sort_name, runtime_ms FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
    // movie 是叶子也是顶层：runtime 写叶子、overview/sort_name 写顶层——同一行
    assert_eq!(overview.as_deref(), Some("少年少女交换身体的故事。"));
    assert_eq!(sort_name.as_deref(), Some("你的名字。"));
    assert_eq!(runtime_ms, Some(106 * 60_000));
}
