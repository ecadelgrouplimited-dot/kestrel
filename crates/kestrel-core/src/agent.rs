//! The build agent: turn a model reply into real files on disk.
//!
//! Kestrel's chat can *answer*; the agent can *act*. Given a request like "build
//! me a portfolio site", the model is instructed to emit a file manifest in a
//! strict, fence-free protocol, which this module parses into [`FileEdit`]s and
//! writes under the project root — refusing any path that escapes it. This is
//! the single-shot wedge of the agentic loop: propose a complete set of files,
//! apply them, and show the result. (A multi-turn tool loop that reads, edits,
//! and verifies iteratively is the next step up from here.)

use crate::providers::{
    run_turn, AgentMessage, ProviderConfig, ToolCall, ToolResult, ToolSpec, TurnResult,
};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A cap on how much text a tool may return to the model, in bytes.
const TOOL_OUTPUT_CAP: usize = 60_000;

/// How long a single `run_command` may run before it is killed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(240);

/// The line that opens a file block: `<<<FILE relative/path>>>`.
pub const FILE_MARKER: &str = "<<<FILE ";
/// The line that closes a file block.
pub const END_MARKER: &str = "<<<END>>>";

/// One file the model wants written, with its project-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: String,
    pub contents: String,
}

/// The result of applying one edit: the absolute path written, or why not.
#[derive(Debug, Clone)]
pub struct AppliedEdit {
    pub path: String,
    pub outcome: Result<PathBuf, String>,
}

impl AppliedEdit {
    pub fn is_ok(&self) -> bool {
        self.outcome.is_ok()
    }
}

/// The system prompt that puts the model in build-agent mode: emit files, and
/// nothing but files, in Kestrel's manifest protocol.
pub fn agent_system_prompt() -> String {
    format!(
        "You are Kestrel's build agent. Turn the user's request into a complete, working set \
         of project files.\n\n\
         Output ONLY files, each in EXACTLY this format, back to back with nothing else:\n\
         {FILE_MARKER}relative/path/to/file>>>\n\
         <the entire raw contents of the file>\n\
         {END_MARKER}\n\n\
         Rules:\n\
         - Paths are relative to the project root (e.g. package.json, src/App.tsx).\n\
         - Emit the ENTIRE contents of every file — never partial snippets, diffs, or ellipses.\n\
         - Do NOT wrap file contents in Markdown code fences.\n\
         - Do NOT write any prose, explanation, or headings before, between, or after the \
           file blocks. The whole reply must be file blocks only.\n\
         - Never use absolute paths or `..`; stay inside the project.\n\
         - Keep the project focused and runnable; prefer fewer, complete files over many stubs.\n\
         - Include a short README.md describing how to run it."
    )
}

/// Parse a model reply into the file edits it declares. Text outside the
/// `<<<FILE …>>> … <<<END>>>` blocks is ignored, so stray prose is harmless.
pub fn parse_file_edits(reply: &str) -> Vec<FileEdit> {
    let mut edits = Vec::new();
    let mut lines = reply.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(FILE_MARKER) else {
            continue;
        };
        let path = rest.trim().trim_end_matches(">>>").trim().to_string();
        if path.is_empty() {
            continue;
        }
        let mut contents = String::new();
        let mut first = true;
        for content_line in lines.by_ref() {
            if content_line.trim() == END_MARKER {
                break;
            }
            if !first {
                contents.push('\n');
            }
            contents.push_str(content_line);
            first = false;
        }
        edits.push(FileEdit { path, contents });
    }
    edits
}

/// Write each edit under `root`, creating parent directories. Any path that is
/// absolute or escapes `root` via `..` is rejected, not written.
pub fn apply_file_edits(root: &Path, edits: &[FileEdit]) -> Vec<AppliedEdit> {
    edits
        .iter()
        .map(|edit| AppliedEdit {
            path: edit.path.clone(),
            outcome: write_one(root, edit),
        })
        .collect()
}

fn write_one(root: &Path, edit: &FileEdit) -> Result<PathBuf, String> {
    let full = safe_join(root, &edit.path)?;
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&full, &edit.contents).map_err(|e| e.to_string())?;
    Ok(full)
}

