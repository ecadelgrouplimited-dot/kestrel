use kestrel_core::{
    build_project_graph, discover_environment, inspect_project, load_config, plan_verification,
    project_symbols, run_verification, EnvironmentReport, ProjectGraph, ProjectInspection,
    VerificationReport, VerifyStep,
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
        "edit" => {
            let rest: Vec<String> = args.collect();
            let Some(opts) = parse_edit_args(&rest) else {
                eprintln!("Usage: kestrel edit <file> \"<instruction>\" [--apply] [--verify] [--revert-on-fail] [--model NAME] [--budget N] [--max-tokens N] [--dry-run]");
                std::process::exit(2);
            };
            run_edit(opts)?;
        }
        "verify" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(env::current_dir()?);
            if !run_verify(path)? {
                std::process::exit(1);
            }
        }
        "env" => {
            print_environment(&discover_environment());
        }
        "run" => {
            let rest: Vec<String> = args.collect();
            let Some((command, path, shell)) = parse_run_args(&rest) else {
                eprintln!("Usage: kestrel run \"<command>\" [path] [--shell default|powershell|pwsh|cmd|bash]");
                std::process::exit(2);
            };
            let root = path.unwrap_or(env::current_dir()?);
            eprintln!("Kestrel run [{shell}] in {}\n$ {command}", root.display());
            std::process::exit(run_shell_command(&shell, &command, &root)?);
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
    println!("  kestrel edit <file> \"<instruction>\" [--apply] [--verify] [--model NAME]");
    println!("                            Propose a reviewed diff for a file (write with --apply)");
    println!("  kestrel verify [path]     Run the project's format/test/build ladder");
    println!("  kestrel env               Show the host environment (shells, WSL, toolchains)");
    println!("  kestrel run \"<command>\" [path] [--shell ...]");
    println!("                            Run a command in a chosen shell, streaming output");
    println!();
    println!("Config: an optional kestrel.toml at the project root sets defaults");
    println!("(model, budget, max_tokens) and can override the verification ladder.");
}

fn print_environment(report: &EnvironmentReport) {
    println!("Kestrel Environment");
    println!("  OS: {} ({})", report.os, report.arch);
    println!();

    let show = |label: &str, tools: &[kestrel_core::ToolInfo]| {
        println!("{label}");
        for tool in tools {
            if tool.found {
                let version = tool.version.as_deref().unwrap_or("(version unknown)");
                println!("  + {:<10} {version}", tool.name);
            } else {
                println!("  - {:<10} not found", tool.name);
            }
        }
        println!();
    };

    show("Shells", &report.shells);
    show("Toolchains", &report.toolchains);

    println!("Cross-boundary");
    if report.wsl.available {
        println!("  + WSL: {}", report.wsl.distros.join(", "));
    } else {
        println!("  - WSL: not installed");
    }
    if report.docker.found {
        let version = report.docker.version.as_deref().unwrap_or("");
        println!("  + Docker: {version}");
    } else {
        println!("  - Docker: not found");
    }
}

/// Parse `run` arguments: `<command>` (positional), an optional path, and
/// `--shell NAME`. Returns `(command, path, shell)`.
fn parse_run_args(args: &[String]) -> Option<(String, Option<PathBuf>, String)> {
    let mut command = None;
    let mut path = None;
    let mut shell = "default".to_string();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--shell" => {
                if let Some(v) = args.get(i + 1) {
                    shell = v.clone();
                }
                i += 2;
            }
            _ => {
                if command.is_none() {
                    command = Some(arg.clone());
                } else if path.is_none() {
                    path = Some(PathBuf::from(arg));
                }
                i += 1;
            }
        }
    }
    Some((command?, path, shell))
}

