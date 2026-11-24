//! Canonical relative path normalization, cross-platform collision detection,
//! and sync rule topology conflict detection.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{LanVaultError, Result};
use crate::protocol::SyncRuleMode;

/// Cross-platform illegal characters in any path segment (Windows + POSIX).
const ILLEGAL_SEGMENT_CHARS: [char; 8] = ['<', '>', ':', '"', '\\', '|', '?', '*'];

/// Windows reserved device names (case-insensitive, even with extensions like `CON.txt`).
const WINDOWS_RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Normalizes a relative path into a canonical POSIX `/`-delimited path key.
///
/// Rejects empty strings, `.`, `..`, leading or trailing `/`, empty segments (`//`),
/// NUL/control characters, Windows reserved device names, trailing spaces or dots on
/// any segment, and cross-platform illegal characters (`< > : " \ | ? *`).
pub fn normalize_canonical_path(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(LanVaultError::InvalidInput(
            "canonical path cannot be empty".to_string(),
        ));
    }
    if raw.starts_with('/') || raw.ends_with('/') {
        return Err(LanVaultError::InvalidInput(format!(
            "canonical path must not have leading or trailing '/': `{raw}`"
        )));
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(LanVaultError::InvalidInput(format!(
            "canonical path contains NUL or control characters: `{raw}`"
        )));
    }

    let mut segments = Vec::new();
    for segment in raw.split('/') {
        if segment.is_empty() {
            return Err(LanVaultError::InvalidInput(format!(
                "canonical path contains empty segment: `{raw}`"
            )));
        }
        if segment == "." || segment == ".." {
            return Err(LanVaultError::InvalidInput(format!(
                "canonical path contains relative traversal segment `{segment}`: `{raw}`"
            )));
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return Err(LanVaultError::InvalidInput(format!(
                "path segment `{segment}` ends with a trailing space or dot: `{raw}`"
            )));
        }
        if let Some(bad) = segment.chars().find(|c| ILLEGAL_SEGMENT_CHARS.contains(c)) {
            return Err(LanVaultError::InvalidInput(format!(
                "path segment `{segment}` contains cross-platform illegal character `{bad}`: `{raw}`"
            )));
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        let upper_stem = stem.to_ascii_uppercase();
        if WINDOWS_RESERVED_NAMES.contains(&upper_stem.as_str()) {
            return Err(LanVaultError::InvalidInput(format!(
                "path segment `{segment}` uses Windows reserved device name `{upper_stem}`: `{raw}`"
            )));
        }
        segments.push(segment);
    }

    Ok(segments.join("/"))
}

/// Returns a case-folded comparison key for detecting case-insensitive path collisions.
pub fn case_fold_key(path_key: &str) -> String {
    path_key.to_lowercase()
}

/// Classification of a detected path collision or invalid path segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathCollisionKind {
    ExactDuplicate,
    CaseFoldCollision,
    InvalidSegment,
}

/// A single path collision or cross-platform validity issue found in a path set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCollision {
    pub kind: PathCollisionKind,
    pub path: String,
    pub conflicts_with: Option<String>,
    pub detail: String,
}

/// Detects exact duplicates, case-folding collisions (e.g. `Foo.txt` vs `foo.txt`),
/// and cross-platform invalid segments across a list of relative paths.
pub fn detect_path_collisions(paths: &[String]) -> Vec<PathCollision> {
    let mut collisions = Vec::new();
    let mut seen_exact: HashSet<String> = HashSet::with_capacity(paths.len());
    let mut seen_folded: HashMap<String, String> = HashMap::with_capacity(paths.len());

    for raw in paths {
        let canonical = match normalize_canonical_path(raw) {
            Ok(p) => p,
            Err(err) => {
                collisions.push(PathCollision {
                    kind: PathCollisionKind::InvalidSegment,
                    path: raw.clone(),
                    conflicts_with: None,
                    detail: err.to_string(),
                });
                continue;
            }
        };

        if !seen_exact.insert(canonical.clone()) {
            collisions.push(PathCollision {
                kind: PathCollisionKind::ExactDuplicate,
                path: canonical.clone(),
                conflicts_with: Some(canonical),
                detail: "duplicate canonical path".to_string(),
            });
            continue;
        }

        let folded = case_fold_key(&canonical);
        if let Some(existing) = seen_folded.get(&folded) {
            collisions.push(PathCollision {
                kind: PathCollisionKind::CaseFoldCollision,
                path: canonical,
                conflicts_with: Some(existing.clone()),
                detail: format!("case-folding collision with `{existing}`"),
            });
        } else {
            seen_folded.insert(folded, canonical);
        }
    }

    collisions
}