/// Join a project-relative path to `root`, rejecting absolute paths and any
/// `..` component so a reply can never write outside the project.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let normalized = rel.replace('\\', "/");
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut out = root.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("`..` is not allowed".to_string()),
            _ => return Err("invalid path".to_string()),
        }
    }
    if out == root {
        return Err("empty path".to_string());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The tool-using agent loop.
//
// This is the real agent: the model is given tools to read files anywhere on
// the machine, fetch URLs, list directories, and write files into the project,
// and it drives a multi-turn loop — inspect, then act — until it is done. It is
// the step up from the single-shot manifest above.
// ---------------------------------------------------------------------------

/// A progress event emitted as the agent works, for live display.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// The model's own narration for a turn.
    Assistant(String),
    /// A read-only tool the agent invoked (human-readable one-liner).
    Tool(String),
    /// A file was written to the project, with its full contents for live
    /// preview in the UI.
    Wrote { path: String, contents: String },
}

/// The system prompt for the tool-using agent loop.
pub fn agent_loop_system_prompt(root: &Path) -> String {
    format!(
        "You are Kestrel, an autonomous coding agent running natively on the user's Windows \
         machine. You have real tools:\n\
         - read_file(path): read any UTF-8 text file (absolute path, or relative to the project).\n\
         - list_dir(path): list a directory's entries.\n\
         - http_get(url): fetch the body of an http(s) URL — an API, or a raw GitHub file such \
           as https://raw.githubusercontent.com/owner/repo/branch/path.\n\
         - search(query): find where text or code appears across the project (returns \
           path:line matches) — use it to understand an existing codebase before changing it.\n\
         - write_file(path, contents): create or overwrite a file inside the project (relative \
           path; `..` and absolute paths are refused).\n\
         - run_command(command): run a shell command in the project root (e.g. `npm install`, \
           `npm run build`, `npx tsc --noEmit`, `cargo test`) and read its output and exit code.\n\
         - git(args): run a git command in the project (clone, status, diff, add, commit, log) \
           to pull a template repo, inspect history, or snapshot your work.\n\
         - verify(): run the project's detected build/test ladder and report pass/fail.\n\n\
         Work step by step: inspect what you need with read_file/list_dir/http_get, then create \
         the project by calling write_file for each file with its ENTIRE contents (never partial \
         snippets). Prefer fewer, complete, runnable files.\n\n\
         Work efficiently: you can call write_file MANY TIMES IN A SINGLE TURN — batch several \
         files together per turn rather than one file per message, so the whole project is \
         created in as few turns as possible. Keep narration to one short line per turn.\n\n\
         VERIFY YOUR WORK: after writing or changing code, actually check it — run the build or \
         type-checker with run_command (or call verify). If it fails, READ the errors, fix the \
         offending files, and run it again. Iterate until it passes or you have made a genuine \
         effort. Do not claim success without verifying.\n\n\
         When you are finished, stop calling tools and reply with a short summary of what you \
         built and what verification showed.\n\n\
         The current project root is: {}",
        root.display()
    )
}

/// The tools the agent may call.
pub fn builtin_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a UTF-8 text file from the user's computer. Accepts an absolute \
                          path or a path relative to the project root."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolSpec {
            name: "list_dir".to_string(),
            description: "List the entries of a directory (absolute or project-relative)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolSpec {
            name: "http_get".to_string(),
            description: "Fetch the text body of an http(s) URL, e.g. an API response or a raw \
                          GitHub file."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"],
            }),
        },
        ToolSpec {
            name: "write_file".to_string(),
            description: "Create or overwrite a file inside the project with the given contents. \
                          The path is relative to the project root; `..` and absolute paths are \
                          refused."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "contents": { "type": "string" },
                },
                "required": ["path", "contents"],
            }),
        },
        ToolSpec {
            name: "search".to_string(),
            description: "Search the project's text files for a query string (case-insensitive) \
                          and return matching `path:line: text` results. Optionally scope to a \
                          sub-path."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                },
                "required": ["query"],
            }),
        },
        ToolSpec {
            name: "git".to_string(),
            description: "Run a git command in the project root, e.g. \"clone <url> .\", \
                          \"status\", \"diff\", \"add -A\", \"commit -m msg\", \"log --oneline\". \
                          A default identity is used for commits if none is configured."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "args": { "type": "string" } },
                "required": ["args"],
            }),
        },
        ToolSpec {
            name: "run_command".to_string(),
            description: "Run a shell command in the project root and return its stdout, stderr, \
                          and exit code. Use this to install dependencies, build, type-check, or \
                          test the project (e.g. `npm install`, `npm run build`, `cargo test`). \
                          Commands are killed after a few minutes."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            }),
        },
        ToolSpec {
            name: "verify".to_string(),
            description: "Run the project's detected verification ladder (its format/test/build \
                          commands) and report pass/fail with failing output."
                .to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
    ]
}

