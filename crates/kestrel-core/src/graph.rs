//! The project dependency graph: the first real edge structure of the Ghost
//! Context Engine.
//!
//! Symbol extraction tells us what each file *defines*. This module joins that
//! with what each file *references* to infer which files depend on which — the
//! `FileNode` / `DependencyEdge` structures named in the technical
//! architecture. That graph is what lets Kestrel select a small, relevant set
//! of files for a task instead of dumping the whole repository into a prompt.
//!
//! Edge inference is intentionally conservative and language-agnostic: a file
//! `A` depends on file `B` when `A` references a project symbol that `B`
//! defines (and `A` does not define locally). Names that are defined in too
//! many files are treated as too ambiguous to carry a reliable edge.

use crate::symbols::{Import, Symbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

/// A name defined in more files than this is considered too ambiguous to
/// produce a trustworthy dependency edge.
const MAX_DEFINITION_FANOUT: usize = 4;

/// Identifiers shorter than this are ignored when matching references to
/// definitions, to suppress incidental collisions.
const MIN_NAME_LEN: usize = 2;

/// One file's contribution to the graph: its declarations, its imports, and
/// the distinct identifiers it references.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub language: String,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub references: Vec<String>,
    /// Character count of the source, used to estimate token cost.
    pub source_bytes: usize,
}

/// A directed dependency from one file to another, annotated with the evidence
/// that justifies it: shared symbol references and/or resolved import
/// specifiers.
#[derive(Debug, Clone)]
pub struct DependencyEdge {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Symbol names defined in `to` and referenced in `from`.
    pub via: Vec<String>,
    /// Import specifiers in `from` that resolved to `to`.
    pub imports: Vec<String>,
}

impl DependencyEdge {
    /// Edge strength: distinct connecting symbols plus resolved imports.
    pub fn weight(&self) -> usize {
        self.via.len() + self.imports.len()
    }

    /// Whether a resolved import backs this edge (a stronger signal than a
    /// bare name reference).
    pub fn is_import_backed(&self) -> bool {
        !self.imports.is_empty()
    }
}

/// The whole-project graph: every contributing file and the inferred edges.
#[derive(Debug, Clone)]
pub struct ProjectGraph {
    pub files: Vec<FileNode>,
    pub edges: Vec<DependencyEdge>,
}

impl ProjectGraph {
    /// Edges where `path` is the dependent (files it relies on).
    pub fn dependencies_of<'a>(&'a self, path: &Path) -> Vec<&'a DependencyEdge> {
        self.edges.iter().filter(|e| e.from == path).collect()
    }

    /// Edges where `path` is the dependency (files that rely on it).
    pub fn dependents_of<'a>(&'a self, path: &Path) -> Vec<&'a DependencyEdge> {
        self.edges.iter().filter(|e| e.to == path).collect()
    }
}

/// Kinds that can be meaningfully referenced by name from another file.
/// Methods (accessed via a receiver) and structural-only kinds are excluded to
/// avoid spurious edges.
fn is_indexable(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Union
            | SymbolKind::Trait
            | SymbolKind::Interface
            | SymbolKind::Class
            | SymbolKind::Constant
            | SymbolKind::TypeAlias
            | SymbolKind::Macro
    )
}

/// The evidence backing one directed edge while it is being assembled.
#[derive(Default)]
struct Evidence {
    symbols: BTreeSet<String>,
    imports: BTreeSet<String>,
}

