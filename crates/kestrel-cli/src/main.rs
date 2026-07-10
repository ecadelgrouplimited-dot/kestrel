use kestrel_core::{
    build_project_graph, inspect_project, project_symbols, ProjectGraph, ProjectInspection,
};
use std::env;
use std::io;
use std::path::{Path, PathBuf};

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };

    match command.as_str() {
        "inspect" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(env::current_dir()?);
            let inspection = inspect_project(path)?;
            print_summary(&inspection);
        }
        "symbols" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(env::current_dir()?);
            print_symbols(path)?;
        }
        "graph" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(env::current_dir()?);
            print_graph(path)?;
        }
        "related" => {
            let Some(target) = args.next().map(PathBuf::from) else {
                eprintln!("Usage: kestrel related <file>");
                std::process::exit(2);
            };
            print_related(target)?;
        }
        "context" => {
            let rest: Vec<String> = args.collect();
            let (target, budget, format) = parse_context_args(&rest);
            let Some(target) = target else {
                eprintln!("Usage: kestrel context <file> [--budget N] [--format summary|prompt]");
                std::process::exit(2);
            };
            print_context(target, budget, format)?;
        }
        "--help" | "-h" | "help" => print_usage(),
        unknown => {
            eprintln!("Unknown command: {unknown}");
            print_usage();
            std::process::exit(2);
        }
    }

    Ok(())
}

fn print_usage() {
    println!("Kestrel CLI");
    println!();
    println!("Usage:");
    println!("  kestrel inspect [path]    Summarize a repository");
    println!("  kestrel symbols [path]    List structural symbols per file");
    println!("  kestrel graph [path]      Show the file dependency graph");
    println!("  kestrel related <file>    Show a file's dependencies and dependents");
    println!("  kestrel context <file> [--budget N] [--format summary|prompt]");
    println!("                            Build a ranked, budget-bounded context pack");
}

fn print_summary(inspection: &ProjectInspection) {
    println!("Kestrel Project Inspection");
    println!("Requested path: {}", inspection.requested_path.display());
    println!("Project root: {}", inspection.project_root.display());
    println!(
        "Git root: {}",
        inspection
            .git_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not detected".to_string())
    );
    println!();

    println!("Inventory");
    println!("  Files: {}", inspection.inventory.total_files);
    println!("  Bytes: {}", inspection.inventory.total_bytes);
    println!();

    println!("Languages");
    if inspection.languages.is_empty() {
        println!("  No known language files detected");
    } else {
        for language in &inspection.languages {
            println!(
                "  {}: {} files, {} bytes",
                language.language, language.files, language.bytes
            );
        }
    }
    println!();

    println!("Symbols");
    let symbols = &inspection.symbols;
    if symbols.total_symbols == 0 {
        println!("  No structural symbols extracted yet");
    } else {
        println!(
            "  {} symbols across {} files",
            symbols.total_symbols, symbols.files_with_symbols
        );
        let by_kind = symbols
            .kind_counts
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  By kind: {by_kind}");
        if !symbols.top_files.is_empty() {
            println!("  Most symbol-dense files:");
            for (path, count) in &symbols.top_files {
                println!("    {count} symbols: {}", path.display());
            }
        }
    }
    println!();

    println!("Project markers");
    if inspection.markers.is_empty() {
        println!("  None detected");
    } else {
        for marker in &inspection.markers {
            println!("  {}: {}", marker.kind, marker.path.display());
        }
    }
    println!();

    println!("Likely commands");
    if inspection.commands.is_empty() {
        println!("  No commands inferred yet");
    } else {
        for command in &inspection.commands {
            println!(
                "  {:?}: {} ({})",
                command.kind, command.command, command.reason
            );
        }
    }
    println!();

    println!("Largest files");
    for file in &inspection.inventory.largest_files {
        println!("  {} bytes: {}", file.bytes, file.path.display());
    }
}

fn print_symbols(path: PathBuf) -> io::Result<()> {
    // Accept a single source file as well as a directory.
    if path.is_file() {
        return match kestrel_core::symbols_for_file(&path)? {
            Some(file) if !file.symbols.is_empty() => {
                let relative = path
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| path.clone());
                print_file_symbols(&relative, &file);
                Ok(())
            }
            _ => {
                println!("No structural symbols found in {}.", path.display());
                Ok(())
            }
        };
    }

    let files = project_symbols(path)?;
    if files.is_empty() {
        println!("No structural symbols found in supported source files.");
        return Ok(());
    }

    let total: usize = files.iter().map(|(_, file)| file.symbols.len()).sum();
    println!(
        "Kestrel Symbols — {total} symbols across {} files",
        files.len()
    );
    println!();

    for (path, file) in &files {
        print_file_symbols(path, file);
    }

    Ok(())
}

