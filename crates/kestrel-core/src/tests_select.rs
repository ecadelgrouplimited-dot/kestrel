//! Test selection: given the changed files, pick the tests worth running.
//!
//! Running the whole suite after a one-file change is wasteful. This selects the
//! test files affected by a change — those that reach a changed file across the
//! dependency graph, or whose module name matches one — and builds a best-effort
//! command to run just them for the detected framework. It reuses the same
//! `ProjectGraph` that powers context packing.

use crate::graph::{build_project_graph, ProjectGraph};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// The tests affected by a set of changes, and how to run them.
#[derive(Debug, Clone, Default)]
pub struct TestSelection {
    /// Affected test files, project-relative.
    pub test_files: Vec<PathBuf>,
    /// A best-effort command to run just those tests, if one could be built.
    pub command: Option<String>,
    /// A human-readable summary.
    pub note: String,
}

/// Which test framework the project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Cargo,
    Pytest,
    Jest,
    Vitest,
    NodeGeneric,
    Unknown,
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// True if a path looks like a test file.
pub fn is_test_path(path: &Path) -> bool {
    let s = normalize(path);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    s.contains("/tests/")
        || s.starts_with("tests/")
        || s.contains("__tests__")
        || s.contains(".test.")
        || s.contains(".spec.")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
}

/// The base module name of a file, stripped of test decoration, so `foo.rs`,
/// `foo.test.ts`, `test_foo.py`, and `foo_test.rs` all reduce to `foo`.
fn base_module(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let base = name.split('.').next().unwrap_or("");
    let base = base.strip_prefix("test_").unwrap_or(base);
    let base = base
        .strip_suffix("_tests")
        .or_else(|| base.strip_suffix("_test"))
        .unwrap_or(base);
    base.to_string()
}

/// Select the test files affected by `changed` (project-relative paths) and a
/// command to run just them.
pub fn select_tests(root: &Path, changed: &[String]) -> TestSelection {
    let graph = build_project_graph(root).ok();
    let changed_bases: HashSet<String> =
        changed.iter().map(|c| base_module(Path::new(c))).collect();
    let changed_norm: HashSet<String> = changed
        .iter()
        .map(|c| c.replace('\\', "/").to_lowercase())
        .collect();

    let mut affected: Vec<PathBuf> = Vec::new();
    if let Some(graph) = &graph {
        for path in graph_affected_tests(graph, &changed_norm) {
            affected.push(path);
        }
        // Also any test file whose module name matches a changed file.
        for file in &graph.files {
            if is_test_path(&file.path) && changed_bases.contains(&base_module(&file.path)) {
                affected.push(file.path.clone());
            }
        }
    }
    // Changed files that are themselves tests always count.
    for c in changed {
        let p = PathBuf::from(c);
        if is_test_path(&p) {
            affected.push(p);
        }
    }
    affected.sort();
    affected.dedup();

    let framework = detect_framework(root);
    let command = build_command(framework, &affected, changed);
    let note = if affected.is_empty() {
        "No tests looked affected by these changes.".to_string()
    } else {
        format!("{} affected test file(s).", affected.len())
    };
    TestSelection {
        test_files: affected,
        command,
        note,
    }
}

