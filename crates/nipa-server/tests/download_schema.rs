//! M4 持久化约束：RSS 条目跨重启去重，完成入库按清单幂等。

use sqlx::sqlite::SqlitePoolOptions;

async fn setup_db() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn subscription_entry_key_is_persistent_and_unique() {
    let db = setup_db().await;
    let sub_id: i64 = sqlx::query_scalar(
        "INSERT INTO subscriptions (rss_url, title, filters, enabled)
         VALUES ('https://mikan.example/rss', 'show', '{}', 1) RETURNING id",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    for _ in 0..2 {
        sqlx::query(
            "INSERT OR IGNORE INTO subscription_entries
             (subscription_id, entry_key, title, source_url, created_at)
             VALUES (?, 'episode-1', 'show 01', 'magnet:?xt=urn:btih:a', unixepoch())",
        )
        .bind(sub_id)
        .execute(&db)
        .await
        .unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_entries")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn ingest_key_distinguishes_changed_manifests() {
    let db = setup_db().await;
    for manifest in ["files-v1", "files-v1", "files-v2"] {
        sqlx::query(
            "INSERT OR IGNORE INTO torrent_ingests
             (info_hash, manifest_hash, state, updated_at)
             VALUES ('hash', ?, 'pending', unixepoch())",
        )
        .bind(manifest)
        .execute(&db)
        .await
        .unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM torrent_ingests")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 2);
}