fn print_file_symbols(path: &Path, file: &kestrel_core::FileSymbols) {
    println!("{} [{}]", path.display(), file.language);
    for symbol in &file.symbols {
        let visibility = if symbol.exported { "+" } else { "-" };
        let container = symbol
            .container
            .as_deref()
            .map(|c| format!(" (in {c})"))
            .unwrap_or_default();
        println!(
            "  {visibility} {:<9} {}{}  @{}",
            symbol.kind.as_str(),
            symbol.name,
            container,
            symbol.line
        );
    }
    println!();
}

/// Format up to three items, noting any remainder.
fn format_list(items: &[String]) -> String {
    const SHOWN: usize = 3;
    if items.len() <= SHOWN {
        items.join(", ")
    } else {
        format!(
            "{}, +{} more",
            items[..SHOWN].join(", "),
            items.len() - SHOWN
        )
    }
}

/// Describe the evidence behind an edge: resolved imports and/or shared symbols.
fn edge_evidence(edge: &kestrel_core::DependencyEdge) -> String {
    let mut parts = Vec::new();
    if !edge.imports.is_empty() {
        parts.push(format!("imports {}", format_list(&edge.imports)));
    }
    if !edge.via.is_empty() {
        parts.push(format!("via {}", format_list(&edge.via)));
    }
    parts.join("; ")
}

fn print_graph(path: PathBuf) -> io::Result<()> {
    let graph = build_project_graph(path)?;
    println!(
        "Kestrel Dependency Graph — {} files, {} edges",
        graph.files.len(),
        graph.edges.len()
    );
    println!();

    if graph.edges.is_empty() {
        println!("No cross-file dependencies inferred.");
        return Ok(());
    }

    const MAX_EDGES: usize = 60;
    for edge in graph.edges.iter().take(MAX_EDGES) {
        println!(
            "  {}  ->  {}   [{}] {}",
            edge.from.display(),
            edge.to.display(),
            edge.weight(),
            edge_evidence(edge)
        );
    }
    if graph.edges.len() > MAX_EDGES {
        println!("  … {} more edges", graph.edges.len() - MAX_EDGES);
    }

    Ok(())
}

fn print_related(target: PathBuf) -> io::Result<()> {
    // Build the graph from the target's directory so its whole repository (or
    // at least its folder) is indexed, then locate the target within it.
    let search_root = if target.is_dir() {
        target.clone()
    } else {
        target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let graph = build_project_graph(&search_root)?;
    let Some(node) = find_node(&graph, &target) else {
        println!(
            "No indexed file matched `{}`. It may be unsupported, empty, or ignored.",
            target.display()
        );
        return Ok(());
    };

    let node_path = node.path.clone();
    println!("Related files for {}", node_path.display());
    println!(
        "  ({} symbols, {} imports)",
        node.symbols.len(),
        node.imports.len()
    );
    println!();

    let dependencies = graph.dependencies_of(&node_path);
    println!("Depends on ({}):", dependencies.len());
    if dependencies.is_empty() {
        println!("  (none inferred)");
    } else {
        for edge in dependencies {
            println!(
                "  [{}] {}   {}",
                edge.weight(),
                edge.to.display(),
                edge_evidence(edge)
            );
        }
    }
    println!();

    let dependents = graph.dependents_of(&node_path);
    println!("Depended on by ({}):", dependents.len());
    if dependents.is_empty() {
        println!("  (none inferred)");
    } else {
        for edge in dependents {
            println!(
                "  [{}] {}   {}",
                edge.weight(),
                edge.from.display(),
                edge_evidence(edge)
            );
        }
    }

    Ok(())
}

/// Default token budget for a context pack when none is supplied.
const DEFAULT_CONTEXT_BUDGET: usize = 8_000;

/// How a context pack is rendered.
#[derive(Clone, Copy, PartialEq)]
enum ContextFormat {
    /// Human-readable ranked summary (default).
    Summary,
    /// Assembled prompt text with file contents, ready to feed a model.
    Prompt,
}

/// Parse `context` arguments: the target file, an optional
/// `--budget N` / `--budget=N` / `-b N`, and `--format summary|prompt`
/// (`--prompt` as a shorthand).
fn parse_context_args(args: &[String]) -> (Option<PathBuf>, usize, ContextFormat) {
    let mut target = None;
    let mut budget = DEFAULT_CONTEXT_BUDGET;
    let mut format = ContextFormat::Summary;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--budget" || arg == "-b" {
            if let Some(value) = args.get(i + 1).and_then(|v| v.parse::<usize>().ok()) {
                budget = value;
            }
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--budget=") {
            if let Ok(value) = value.parse::<usize>() {
                budget = value;
            }
            i += 1;
            continue;
        }
        if arg == "--prompt" {
            format = ContextFormat::Prompt;
            i += 1;
            continue;
        }
        if arg == "--format" || arg == "--format=summary" || arg == "--format=prompt" {
            let value = arg
                .strip_prefix("--format=")
                .map(str::to_string)
                .or_else(|| args.get(i + 1).cloned());
            if value.as_deref() == Some("prompt") {
                format = ContextFormat::Prompt;
            } else {
                format = ContextFormat::Summary;
            }
            i += if arg == "--format" { 2 } else { 1 };
            continue;
        }
        if target.is_none() {
            target = Some(PathBuf::from(arg));
        }
        i += 1;
    }
    (target, budget.max(1), format)
}