/// Map a shell name to its program and the argument(s) that precede a command
/// string (e.g. `("cmd", ["/C"])`). `default` picks the platform shell.
fn shell_invocation(shell: &str) -> (&'static str, &'static [&'static str]) {
    match shell {
        "powershell" => ("powershell", &["-NoProfile", "-Command"]),
        "pwsh" => ("pwsh", &["-NoProfile", "-Command"]),
        "cmd" => ("cmd", &["/C"]),
        "bash" => ("bash", &["-c"]),
        "sh" => ("sh", &["-c"]),
        _ if cfg!(windows) => ("cmd", &["/C"]),
        _ => ("sh", &["-c"]),
    }
}

/// Run a command in the chosen shell from `root`, inheriting stdio so its
/// output streams live. Returns the command's exit code.
fn run_shell_command(shell: &str, command: &str, root: &Path) -> io::Result<i32> {
    let (program, prefix) = shell_invocation(shell);
    let status = std::process::Command::new(program)
        .args(prefix)
        .arg(command)
        .current_dir(root)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

/// Build verification steps from a list of config command strings, labeling
/// each with its leading token.
fn steps_from_commands(commands: &[String]) -> Vec<VerifyStep> {
    commands
        .iter()
        .map(|command| VerifyStep {
            label: command
                .split_whitespace()
                .next()
                .unwrap_or("step")
                .to_string(),
            command: command.clone(),
        })
        .collect()
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
    budget: Option<usize>,
    model: Option<String>,
    max_tokens: Option<u64>,
    dry_run: bool,
}

/// Parse `ask` arguments: `<question>` then an optional path, plus
/// `--budget`/`-b`, `--model`, `--max-tokens`, and `--dry-run`. Unset options
/// stay `None` so a `kestrel.toml` default can fill them.
fn parse_ask_args(args: &[String]) -> Option<AskOptions> {
    let mut question = None;
    let mut path = None;
    let mut budget = None;
    let mut model = None;
    let mut max_tokens = None;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--budget" | "-b" => {
                budget = args.get(i + 1).and_then(|v| v.parse().ok());
                i += 2;
            }
            "--model" => {
                model = args.get(i + 1).cloned();
                i += 2;
            }
            "--max-tokens" => {
                max_tokens = args.get(i + 1).and_then(|v| v.parse().ok());
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
        budget,
        model,
        max_tokens,
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

/// POST an assembled Messages body to the Anthropic API via the system `curl`,
/// returning the parsed response or an error (including surfaced API errors).
fn send_anthropic(body: &serde_json::Value, api_key: &str) -> io::Result<serde_json::Value> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let tmp = env::temp_dir().join(format!("kestrel-req-{}.json", std::process::id()));
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
        return Err(io::Error::other(format!(
            "curl request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
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
        return Err(io::Error::other(format!("API error: {message}")));
    }
    Ok(response)
}

/// Concatenate all `text` content blocks of a Messages response.
fn response_text(response: &serde_json::Value) -> String {
    let mut text = String::new();
    if let Some(blocks) = response.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
    }
    text
}

/// Print the token usage line for a Messages response to stderr.
fn print_usage_line(response: &serde_json::Value) {
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
    let defaults = load_config(&root).config().defaults;
    let model = opts
        .model
        .clone()
        .or(defaults.model)
        .unwrap_or_else(|| DEFAULT_ASK_MODEL.to_string());
    let budget = opts
        .budget
        .or(defaults.budget)
        .unwrap_or(DEFAULT_ASK_BUDGET)
        .max(1);
    let max_tokens = opts
        .max_tokens
        .or(defaults.max_tokens)
        .unwrap_or(DEFAULT_ASK_MAX_TOKENS)
        .max(1);

    let graph = build_project_graph(&root)?;
    let pack = kestrel_core::build_context_pack_for_query(&graph, &opts.question, budget);

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
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": user_content }],
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();

    eprintln!(
        "Kestrel ask — model {}, {} context files (~{} tokens), seed: {}",
        model,
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

    let response = send_anthropic(&body, &api_key)?;
    let stop = response
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if stop == "refusal" {
        eprintln!("(The model declined to answer this request.)");
    }
    println!("{}", response_text(&response).trim());
    if stop == "max_tokens" {
        eprintln!("\n(Answer truncated at max_tokens; raise it with --max-tokens.)");
    }
    print_usage_line(&response);

    Ok(())
}

/// Extract the contents of the first fenced code block in `text`, or the whole
/// trimmed text if there is no fence. The model is asked to return the updated
/// file inside a single fence; this recovers it robustly.
fn extract_code_block(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(open) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("```"))
    else {
        return format!("{}\n", text.trim());
    };
    let close = lines[open + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with("```"))
        .map(|rel| open + 1 + rel)
        .unwrap_or(lines.len());
    let inner = lines[open + 1..close].join("\n");
    if inner.is_empty() {
        String::new()
    } else {
        format!("{inner}\n")
    }
}

/// Render a unified diff between `old` and `new`, or `None` if identical.
fn render_unified_diff(old: &str, new: &str, path: &str) -> Option<String> {
    if old == new {
        return None;
    }
    let diff = similar::TextDiff::from_lines(old, new);
    Some(
        diff.unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string(),
    )
}

/// A Markdown fence tag for a file based on its extension.
fn fence_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => {
            "typescript"
        }
        Some("py" | "pyw") => "python",
        _ => "",
    }
}

struct EditOptions {
    file: PathBuf,
    instruction: String,
    budget: Option<usize>,
    model: Option<String>,
    max_tokens: Option<u64>,
    apply: bool,
    dry_run: bool,
    verify: bool,
    revert_on_fail: bool,
}

/// Parse `edit` arguments: `<file>` and `<instruction>` (positional), plus
/// `--apply`, `--dry-run`, `--model`, `--budget`/`-b`, and `--max-tokens`.
fn parse_edit_args(args: &[String]) -> Option<EditOptions> {
    let mut file = None;
    let mut instruction = None;
    let mut budget = None;
    let mut model = None;
    let mut max_tokens = None;
    let mut apply = false;
    let mut dry_run = false;
    let mut verify = false;
    let mut revert_on_fail = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--budget" | "-b" => {
                budget = args.get(i + 1).and_then(|v| v.parse().ok());
                i += 2;
            }
            "--model" => {
                model = args.get(i + 1).cloned();
                i += 2;
            }
            "--max-tokens" => {
                max_tokens = args.get(i + 1).and_then(|v| v.parse().ok());
                i += 2;
            }
            "--apply" => {
                apply = true;
                i += 1;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--verify" => {
                verify = true;
                i += 1;
            }
            "--revert-on-fail" => {
                revert_on_fail = true;
                i += 1;
            }
            _ => {
                if file.is_none() {
                    file = Some(PathBuf::from(arg));
                } else if instruction.is_none() {
                    instruction = Some(arg.clone());
                }
                i += 1;
            }
        }
    }
    Some(EditOptions {
        file: file?,
        instruction: instruction?,
        budget,
        model,
        max_tokens,
        apply,
        dry_run,
        verify,
        revert_on_fail,
    })
}

