//! Context packing: turn the dependency graph into a ranked, budget-bounded
//! selection of the files most relevant to a seed.
//!
//! This is the payoff of the Ghost Context Engine and the `ContextPack`
//! structure named in the technical architecture. Instead of dumping a whole
//! repository into a prompt, Kestrel starts from a seed file, spreads
//! relevance outward across dependency edges (in both directions), ranks every
//! reachable file, and fills a token budget greedily — recording *why* each
//! file was included. It is the concrete answer to the product requirement to
//! "select relevant files without full-repo dumping" and "explain why a file
//! was included".

use crate::graph::ProjectGraph;
use crate::symbols::Symbol;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Relevance falloff applied per additional hop away from the seed.
const HOP_DECAY: f64 = 0.5;

/// Approximate characters per token for cost estimation. A deliberately rough
/// heuristic — good enough to keep a budget honest without a tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// Estimate the token cost of a piece of source given its character count.
pub fn estimate_tokens(chars: usize) -> usize {
    if chars == 0 {
        0
    } else {
        chars.div_ceil(CHARS_PER_TOKEN)
    }
}

/// One file's place in a context pack.
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub path: PathBuf,
    pub language: String,
    /// Human-readable justification for the file's inclusion.
    pub reason: String,
    /// Graph distance from the seed (0 for the seed itself).
    pub hops: usize,
    /// Spreading-activation relevance score (higher is more relevant).
    pub relevance: f64,
    /// The file's declared symbols (its structural surface).
    pub symbols: Vec<Symbol>,
    /// Estimated token cost of including the file's full source.
    pub estimated_tokens: usize,
}

/// A ranked, budget-bounded selection of files relevant to a seed.
#[derive(Debug, Clone)]
pub struct ContextPack {
    pub seed: PathBuf,
    pub budget_tokens: usize,
    pub used_tokens: usize,
    /// Included files, most relevant first (the seed is always first).
    pub entries: Vec<ContextEntry>,
    /// Relevant files that did not fit the budget, still ranked.
    pub omitted: Vec<ContextEntry>,
}