/// Map a language label to a Markdown code-fence language tag.
fn fence_tag(language: &str) -> &str {
    match language {
        "Rust" => "rust",
        "TypeScript/JavaScript" => "typescript",
        "Python" => "python",
        _ => "",
    }
}

fn print_context(target: PathBuf, budget: usize, format: ContextFormat) -> io::Result<()> {
    let search_root = if target.is_dir() {
        target.clone()
    } else {
        target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let graph = build_project_graph(&search_root)?;
    let Some(node) = find_node(&graph, &target) else {
        println!(
            "No indexed file matched `{}`. It may be unsupported, empty, or ignored.",
            target.display()
        );
        return Ok(());
    };

    let seed = node.path.clone();
    let Some(pack) = kestrel_core::build_context_pack(&graph, &seed, budget) else {
        println!("Could not build a context pack for {}.", seed.display());
        return Ok(());
    };

    if format == ContextFormat::Prompt {
        return print_context_prompt(&graph, &pack);
    }

    println!("Kestrel Context Pack — seed: {}", pack.seed.display());
    println!(
        "Budget: {} / {} tokens used across {} files ({} omitted)",
        pack.used_tokens,
        pack.budget_tokens,
        pack.entries.len(),
        pack.omitted.len()
    );
    println!();

    const MAX_SYMBOLS: usize = 8;
    for entry in &pack.entries {
        println!(
            "{}  [{}]  ~{} tok   {}",
            entry.path.display(),
            entry.language,
            entry.estimated_tokens,
            entry.reason
        );
        for symbol in entry.symbols.iter().take(MAX_SYMBOLS) {
            let visibility = if symbol.exported { "+" } else { "-" };
            println!("    {visibility} {} {}", symbol.kind.as_str(), symbol.name);
        }
        if entry.symbols.len() > MAX_SYMBOLS {
            println!("    … {} more symbols", entry.symbols.len() - MAX_SYMBOLS);
        }
        println!();
    }

    if !pack.omitted.is_empty() {
        println!("Omitted (relevant, over budget):");
        for entry in &pack.omitted {
            println!(
                "  {}  ~{} tok   {}",
                entry.path.display(),
                entry.estimated_tokens,
                entry.reason
            );
        }
    }

    Ok(())
}

/// Render a context pack as assembled prompt text: a header plus each included
/// file's full source in a fenced block, ready to paste or pipe into a model.
fn print_context_prompt(graph: &ProjectGraph, pack: &kestrel_core::ContextPack) -> io::Result<()> {
    println!(
        "# Context for {} ({} files, ~{} tokens)",
        pack.seed.display(),
        pack.entries.len(),
        pack.used_tokens
    );
    println!();
    println!(
        "The following files were selected as the most relevant context for `{}`.",
        pack.seed.display()
    );
    println!();

    for entry in &pack.entries {
        println!("## {} — {}", entry.path.display(), entry.reason);
        let absolute = graph.root.join(&entry.path);
        match std::fs::read_to_string(&absolute) {
            Ok(source) => {
                println!("```{}", fence_tag(&entry.language));
                print!("{}", source);
                if !source.ends_with('\n') {
                    println!();
                }
                println!("```");
            }
            Err(err) => {
                println!("_(could not read file: {err})_");
            }
        }
        println!();
    }

    if !pack.omitted.is_empty() {
        println!("<!-- Omitted for budget:");
        for entry in &pack.omitted {
            println!(
                "  {} (~{} tok)",
                entry.path.display(),
                entry.estimated_tokens
            );
        }
        println!("-->");
    }

    Ok(())
}

/// Find the graph node whose relative path is the longest suffix match of the
/// requested target path (which is typically absolute or repo-relative).
fn find_node<'a>(graph: &'a ProjectGraph, target: &Path) -> Option<&'a kestrel_core::FileNode> {
    let target_components: Vec<_> = target
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();

    let mut best: Option<(&kestrel_core::FileNode, usize)> = None;
    for node in &graph.files {
        let node_components: Vec<_> = node
            .path
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();
        if node_components.len() <= target_components.len()
            && target_components.ends_with(&node_components)
        {
            let score = node_components.len();
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((node, score));
            }
        }
    }
    best.map(|(node, _)| node)
}
