//! diff 扫描（§4.1 L0 变更检测）：对比"本次发现"与"库中已知"，产出扫描计划。
//!
//! 纯函数、无 IO——DB 读写在 nipa-server 侧（回流约束 §3.1），这里只做集合对比。

use crate::hash::fingerprint;
use crate::walk::DiscoveredFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 库中已知文件（§9 media_files 的对比投影，由调用方从 DB 读出）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownFile {
    /// '/' 分隔的规范化相对路径。
    pub rel_path: String,
    pub size: u64,
    /// Unix mtime，毫秒。
    pub mtime: i64,
    /// 上次入库时的 `sha256(size|mtime)[:16]`；历史数据可能缺失。
    pub fingerprint: Option<String>,
}

/// 移动候选：rel_path 变了但 size 相同的配对提示。
///
/// 仅是提示——最终由 [`crate::dandan_hash`] 确认（`(size, dandan_hash)` 命中才算
/// 移动，迁移 file_item 关联而非重刮，§4.1 移动检测）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedCandidate {
    /// 原路径（本次消失的已知文件）。
    pub from_rel_path: String,
    /// 新路径（本次新发现的文件）。
    pub to_rel_path: String,
    /// 双方一致的文件大小（配对依据）。
    pub size: u64,
}

/// [`plan_scan`] 的产出。
///
/// 注意：`new_files` / `missing_files` 保持**完整原始 diff**——被列入
/// `moved_candidates` 的文件不会从两者中剔除（移动尚未确认，调用方在
/// hash 确认后自行收敛）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanPlan {
    /// 库中没有的新路径：需要算 dandan_hash 并走管线。
    pub new_files: Vec<DiscoveredFile>,
    /// 路径已知但指纹变了（size/mtime 变化或历史指纹缺失）：需重算 hash。
    pub changed_files: Vec<DiscoveredFile>,
    /// 疑似移动的配对提示（见 [`MovedCandidate`]）。
    pub moved_candidates: Vec<MovedCandidate>,
    /// 本次没扫到的已知路径（rel_path）。可能是删除，也可能是 NAS 掉线——
    /// 入库侧走软删除 + 宽限期（§9 生命周期），这里只报告。
    pub missing_files: Vec<String>,
}

