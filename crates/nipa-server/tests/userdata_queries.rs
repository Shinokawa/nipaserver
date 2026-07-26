//! 批次 B 查询集成测试：sqlx 内存库 + 全量迁移，种子数据验证
//! resume / next-up / latest 三查询语义（docs/07 验收项）。
//!
//! nipa-server 是 bin crate（无 lib target），经 `#[path]` 包含同一份
//! userdata.rs 源码——测试跑的 SQL 与线上完全一致。

#![allow(dead_code)]

#[path = "../src/userdata.rs"]
mod userdata;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

const USER: i64 = 1;

async fn setup_db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1) // 内存库：单连接防 :memory: 分裂
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    sqlx::migrate!("./migrations").run(&pool).await.expect("migrations");
    pool
}

/// 建库 + series/season/episode 三级树，返回 (series_id, vec<episode_id>)。
/// episodes: &[(season_no, episode_no)]
async fn seed_series(
    db: &SqlitePool,
    library_id: i64,
    title: &str,
    episodes: &[(i64, i64)],
) -> (i64, Vec<i64>) {
    let series: i64 = sqlx::query_scalar(
        "INSERT INTO items (library_id, kind, title, added_at)
         VALUES (?, 'series', ?, unixepoch()) RETURNING id",
    )
    .bind(library_id)
    .bind(title)
    .fetch_one(db)
    .await
    .unwrap();
    let mut ep_ids = Vec::new();
    let mut season_ids = std::collections::HashMap::new();
    for &(s, e) in episodes {
        let season_id = match season_ids.get(&s) {
            Some(id) => *id,
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO items (library_id, kind, parent_id, title, season_no, added_at)
                     VALUES (?, 'season', ?, ?, ?, unixepoch()) RETURNING id",
                )
                .bind(library_id)
                .bind(series)
                .bind(format!("第 {s} 季"))
                .bind(s)
                .fetch_one(db)
                .await
                .unwrap();
                season_ids.insert(s, id);
                id
            }
        };
        let ep: i64 = sqlx::query_scalar(
            "INSERT INTO items (library_id, kind, parent_id, series_id, title,
                                season_no, episode_no, added_at)
             VALUES (?, 'episode', ?, ?, ?, ?, ?, unixepoch()) RETURNING id",
        )
        .bind(library_id)
        .bind(season_id)
        .bind(series)
        .bind(format!("{title} S{s:02}E{e:02}"))
        .bind(s)
        .bind(e)
        .fetch_one(db)
        .await
        .unwrap();
        ep_ids.push(ep);
    }
    (series, ep_ids)
}

async fn seed_library(db: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO libraries (name, path, kind) VALUES (?, ?, 'anime') RETURNING id",
    )
    .bind(name)
    .bind(format!("/tmp/{name}"))
    .fetch_one(db)
    .await
    .unwrap()
}

/// 写用户态（模拟进度上报后的落库结果）。
async fn seed_watch(
    db: &SqlitePool,
    item_id: i64,
    position_ms: i64,
    played: bool,
    last_played_at: i64,
) {
    sqlx::query(
        "INSERT INTO watch_history
           (user_id, item_id, position_ms, duration_ms, played, play_count,
            updated_at, last_played_at)
         VALUES (?, ?, ?, 1440000, ?, 1, ?, ?)
         ON CONFLICT(user_id, item_id) DO UPDATE SET
           position_ms = excluded.position_ms,
           played = excluded.played,
           last_played_at = excluded.last_played_at",
    )
    .bind(USER)
    .bind(item_id)
    .bind(position_ms)
    .bind(played as i64)
    .bind(last_played_at)
    .bind(last_played_at)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn resume_returns_in_progress_ordered_by_last_played() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (_, eps) = seed_series(&db, lib, "剧A", &[(1, 1), (1, 2)]).await;
    // 电影
    let movie: i64 = sqlx::query_scalar(
        "INSERT INTO items (library_id, kind, title, added_at)
         VALUES (?, 'movie', '电影B', unixepoch()) RETURNING id",
    )
    .bind(lib)
    .fetch_one(&db)
    .await
    .unwrap();

    seed_watch(&db, eps[0], 600_000, false, 100).await; // 在看，较早
    seed_watch(&db, movie, 1_200_000, false, 200).await; // 在看，最近
    seed_watch(&db, eps[1], 0, true, 300).await; // 已看完（position 归零）→ 不进 resume

    let rows = userdata::query_resume(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, movie); // last_played_at DESC
    assert_eq!(rows[1].id, eps[0]);
    // episode 带 series 标题
    assert_eq!(rows[1].series_title.as_deref(), Some("剧A"));
    assert_eq!(rows[0].series_title, None);
}