/// A short human description of a tool call, for the progress log.
pub fn describe_call(call: &ToolCall) -> String {
    let arg = |key: &str| {
        call.input
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match call.name.as_str() {
        "read_file" | "list_dir" => format!("{}({})", call.name, arg("path")),
        "http_get" => format!("http_get({})", arg("url")),
        "search" => format!("search({})", arg("query")),
        "write_file" => format!("write_file({})", arg("path")),
        "run_command" => format!("run_command: {}", arg("command")),
        "git" => format!("git {}", arg("args")),
        "verify" => "verify()".to_string(),
        other => other.to_string(),
    }
}

/// Execute one tool call, returning the text to feed back to the model.
pub fn execute_tool(root: &Path, call: &ToolCall) -> String {
    let arg = |key: &str| {
        call.input
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match call.name.as_str() {
        "read_file" => {
            let path = resolve_read_path(root, &arg("path"));
            match std::fs::read_to_string(&path) {
                Ok(text) => cap(text),
                Err(err) => format!("error: {err}"),
            }
        }
        "list_dir" => {
            let path = resolve_read_path(root, &arg("path"));
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut out = String::new();
                    for entry in entries.flatten() {
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        out.push_str(&entry.file_name().to_string_lossy());
                        out.push_str(if is_dir { "/\n" } else { "\n" });
                    }
                    if out.is_empty() {
                        "(empty directory)".to_string()
                    } else {
                        cap(out)
                    }
                }
                Err(err) => format!("error: {err}"),
            }
        }
        "http_get" => http_get(&arg("url")),
        "search" => {
            let query = arg("query");
            if query.trim().is_empty() {
                "error: empty query".to_string()
            } else {
                let scope = arg("path");
                let scope = if scope.trim().is_empty() {
                    None
                } else {
                    Some(scope)
                };
                search_project(root, &query, scope.as_deref(), 200)
            }
        }
        "run_command" => {
            let command = arg("command");
            if command.trim().is_empty() {
                "error: empty command".to_string()
            } else {
                run_shell(root, &command, COMMAND_TIMEOUT)
            }
        }
        "git" => {
            let args = arg("args");
            if args.trim().is_empty() {
                "error: empty git args".to_string()
            } else {
                run_git(root, &args, COMMAND_TIMEOUT)
            }
        }
        "verify" => project_verify(root),
        "write_file" => match safe_join(root, &arg("path")) {
            Ok(full) => {
                if let Some(parent) = full.parent() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        return format!("error: {err}");
                    }
                }
                match std::fs::write(&full, arg("contents")) {
                    Ok(()) => format!("wrote {}", full.display()),
                    Err(err) => format!("error: {err}"),
                }
            }
            Err(err) => format!("error: {err}"),
        },
        other => format!("error: unknown tool {other}"),
    }
}

/// Resolve a tool path for reading: absolute paths are used as-is (the agent
/// may read anywhere on the machine); relative paths are joined to the project.
fn resolve_read_path(root: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    }
}