/// Build a context pack for `seed` within `budget_tokens`, or `None` if the
/// seed path is not a node in the graph. Pure over the graph, so ranking is
/// deterministic and testable.
pub fn build_context_pack(
    graph: &ProjectGraph,
    seed: &Path,
    budget_tokens: usize,
) -> Option<ContextPack> {
    let index: HashMap<&Path, usize> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.as_path(), i))
        .collect();
    let seed_idx = *index.get(seed)?;
    let n = graph.files.len();

    // Undirected adjacency with edge weights.
    let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for edge in &graph.edges {
        if let (Some(&a), Some(&b)) = (index.get(edge.from.as_path()), index.get(edge.to.as_path()))
        {
            let w = edge.weight();
            adjacency[a].push((b, w));
            adjacency[b].push((a, w));
        }
    }

    // Shortest hop distance from the seed.
    let mut hops = vec![usize::MAX; n];
    hops[seed_idx] = 0;
    let mut queue = VecDeque::from([seed_idx]);
    while let Some(u) = queue.pop_front() {
        for &(v, _) in &adjacency[u] {
            if hops[v] == usize::MAX {
                hops[v] = hops[u] + 1;
                queue.push_back(v);
            }
        }
    }

    // Spreading activation: a node's relevance is the decayed sum of edge
    // weights connecting it to the seed-ward frontier.
    let max_hop = hops
        .iter()
        .copied()
        .filter(|&h| h != usize::MAX)
        .max()
        .unwrap_or(0);
    let mut score = vec![0f64; n];
    score[seed_idx] = 1.0;
    for level in 1..=max_hop {
        for v in 0..n {
            if hops[v] != level {
                continue;
            }
            let mut s = 0.0;
            for &(u, w) in &adjacency[v] {
                if hops[u] + 1 == level {
                    s += w as f64 * score[u];
                }
            }
            score[v] = s * HOP_DECAY.powi((level as i32) - 1);
        }
    }

    // Rank all reachable files: seed first (hop 0), then by relevance.
    let mut order: Vec<usize> = (0..n).filter(|&i| hops[i] != usize::MAX).collect();
    order.sort_by(|&a, &b| {
        hops[a]
            .cmp(&hops[b])
            .then_with(|| {
                score[b]
                    .partial_cmp(&score[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| graph.files[a].path.cmp(&graph.files[b].path))
    });

    let mut entries = Vec::new();
    let mut omitted = Vec::new();
    let mut used = 0usize;
    for &i in &order {
        let file = &graph.files[i];
        let estimated_tokens = estimate_tokens(file.source_bytes);
        let entry = ContextEntry {
            path: file.path.clone(),
            language: file.language.clone(),
            reason: reason_for(graph, seed, &file.path, hops[i]),
            hops: hops[i],
            relevance: score[i],
            symbols: file.symbols.clone(),
            estimated_tokens,
        };
        // The seed is always included; others must fit the remaining budget.
        if i == seed_idx || used + estimated_tokens <= budget_tokens {
            used += estimated_tokens;
            entries.push(entry);
        } else {
            omitted.push(entry);
        }
    }

    Some(ContextPack {
        seed: graph.files[seed_idx].path.clone(),
        budget_tokens,
        used_tokens: used,
        entries,
        omitted,
    })
}

/// Compose a human-readable reason for including `node`, given its distance
/// from the seed and the edges that connect them.
fn reason_for(graph: &ProjectGraph, seed: &Path, node: &Path, hops: usize) -> String {
    if hops == 0 {
        return "seed file".to_string();
    }
    if hops == 1 {
        let mut forward_terms = Vec::new(); // seed depends on node
        let mut backward_terms = Vec::new(); // node depends on seed
        for edge in &graph.edges {
            if edge.from == seed && edge.to == node {
                forward_terms = edge_terms(&edge.imports, &edge.via);
            } else if edge.from == node && edge.to == seed {
                backward_terms = edge_terms(&edge.imports, &edge.via);
            }
        }
        return match (!forward_terms.is_empty(), !backward_terms.is_empty()) {
            (true, true) => "mutual dependency with the seed".to_string(),
            (true, false) => format!("used by the seed via {}", forward_terms.join(", ")),
            (false, true) => format!("depends on the seed via {}", backward_terms.join(", ")),
            // Reachable at one hop but only via the opposite orientation of a
            // multi-file pair; fall back to a generic description.
            (false, false) => "directly related to the seed".to_string(),
        };
    }
    format!("{hops} hops from the seed")
}

/// Up to three connecting terms, imports preferred over bare references.
fn edge_terms(imports: &[String], via: &[String]) -> Vec<String> {
    imports.iter().chain(via.iter()).take(3).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{build_graph_from_files, FileNode};
    use crate::symbols::SymbolKind;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line: 1,
            container: None,
            exported: true,
            signature: String::new(),
        }
    }

    fn file(path: &str, defs: &[&str], refs: &[&str], bytes: usize) -> FileNode {
        FileNode {
            path: PathBuf::from(path),
            language: "Rust".to_string(),
            symbols: defs.iter().map(|n| sym(n)).collect(),
            imports: Vec::new(),
            references: refs.iter().map(|s| s.to_string()).collect(),
            source_bytes: bytes,
        }
    }

    #[test]
    fn seed_is_always_first_and_neighbors_follow() {
        // seed uses B; C uses seed. Both are one hop away.
        let files = vec![
            file("seed.rs", &["seed_fn"], &["b_fn"], 400),
            file("b.rs", &["b_fn"], &[], 400),
            file("c.rs", &["c_fn"], &["seed_fn"], 400),
        ];
        let graph = build_graph_from_files(files);
        let pack = build_context_pack(&graph, Path::new("seed.rs"), 10_000).unwrap();

        assert_eq!(pack.entries[0].path, PathBuf::from("seed.rs"));
        assert_eq!(pack.entries[0].reason, "seed file");
        let paths: Vec<_> = pack.entries.iter().map(|e| e.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("b.rs")));
        assert!(paths.contains(&PathBuf::from("c.rs")));
        // 100 chars each -> 100 tokens each, 3 files.
        assert_eq!(pack.used_tokens, 300);
        assert!(pack.omitted.is_empty());
    }

    #[test]
    fn reasons_describe_edge_direction() {
        let files = vec![
            file("seed.rs", &["seed_fn"], &["b_fn"], 40),
            file("b.rs", &["b_fn"], &[], 40),
            file("c.rs", &["c_fn"], &["seed_fn"], 40),
        ];
        let graph = build_graph_from_files(files);
        let pack = build_context_pack(&graph, Path::new("seed.rs"), 10_000).unwrap();
        let reason = |p: &str| {
            pack.entries
                .iter()
                .find(|e| e.path == Path::new(p))
                .map(|e| e.reason.clone())
                .unwrap()
        };
        assert!(reason("b.rs").starts_with("used by the seed via"));
        assert!(reason("c.rs").starts_with("depends on the seed via"));
    }

    #[test]
    fn budget_omits_lower_ranked_files() {
        // Two neighbors: b is strongly connected (2 shared names), c weakly (1).
        // Budget fits the seed plus exactly one neighbor.
        let files = vec![
            file("seed.rs", &["seed_fn"], &["b_one", "b_two", "c_one"], 400),
            file("b.rs", &["b_one", "b_two"], &[], 400),
            file("c.rs", &["c_one"], &[], 400),
        ];
        let graph = build_graph_from_files(files);
        // 100 tokens per file; budget 250 fits seed + one neighbor.
        let pack = build_context_pack(&graph, Path::new("seed.rs"), 250).unwrap();
        assert_eq!(pack.entries.len(), 2);
        assert_eq!(pack.entries[0].path, PathBuf::from("seed.rs"));
        // b is more relevant than c, so it wins the single remaining slot.
        assert_eq!(pack.entries[1].path, PathBuf::from("b.rs"));
        assert_eq!(pack.omitted.len(), 1);
        assert_eq!(pack.omitted[0].path, PathBuf::from("c.rs"));
    }

    #[test]
    fn seed_included_even_when_over_budget() {
        let files = vec![file("seed.rs", &["seed_fn"], &[], 4_000)];
        let graph = build_graph_from_files(files);
        let pack = build_context_pack(&graph, Path::new("seed.rs"), 10).unwrap();
        assert_eq!(pack.entries.len(), 1);
        assert_eq!(pack.used_tokens, 1_000);
    }

    #[test]
    fn unknown_seed_returns_none() {
        let graph = build_graph_from_files(vec![file("a.rs", &[], &[], 10)]);
        assert!(build_context_pack(&graph, Path::new("missing.rs"), 100).is_none());
    }
}