/// Propose an edit to a single file: build context, ask the model for the full
/// updated file, show a unified diff, and write it only when `--apply` is given.
fn run_edit(opts: EditOptions) -> io::Result<()> {
    if !opts.file.is_file() {
        eprintln!("Not a readable file: {}", opts.file.display());
        std::process::exit(2);
    }
    let current = std::fs::read_to_string(&opts.file)?;

    let search_root = opts
        .file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let graph = build_project_graph(&search_root)?;

    let defaults = load_config(&graph.root).config().defaults;
    let model = opts
        .model
        .clone()
        .or(defaults.model)
        .unwrap_or_else(|| DEFAULT_ASK_MODEL.to_string());
    let budget = opts
        .budget
        .or(defaults.budget)
        .unwrap_or(DEFAULT_ASK_BUDGET)
        .max(1);
    let max_tokens = opts
        .max_tokens
        .or(defaults.max_tokens)
        .unwrap_or(8_192)
        .max(1);

    // Background context: files related to the target, excluding the target.
    let mut background = String::new();
    if let Some(node) = find_node(&graph, &opts.file) {
        if let Some(pack) = kestrel_core::build_context_pack(&graph, &node.path, budget) {
            for entry in &pack.entries {
                if entry.path == node.path {
                    continue;
                }
                if let Ok(src) = std::fs::read_to_string(graph.root.join(&entry.path)) {
                    background.push_str(&format!(
                        "### {} ({})\n```{}\n{}",
                        entry.path.display(),
                        entry.reason,
                        fence_tag(&entry.language),
                        src
                    ));
                    if !src.ends_with('\n') {
                        background.push('\n');
                    }
                    background.push_str("```\n\n");
                }
            }
        }
    }

    let display_path = opts.file.display().to_string();
    let fence = fence_for_path(&opts.file);
    let current_fenced = if current.ends_with('\n') {
        current.clone()
    } else {
        format!("{current}\n")
    };
    let user_content = format!(
        "{background}You are editing this file.\n\nTARGET FILE: {display_path}\n```{fence}\n{current_fenced}```\n\n\
         INSTRUCTION: {}\n\nReturn the COMPLETE updated contents of {display_path} in a single fenced \
         code block, and nothing else. Preserve all unrelated code exactly.",
        opts.instruction
    );
    let system =
        "You are Kestrel, a precise code-editing assistant. You are given a file's current \
         contents and an instruction. Return the complete, updated contents of that file inside a \
         single fenced code block, and nothing else — no prose, no partial snippets, no ellipses. \
         Preserve unrelated code exactly. If the instruction cannot be satisfied, return the file \
         unchanged.";

    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": user_content }],
    });

    eprintln!(
        "Kestrel edit — model {}, target {} (+{} context bytes)",
        model,
        display_path,
        background.len()
    );

    if opts.dry_run {
        println!("{}", serde_json::to_string(&body).unwrap_or_default());
        return Ok(());
    }

    let Some(api_key) = env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) else {
        eprintln!("ANTHROPIC_API_KEY is not set. Set it, or use --dry-run to inspect the request.");
        std::process::exit(2);
    };

    let response = send_anthropic(&body, &api_key)?;
    if response.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
        eprintln!("The model declined to perform this edit.");
        print_usage_line(&response);
        std::process::exit(1);
    }
    let proposed = extract_code_block(&response_text(&response));

    match render_unified_diff(&current, &proposed, &display_path) {
        None => {
            eprintln!("No changes proposed — the file already satisfies the instruction.");
        }
        Some(diff) => {
            print!("{diff}");
            if !opts.apply {
                eprintln!("\nProposed diff shown above. Re-run with --apply to write the changes.");
            } else {
                std::fs::write(&opts.file, &proposed)?;
                eprintln!("\nApplied changes to {display_path}.");
                print_usage_line(&response);
                return verify_after_edit(&opts, &graph.root, &current, &display_path);
            }
        }
    }
    print_usage_line(&response);

    Ok(())
}

