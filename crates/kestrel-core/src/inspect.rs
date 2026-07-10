use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProjectInspection {
    pub requested_path: PathBuf,
    pub project_root: PathBuf,
    pub git_root: Option<PathBuf>,
    pub inventory: FileInventory,
    pub languages: Vec<LanguageSummary>,
    pub markers: Vec<ProjectMarker>,
    pub commands: Vec<CommandSuggestion>,
    pub symbols: SymbolSummary,
}

/// An aggregate view of the structural symbols discovered across the project.
/// This is the repository-level output of the Ghost Context Engine seed.
#[derive(Debug, Clone, Default)]
pub struct SymbolSummary {
    pub total_symbols: usize,
    pub files_with_symbols: usize,
    /// Symbol counts by kind, sorted by count descending then name.
    pub kind_counts: Vec<(String, usize)>,
    /// The most symbol-dense files, sorted by count descending (capped).
    pub top_files: Vec<(PathBuf, usize)>,
}

#[derive(Debug, Clone)]
pub struct FileInventory {
    pub total_files: usize,
    pub total_bytes: u64,
    pub largest_files: Vec<FileEntry>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct LanguageSummary {
    pub language: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ProjectMarker {
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CommandSuggestion {
    pub kind: CommandKind,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum CommandKind {
    Install,
    Run,
    Test,
    Lint,
    Build,
    Format,
}

pub fn inspect_project(path: impl AsRef<Path>) -> io::Result<ProjectInspection> {
    let requested_path = path.as_ref().canonicalize()?;
    let project_root = find_git_root(&requested_path).unwrap_or_else(|| requested_path.clone());
    let git_root = find_git_root(&requested_path);

    let mut language_stats: BTreeMap<&'static str, (usize, u64)> = BTreeMap::new();
    let mut largest_files = Vec::new();
    let mut total_files = 0usize;
    let mut total_bytes = 0u64;
    let mut seen_marker_names = BTreeSet::new();
    let mut markers = Vec::new();

    let mut symbol_kind_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut per_file_symbol_counts: Vec<(PathBuf, usize)> = Vec::new();
    let mut total_symbols = 0usize;
    let mut files_with_symbols = 0usize;

    let ignore_rules = IgnoreRules::load(&project_root);
    let files = collect_files(&project_root, &ignore_rules)?;

    for path in files {
        let metadata = fs::metadata(&path)?;
        let bytes = metadata.len();
        let relative_path = path
            .strip_prefix(&project_root)
            .unwrap_or(&path)
            .to_path_buf();

        total_files += 1;
        total_bytes += bytes;

        if let Some(language) = language_for_path(&path) {
            let stat = language_stats.entry(language).or_default();
            stat.0 += 1;
            stat.1 += bytes;
        }

        if let Some(marker_kind) = marker_for_path(&relative_path) {
            if seen_marker_names.insert(relative_path.clone()) {
                markers.push(ProjectMarker {
                    kind: marker_kind.to_string(),
                    path: relative_path.clone(),
                });
            }
        }

        if bytes <= SYMBOL_FILE_SIZE_CAP {
            if let Some(extractor) = crate::symbols::extractor_for_path(&path) {
                if let Ok(source) = fs::read_to_string(&path) {
                    let extracted = extractor.extract(&source);
                    if !extracted.is_empty() {
                        files_with_symbols += 1;
                        total_symbols += extracted.len();
                        per_file_symbol_counts.push((relative_path.clone(), extracted.len()));
                        for symbol in &extracted {
                            *symbol_kind_counts.entry(symbol.kind.as_str()).or_default() += 1;
                        }
                    }
                }
            }
        }

        largest_files.push(FileEntry {
            path: relative_path,
            bytes,
        });
    }

    largest_files.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    largest_files.truncate(10);

    let languages = language_stats
        .into_iter()
        .map(|(language, (files, bytes))| LanguageSummary {
            language: language.to_string(),
            files,
            bytes,
        })
        .collect();

    let commands = suggest_commands(&markers);

    let mut kind_counts: Vec<(String, usize)> = symbol_kind_counts
        .into_iter()
        .map(|(kind, count)| (kind.to_string(), count))
        .collect();
    kind_counts.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), name.clone()));

    per_file_symbol_counts.sort_by_key(|(path, count)| (std::cmp::Reverse(*count), path.clone()));
    per_file_symbol_counts.truncate(10);

    let symbols = SymbolSummary {
        total_symbols,
        files_with_symbols,
        kind_counts,
        top_files: per_file_symbol_counts,
    };

    Ok(ProjectInspection {
        requested_path,
        project_root,
        git_root,
        inventory: FileInventory {
            total_files,
            total_bytes,
            largest_files,
        },
        languages,
        markers,
        commands,
        symbols,
    })
}

/// Files larger than this are skipped by symbol extraction to keep `inspect`
/// responsive on repositories containing large generated or vendored files.
pub(crate) const SYMBOL_FILE_SIZE_CAP: u64 = 1_500_000;

/// Resolve a project root (Git root if present, else the given path) and
/// collect every non-ignored file under it. Shared by the inspection, symbol,
/// and graph entry points so they all honor the same ignore rules.
pub(crate) fn walk_project(path: impl AsRef<Path>) -> io::Result<(PathBuf, Vec<PathBuf>)> {
    let requested = path.as_ref().canonicalize()?;
    let project_root = find_git_root(&requested).unwrap_or_else(|| requested.clone());
    let ignore_rules = IgnoreRules::load(&project_root);
    let files = collect_files(&project_root, &ignore_rules)?;
    Ok((project_root, files))
}

/// Walk a project (honoring ignore rules) and return the extracted symbols for
/// every supported source file that contains at least one symbol. Results are
/// sorted by relative path. This is the per-file view behind `kestrel symbols`.
pub fn project_symbols(
    path: impl AsRef<Path>,
) -> io::Result<Vec<(PathBuf, crate::symbols::FileSymbols)>> {
    let (project_root, files) = walk_project(path)?;

    let mut result = Vec::new();
    for path in files {
        if fs::metadata(&path)?.len() > SYMBOL_FILE_SIZE_CAP {
            continue;
        }
        if let Some(file_symbols) = crate::symbols::symbols_for_file(&path)? {
            if !file_symbols.symbols.is_empty() {
                let relative = path
                    .strip_prefix(&project_root)
                    .unwrap_or(&path)
                    .to_path_buf();
                result.push((relative, file_symbols));
            }
        }
    }

    result.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(result)
}

fn collect_files(root: &Path, ignore_rules: &IgnoreRules) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_inner(root, root, ignore_rules, &mut files)?;
    Ok(files)
}

