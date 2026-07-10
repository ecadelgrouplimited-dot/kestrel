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
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// Relevance falloff applied per additional hop away from the seed.
const HOP_DECAY: f64 = 0.5;

/// Approximate characters per token for cost estimation. A deliberately rough
/// heuristic — good enough to keep a budget honest without a tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// The most query-matching files to use as seeds before expanding the graph.
const MAX_QUERY_SEEDS: usize = 6;

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

/// A ranked, budget-bounded selection of files relevant to a seed or query.
#[derive(Debug, Clone)]
pub struct ContextPack {
    /// Human-readable description of what the pack was built for.
    pub seed: String,
    pub budget_tokens: usize,
    pub used_tokens: usize,
    /// Included files, most relevant first.
    pub entries: Vec<ContextEntry>,
    /// Relevant files that did not fit the budget, still ranked.
    pub omitted: Vec<ContextEntry>,
}

/// Build a context pack seeded from a single file, or `None` if the seed path
/// is not a node in the graph.
pub fn build_context_pack(
    graph: &ProjectGraph,
    seed: &Path,
    budget_tokens: usize,
) -> Option<ContextPack> {
    let seed_idx = graph.files.iter().position(|f| f.path == seed)?;
    let mut reasons = HashMap::new();
    reasons.insert(seed_idx, "seed file".to_string());
    Some(assemble(
        graph,
        &[(seed_idx, 1.0)],
        &reasons,
        Some(seed),
        Some(seed_idx),
        format!("file {}", seed.display()),
        budget_tokens,
    ))
}

/// Build a context pack seeded from a natural-language query: files whose
/// symbols or path match the query terms become seeds, and relevance spreads
/// outward across the dependency graph from all of them.
pub fn build_context_pack_for_query(
    graph: &ProjectGraph,
    query: &str,
    budget_tokens: usize,
) -> ContextPack {
    let matches = query_matches(graph, query);
    let seeds: Vec<(usize, f64)> = matches.iter().map(|m| (m.idx, m.score)).collect();
    let reasons: HashMap<usize, String> = matches
        .iter()
        .map(|m| (m.idx, format!("matches query: {}", m.terms.join(", "))))
        .collect();
    assemble(
        graph,
        &seeds,
        &reasons,
        None,
        None,
        format!("query \"{query}\""),
        budget_tokens,
    )
}