/// Build the dependency graph from already-extracted file nodes. This is a
/// pure function (no IO), which keeps edge inference deterministic and unit
/// testable independent of the filesystem.
pub fn build_graph_from_files(files: Vec<FileNode>) -> ProjectGraph {
    // Map each indexable definition name to the files that define it.
    let mut definitions: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, file) in files.iter().enumerate() {
        let mut seen = BTreeSet::new();
        for symbol in &file.symbols {
            if is_indexable(symbol.kind)
                && symbol.name.len() >= MIN_NAME_LEN
                && seen.insert(symbol.name.as_str())
            {
                definitions.entry(symbol.name.as_str()).or_default().push(i);
            }
        }
    }

    // All names defined locally in each file, used to prefer local resolution.
    let local_defs: Vec<BTreeSet<&str>> = files
        .iter()
        .map(|file| file.symbols.iter().map(|s| s.name.as_str()).collect())
        .collect();

    // Index every file by its normalized relative path, for import resolution.
    let path_index: BTreeMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, file)| (norm_path(&file.path), i))
        .collect();

    let mut edges: BTreeMap<(usize, usize), Evidence> = BTreeMap::new();

    // Reference evidence: a file uses a name another file defines.
    for (i, file) in files.iter().enumerate() {
        for name in &file.references {
            if name.len() < MIN_NAME_LEN || local_defs[i].contains(name.as_str()) {
                continue;
            }
            let Some(def_files) = definitions.get(name.as_str()) else {
                continue;
            };
            if def_files.len() > MAX_DEFINITION_FANOUT {
                continue;
            }
            for &j in def_files {
                if j != i {
                    edges
                        .entry((i, j))
                        .or_default()
                        .symbols
                        .insert(name.clone());
                }
            }
        }
    }

    // Import evidence: a specifier resolves to a concrete project file.
    for (i, file) in files.iter().enumerate() {
        let from_dir = parent_dir(&norm_path(&file.path)).to_string();
        for import in &file.imports {
            for j in resolve_import(&file.language, import, &from_dir, &path_index) {
                if j != i {
                    edges
                        .entry((i, j))
                        .or_default()
                        .imports
                        .insert(import.module.clone());
                }
            }
        }
    }

    let mut edges: Vec<DependencyEdge> = edges
        .into_iter()
        .map(|((i, j), evidence)| DependencyEdge {
            from: files[i].path.clone(),
            to: files[j].path.clone(),
            via: evidence.symbols.into_iter().collect(),
            imports: evidence.imports.into_iter().collect(),
        })
        .collect();
    edges.sort_by(|a, b| {
        b.weight()
            .cmp(&a.weight())
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });

    ProjectGraph { files, edges }
}

// ---------------------------------------------------------------------------
// Import specifier resolution
// ---------------------------------------------------------------------------

/// Normalize a relative path to forward-slash-joined components.
fn norm_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The directory portion of a normalized path (everything before the last
/// `/`), or the empty string for a top-level file.
fn parent_dir(normalized: &str) -> &str {
    match normalized.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    }
}