/// Fetch a URL's body via the system `curl`.
fn http_get(url: &str) -> String {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return "error: only http(s) URLs are allowed".to_string();
    }
    match std::process::Command::new("curl")
        .args(["-sSL", "--max-time", "30"])
        .arg(url)
        .output()
    {
        Ok(out) if out.status.success() => cap(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => format!("error: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(err) => format!("error: curl failed: {err}"),
    }
}

/// Truncate tool output to the cap, noting how much was dropped.
fn cap(mut text: String) -> String {
    if text.len() > TOOL_OUTPUT_CAP {
        let dropped = text.len() - TOOL_OUTPUT_CAP;
        text.truncate(floor_char_boundary(&text, TOOL_OUTPUT_CAP));
        text.push_str(&format!("\n… [truncated {dropped} bytes]"));
    }
    text
}

/// Keep the last `max_bytes` of `text` (build errors usually surface at the
/// end), prefixed with an ellipsis when truncated.
fn tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let start = text.len() - max_bytes;
    let start = (start..text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());
    format!("…\n{}", &text[start..])
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    (0..=index)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0)
}

/// Run `command` in `root` via the platform shell, capturing stdout/stderr and
/// the exit code, killing it after `timeout`. Output tails are returned so the
/// model sees the relevant end of a long build log.
fn run_shell(root: &Path, command: &str, timeout: Duration) -> String {
    let mut shell = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    shell
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match shell.spawn() {
        Ok(child) => child,
        Err(err) => return format!("error: could not start command: {err}"),
    };

    // Drain the pipes on threads so a chatty process cannot deadlock on a full
    // pipe buffer while we wait.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(pipe) = out_pipe.as_mut() {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(pipe) = err_pipe.as_mut() {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    let mut result = match status {
        Some(status) => format!(
            "exit code: {}\n",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "terminated".to_string())
        ),
        None => format!("TIMED OUT after {}s — process killed\n", timeout.as_secs()),
    };
    if !stdout.trim().is_empty() {
        result.push_str("--- stdout ---\n");
        result.push_str(&tail(&stdout, 8000));
        result.push('\n');
    }
    if !stderr.trim().is_empty() {
        result.push_str("--- stderr ---\n");
        result.push_str(&tail(&stderr, 8000));
        result.push('\n');
    }
    if stdout.trim().is_empty() && stderr.trim().is_empty() {
        result.push_str("(no output)\n");
    }
    cap(result)
}

/// Search the project's text files for `query` (case-insensitive substring),
/// returning up to `max` `path:line: text` matches. Build/VCS dirs are skipped.
fn search_project(root: &Path, query: &str, scope: Option<&str>, max: usize) -> String {
    let needle = query.to_lowercase();
    let base = match scope {
        Some(s) => resolve_read_path(root, s),
        None => root.to_path_buf(),
    };
    let mut out = String::new();
    let mut matches = 0;

    if base.is_file() {
        grep_file(root, &base, &needle, &mut out, &mut matches, max);
    } else {
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            if matches >= max {
                break;
            }
            let Ok(entries) = crate::read_dir_entries(&dir) else {
                continue;
            };
            for entry in entries {
                if entry.is_dir {
                    stack.push(entry.path);
                } else {
                    grep_file(root, &entry.path, &needle, &mut out, &mut matches, max);
                    if matches >= max {
                        break;
                    }
                }
            }
        }
    }

    if out.is_empty() {
        format!("no matches for \"{query}\"")
    } else {
        if matches >= max {
            out.push_str("… [more matches omitted]\n");
        }
        cap(out)
    }
}