fn collect_files_inner(
    root: &Path,
    current: &Path,
    ignore_rules: &IgnoreRules,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let relative_path = path.strip_prefix(root).unwrap_or(&path);

        if ignore_rules.should_ignore(relative_path) {
            continue;
        }

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_inner(root, &path, ignore_rules, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
struct IgnoreRules {
    exact: BTreeSet<String>,
    prefixes: BTreeSet<String>,
    suffixes: BTreeSet<String>,
}

impl IgnoreRules {
    fn load(root: &Path) -> Self {
        let mut rules = Self::default();

        for default in [
            ".git/",
            "target/",
            "node_modules/",
            ".next/",
            "dist/",
            "build/",
            ".venv/",
            "__pycache__/",
        ] {
            rules.add(default);
        }

        if let Ok(contents) = fs::read_to_string(root.join(".gitignore")) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                    continue;
                }
                rules.add(trimmed);
            }
        }

        rules
    }

    fn add(&mut self, pattern: &str) {
        let normalized = pattern.trim_start_matches('/').replace('\\', "/");

        if normalized.ends_with('/') {
            self.prefixes
                .insert(normalized.trim_end_matches('/').to_string());
        } else if let Some(suffix) = normalized.strip_prefix("*.") {
            self.suffixes.insert(format!(".{suffix}"));
        } else {
            self.exact.insert(normalized);
        }
    }

    fn should_ignore(&self, relative_path: &Path) -> bool {
        let normalized = relative_path.to_string_lossy().replace('\\', "/");

        self.exact.contains(&normalized)
            || self.prefixes.iter().any(|prefix| {
                normalized == *prefix || normalized.starts_with(&format!("{prefix}/"))
            })
            || self
                .suffixes
                .iter()
                .any(|suffix| normalized.ends_with(suffix))
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if current.join(".git").exists() {
            return Some(current);
        }

        if !current.pop() {
            return None;
        }
    }
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("c") | Some("h") => Some("C"),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => Some("C++"),
        Some("cs") => Some("C#"),
        Some("css") => Some("CSS"),
        Some("go") => Some("Go"),
        Some("html") | Some("htm") => Some("HTML"),
        Some("java") => Some("Java"),
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => Some("JavaScript"),
        Some("json") => Some("JSON"),
        Some("md") | Some("mdx") => Some("Markdown"),
        Some("php") => Some("PHP"),
        Some("py") | Some("pyw") => Some("Python"),
        Some("rb") => Some("Ruby"),
        Some("rs") => Some("Rust"),
        Some("toml") => Some("TOML"),
        Some("ts") | Some("tsx") => Some("TypeScript"),
        Some("yaml") | Some("yml") => Some("YAML"),
        _ => None,
    }
}