/// 对比新发现与库中已知，产出扫描计划。
///
/// 判定规则：
/// - `rel_path` 未知 → `new_files`；
/// - `rel_path` 已知且 `fingerprint(size, mtime)` 与存量一致 → 无事发生；
/// - `rel_path` 已知但指纹不一致（或存量指纹缺失）→ `changed_files`；
/// - 已知但本次未发现 → `missing_files`；
/// - 新文件与缺失文件 size 相同 → 追加 `moved_candidates` 配对提示
///   （按 rel_path 排序做贪心一对一配对，保证确定性）。
///
/// 所有输出按 rel_path 升序，结果确定。
pub fn plan_scan(discovered: &[DiscoveredFile], known: &[KnownFile]) -> ScanPlan {
    // BTreeMap：既做 rel_path 索引，又保证遍历顺序确定。
    let known_by_path: BTreeMap<&str, &KnownFile> =
        known.iter().map(|k| (k.rel_path.as_str(), k)).collect();
    let mut seen_known: BTreeMap<&str, bool> =
        known_by_path.keys().map(|&p| (p, false)).collect();

    let mut plan = ScanPlan::default();

    let mut sorted_discovered: Vec<&DiscoveredFile> = discovered.iter().collect();
    sorted_discovered.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    for file in sorted_discovered {
        match known_by_path.get(file.rel_path.as_str()) {
            None => plan.new_files.push(file.clone()),
            Some(existing) => {
                seen_known.insert(existing.rel_path.as_str(), true);
                let current = fingerprint(file.size, file.mtime);
                if existing.fingerprint.as_deref() != Some(current.as_str()) {
                    plan.changed_files.push(file.clone());
                }
            }
        }
    }

    plan.missing_files = seen_known
        .iter()
        .filter(|&(_, &seen)| !seen)
        .map(|(&path, _)| path.to_string())
        .collect();

    // 移动候选：missing × new 按 size 贪心一对一配对。
    // 按 size 分桶（桶内保持 rel_path 升序），每个新文件至多消费一个同 size 缺失文件。
    let mut missing_by_size: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
    for path in &plan.missing_files {
        let k = known_by_path[path.as_str()];
        missing_by_size.entry(k.size).or_default().push(path);
    }
    for file in &plan.new_files {
        let Some(bucket) = missing_by_size.get_mut(&file.size) else {
            continue;
        };
        if bucket.is_empty() {
            continue;
        }
        let from = bucket.remove(0);
        plan.moved_candidates.push(MovedCandidate {
            from_rel_path: from.to_string(),
            to_rel_path: file.rel_path.clone(),
            size: file.size,
        });
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(rel_path: &str, size: u64, mtime: i64) -> DiscoveredFile {
        DiscoveredFile {
            rel_path: rel_path.to_string(),
            raw_path: None,
            size,
            mtime,
        }
    }

    /// 指纹与 (size, mtime) 一致的已知文件（即"未变更"基线）。
    fn known(rel_path: &str, size: u64, mtime: i64) -> KnownFile {
        KnownFile {
            rel_path: rel_path.to_string(),
            size,
            mtime,
            fingerprint: Some(fingerprint(size, mtime)),
        }
    }

    #[test]
    fn empty_inputs_yield_empty_plan() {
        assert_eq!(plan_scan(&[], &[]), ScanPlan::default());
    }

    #[test]
    fn unchanged_file_appears_nowhere() {
        let d = [discovered("a/ep01.mkv", 100, 1000)];
        let k = [known("a/ep01.mkv", 100, 1000)];
        assert_eq!(plan_scan(&d, &k), ScanPlan::default());
    }

    #[test]
    fn unknown_path_is_new() {
        let d = [discovered("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&d, &[]);
        assert_eq!(plan.new_files, d.to_vec());
        assert!(plan.changed_files.is_empty());
        assert!(plan.moved_candidates.is_empty());
        assert!(plan.missing_files.is_empty());
    }

    #[test]
    fn fingerprint_mismatch_is_changed() {
        // mtime 变了 → 指纹不一致。
        let d = [discovered("a/ep01.mkv", 100, 2000)];
        let k = [known("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&d, &k);
        assert_eq!(plan.changed_files, d.to_vec());
        assert!(plan.new_files.is_empty());
        assert!(plan.missing_files.is_empty());
    }

    #[test]
    fn size_change_is_changed_not_new() {
        let d = [discovered("a/ep01.mkv", 999, 1000)];
        let k = [known("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&d, &k);
        assert_eq!(plan.changed_files.len(), 1);
        assert!(plan.new_files.is_empty());
    }

    #[test]
    fn missing_stored_fingerprint_forces_changed() {
        let d = [discovered("a/ep01.mkv", 100, 1000)];
        let k = [KnownFile {
            rel_path: "a/ep01.mkv".into(),
            size: 100,
            mtime: 1000,
            fingerprint: None, // 历史数据缺指纹 → 必须重算 hash
        }];
        let plan = plan_scan(&d, &k);
        assert_eq!(plan.changed_files, d.to_vec());
    }

    #[test]
    fn absent_known_file_is_missing() {
        let k = [known("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&[], &k);
        assert_eq!(plan.missing_files, vec!["a/ep01.mkv".to_string()]);
        assert!(plan.moved_candidates.is_empty());
    }

    #[test]
    fn rename_with_same_size_yields_moved_candidate() {
        // 自动整理：a/ep01.mkv → b/Bocchi S01E01.mkv，size 不变。
        let d = [discovered("b/Bocchi S01E01.mkv", 100, 2000)];
        let k = [known("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&d, &k);

        assert_eq!(
            plan.moved_candidates,
            vec![MovedCandidate {
                from_rel_path: "a/ep01.mkv".into(),
                to_rel_path: "b/Bocchi S01E01.mkv".into(),
                size: 100,
            }]
        );
        // 原始 diff 保持完整：移动未经 hash 确认前，new/missing 照常报告。
        assert_eq!(plan.new_files.len(), 1);
        assert_eq!(plan.missing_files, vec!["a/ep01.mkv".to_string()]);
    }

    #[test]
    fn size_mismatch_is_not_a_moved_candidate() {
        let d = [discovered("b/other.mkv", 200, 2000)];
        let k = [known("a/ep01.mkv", 100, 1000)];
        let plan = plan_scan(&d, &k);
        assert!(plan.moved_candidates.is_empty());
        assert_eq!(plan.new_files.len(), 1);
        assert_eq!(plan.missing_files.len(), 1);
    }

    #[test]
    fn moved_pairing_is_greedy_one_to_one_and_deterministic() {
        // 两个同 size 缺失文件、两个同 size 新文件 → 按 rel_path 顺序一对一。
        let d = [
            discovered("new/b.mkv", 100, 1),
            discovered("new/a.mkv", 100, 1),
        ];
        let k = [known("old/y.mkv", 100, 1), known("old/x.mkv", 100, 1)];
        let plan = plan_scan(&d, &k);

        assert_eq!(
            plan.moved_candidates,
            vec![
                MovedCandidate {
                    from_rel_path: "old/x.mkv".into(),
                    to_rel_path: "new/a.mkv".into(),
                    size: 100,
                },
                MovedCandidate {
                    from_rel_path: "old/y.mkv".into(),
                    to_rel_path: "new/b.mkv".into(),
                    size: 100,
                },
            ]
        );
    }

    #[test]
    fn one_missing_pairs_with_at_most_one_new() {
        let d = [
            discovered("new/a.mkv", 100, 1),
            discovered("new/b.mkv", 100, 1),
        ];
        let k = [known("old/x.mkv", 100, 1)];
        let plan = plan_scan(&d, &k);
        assert_eq!(plan.moved_candidates.len(), 1);
        assert_eq!(plan.moved_candidates[0].from_rel_path, "old/x.mkv");
        assert_eq!(plan.moved_candidates[0].to_rel_path, "new/a.mkv");
    }

    #[test]
    fn mixed_scenario_all_branches_at_once() {
        let d = [
            discovered("keep.mkv", 10, 100),      // 未变更
            discovered("touched.mkv", 20, 999),   // mtime 变了 → changed
            discovered("brand-new.mkv", 30, 100), // 全新（无同 size 缺失）
            discovered("moved-to.mkv", 40, 100),  // 与 moved-from 同 size → 移动候选
        ];
        let k = [
            known("keep.mkv", 10, 100),
            known("touched.mkv", 20, 100),
            known("moved-from.mkv", 40, 50),
            known("gone.mkv", 99, 50), // 消失且无同 size 新文件
        ];
        let plan = plan_scan(&d, &k);

        assert_eq!(
            plan.new_files.iter().map(|f| f.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["brand-new.mkv", "moved-to.mkv"]
        );
        assert_eq!(
            plan.changed_files.iter().map(|f| f.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["touched.mkv"]
        );
        assert_eq!(
            plan.missing_files,
            vec!["gone.mkv".to_string(), "moved-from.mkv".to_string()]
        );
        assert_eq!(
            plan.moved_candidates,
            vec![MovedCandidate {
                from_rel_path: "moved-from.mkv".into(),
                to_rel_path: "moved-to.mkv".into(),
                size: 40,
            }]
        );
    }
}