/// After an applied edit, optionally run the verification ladder and, on
/// failure, revert the file when `--revert-on-fail` is set.
fn verify_after_edit(
    opts: &EditOptions,
    root: &Path,
    original: &str,
    display_path: &str,
) -> io::Result<()> {
    if !opts.verify {
        return Ok(());
    }
    let inspection = inspect_project(root)?;
    let configured = load_config(root).config().verify.steps;
    let steps = if configured.is_empty() {
        plan_verification(&inspection.markers)
    } else {
        steps_from_commands(&configured)
    };
    if steps.is_empty() {
        eprintln!("No verification commands detected for this project — skipping verification.");
        return Ok(());
    }

    eprintln!("\nVerifying the change...");
    let report = run_verification(&inspection.project_root, &steps);
    print_verification(&report);

    if report.passed {
        return Ok(());
    }
    if opts.revert_on_fail {
        std::fs::write(&opts.file, original)?;
        eprintln!("Verification failed — reverted {display_path} to its previous contents.");
    } else {
        eprintln!(
            "Verification failed. The change is still applied; revert it with your VCS, or use \
             --revert-on-fail to auto-revert on failure."
        );
    }
    std::process::exit(1);
}

/// Run the detected verification ladder for a project and print the report.
/// Returns whether verification passed (or there was nothing to run).
fn run_verify(path: PathBuf) -> io::Result<bool> {
    let inspection = inspect_project(&path)?;
    let load = load_config(&inspection.project_root);
    if let kestrel_core::ConfigLoad::Invalid(err) = &load {
        eprintln!("Warning: ignoring invalid kestrel.toml ({err}).");
    }
    let configured = load.config().verify.steps;
    let (source, steps) = if configured.is_empty() {
        ("detected", plan_verification(&inspection.markers))
    } else {
        ("kestrel.toml", steps_from_commands(&configured))
    };
    if steps.is_empty() {
        eprintln!("No verification commands detected for this project.");
        return Ok(true);
    }
    eprintln!(
        "Kestrel verify — {} step(s) ({source}) in {}",
        steps.len(),
        inspection.project_root.display()
    );
    let report = run_verification(&inspection.project_root, &steps);
    print_verification(&report);
    Ok(report.passed)
}

