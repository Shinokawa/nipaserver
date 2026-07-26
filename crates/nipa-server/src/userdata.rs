//! 用户数据纯逻辑与查询（Jellyfin 对标批次 B）。
//!
//! played 判定语义照抄 docs/research/jellyfin-full/userdata-playback.md §1.2；
//! Resume/NextUp/Latest 查询照 §3 与 §5.4/§5.5 的 SQL 思路。
//!
//! 本文件刻意只依赖 serde/sqlx——tests/userdata_queries.rs 经 `#[path]`
//! 包含同一份源码，保证测试跑的查询与线上一致（bin crate 无 lib target）。

use serde::Serialize;
use sqlx::SqlitePool;

// ===== played 判定（Jellyfin UpdatePlayState 语义） =====

pub const MIN_RESUME_PCT: f64 = 5.0;
pub const MAX_RESUME_PCT: f64 = 90.0;
pub const MIN_RESUME_DURATION_MS: i64 = 300_000;

/// 判定结果：写库用的 position 与"是否置 played=1"。
/// 注意：只置 true 不置 false——mark_played=false 表示"不动 played"，
/// 清除走显式 MarkUnplayed 接口。
#[derive(Debug, PartialEq, Eq)]
pub struct PlayState {
    pub position_ms: i64,
    pub mark_played: bool,
}

/// 对一次进度上报跑 Jellyfin §1.2 规则：
/// - runtime 未知 → 视为播完（position 归零，played=true）；
/// - pct < 5% → 刚开始，position 归零，played 不动；
/// - pct > 90% 或距结尾 <1s → 看完；
/// - 5% ≤ pct ≤ 90% 但总长 < 300s → 短视频直接算看完；
/// - 否则正常记录续播点。
pub fn resolve_play_state(position_ms: i64, runtime_ms: Option<i64>) -> PlayState {
    let Some(runtime) = runtime_ms.filter(|r| *r > 0) else {
        return PlayState {
            position_ms: 0,
            mark_played: true,
        };
    };
    let pct = position_ms as f64 * 100.0 / runtime as f64;
    if pct < MIN_RESUME_PCT {
        PlayState {
            position_ms: 0,
            mark_played: false,
        }
    } else if pct > MAX_RESUME_PCT
        || position_ms >= runtime - 1000
        || runtime < MIN_RESUME_DURATION_MS
    {
        PlayState {
            position_ms: 0,
            mark_played: true,
        }
    } else {
        PlayState {
            position_ms,
            mark_played: false,
        }
    }
}

// ===== Resume（继续观看，§3.2/§5.4） =====

/// (?1=user_id, ?2=limit)。写入端保证 <5% 与 >90% 已归零，
/// `position_ms > 0 AND played = 0` 即等价 Jellyfin IsResumable。
pub const RESUME_SQL: &str = "\
    SELECT i.id, i.kind, i.title, i.series_id, sr.title AS series_title,
           i.season_no, i.episode_no,
           COALESCE(i.poster_path, sr.poster_path) AS poster_path,
           wh.position_ms, wh.duration_ms, i.runtime_ms, wh.last_played_at
    FROM watch_history wh
    JOIN items i ON i.id = wh.item_id AND i.deleted_at IS NULL
    LEFT JOIN items sr ON sr.id = i.series_id
    WHERE wh.user_id = ?1 AND wh.position_ms > 0 AND wh.played = 0
      AND i.kind IN ('movie', 'episode')
    ORDER BY wh.last_played_at DESC
    LIMIT ?2";

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ResumeRow {
    pub id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub series_id: Option<i64>,
    pub series_title: Option<String>,
    pub season_no: Option<i64>,
    pub episode_no: Option<i64>,
    pub poster_path: Option<String>,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub runtime_ms: Option<i64>,
    pub last_played_at: Option<i64>,
}