/// The shared ranking + budget-fill core, seeded from one or more files.
/// `primary` (if given) tunes the wording of non-seed reasons and is the one
/// file force-included even when it exceeds the budget.
#[allow(clippy::too_many_arguments)]
fn assemble(
    graph: &ProjectGraph,
    seeds: &[(usize, f64)],
    seed_reasons: &HashMap<usize, String>,
    primary: Option<&Path>,
    force_idx: Option<usize>,
    seed_label: String,
    budget_tokens: usize,
) -> ContextPack {
    let n = graph.files.len();
    let index: HashMap<&Path, usize> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.as_path(), i))
        .collect();

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

    let seed_set: HashSet<usize> = seeds.iter().map(|(i, _)| *i).collect();

    // Multi-source shortest hop distance from every seed.
    let mut hops = vec![usize::MAX; n];
    let mut queue = VecDeque::new();
    for &(i, _) in seeds {
        if hops[i] == usize::MAX {
            hops[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(u) = queue.pop_front() {
        for &(v, _) in &adjacency[u] {
            if hops[v] == usize::MAX {
                hops[v] = hops[u] + 1;
                queue.push_back(v);
            }
        }
    }

    // Spreading activation: seeds keep their initial score; every other node's
    // relevance is the decayed sum of edge weights to the seed-ward frontier.
    let max_hop = hops
        .iter()
        .copied()
        .filter(|&h| h != usize::MAX)
        .max()
        .unwrap_or(0);
    let mut score = vec![0f64; n];
    for &(i, s) in seeds {
        score[i] = score[i].max(s);
    }
    for level in 1..=max_hop {
        for v in 0..n {
            if hops[v] != level || seed_set.contains(&v) {
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

    // Rank all reachable files: seeds first (hop 0), then by relevance.
    let mut order: Vec<usize> = (0..n).filter(|&i| hops[i] != usize::MAX).collect();
    order.sort_by(|&a, &b| {
        hops[a]
            .cmp(&hops[b])
            .then_with(|| score[b].partial_cmp(&score[a]).unwrap_or(Ordering::Equal))
            .then_with(|| graph.files[a].path.cmp(&graph.files[b].path))
    });

    let mut entries = Vec::new();
    let mut omitted = Vec::new();
    let mut used = 0usize;
    for &i in &order {
        let file = &graph.files[i];
        let estimated_tokens = estimate_tokens(file.source_bytes);
        let reason = if let Some(reason) = seed_reasons.get(&i) {
            reason.clone()
        } else if let Some(primary) = primary {
            reason_for(graph, primary, &file.path, hops[i])
        } else {
            format!("{} hops from a query match", hops[i])
        };
        let entry = ContextEntry {
            path: file.path.clone(),
            language: file.language.clone(),
            reason,
            hops: hops[i],
            relevance: score[i],
            symbols: file.symbols.clone(),
            estimated_tokens,
        };
        if force_idx == Some(i) || used + estimated_tokens <= budget_tokens {
            used += estimated_tokens;
            entries.push(entry);
        } else {
            omitted.push(entry);
        }
    }

    ContextPack {
        seed: seed_label,
        budget_tokens,
        used_tokens: used,
        entries,
        omitted,
    }
}

/// A file matched by a query, with its score and the matched terms/symbols.
struct QueryMatch {
    idx: usize,
    score: f64,
    terms: Vec<String>,
}

/// Score every file against the query's terms by symbol-name and path matches,
/// returning the top seeds. Exact symbol-name matches weigh most.
fn query_matches(graph: &ProjectGraph, query: &str) -> Vec<QueryMatch> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(str::to_lowercase)
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (idx, file) in graph.files.iter().enumerate() {
        let mut score = 0.0;
        let mut matched: BTreeSet<String> = BTreeSet::new();
        let path_lower = file.path.to_string_lossy().to_lowercase();
        for term in &terms {
            for symbol in &file.symbols {
                let name = symbol.name.to_lowercase();
                if name == *term {
                    score += 3.0;
                    matched.insert(symbol.name.clone());
                } else if name.contains(term) {
                    score += 1.5;
                    matched.insert(symbol.name.clone());
                }
            }
            if path_lower.contains(term) {
                score += 1.0;
                matched.insert(term.clone());
            }
        }
        if score > 0.0 {
            out.push(QueryMatch {
                idx,
                score,
                terms: matched.into_iter().take(4).collect(),
            });
        }
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| graph.files[a.idx].path.cmp(&graph.files[b.idx].path))
    });
    out.truncate(MAX_QUERY_SEEDS);
    out
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

    #[test]
    fn query_seeds_from_symbol_matches_and_expands() {
        let files = vec![
            file("auth.rs", &["authenticate", "AuthToken"], &[], 400),
            file("db.rs", &["connect"], &["AuthToken"], 400), // references AuthToken
            file("ui.rs", &["render"], &[], 400),             // unrelated
        ];
        let graph = build_graph_from_files(files);
        let pack = build_context_pack_for_query(&graph, "auth token", 10_000);

        assert_eq!(pack.entries[0].path, PathBuf::from("auth.rs"));
        assert!(pack.entries[0].reason.starts_with("matches query"));
        let paths: Vec<_> = pack.entries.iter().map(|e| e.path.clone()).collect();
        // db.rs is pulled in by graph proximity to the matched seed.
        assert!(paths.contains(&PathBuf::from("db.rs")));
        // ui.rs neither matches nor connects, so it is excluded.
        assert!(!paths.contains(&PathBuf::from("ui.rs")));
    }

    #[test]
    fn query_with_no_matches_is_empty() {
        let graph = build_graph_from_files(vec![file("a.rs", &["foo"], &[], 400)]);
        let pack = build_context_pack_for_query(&graph, "nonexistentxyz", 10_000);
        assert!(pack.entries.is_empty());
        assert!(pack.omitted.is_empty());
    }
}