/// Topology descriptor of a sync rule used for conflict and cycle detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTopologySpec {
    pub rule_id: String,
    pub mode: SyncRuleMode,
    pub source_device_id: String,
    pub source_root: String,
    /// List of `(target_device_id, target_root)` bindings for this rule.
    pub targets: Vec<(String, String)>,
}

/// Normalizes an absolute filesystem root path for cross-rule overlap comparison.
fn normalize_root_for_topology(raw: &str) -> String {
    let stripped = crate::strip_windows_verbatim(raw);
    let mut s = stripped.replace('\\', "/");
    while s.contains("//") && !s.starts_with("//") {
        s = s.replace("//", "/");
    }
    // Preserve POSIX root `/` or Windows drive root `c:/`, strip trailing `/` otherwise.
    let is_windows_drive_root = s.len() == 3 && s.as_bytes()[1] == b':' && s.as_bytes()[2] == b'/';
    while s.len() > 1 && s.ends_with('/') && !is_windows_drive_root {
        s.pop();
        if s.len() == 3 && s.as_bytes()[1] == b':' && s.as_bytes()[2] == b'/' {
            break;
        }
    }
    s.to_lowercase()
}

/// Returns `true` if two normalized filesystem roots are identical or one is an ancestor of the other.
fn roots_overlap(root_a: &str, root_b: &str) -> bool {
    let a = normalize_root_for_topology(root_a);
    let b = normalize_root_for_topology(root_b);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b || a == "/" || b == "/" {
        return true;
    }
    let is_prefix = |parent: &str, child: &str| -> bool {
        if !child.starts_with(parent) {
            return false;
        }
        if parent.ends_with('/') {
            true
        } else {
            child.as_bytes().get(parent.len()) == Some(&b'/')
        }
    };
    is_prefix(&a, &b) || is_prefix(&b, &a)
}