fn marker_for_path(relative_path: &Path) -> Option<&'static str> {
    let normalized = relative_path.to_string_lossy().replace('\\', "/");
    match normalized.as_str() {
        "Cargo.toml" => Some("rust_cargo"),
        "package.json" => Some("node_package"),
        "pnpm-lock.yaml" => Some("pnpm_lock"),
        "yarn.lock" => Some("yarn_lock"),
        "package-lock.json" => Some("npm_lock"),
        "pyproject.toml" => Some("python_project"),
        "requirements.txt" => Some("python_requirements"),
        "uv.lock" => Some("uv_lock"),
        "go.mod" => Some("go_module"),
        "global.json" => Some("dotnet_global"),
        path if path.ends_with(".sln") => Some("dotnet_solution"),
        path if path.ends_with(".csproj") => Some("dotnet_project"),
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
            Some("docker_compose")
        }
        "Dockerfile" => Some("dockerfile"),
        _ => None,
    }
}

fn suggest_commands(markers: &[ProjectMarker]) -> Vec<CommandSuggestion> {
    let kinds: BTreeSet<_> = markers.iter().map(|marker| marker.kind.as_str()).collect();
    let mut commands = Vec::new();

    if kinds.contains("rust_cargo") {
        commands.extend([
            command(CommandKind::Format, "cargo fmt", "Cargo workspace detected"),
            command(CommandKind::Test, "cargo test", "Cargo workspace detected"),
            command(
                CommandKind::Build,
                "cargo build",
                "Cargo workspace detected",
            ),
        ]);
    }

    if kinds.contains("node_package") {
        let runner = if kinds.contains("pnpm_lock") {
            "pnpm"
        } else if kinds.contains("yarn_lock") {
            "yarn"
        } else {
            "npm"
        };
        commands.push(command(
            CommandKind::Install,
            format!("{runner} install"),
            "Node package manifest detected",
        ));
        commands.push(command(
            CommandKind::Test,
            format!("{runner} test"),
            "Node package manifest detected",
        ));
        commands.push(command(
            CommandKind::Build,
            format!("{runner} run build"),
            "Node package manifest detected",
        ));
    }

    if kinds.contains("python_project") || kinds.contains("python_requirements") {
        commands.push(command(
            CommandKind::Test,
            "python -m pytest",
            "Python project markers detected",
        ));
    }

    if kinds.contains("go_module") {
        commands.push(command(
            CommandKind::Test,
            "go test ./...",
            "Go module detected",
        ));
        commands.push(command(
            CommandKind::Build,
            "go build ./...",
            "Go module detected",
        ));
    }

    if kinds.contains("dotnet_solution") || kinds.contains("dotnet_project") {
        commands.push(command(
            CommandKind::Build,
            "dotnet build",
            ".NET project markers detected",
        ));
        commands.push(command(
            CommandKind::Test,
            "dotnet test",
            ".NET project markers detected",
        ));
    }

    commands
}

fn command(
    kind: CommandKind,
    command: impl Into<String>,
    reason: impl Into<String>,
) -> CommandSuggestion {
    CommandSuggestion {
        kind,
        command: command.into(),
        reason: reason.into(),
    }
}