/// Test files within two hops of any changed file across the dependency graph.
fn graph_affected_tests(graph: &ProjectGraph, changed_norm: &HashSet<String>) -> Vec<PathBuf> {
    let n = graph.files.len();
    let index: HashMap<&Path, usize> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.as_path(), i))
        .collect();
    let mut adjacency = vec![Vec::new(); n];
    for edge in &graph.edges {
        if let (Some(&a), Some(&b)) = (index.get(edge.from.as_path()), index.get(edge.to.as_path()))
        {
            adjacency[a].push(b);
            adjacency[b].push(a);
        }
    }
    let mut dist = vec![usize::MAX; n];
    let mut queue = VecDeque::new();
    for (i, file) in graph.files.iter().enumerate() {
        if changed_norm.contains(&normalize(&file.path)) {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(u) = queue.pop_front() {
        if dist[u] >= 2 {
            continue;
        }
        for &v in &adjacency[u] {
            if dist[v] == usize::MAX {
                dist[v] = dist[u] + 1;
                queue.push_back(v);
            }
        }
    }
    graph
        .files
        .iter()
        .enumerate()
        .filter(|(i, f)| dist[*i] != usize::MAX && is_test_path(&f.path))
        .map(|(_, f)| f.path.clone())
        .collect()
}

fn detect_framework(root: &Path) -> Framework {
    if root.join("Cargo.toml").exists() {
        return Framework::Cargo;
    }
    if root.join("package.json").exists() {
        let text = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
        if text.contains("vitest") {
            return Framework::Vitest;
        }
        if text.contains("jest") {
            return Framework::Jest;
        }
        return Framework::NodeGeneric;
    }
    if root.join("pyproject.toml").exists()
        || root.join("pytest.ini").exists()
        || root.join("setup.py").exists()
        || root.join("requirements.txt").exists()
    {
        return Framework::Pytest;
    }
    Framework::Unknown
}

fn build_command(framework: Framework, affected: &[PathBuf], changed: &[String]) -> Option<String> {
    let affected_paths: Vec<String> = affected.iter().map(|p| normalize(p)).collect();
    let changed_src: Vec<String> = changed
        .iter()
        .filter(|c| {
            let low = c.to_lowercase();
            [".ts", ".tsx", ".js", ".jsx"]
                .iter()
                .any(|e| low.ends_with(e))
        })
        .map(|c| c.replace('\\', "/"))
        .collect();

    match framework {
        Framework::Cargo => {
            let mut stems: Vec<String> = affected.iter().map(|p| base_module(p)).collect();
            stems.sort();
            stems.dedup();
            if stems.is_empty() {
                Some("cargo test".to_string())
            } else {
                Some(format!("cargo test {}", stems.join(" ")))
            }
        }
        Framework::Pytest => {
            if affected_paths.is_empty() {
                None
            } else {
                Some(format!("pytest {}", affected_paths.join(" ")))
            }
        }
        Framework::Jest => {
            if !changed_src.is_empty() {
                Some(format!(
                    "npx jest --findRelatedTests {}",
                    changed_src.join(" ")
                ))
            } else if !affected_paths.is_empty() {
                Some(format!("npx jest {}", affected_paths.join(" ")))
            } else {
                None
            }
        }
        Framework::Vitest | Framework::NodeGeneric => {
            if !changed_src.is_empty() {
                Some(format!(
                    "npx vitest related {} --run",
                    changed_src.join(" ")
                ))
            } else if !affected_paths.is_empty() {
                Some(format!("npx vitest run {}", affected_paths.join(" ")))
            } else {
                None
            }
        }
        Framework::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_test_paths() {
        assert!(is_test_path(Path::new("src/foo.test.ts")));
        assert!(is_test_path(Path::new("tests/api.rs")));
        assert!(is_test_path(Path::new("pkg/test_utils.py")));
        assert!(is_test_path(Path::new("src/user_test.rs")));
        assert!(!is_test_path(Path::new("src/user.rs")));
    }

    #[test]
    fn base_module_strips_decoration() {
        assert_eq!(base_module(Path::new("foo.rs")), "foo");
        assert_eq!(base_module(Path::new("foo.test.ts")), "foo");
        assert_eq!(base_module(Path::new("test_foo.py")), "foo");
        assert_eq!(base_module(Path::new("foo_test.rs")), "foo");
    }

    #[test]
    fn selects_matching_test_and_builds_command() {
        let dir = std::env::temp_dir().join(format!("kestrel-tsel-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::write(dir.join("src/user.rs"), "pub fn u() {}\n").unwrap();
        std::fs::write(dir.join("src/user_test.rs"), "fn t() {}\n").unwrap();

        let selection = select_tests(&dir, &["src/user.rs".to_string()]);
        assert!(selection
            .test_files
            .iter()
            .any(|p| p.ends_with("user_test.rs")));
        let command = selection.command.unwrap();
        assert!(command.starts_with("cargo test"));
        assert!(command.contains("user"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