/// Resolve a `spec` (like `./service` or `../a/b`) relative to `base_dir`,
/// collapsing `.` and `..` segments. Returns the joined, normalized path.
fn join_relative(base_dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for segment in spec.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

const TS_EXTENSIONS: &[&str] = &["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs"];

/// Dispatch import resolution by language, returning the file indices the
/// import points at (empty for external packages or unresolved specifiers).
fn resolve_import(
    language: &str,
    import: &Import,
    from_dir: &str,
    index: &BTreeMap<String, usize>,
) -> Vec<usize> {
    match language {
        "TypeScript/JavaScript" => resolve_ts(&import.module, from_dir, index)
            .into_iter()
            .collect(),
        "Python" => resolve_python(&import.module, &import.names, from_dir, index),
        _ => Vec::new(),
    }
}

/// Resolve a relative ES-module specifier to a project file, trying the usual
/// TypeScript/JavaScript extension and `index.*` conventions.
fn resolve_ts(spec: &str, from_dir: &str, index: &BTreeMap<String, usize>) -> Option<usize> {
    if !spec.starts_with('.') {
        // Bare (`react`) and alias (`@/…`) specifiers need package/tsconfig
        // resolution we do not attempt; they are almost never project files.
        return None;
    }
    let base = join_relative(from_dir, spec);
    if let Some(&i) = index.get(&base) {
        return Some(i);
    }
    for ext in TS_EXTENSIONS {
        if let Some(&i) = index.get(&format!("{base}.{ext}")) {
            return Some(i);
        }
        if let Some(&i) = index.get(&format!("{base}/index.{ext}")) {
            return Some(i);
        }
    }
    None
}

/// Resolve a Python module specifier (absolute from the project root or
/// relative via leading dots) to project files: the module file itself and any
/// imported names that are themselves submodules.
fn resolve_python(
    module: &str,
    names: &[String],
    from_dir: &str,
    index: &BTreeMap<String, usize>,
) -> Vec<usize> {
    let dots = module.chars().take_while(|&c| c == '.').count();
    let modpath = &module[dots..];

    let mut base: Vec<String> = if dots == 0 {
        // Absolute: resolve from the project root.
        Vec::new()
    } else {
        let mut parts: Vec<String> = if from_dir.is_empty() {
            Vec::new()
        } else {
            from_dir.split('/').map(str::to_string).collect()
        };
        // Each dot beyond the first ascends one package level.
        for _ in 0..dots.saturating_sub(1) {
            parts.pop();
        }
        parts
    };
    for segment in modpath.split('.').filter(|s| !s.is_empty()) {
        base.push(segment.to_string());
    }

    let mut out = Vec::new();
    py_candidates(&base, index, &mut out);
    // `from pkg import thing` may pull in a submodule `thing`.
    for name in names {
        let mut sub = base.clone();
        sub.push(name.clone());
        py_candidates(&sub, index, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Push the indices of `parts.py` and `parts/__init__.py` if they exist.
fn py_candidates(parts: &[String], index: &BTreeMap<String, usize>, out: &mut Vec<usize>) {
    if parts.is_empty() {
        return;
    }
    let joined = parts.join("/");
    if let Some(&i) = index.get(&format!("{joined}.py")) {
        out.push(i);
    }
    if let Some(&i) = index.get(&format!("{joined}/__init__.py")) {
        out.push(i);
    }
}

/// Walk a project, extract per-file structure, and build its dependency graph.
pub fn build_project_graph(path: impl AsRef<Path>) -> io::Result<ProjectGraph> {
    let (project_root, files) = crate::inspect::walk_project(path)?;

    let mut nodes = Vec::new();
    for file_path in files {
        if std::fs::metadata(&file_path)?.len() > crate::inspect::SYMBOL_FILE_SIZE_CAP {
            continue;
        }
        let Some(extractor) = crate::symbols::extractor_for_path(&file_path) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&file_path) else {
            continue;
        };

        let symbols = extractor.extract(&source);
        let imports = extractor.imports(&source);
        if symbols.is_empty() && imports.is_empty() {
            continue;
        }
        let references = extractor.referenced_identifiers(&source);
        let source_bytes = source.chars().count();
        let relative = file_path
            .strip_prefix(&project_root)
            .unwrap_or(&file_path)
            .to_path_buf();

        nodes.push(FileNode {
            path: relative,
            language: extractor.language().to_string(),
            symbols,
            imports,
            references,
            source_bytes,
        });
    }

    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(build_graph_from_files(nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind,
            line: 1,
            container: None,
            exported: true,
            signature: String::new(),
        }
    }

    fn node(path: &str, defs: Vec<Symbol>, refs: &[&str]) -> FileNode {
        FileNode {
            path: PathBuf::from(path),
            language: "Rust".to_string(),
            symbols: defs,
            imports: Vec::new(),
            references: refs.iter().map(|s| s.to_string()).collect(),
            source_bytes: 0,
        }
    }

    fn imp(module: &str, names: &[&str]) -> Import {
        Import {
            module: module.to_string(),
            names: names.iter().map(|s| s.to_string()).collect(),
            line: 1,
        }
    }

    fn lang_node(path: &str, language: &str, defs: Vec<Symbol>, imports: Vec<Import>) -> FileNode {
        FileNode {
            path: PathBuf::from(path),
            language: language.to_string(),
            symbols: defs,
            imports,
            references: Vec::new(),
            source_bytes: 0,
        }
    }

    #[test]
    fn reference_produces_dependency_edge() {
        let files = vec![
            node(
                "lib.rs",
                vec![sym("Widget", SymbolKind::Struct)],
                &["Widget"],
            ),
            node(
                "app.rs",
                vec![sym("run", SymbolKind::Function)],
                &["Widget", "println"],
            ),
        ];
        let graph = build_graph_from_files(files);
        let deps = graph.dependencies_of(Path::new("app.rs"));
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, PathBuf::from("lib.rs"));
        assert_eq!(deps[0].via, vec!["Widget"]);
        assert_eq!(graph.dependents_of(Path::new("lib.rs")).len(), 1);
    }

    #[test]
    fn locally_defined_names_do_not_create_edges() {
        // Both files define `Config`; `b` referencing `Config` resolves locally.
        let files = vec![
            node("a.rs", vec![sym("Config", SymbolKind::Struct)], &[]),
            node("b.rs", vec![sym("Config", SymbolKind::Struct)], &["Config"]),
        ];
        let graph = build_graph_from_files(files);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn ambiguous_names_are_skipped() {
        // `Common` is defined in five files: too ambiguous to link.
        let mut files: Vec<FileNode> = (0..5)
            .map(|i| {
                node(
                    &format!("def{i}.rs"),
                    vec![sym("Common", SymbolKind::Struct)],
                    &[],
                )
            })
            .collect();
        files.push(node(
            "user.rs",
            vec![sym("go", SymbolKind::Function)],
            &["Common"],
        ));
        let graph = build_graph_from_files(files);
        assert!(graph.dependencies_of(Path::new("user.rs")).is_empty());
    }

    #[test]
    fn methods_are_not_indexed_as_targets() {
        let files = vec![
            node("svc.rs", vec![sym("connect", SymbolKind::Method)], &[]),
            node(
                "call.rs",
                vec![sym("main", SymbolKind::Function)],
                &["connect"],
            ),
        ];
        let graph = build_graph_from_files(files);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn ts_relative_import_resolves_to_file() {
        let ts = "TypeScript/JavaScript";
        let files = vec![
            lang_node(
                "src/app.ts",
                ts,
                vec![],
                vec![imp("./service", &["UserService"])],
            ),
            lang_node(
                "src/service.ts",
                ts,
                vec![sym("UserService", SymbolKind::Class)],
                vec![],
            ),
        ];
        let graph = build_graph_from_files(files);
        let deps = graph.dependencies_of(Path::new("src/app.ts"));
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, PathBuf::from("src/service.ts"));
        assert!(deps[0].is_import_backed());
        assert_eq!(deps[0].imports, vec!["./service"]);
    }

    #[test]
    fn ts_index_file_and_parent_import_resolve() {
        let ts = "TypeScript/JavaScript";
        let files = vec![
            lang_node(
                "a/b/main.ts",
                ts,
                vec![],
                vec![imp("./widgets", &[]), imp("../shared", &[])],
            ),
            lang_node("a/b/widgets/index.ts", ts, vec![], vec![imp("./noop", &[])]),
            lang_node("a/shared.tsx", ts, vec![], vec![imp("./noop", &[])]),
        ];
        let graph = build_graph_from_files(files);
        let targets: Vec<_> = graph
            .dependencies_of(Path::new("a/b/main.ts"))
            .iter()
            .map(|e| e.to.clone())
            .collect();
        assert!(targets.contains(&PathBuf::from("a/b/widgets/index.ts")));
        assert!(targets.contains(&PathBuf::from("a/shared.tsx")));
    }

    #[test]
    fn python_relative_import_resolves() {
        let py = "Python";
        let files = vec![
            lang_node("pkg/main.py", py, vec![], vec![imp(".util", &["helper"])]),
            lang_node(
                "pkg/util.py",
                py,
                vec![sym("helper", SymbolKind::Function)],
                vec![],
            ),
        ];
        let graph = build_graph_from_files(files);
        let deps = graph.dependencies_of(Path::new("pkg/main.py"));
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, PathBuf::from("pkg/util.py"));
        assert!(deps[0].is_import_backed());
    }

    #[test]
    fn python_from_package_imports_submodule() {
        let py = "Python";
        let files = vec![
            lang_node("pkg/__init__.py", py, vec![], vec![imp(".", &["sub"])]),
            lang_node("pkg/sub.py", py, vec![], vec![imp(".", &["x"])]),
        ];
        let graph = build_graph_from_files(files);
        let deps = graph.dependencies_of(Path::new("pkg/__init__.py"));
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, PathBuf::from("pkg/sub.py"));
    }

    #[test]
    fn external_imports_do_not_resolve() {
        let ts = "TypeScript/JavaScript";
        let files = vec![
            lang_node(
                "app.ts",
                ts,
                vec![],
                vec![imp("react", &["useState"]), imp("@/aliased", &[])],
            ),
            lang_node(
                "other.ts",
                ts,
                vec![sym("thing", SymbolKind::Function)],
                vec![],
            ),
        ];
        let graph = build_graph_from_files(files);
        assert!(graph.edges.is_empty());
    }
}