pub async fn query_resume(
    db: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<ResumeRow>> {
    sqlx::query_as(RESUME_SQL)
        .bind(user_id)
        .bind(limit)
        .fetch_all(db)
        .await
}

// ===== NextUp（两阶段，§3.1/§5.5） =====

/// 阶段一：最近在看的剧（?1=user_id, ?2=limit）。
/// 只有播过（有 last_played_at）的剧才进 NextUp，最近看的排最前。
pub const NEXTUP_RECENT_SERIES_SQL: &str = "\
    SELECT i.series_id, MAX(wh.last_played_at) AS last_watched
    FROM watch_history wh
    JOIN items i ON i.id = wh.item_id AND i.kind = 'episode'
         AND i.deleted_at IS NULL AND i.series_id IS NOT NULL
    WHERE wh.user_id = ?1 AND wh.last_played_at IS NOT NULL
    GROUP BY i.series_id
    ORDER BY last_watched DESC
    LIMIT ?2";

/// 阶段二 a：该剧"最后看完的位置"——played=1 且排除第 0 季，
/// 取 (season_no, episode_no) 字典序最大者（不是最近播的）。
pub const NEXTUP_LAST_PLAYED_POS_SQL: &str = "\
    SELECT e.season_no, e.episode_no
    FROM items e
    JOIN watch_history w ON w.item_id = e.id AND w.user_id = ?1 AND w.played = 1
    WHERE e.series_id = ?2 AND e.kind = 'episode' AND e.deleted_at IS NULL
      AND COALESCE(e.season_no, 1) != 0
    ORDER BY e.season_no DESC, e.episode_no DESC
    LIMIT 1";

/// 阶段二 b：位置之后第一个未看集（?3/?4 = 最后看完的 (season_no, episode_no)，
/// 无已看集时传 (-1, -1) 哨兵 → 条件对全部集成立，取全剧最小未播集）。
/// 跨季由 (season_no, episode_no) 字典序保证；排除第 0 季与虚拟占位集。
pub const NEXTUP_NEXT_EPISODE_SQL: &str = "\
    SELECT e.id, e.title, e.season_no, e.episode_no, e.air_date, e.runtime_ms,
           COALESCE(e.poster_path, sr.poster_path) AS poster_path,
           sr.title AS series_title
    FROM items e
    LEFT JOIN watch_history w ON w.item_id = e.id AND w.user_id = ?1
    LEFT JOIN items sr ON sr.id = e.series_id
    WHERE e.series_id = ?2 AND e.kind = 'episode' AND e.deleted_at IS NULL
      AND COALESCE(e.season_no, 1) != 0 AND e.is_virtual = 0
      AND COALESCE(w.played, 0) = 0
      AND (e.season_no > ?3 OR (e.season_no = ?3 AND e.episode_no > ?4))
    ORDER BY e.season_no, e.episode_no
    LIMIT 1";

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NextUpEpisode {
    pub id: i64,
    pub title: Option<String>,
    pub season_no: Option<i64>,
    pub episode_no: Option<i64>,
    pub air_date: Option<String>,
    pub runtime_ms: Option<i64>,
    pub poster_path: Option<String>,
    pub series_title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NextUpRow {
    pub series_id: i64,
    /// 该剧最近观看时间（最终排序键，DESC）。
    pub last_played_at: i64,
    #[serde(flatten)]
    pub episode: NextUpEpisode,
}

/// 两阶段 NextUp。阶段一多取一些候选（有的剧已全部看完、算不出下一集）。
pub async fn query_next_up(
    db: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<NextUpRow>> {
    let candidates: Vec<(i64, i64)> = sqlx::query_as(NEXTUP_RECENT_SERIES_SQL)
        .bind(user_id)
        .bind(limit * 2)
        .fetch_all(db)
        .await?;
    let mut out = Vec::new();
    for (series_id, last_watched) in candidates {
        if out.len() as i64 >= limit {
            break;
        }
        // 最后看完的位置；一集都没看完 → (-1, -1) 哨兵取全剧第一未播集
        let pos: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(NEXTUP_LAST_PLAYED_POS_SQL)
            .bind(user_id)
            .bind(series_id)
            .fetch_optional(db)
            .await?;
        let (s, e) = match pos {
            Some((s, e)) => (s.unwrap_or(-1), e.unwrap_or(-1)),
            None => (-1, -1),
        };
        let next: Option<NextUpEpisode> = sqlx::query_as(NEXTUP_NEXT_EPISODE_SQL)
            .bind(user_id)
            .bind(series_id)
            .bind(s)
            .bind(e)
            .fetch_optional(db)
            .await?;
        if let Some(episode) = next {
            out.push(NextUpRow {
                series_id,
                last_played_at: last_watched,
                episode,
            });
        }
    }
    Ok(out)
}

// ===== Latest（按库分组的最新添加，§3.3 简化：series/movie 本体即卡片） =====

/// (?1 = 每库条数)。episode 折叠到 series 的语义由"只取 series/movie"天然满足。
pub const LATEST_SQL: &str = "\
    SELECT t.id, t.library_id, l.name AS library_name, t.kind, t.title, t.year,
           t.air_date, t.poster_path, t.added_at
    FROM (
      SELECT id, library_id, kind, title, year, air_date, poster_path, added_at,
             ROW_NUMBER() OVER (
               PARTITION BY library_id ORDER BY added_at DESC, id DESC) AS rn
      FROM items
      WHERE kind IN ('series', 'movie') AND deleted_at IS NULL
    ) t JOIN libraries l ON l.id = t.library_id
    WHERE t.rn <= ?1
    ORDER BY t.library_id, t.rn";

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LatestRow {
    pub id: i64,
    pub library_id: i64,
    pub library_name: Option<String>,
    pub kind: String,
    pub title: Option<String>,
    pub year: Option<i64>,
    pub air_date: Option<String>,
    pub poster_path: Option<String>,
    pub added_at: Option<i64>,
}

pub async fn query_latest(db: &SqlitePool, per_library: i64) -> sqlx::Result<Vec<LatestRow>> {
    sqlx::query_as(LATEST_SQL)
        .bind(per_library)
        .fetch_all(db)
        .await
}

// ===== 单元测试：played 判定全分支 =====

#[cfg(test)]
mod tests {
    use super::*;

    fn ps(position_ms: i64, mark_played: bool) -> PlayState {
        PlayState {
            position_ms,
            mark_played,
        }
    }

    #[test]
    fn runtime_unknown_assumes_played() {
        // §1.2 规则 6：runtime 未知只能假定播完
        assert_eq!(resolve_play_state(123_456, None), ps(0, true));
        assert_eq!(resolve_play_state(0, None), ps(0, true));
        // runtime=0 视同未知
        assert_eq!(resolve_play_state(500, Some(0)), ps(0, true));
    }

    #[test]
    fn below_min_pct_resets_position_keeps_played() {
        // 规则 2：<5% 刚开始，position 归零，played 不动
        let runtime = 3_600_000; // 1h
        assert_eq!(resolve_play_state(0, Some(runtime)), ps(0, false));
        assert_eq!(resolve_play_state(179_999, Some(runtime)), ps(0, false)); // 4.99..%
    }

    #[test]
    fn min_pct_boundary_is_inclusive_resume() {
        // pct == 5% 不属于 "< 5%"，正常记录续播点
        let runtime = 3_600_000;
        assert_eq!(
            resolve_play_state(180_000, Some(runtime)),
            ps(180_000, false)
        );
    }

    #[test]
    fn above_max_pct_marks_played() {
        // 规则 3：>90% 判看完，position 归零
        let runtime = 3_600_000;
        assert_eq!(resolve_play_state(3_240_001, Some(runtime)), ps(0, true)); // 90.00..1%
    }

    #[test]
    fn max_pct_boundary_keeps_resume() {
        // pct == 90% 不属于 ">90%"（且距结尾 >1s），仍记续播点
        let runtime = 3_600_000;
        assert_eq!(
            resolve_play_state(3_240_000, Some(runtime)),
            ps(3_240_000, false)
        );
    }

    #[test]
    fn within_one_second_of_end_marks_played() {
        // 规则 3 后半：position >= runtime - 1s 判看完
        let runtime = 10_000;
        assert_eq!(resolve_play_state(9_000, Some(runtime)), ps(0, true));
    }

    #[test]
    fn short_video_midway_marks_played() {
        // 规则 4：5%~90% 但总长 <300s → 短视频直接看完
        let runtime = 200_000; // 200s
        assert_eq!(resolve_play_state(100_000, Some(runtime)), ps(0, true)); // 50%
    }

    #[test]
    fn normal_midway_records_position() {
        // 规则 5：正常记录续播点，played 不动
        let runtime = 600_000; // 10min
        assert_eq!(
            resolve_play_state(300_000, Some(runtime)),
            ps(300_000, false)
        );
    }

    #[test]
    fn long_runtime_exact_min_duration_records_position() {
        // 总长恰为 300s：不满足 "< 300s"，中段仍记续播点
        assert_eq!(
            resolve_play_state(150_000, Some(MIN_RESUME_DURATION_MS)),
            ps(150_000, false)
        );
    }
}