#[tokio::test]
async fn resume_excludes_played_and_zero_position() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (_, eps) = seed_series(&db, lib, "剧A", &[(1, 1), (1, 2), (1, 3)]).await;
    seed_watch(&db, eps[0], 0, false, 100).await; // <5% 已归零 → 不进
    seed_watch(&db, eps[1], 500_000, true, 200).await; // 异常：有 position 但 played → 不进
    let rows = userdata::query_resume(&db, USER, 12).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn next_up_picks_first_unplayed_after_max_watched() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (series, eps) =
        seed_series(&db, lib, "剧A", &[(1, 1), (1, 2), (1, 3)]).await;
    // 看完 E1、E2 → next 应为 E3
    seed_watch(&db, eps[0], 0, true, 100).await;
    seed_watch(&db, eps[1], 0, true, 200).await;

    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].series_id, series);
    assert_eq!(rows[0].episode.id, eps[2]);
    assert_eq!(rows[0].last_played_at, 200);
}

#[tokio::test]
async fn next_up_crosses_season_boundary() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    // S1E12 看完 → 下一集 S2E1（跨季按 (season_no, episode_no) 字典序）
    let (_, eps) = seed_series(&db, lib, "剧A", &[(1, 11), (1, 12), (2, 1), (2, 2)]).await;
    seed_watch(&db, eps[0], 0, true, 100).await;
    seed_watch(&db, eps[1], 0, true, 200).await;

    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].episode.id, eps[2]); // S2E01
    assert_eq!(rows[0].episode.season_no, Some(2));
    assert_eq!(rows[0].episode.episode_no, Some(1));
}

#[tokio::test]
async fn next_up_uses_max_episode_not_most_recent() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    // Jellyfin 语义：位置取"集号最大的已播集"，不是最近播的——
    // 先看 E3（较早），再回头看 E1（最近）→ next 仍是 E4
    let (_, eps) = seed_series(&db, lib, "剧A", &[(1, 1), (1, 2), (1, 3), (1, 4)]).await;
    seed_watch(&db, eps[2], 0, true, 100).await; // E3 早看完
    seed_watch(&db, eps[0], 0, true, 500).await; // E1 最近看完

    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].episode.id, eps[3]); // E4
}

#[tokio::test]
async fn next_up_orders_series_by_recency_and_skips_finished() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (series_a, a) = seed_series(&db, lib, "剧A", &[(1, 1), (1, 2)]).await;
    let (series_b, b) = seed_series(&db, lib, "剧B", &[(1, 1), (1, 2)]).await;
    let (_series_c, c) = seed_series(&db, lib, "剧C", &[(1, 1)]).await;
    seed_watch(&db, a[0], 0, true, 100).await; // 剧A 较早
    seed_watch(&db, b[0], 0, true, 300).await; // 剧B 最近
    seed_watch(&db, c[0], 0, true, 500).await; // 剧C 全部看完 → 无下一集，跳过

    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].series_id, series_b); // 最近的剧在前
    assert_eq!(rows[1].series_id, series_a);
    assert_eq!(rows[0].episode.id, b[1]);
    assert_eq!(rows[1].episode.id, a[1]);
}