/// Append `path:line: text` matches for `needle` in one file (skipping large or
/// non-UTF-8 files) to `out`, stopping at `max` total matches.
fn grep_file(
    root: &Path,
    path: &Path,
    needle: &str,
    out: &mut String,
    matches: &mut usize,
    max: usize,
) {
    if std::fs::metadata(path)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(true)
    {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let rel = path.strip_prefix(root).unwrap_or(path);
    for (i, line) in text.lines().enumerate() {
        if *matches >= max {
            return;
        }
        if line.to_lowercase().contains(needle) {
            let trimmed = line.trim();
            let shown = if trimmed.len() > 200 {
                &trimmed[..floor_char_boundary(trimmed, 200)]
            } else {
                trimmed
            };
            out.push_str(&format!("{}:{}: {}\n", rel.display(), i + 1, shown));
            *matches += 1;
        }
    }
}

/// Run a git command in `root`, injecting a fallback identity for commits so
/// they don't fail when git has no configured `user.name`/`user.email`.
fn run_git(root: &Path, args: &str, timeout: Duration) -> String {
    let command = if args.trim_start().starts_with("commit") {
        format!("git -c user.name=Kestrel -c user.email=kestrel@local {args}")
    } else {
        format!("git {args}")
    };
    run_shell(root, &command, timeout)
}

/// Run the project's detected verification ladder and format the outcome.
fn project_verify(root: &Path) -> String {
    let inspection = match crate::inspect_project(root) {
        Ok(inspection) => inspection,
        Err(err) => return format!("error: {err}"),
    };
    let configured = crate::load_config(&inspection.project_root)
        .config()
        .verify
        .steps;
    let steps = if configured.is_empty() {
        crate::plan_verification(&inspection.markers)
    } else {
        configured
            .iter()
            .map(|c| crate::VerifyStep {
                label: c.split_whitespace().next().unwrap_or("step").to_string(),
                command: c.clone(),
            })
            .collect()
    };
    if steps.is_empty() {
        return "no verification commands were detected for this project (no build/test ladder \
                found). Use run_command to build or test it directly."
            .to_string();
    }
    let report = crate::run_verification(&inspection.project_root, &steps);
    let mut out = format!(
        "verification {}\n",
        if report.passed { "PASSED" } else { "FAILED" }
    );
    for step in &report.steps {
        out.push_str(&format!(
            "[{}] {} ({} ms)\n",
            if step.success { "PASS" } else { "FAIL" },
            step.command,
            step.duration_ms
        ));
        if !step.success {
            let detail = if step.stderr_tail.is_empty() {
                &step.stdout_tail
            } else {
                &step.stderr_tail
            };
            out.push_str(detail);
            out.push('\n');
        }
    }
    cap(out)
}

/// Run the tool-using agent loop until the model stops calling tools or the
/// step limit is hit. `on_event` is called with progress as it happens.
pub fn run_agent(
    config: &ProviderConfig,
    model: &str,
    user_prompt: &str,
    root: &Path,
    max_steps: usize,
    mut on_event: impl FnMut(AgentEvent),
) -> Result<String, String> {
    let system = agent_loop_system_prompt(root);
    let tools = builtin_tools();
    let mut messages = vec![AgentMessage::User(user_prompt.to_string())];

    for _ in 0..max_steps {
        let turn: TurnResult = match run_turn(config, model, 8192, Some(&system), &messages, &tools)
        {
            Ok(inner) => inner?,
            Err(err) => return Err(err.to_string()),
        };
        if !turn.text.trim().is_empty() {
            on_event(AgentEvent::Assistant(turn.text.clone()));
        }
        if turn.calls.is_empty() {
            return Ok(turn.text);
        }
        messages.push(AgentMessage::Assistant {
            text: turn.text.clone(),
            calls: turn.calls.clone(),
        });
        let mut results = Vec::new();
        for call in &turn.calls {
            let content = if call.name == "write_file" {
                let path = call
                    .input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let contents = call
                    .input
                    .get("contents")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let out = execute_tool(root, call);
                on_event(AgentEvent::Wrote { path, contents });
                out
            } else {
                on_event(AgentEvent::Tool(describe_call(call)));
                let out = execute_tool(root, call);
                // Surface build/verify results in the transcript, not just to
                // the model, so the user can watch it check its own work.
                if matches!(call.name.as_str(), "run_command" | "verify" | "git") {
                    on_event(AgentEvent::Assistant(tail(&out, 1200)));
                }
                out
            };
            results.push(ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                content,
            });
        }
        messages.push(AgentMessage::ToolResults(results));
    }
    Err(format!(
        "agent stopped after {max_steps} steps without finishing"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_and_write_tools_hit_disk() {
        let dir = std::env::temp_dir().join(format!(
            "kestrel-tools-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let write = ToolCall {
            id: "1".to_string(),
            name: "write_file".to_string(),
            input: serde_json::json!({"path": "notes/hello.txt", "contents": "hi there"}),
        };
        let out = execute_tool(&dir, &write);
        assert!(out.starts_with("wrote"));
        assert!(dir.join("notes/hello.txt").is_file());

        let read = ToolCall {
            id: "2".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "notes/hello.txt"}),
        };
        assert_eq!(execute_tool(&dir, &read), "hi there");

        let escape = ToolCall {
            id: "3".to_string(),
            name: "write_file".to_string(),
            input: serde_json::json!({"path": "../evil.txt", "contents": "no"}),
        };
        assert!(execute_tool(&dir, &escape).starts_with("error:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_command_tool_captures_output_and_exit_code() {
        let dir = std::env::temp_dir().join(format!("kestrel-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let call = ToolCall {
            id: "1".to_string(),
            name: "run_command".to_string(),
            input: serde_json::json!({ "command": "echo kestrel_ok" }),
        };
        let out = execute_tool(&dir, &call);
        assert!(out.contains("exit code: 0"), "got: {out}");
        assert!(out.contains("kestrel_ok"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_tool_finds_matches_with_locations() {
        let dir = std::env::temp_dir().join(format!("kestrel-search-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "// beta helper\n").unwrap();

        let call = ToolCall {
            id: "1".to_string(),
            name: "search".to_string(),
            input: serde_json::json!({ "query": "beta" }),
        };
        let out = execute_tool(&dir, &call);
        assert!(out.contains("a.rs:2"), "got: {out}");
        assert!(out.contains("b.rs:1"), "got: {out}");

        let miss = execute_tool(
            &dir,
            &ToolCall {
                id: "2".to_string(),
                name: "search".to_string(),
                input: serde_json::json!({ "query": "zzznope" }),
            },
        );
        assert!(miss.contains("no matches"), "got: {miss}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_tool_runs_git() {
        let dir = std::env::temp_dir().join(format!("kestrel-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let call = ToolCall {
            id: "1".to_string(),
            name: "git".to_string(),
            input: serde_json::json!({ "args": "--version" }),
        };
        let out = execute_tool(&dir, &call);
        assert!(out.to_lowercase().contains("git version"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_reports_when_no_ladder_detected() {
        let dir = std::env::temp_dir().join(format!("kestrel-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "hi").unwrap();
        let call = ToolCall {
            id: "1".to_string(),
            name: "verify".to_string(),
            input: serde_json::json!({}),
        };
        let out = execute_tool(&dir, &call);
        assert!(out.to_lowercase().contains("no verification"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn describe_call_is_human_readable() {
        let call = ToolCall {
            id: "1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "src/main.rs"}),
        };
        assert_eq!(describe_call(&call), "read_file(src/main.rs)");
    }

    #[test]
    fn parses_multiple_files_preserving_content() {
        let reply = "\
<<<FILE package.json>>>
{
  \"name\": \"demo\"
}
<<<END>>>
some stray prose the model shouldn't have written
<<<FILE src/main.rs>>>
fn main() {
    println!(\"hi\");
}
<<<END>>>";
        let edits = parse_file_edits(reply);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].path, "package.json");
        assert!(edits[0].contents.contains("\"name\": \"demo\""));
        assert_eq!(edits[1].path, "src/main.rs");
        assert!(edits[1].contents.contains("println!(\"hi\")"));
    }

    #[test]
    fn applies_edits_to_disk_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!(
            "kestrel-agent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let edits = vec![
            FileEdit {
                path: "src/app.js".to_string(),
                contents: "console.log(1)".to_string(),
            },
            FileEdit {
                path: "../escape.txt".to_string(),
                contents: "nope".to_string(),
            },
        ];
        let applied = apply_file_edits(&dir, &edits);
        assert!(applied[0].is_ok());
        assert!(dir.join("src/app.js").is_file());
        assert!(!applied[1].is_ok());
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absolute_paths_are_rejected() {
        let root = Path::new("E:/proj");
        assert!(safe_join(root, "C:/Windows/system32").is_err());
        assert!(safe_join(root, "/etc/passwd").is_err());
        assert!(safe_join(root, "a/../../b").is_err());
        assert!(safe_join(root, "a/b.txt").is_ok());
    }
}