/// Detects topology conflicts across a set of sync rules:
/// - Same-host identical or overlapping source/target roots within a rule
/// - Overlapping targets on the same device (within a rule or across rules)
/// - Mutual mirrors (`A -> B` and `B -> A`)
/// - Directed cycles across `MirrorSync` rules (`A -> B -> C -> A`)
pub fn detect_rule_topology_conflicts(rules: &[RuleTopologySpec]) -> Vec<String> {
    let mut conflicts = Vec::new();

    // 1. Intra-rule checks: same-host overlapping source/target and overlapping targets.
    for rule in rules {
        for (idx, (t_dev, t_root)) in rule.targets.iter().enumerate() {
            if rule.source_device_id == *t_dev && roots_overlap(&rule.source_root, t_root) {
                conflicts.push(format!(
                    "rule `{}` has overlapping source and target root on device `{}` (`{}` <-> `{}`)",
                    rule.rule_id, t_dev, rule.source_root, t_root
                ));
            }
            for (other_dev, other_root) in &rule.targets[idx + 1..] {
                if t_dev == other_dev && roots_overlap(t_root, other_root) {
                    conflicts.push(format!(
                        "rule `{}` has overlapping target roots on device `{}` (`{}` <-> `{}`)",
                        rule.rule_id, t_dev, t_root, other_root
                    ));
                }
            }
        }
    }

    // 2. Cross-rule overlapping targets on the same device.
    for i in 0..rules.len() {
        for j in (i + 1)..rules.len() {
            let a = &rules[i];
            let b = &rules[j];
            for (dev_a, root_a) in &a.targets {
                for (dev_b, root_b) in &b.targets {
                    if dev_a == dev_b && roots_overlap(root_a, root_b) {
                        conflicts.push(format!(
                            "rules `{}` and `{}` write to overlapping target roots on device `{}` (`{}` <-> `{}`)",
                            a.rule_id, b.rule_id, dev_a, root_a, root_b
                        ));
                    }
                }
            }
        }
    }

    // 3. Mutual mirror and directed cycle detection across MirrorSync rules.
    struct MirrorEdge<'a> {
        rule_id: &'a str,
        src_dev: &'a str,
        src_root: &'a str,
        tgt_dev: &'a str,
        tgt_root: &'a str,
    }

    let mut edges: Vec<MirrorEdge<'_>> = Vec::new();
    for rule in rules {
        if rule.mode != SyncRuleMode::MirrorSync {
            continue;
        }
        for (tgt_dev, tgt_root) in &rule.targets {
            edges.push(MirrorEdge {
                rule_id: &rule.rule_id,
                src_dev: &rule.source_device_id,
                src_root: &rule.source_root,
                tgt_dev,
                tgt_root,
            });
        }
    }

    // Check explicit mutual mirrors (2-cycles).
    let mut mutual_pairs: HashSet<(usize, usize)> = HashSet::new();
    for i in 0..edges.len() {
        for j in (i + 1)..edges.len() {
            let e1 = &edges[i];
            let e2 = &edges[j];
            let forward = e1.tgt_dev == e2.src_dev && roots_overlap(e1.tgt_root, e2.src_root);
            let backward = e2.tgt_dev == e1.src_dev && roots_overlap(e2.tgt_root, e1.src_root);
            if forward && backward {
                mutual_pairs.insert((i, j));
                conflicts.push(format!(
                    "mutual mirror conflict between rule `{}` ({}:{} -> {}:{}) and rule `{}` ({}:{} -> {}:{})",
                    e1.rule_id,
                    e1.src_dev,
                    e1.src_root,
                    e1.tgt_dev,
                    e1.tgt_root,
                    e2.rule_id,
                    e2.src_dev,
                    e2.src_root,
                    e2.tgt_dev,
                    e2.tgt_root
                ));
            }
        }
    }

    // Build adjacency list over mirror edges for general directed cycle detection (length >= 3).
    let n = edges.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            if edges[i].tgt_dev == edges[j].src_dev
                && roots_overlap(edges[i].tgt_root, edges[j].src_root)
            {
                adj[i].push(j);
            }
        }
    }

    // 3-color DFS: 0 = unvisited, 1 = on stack, 2 = done.
    let mut color = vec![0u8; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut reported_cycles: HashSet<Vec<String>> = HashSet::new();

    fn dfs(
        u: usize,
        adj: &[Vec<usize>],
        edges: &[MirrorEdge<'_>],
        mutual_pairs: &HashSet<(usize, usize)>,
        color: &mut [u8],
        stack: &mut Vec<usize>,
        reported_cycles: &mut HashSet<Vec<String>>,
        conflicts: &mut Vec<String>,
    ) {
        color[u] = 1;
        stack.push(u);
        for &v in &adj[u] {
            if color[v] == 0 {
                dfs(
                    v,
                    adj,
                    edges,
                    mutual_pairs,
                    color,
                    stack,
                    reported_cycles,
                    conflicts,
                );
            } else if color[v] == 1 {
                if let Some(pos) = stack.iter().position(|&node| node == v) {
                    let cycle_nodes = &stack[pos..];
                    // Skip 2-cycles already reported as mutual mirrors.
                    if cycle_nodes.len() == 2 {
                        let pair = (
                            cycle_nodes[0].min(cycle_nodes[1]),
                            cycle_nodes[0].max(cycle_nodes[1]),
                        );
                        if mutual_pairs.contains(&pair) {
                            continue;
                        }
                    }
                    let mut rule_ids: Vec<String> = cycle_nodes
                        .iter()
                        .map(|&idx| edges[idx].rule_id.to_string())
                        .collect();
                    let mut canonical_key = rule_ids.clone();
                    canonical_key.sort();
                    if reported_cycles.insert(canonical_key) {
                        rule_ids.push(edges[cycle_nodes[0]].rule_id.to_string());
                        conflicts.push(format!(
                            "directed mirror cycle detected: {}",
                            rule_ids.join(" -> ")
                        ));
                    }
                }
            }
        }
        stack.pop();
        color[u] = 2;
    }

    for i in 0..n {
        if color[i] == 0 {
            dfs(
                i,
                &adj,
                &edges,
                &mutual_pairs,
                &mut color,
                &mut stack,
                &mut reported_cycles,
                &mut conflicts,
            );
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_canonical_path_accepts_valid_paths() {
        assert_eq!(
            normalize_canonical_path("docs/report.pdf").unwrap(),
            "docs/report.pdf"
        );
        assert_eq!(
            normalize_canonical_path("a/b/c.tar.gz").unwrap(),
            "a/b/c.tar.gz"
        );
        assert_eq!(
            normalize_canonical_path("照片/2026/春节.jpg").unwrap(),
            "照片/2026/春节.jpg"
        );
    }

    #[test]
    fn normalize_canonical_path_rejects_invalid_forms() {
        assert!(normalize_canonical_path("").is_err());
        assert!(normalize_canonical_path(".").is_err());
        assert!(normalize_canonical_path("..").is_err());
        assert!(normalize_canonical_path("a/../b").is_err());
        assert!(normalize_canonical_path("a/./b").is_err());
        assert!(normalize_canonical_path("/leading").is_err());
        assert!(normalize_canonical_path("trailing/").is_err());
        assert!(normalize_canonical_path("a//b").is_err());
        assert!(normalize_canonical_path("a\0b").is_err());
        assert!(normalize_canonical_path("a\nb").is_err());
        assert!(normalize_canonical_path("dir/CON").is_err());
        assert!(normalize_canonical_path("dir/nul.txt").is_err());
        assert!(normalize_canonical_path("COM1").is_err());
        assert!(normalize_canonical_path("lpt9.log").is_err());
        assert!(normalize_canonical_path("dir/file ").is_err());
        assert!(normalize_canonical_path("dir/file.").is_err());
        for bad in ["a<b", "a>b", "a:b", "a\"b", "a\\b", "a|b", "a?b", "a*b"] {
            assert!(
                normalize_canonical_path(bad).is_err(),
                "expected `{bad}` to be rejected"
            );
        }
    }

    #[test]
    fn detect_path_collisions_catches_duplicates_case_folds_and_invalids() {
        let paths = vec![
            "Docs/Readme.md".to_string(),
            "Docs/Readme.md".to_string(),
            "docs/readme.md".to_string(),
            "bad/CON.txt".to_string(),
        ];
        let collisions = detect_path_collisions(&paths);
        assert_eq!(collisions.len(), 3);
        assert_eq!(collisions[0].kind, PathCollisionKind::ExactDuplicate);
        assert_eq!(collisions[1].kind, PathCollisionKind::CaseFoldCollision);
        assert_eq!(
            collisions[1].conflicts_with.as_deref(),
            Some("Docs/Readme.md")
        );
        assert_eq!(collisions[2].kind, PathCollisionKind::InvalidSegment);
    }

    #[test]
    fn topology_detects_same_host_overlap_mutual_mirror_and_cycle() {
        let same_host = vec![RuleTopologySpec {
            rule_id: "r1".to_string(),
            mode: SyncRuleMode::MirrorSync,
            source_device_id: "dev-a".to_string(),
            source_root: r"C:\Work\Docs".to_string(),
            targets: vec![("dev-a".to_string(), "c:/work/docs/sub".to_string())],
        }];
        assert!(!detect_rule_topology_conflicts(&same_host).is_empty());

        let mutual = vec![
            RuleTopologySpec {
                rule_id: "m1".to_string(),
                mode: SyncRuleMode::MirrorSync,
                source_device_id: "dev-a".to_string(),
                source_root: "/data/a".to_string(),
                targets: vec![("dev-b".to_string(), "/data/b".to_string())],
            },
            RuleTopologySpec {
                rule_id: "m2".to_string(),
                mode: SyncRuleMode::MirrorSync,
                source_device_id: "dev-b".to_string(),
                source_root: "/data/b".to_string(),
                targets: vec![("dev-a".to_string(), "/data/a".to_string())],
            },
        ];
        let mutual_conflicts = detect_rule_topology_conflicts(&mutual);
        assert!(mutual_conflicts.iter().any(|c| c.contains("mutual mirror")));

        let cycle = vec![
            RuleTopologySpec {
                rule_id: "c1".to_string(),
                mode: SyncRuleMode::MirrorSync,
                source_device_id: "dev-a".to_string(),
                source_root: "/sync".to_string(),
                targets: vec![("dev-b".to_string(), "/sync".to_string())],
            },
            RuleTopologySpec {
                rule_id: "c2".to_string(),
                mode: SyncRuleMode::MirrorSync,
                source_device_id: "dev-b".to_string(),
                source_root: "/sync".to_string(),
                targets: vec![("dev-c".to_string(), "/sync".to_string())],
            },
            RuleTopologySpec {
                rule_id: "c3".to_string(),
                mode: SyncRuleMode::MirrorSync,
                source_device_id: "dev-c".to_string(),
                source_root: "/sync".to_string(),
                targets: vec![("dev-a".to_string(), "/sync".to_string())],
            },
        ];
        let cycle_conflicts = detect_rule_topology_conflicts(&cycle);
        assert!(cycle_conflicts
            .iter()
            .any(|c| c.contains("directed mirror cycle")));
    }

    #[test]
    fn topology_allows_clean_fanout_rules() {
        let clean = vec![RuleTopologySpec {
            rule_id: "fanout".to_string(),
            mode: SyncRuleMode::MirrorSync,
            source_device_id: "dev-a".to_string(),
            source_root: "/data/photos".to_string(),
            targets: vec![
                ("dev-b".to_string(), "/mirror/photos".to_string()),
                ("dev-c".to_string(), "/mirror/photos".to_string()),
            ],
        }];
        assert!(detect_rule_topology_conflicts(&clean).is_empty());
    }
}