/// Print a verification report to stderr (per-step status, plus failing output).
fn print_verification(report: &VerificationReport) {
    for step in &report.steps {
        let status = if step.success { "PASS" } else { "FAIL" };
        eprintln!(
            "  [{status}] {} — `{}` ({} ms)",
            step.label, step.command, step.duration_ms
        );
        if !step.success {
            if !step.stderr_tail.is_empty() {
                for line in step.stderr_tail.lines() {
                    eprintln!("      {line}");
                }
            } else if !step.stdout_tail.is_empty() {
                for line in step.stdout_tail.lines() {
                    eprintln!("      {line}");
                }
            }
        }
    }
    for step in &report.skipped {
        eprintln!("  [SKIP] {} — `{}`", step.label, step.command);
    }
    eprintln!(
        "Verification {}",
        if report.passed { "PASSED" } else { "FAILED" }
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_block_with_language_tag() {
        let text = "Here you go:\n```rust\nfn main() {}\n```\nDone.";
        assert_eq!(extract_code_block(text), "fn main() {}\n");
    }

    #[test]
    fn extract_code_block_without_fence_returns_trimmed() {
        let text = "  fn main() {}  ";
        assert_eq!(extract_code_block(text), "fn main() {}\n");
    }

    #[test]
    fn extract_code_block_takes_first_block_and_inner_only() {
        let text = "```\nline one\nline two\n```";
        assert_eq!(extract_code_block(text), "line one\nline two\n");
    }

    #[test]
    fn unified_diff_none_when_identical() {
        assert!(render_unified_diff("a\nb\n", "a\nb\n", "f.rs").is_none());
    }

    #[test]
    fn unified_diff_shows_changes() {
        let diff = render_unified_diff("a\nb\nc\n", "a\nB\nc\n", "f.rs").expect("a diff");
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
        assert!(diff.contains("a/f.rs"));
        assert!(diff.contains("b/f.rs"));
    }

    #[test]
    fn fence_for_path_maps_extensions() {
        assert_eq!(fence_for_path(Path::new("x.rs")), "rust");
        assert_eq!(fence_for_path(Path::new("x.tsx")), "typescript");
        assert_eq!(fence_for_path(Path::new("x.py")), "python");
        assert_eq!(fence_for_path(Path::new("x.txt")), "");
    }

    #[test]
    fn shell_invocation_maps_known_shells() {
        assert_eq!(shell_invocation("cmd"), ("cmd", &["/C"][..]));
        assert_eq!(
            shell_invocation("powershell"),
            ("powershell", &["-NoProfile", "-Command"][..])
        );
        assert_eq!(shell_invocation("bash"), ("bash", &["-c"][..]));
        // Unknown shells fall back to the platform default.
        let (prog, _) = shell_invocation("nonsense");
        assert!(prog == "cmd" || prog == "sh");
    }

    #[test]
    fn parse_run_args_extracts_command_path_and_shell() {
        let args = vec![
            "cargo test".to_string(),
            "src".to_string(),
            "--shell".to_string(),
            "powershell".to_string(),
        ];
        let (command, path, shell) = parse_run_args(&args).unwrap();
        assert_eq!(command, "cargo test");
        assert_eq!(path, Some(PathBuf::from("src")));
        assert_eq!(shell, "powershell");
        assert!(parse_run_args(&[]).is_none());
    }
}
