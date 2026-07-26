//! SQLite 连接与迁移（开发文档 §2.3/§9）。
//!
//! TODO(M1): 拆分"单写连接 + 读连接池"（§2.3），当前先用单池。

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;

/// 打开 data_dir 下的 nipa.db（WAL、外键开启）并执行迁移。
pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("创建数据目录 {} 失败", data_dir.display()))?;
    let db_path = data_dir.join("nipa.db");

    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("打开数据库 {} 失败", db_path.display()))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("数据库迁移失败")?;

    tracing::info!(db = %db_path.display(), "数据库就绪（WAL、foreign_keys=ON）");
    Ok(pool)
}