#[tokio::test]
async fn next_up_excludes_specials_and_virtual() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (_, eps) = seed_series(&db, lib, "剧A", &[(0, 1), (1, 1), (1, 2), (1, 3)]).await;
    // E2 标虚拟占位（缺失集）
    sqlx::query("UPDATE items SET is_virtual = 1 WHERE id = ?")
        .bind(eps[2])
        .execute(&db)
        .await
        .unwrap();
    seed_watch(&db, eps[1], 0, true, 100).await; // 看完 S1E1
    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    // S0E1 与虚拟 S1E2 都被排除 → next 是 S1E3
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].episode.id, eps[3]);
}

#[tokio::test]
async fn next_up_never_watched_series_absent() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    seed_series(&db, lib, "剧A", &[(1, 1), (1, 2)]).await;
    // 没有任何观看记录 → NextUp 为空（只有播过的剧才进）
    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn next_up_in_progress_series_starts_from_first_episode() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    let (_, eps) = seed_series(&db, lib, "剧A", &[(1, 1), (1, 2)]).await;
    // E1 在看中（未看完）→ 该剧进候选（有 last_played_at），
    // 无已看完集 → next 取全剧最小未播集 = E1 本身（enableResumable=true 语义）
    seed_watch(&db, eps[0], 600_000, false, 100).await;
    let rows = userdata::query_next_up(&db, USER, 12).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].episode.id, eps[0]);
}

#[tokio::test]
async fn latest_groups_by_library_ordered_by_added_at() {
    let db = setup_db().await;
    let lib_a = seed_library(&db, "anime").await;
    let lib_b = seed_library(&db, "movies").await;
    // 手动控制 added_at
    let mut ids = Vec::new();
    for (lib, title, added) in [
        (lib_a, "老剧", 100),
        (lib_a, "新剧", 300),
        (lib_b, "老电影", 150),
        (lib_b, "新电影", 250),
    ] {
        let kind = if lib == lib_a { "series" } else { "movie" };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items (library_id, kind, title, added_at)
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(lib)
        .bind(kind)
        .bind(title)
        .bind(added)
        .fetch_one(&db)
        .await
        .unwrap();
        ids.push(id);
    }
    // episode 不应出现在 latest（折叠语义：只取顶层实体）
    seed_series(&db, lib_a, "带集的剧", &[(1, 1)]).await;

    let rows = userdata::query_latest(&db, 2).await.unwrap();
    // lib_a：带集的剧（unixepoch 最新）+ 新剧；lib_b：新电影 + 老电影
    let lib_a_rows: Vec<_> = rows.iter().filter(|r| r.library_id == lib_a).collect();
    let lib_b_rows: Vec<_> = rows.iter().filter(|r| r.library_id == lib_b).collect();
    assert_eq!(lib_a_rows.len(), 2);
    assert_eq!(lib_b_rows.len(), 2);
    assert_eq!(lib_a_rows[0].title.as_deref(), Some("带集的剧"));
    assert_eq!(lib_a_rows[1].title.as_deref(), Some("新剧"));
    assert_eq!(lib_b_rows[0].title.as_deref(), Some("新电影"));
    assert_eq!(lib_b_rows[1].title.as_deref(), Some("老电影"));
    assert!(rows.iter().all(|r| r.kind != "episode" && r.kind != "season"));
}

#[tokio::test]
async fn migration_0006_backfills_series_id() {
    let db = setup_db().await;
    let lib = seed_library(&db, "anime").await;
    // 模拟 0005 前的旧数据：episode 无 series_id
    let (series, eps) = seed_series(&db, lib, "剧A", &[(1, 1)]).await;
    sqlx::query("UPDATE items SET series_id = NULL WHERE id = ?")
        .bind(eps[0])
        .execute(&db)
        .await
        .unwrap();
    // 0006 的回填语句（迁移只在建库时跑一次，这里重跑同一 SQL 验证语义）
    sqlx::query(
        "UPDATE items SET series_id = (
           SELECT s.parent_id FROM items s WHERE s.id = items.parent_id
         ) WHERE kind = 'episode' AND series_id IS NULL",
    )
    .execute(&db)
    .await
    .unwrap();
    let got: Option<i64> = sqlx::query_scalar("SELECT series_id FROM items WHERE id = ?")
        .bind(eps[0])
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(got, Some(series));
}
