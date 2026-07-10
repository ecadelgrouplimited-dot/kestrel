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
            let opts = parse_context_args(&rest);
            print_context(opts)?;
        }
        "ask" => {
            let rest: Vec<String> = args.collect();
            let Some(opts) = parse_ask_args(&rest) else {
                eprintln!("Usage: kestrel ask \"<question>\" [path] [--budget N] [--model NAME] [--max-tokens N] [--dry-run]");
                std::process::exit(2);
            };
            run_ask(opts)?;
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
    println!("  kestrel context [path] --query \"...\" [--budget N] [--format ...]");
    println!("                            Build a context pack from a natural-language query");
    println!("  kestrel ask \"<question>\" [path] [--model NAME] [--dry-run]");
    println!("                            Answer a question about the codebase using a model");
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

/// Parsed options for the `context` command.
struct ContextOptions {
    target: Option<PathBuf>,
    query: Option<String>,
    budget: usize,
    format: ContextFormat,
}

/// Parse `context` arguments: an optional target path, `--query "..."` (`-q`),
/// `--budget N` (`-b`), and `--format summary|prompt` (`--prompt` shorthand).
fn parse_context_args(args: &[String]) -> ContextOptions {
    let mut target = None;
    let mut query = None;
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
        if arg == "--query" || arg == "-q" {
            query = args.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--query=") {
            query = Some(value.to_string());
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
    ContextOptions {
        target,
        query,
        budget: budget.max(1),
        format,
    }
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

fn print_context(opts: ContextOptions) -> io::Result<()> {
    // Query mode: seed from a natural-language query over the whole project.
    if let Some(query) = opts.query {
        let root = match opts.target {
            Some(path) => path,
            None => env::current_dir()?,
        };
        let graph = build_project_graph(&root)?;
        let pack = kestrel_core::build_context_pack_for_query(&graph, &query, opts.budget);
        if pack.entries.is_empty() {
            println!("No files matched the query \"{query}\".");
            return Ok(());
        }
        return render_pack(&graph, &pack, opts.format);
    }

    // File-seed mode.
    let Some(target) = opts.target else {
        eprintln!("Usage: kestrel context <file> [--budget N] [--format summary|prompt]");
        eprintln!("       kestrel context [path] --query \"...\"");
        std::process::exit(2);
    };
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
    let Some(pack) = kestrel_core::build_context_pack(&graph, &seed, opts.budget) else {
        println!("Could not build a context pack for {}.", seed.display());
        return Ok(());
    };

    render_pack(&graph, &pack, opts.format)
}

/// Render a built pack in the requested format.
fn render_pack(
    graph: &ProjectGraph,
    pack: &kestrel_core::ContextPack,
    format: ContextFormat,
) -> io::Result<()> {
    if format == ContextFormat::Prompt {
        return print_context_prompt(graph, pack);
    }

    println!("Kestrel Context Pack — {}", pack.seed);
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
        pack.seed,
        pack.entries.len(),
        pack.used_tokens
    );
    println!();
    println!(
        "The following files were selected as the most relevant context for {}.",
        pack.seed
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

/// Default token budget for the context assembled behind `ask`.
const DEFAULT_ASK_BUDGET: usize = 12_000;
/// Default model for `ask`. Anthropic's most capable Opus-tier model.
const DEFAULT_ASK_MODEL: &str = "claude-opus-4-8";
/// Default response token cap (kept under the SDK's non-streaming limit).
const DEFAULT_ASK_MAX_TOKENS: u64 = 4_096;

struct AskOptions {
    question: String,
    path: Option<PathBuf>,
    budget: usize,
    model: String,
    max_tokens: u64,
    dry_run: bool,
}

/// Parse `ask` arguments: `<question>` then an optional path, plus
/// `--budget`/`-b`, `--model`, `--max-tokens`, and `--dry-run`.
fn parse_ask_args(args: &[String]) -> Option<AskOptions> {
    let mut question = None;
    let mut path = None;
    let mut budget = DEFAULT_ASK_BUDGET;
    let mut model = DEFAULT_ASK_MODEL.to_string();
    let mut max_tokens = DEFAULT_ASK_MAX_TOKENS;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--budget" | "-b" => {
                if let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                    budget = v;
                }
                i += 2;
            }
            "--model" => {
                if let Some(v) = args.get(i + 1) {
                    model = v.clone();
                }
                i += 2;
            }
            "--max-tokens" => {
                if let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                    max_tokens = v;
                }
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            _ => {
                if question.is_none() {
                    question = Some(arg.clone());
                } else if path.is_none() {
                    path = Some(PathBuf::from(arg));
                }
                i += 1;
            }
        }
    }
    Some(AskOptions {
        question: question?,
        path,
        budget: budget.max(1),
        model,
        max_tokens: max_tokens.max(1),
        dry_run,
    })
}

/// Concatenate the included files' contents into a single context blob.
fn assemble_context(graph: &ProjectGraph, pack: &kestrel_core::ContextPack) -> String {
    let mut out = String::new();
    for entry in &pack.entries {
        match std::fs::read_to_string(graph.root.join(&entry.path)) {
            Ok(source) => {
                out.push_str(&format!(
                    "### {} ({})\n",
                    entry.path.display(),
                    entry.reason
                ));
                out.push_str(&format!("```{}\n{}", fence_tag(&entry.language), source));
                if !source.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n\n");
            }
            Err(_) => continue,
        }
    }
    out
}

/// Answer a natural-language question about the codebase: seed a context pack
/// from the question, assemble an Anthropic Messages request, and call the API
/// via the system `curl` (no bundled TLS stack). `--dry-run` prints the request
/// without sending it.
fn run_ask(opts: AskOptions) -> io::Result<()> {
    let root = match &opts.path {
        Some(p) => p.clone(),
        None => env::current_dir()?,
    };
    let graph = build_project_graph(&root)?;
    let pack = kestrel_core::build_context_pack_for_query(&graph, &opts.question, opts.budget);

    if pack.entries.is_empty() {
        eprintln!(
            "No relevant files matched \"{}\" — answering without codebase context.",
            opts.question
        );
    }

    let context = assemble_context(&graph, &pack);
    let user_content = format!(
        "Here is relevant context from the codebase:\n\n{context}\n---\n\nQuestion: {}\n\n\
         Answer using the context above and cite the file paths you rely on. If the context does \
         not contain enough information to answer, say so explicitly rather than guessing.",
        opts.question
    );
    let system = "You are Kestrel, a precise coding assistant. Answer questions about the user's \
         codebase using the provided file excerpts. Be concise, concrete, and cite file paths. If \
         the provided context does not contain the answer, say so rather than guessing.";

    let body = serde_json::json!({
        "model": opts.model,
        "max_tokens": opts.max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": user_content }],
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();

    eprintln!(
        "Kestrel ask — model {}, {} context files (~{} tokens), seed: {}",
        opts.model,
        pack.entries.len(),
        pack.used_tokens,
        pack.seed
    );

    if opts.dry_run {
        println!("{body_str}");
        return Ok(());
    }

    let api_key = env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty());
    let Some(api_key) = api_key else {
        eprintln!(
            "ANTHROPIC_API_KEY is not set — printing the assembled prompt instead of calling the API."
        );
        eprintln!(
            "(Set ANTHROPIC_API_KEY to get an answer, or use --dry-run for the raw request.)"
        );
        println!("{user_content}");
        return Ok(());
    };

    // Send the body from a temp file so large context and quoting are safe.
    let tmp = env::temp_dir().join(format!("kestrel-ask-{}.json", std::process::id()));
    std::fs::write(&tmp, &body_str)?;
    let result = std::process::Command::new("curl")
        .args([
            "-sS",
            "https://api.anthropic.com/v1/messages",
            "-H",
            "content-type: application/json",
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
            &format!("x-api-key: {api_key}"),
            "-d",
            &format!("@{}", tmp.display()),
        ])
        .output();
    let _ = std::fs::remove_file(&tmp);

    let output = result?;
    if !output.status.success() {
        eprintln!(
            "curl request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        std::process::exit(1);
    }

    let response: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unexpected response (not JSON): {e}\n{}",
                String::from_utf8_lossy(&output.stdout)
            ),
        )
    })?;

    if response.get("type").and_then(|t| t.as_str()) == Some("error") {
        let message = response
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        eprintln!("API error: {message}");
        std::process::exit(1);
    }

    let mut answer = String::new();
    if let Some(blocks) = response.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    answer.push_str(text);
                }
            }
        }
    }

    let stop = response
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if stop == "refusal" {
        eprintln!("(The model declined to answer this request.)");
    }
    println!("{}", answer.trim());
    if stop == "max_tokens" {
        eprintln!("\n(Answer truncated at max_tokens; raise it with --max-tokens.)");
    }
    if let Some(usage) = response.get("usage") {
        let input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        eprintln!("\n[tokens: {input} in / {output} out]");
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
