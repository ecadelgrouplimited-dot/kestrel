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
    run_turn, run_turn_streaming, AgentMessage, ChatMessage, ProviderConfig, ToolCall, ToolResult,
    ToolSpec, TurnEvent, TurnResult, Usage,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// A file is being written *right now*, streamed token-by-token as the model
    /// emits it — the (partial) contents so far, for real-time preview before the
    /// write actually lands. Superseded by `Wrote` once the tool runs.
    Writing { path: String, contents: String },
    /// Token usage from a completed turn, for the live meter.
    Usage(Usage),
    /// The agent's task plan changed — the live TODO ledger for the UI.
    Plan(crate::plan::Plan),
}

/// Which product surface the agent is running as. The autonomy engine is the
/// same for both — only the tool pack and the system prompt differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Kestrel Build — the coding agent.
    #[default]
    Build,
    /// Kestrel Work — everyday knowledge work (research, documents, data).
    Work,
}

/// The tools a profile may use. Work gets the autonomy, file, and research
/// tools; the code-specific ones (git, verify, toolchains, servers, symbol
/// navigation) stay with Build.
pub fn tools_for(profile: Profile) -> Vec<ToolSpec> {
    match profile {
        Profile::Build => builtin_tools(),
        Profile::Work => {
            const WORK_TOOLS: &[&str] = &[
                // Autonomy
                "update_plan",
                "remember",
                "spawn_subagent",
                // Files & content
                "read_file",
                "read_doc",
                "write_file",
                "write_doc",
                "write_sheet",
                "check_doc",
                "edit_file",
                "list_dir",
                "search",
                // Research
                "web_search",
                "http_get",
                "check_page",
                // System
                "run_command",
                "open_url",
                "screenshot",
            ];
            builtin_tools()
                .into_iter()
                .filter(|t| WORK_TOOLS.contains(&t.name.as_str()))
                .collect()
        }
    }
}

/// The system prompt for a profile.
pub fn system_prompt_for(profile: Profile, root: &Path) -> String {
    match profile {
        Profile::Build => agent_loop_system_prompt(root),
        Profile::Work => work_system_prompt(root),
    }
}

/// The system prompt for Kestrel Work — everyday knowledge work rather than code.
pub fn work_system_prompt(root: &Path) -> String {
    format!(
        "You are Kestrel Work, an autonomous work assistant running natively on the user's \
         Windows machine. You do real knowledge work — research, writing and editing documents, \
         working with data, and organising files — directly in their files, not just in chat.\n\n\
         Your tools:\n\
         - update_plan(goal, steps): your task checklist. For ANY non-trivial request, call this \
           FIRST to break the work into concrete steps, then keep it current as you go (mark a \
           step done, set the next active). Do NOT report finished while steps remain.\n\
         - web_search(query) / http_get(url) / check_page(url, expect): research. Search for \
           sources, read them, and confirm facts. NEVER assert a fact you have not checked — \
           cite where each claim came from.\n\
         - read_doc(path): read a REAL document — Word (.docx/.doc/.rtf), PDF, or Excel \
           (.xlsx, as TSV per sheet). Always use this for those formats; read_file only handles \
           plain text and will return garbage for them.\n\
         - read_file(path) / write_file(path, contents) / edit_file(path, old, new) / \
           list_dir(path) / search(query): read and produce text documents and data files in the \
           user's workspace. edit_file makes targeted revisions without rewriting a whole \
           document.\n\
         - run_command(command): run a command (PowerShell is available) to convert, inspect, or \
           process files when needed.\n\
         - remember(note, category): save durable facts about this workspace (house style, where \
           things live, recurring formats) so future sessions start knowing them.\n\
         - spawn_subagent(task): delegate a big self-contained chunk (e.g. \"research and \
           summarise these 8 sources\") to keep your own context lean.\n\
         - open_url(url) / screenshot(): show the user something, or capture the screen.\n\n\
         HOW TO WORK:\n\
         1. PLAN first for anything multi-step.\n\
         2. RESEARCH before you assert. Prefer primary sources; note the URL for each key claim.\n\
         3. PRODUCE REAL FILES in the format the user actually wants — do not just paste a draft \
            into chat. Use write_doc(path.docx, markdown) for a Word document and \
            write_sheet(path.xlsx, csv) for a spreadsheet; both are written directly, so never \
            tell the user to convert anything themselves. Use write_file for .md/.txt/.csv. \
            Structure the document properly: a clear title, sections, and a short summary up \
            front.\n\
         4. CHECK YOUR WORK before you finish, the way a careful colleague would. Run \
            check_doc(path, expect=[…]) on what you produced, listing the sections, figures, and \
            key facts that must appear. Treat a FAIL like a broken build: fix the document and \
            re-check until it passes. Also confirm the numbers agree between the text and the \
            data, and that every claim is sourced. In spreadsheets, put real formulas (e.g. \
            \"=SUM(B2:B9)\") in total rows rather than hard-coded numbers, so totals stay \
            correct.\n\
         5. Be accurate over impressive. If something can't be verified, say so plainly rather \
            than inventing it. Never fabricate data, quotes, figures, or citations.\n\n\
         Write files only inside the workspace folder below. When you are done, stop calling \
         tools and reply with a short summary of what you produced, where it is saved, and \
         anything the user should check.\n\n\
         {}The workspace folder is: {}",
        memory_prompt(root),
        root.display()
    )
}

/// The system prompt for the tool-using agent loop.
pub fn agent_loop_system_prompt(root: &Path) -> String {
    format!(
        "You are Kestrel, an autonomous coding agent running natively on the user's Windows \
         machine. You have real tools:\n\
         - update_plan(goal, steps): your task checklist. For ANY non-trivial task, call this \
           FIRST to break the goal into concrete, verifiable steps; then call it again as you \
           work to mark each step done and set the next one active. Pass the full list each time. \
           Keep steps outcome-focused (\"scaffold the app and get it building\", not \"write \
           index.html\"). Do NOT report the task finished while steps remain unless you explain \
           why they are unnecessary.\n\
         - remember(note, category): save a durable fact about THIS project (a convention, the \
           build/run/test command, an architecture note, a gotcha, a decision) to persistent \
           memory, so future runs start knowing it.\n\
         - spawn_subagent(task): delegate a big, self-contained chunk of work to a fresh \
           sub-agent with its own clean context; it returns a result summary. Use it to keep your \
           own context lean on large tasks.\n\
         - read_file(path): read any UTF-8 text file (absolute path, or relative to the project).\n\
         - web_search(query): search the web to discover current docs/APIs, then http_get the \
           best result. definition(name)/references(name)/outline(path): precise code navigation \
           — jump to a symbol, find everything that uses it, or map a file's structure before \
           editing. rename_symbol(old,new): rename a symbol across the whole project safely \
           (whole-word), then verify.\n\
         - list_dir(path): list a directory's entries.\n\
         - http_get(url): fetch the body of an http(s) URL — an API, or a raw GitHub file such \
           as https://raw.githubusercontent.com/owner/repo/branch/path.\n\
         - search(query): find where text or code appears across the project (returns \
           path:line matches) — use it to understand an existing codebase before changing it.\n\
         - write_file(path, contents): create or overwrite a file inside the project (relative \
           path; `..` and absolute paths are refused).\n\
         - edit_file(path, old, new): replace the exact snippet `old` with `new` in an existing \
           file (`old` must occur exactly once). Prefer this for small changes — it is far more \
           token-efficient than rewriting a whole file.\n\
         - run_command(command): run a shell command in the project root (e.g. `npm install`, \
           `npm run build`, `npx tsc --noEmit`, `cargo test`) and read its output and exit code.\n\
         - git(args): run a git command in the project (clone, status, diff, add, commit, log) \
           to pull a template repo, inspect history, or snapshot your work.\n\
         - verify(): run the project's detected build/test ladder and report pass/fail.\n\
         - install_tool(command[, package]): check whether a CLI tool exists and install it via \
           winget if missing. Use this FIRST when a build needs a toolchain that may be absent \
           (e.g. composer/php for Laravel, node, python).\n\
         - start_app(command) / app_logs(pid) / list_apps() / stop_app(pid): run a dev server \
           or app in the background, read its output/logs, see what's running, and stop it.\n\
         - open_url(url): open a preview in the user's browser.\n\
         - screenshot(): capture the screen for visual review.\n\n\
         When a project needs tools that may not be installed, check with install_tool before \
         building. NEVER run a server or any long-running process (e.g. `node server.js`, \
         `npm run dev`, `php artisan serve`, watchers) with run_command — it will block. Use \
         start_app for those; then http_check(url) to confirm it's up, app_logs(pid) to read \
         the server's output and debug it, http_get to hit an endpoint, and open_url to preview. \
         To restart after a fix, just call start_app again (it stops the previous instance). When \
         something is broken (e.g. a bad database query), read app_logs and the code, fix it, \
         restart, and re-check.\n\n\
         ANY LANGUAGE, ANY FRAMEWORK: you can build in whatever stack the user asks for — Rust, \
         Go, Python, TypeScript/Node, React/Next/Vue/Svelte, Swift, Kotlin, C#/.NET, PHP/Laravel, \
         Ruby/Rails, Flutter, Unity, embedded, CLIs, games, anything. If the user doesn't specify \
         a stack, pick the one best suited to the goal and say why. NEVER refuse or downgrade a \
         request because a stack is unfamiliar.\n\n\
         RESEARCH WHAT YOU DON'T KNOW: when a framework, API, library version, or file format is \
         unfamiliar or may have changed, do not guess — use web_search to find the official docs, \
         then http_get to read them (or a package registry: crates.io, npm, PyPI, pkg.go.dev, \
         Packagist), to confirm the current API, the right dependency versions, and the correct \
         project layout BEFORE writing code. When editing existing code, use definition/references/\
         outline to understand it precisely and rename_symbol for safe refactors. Verify commands and config against reality. A short research step up \
         front prevents broken builds.\n\n\
         Scaffold with the ecosystem's own tools when that's the idiomatic path (e.g. \
         `cargo new`, `npm create vite@latest`, `npx create-next-app`, `dotnet new`, \
         `composer create-project`) via run_command, then edit from there — but never run a \
         command that blocks waiting for a server; use start_app for those.\n\n\
         Work step by step: inspect what you need with read_file/list_dir/http_get, then create \
         the project by calling write_file for each file with its ENTIRE contents (never partial \
         snippets). Prefer fewer, complete, runnable files.\n\n\
         Work efficiently: you can call write_file MANY TIMES IN A SINGLE TURN — batch several \
         files together per turn rather than one file per message, so the whole project is \
         created in as few turns as possible. Keep narration to one short line per turn. When \
         CHANGING an existing file, use edit_file to replace just the relevant snippet rather \
         than rewriting the whole file.\n\n\
         VERIFY YOUR WORK: after writing or changing code, actually check it — run the build or \
         type-checker with run_command (or call verify). If it fails, READ the errors, fix the \
         offending files, and run it again. Iterate until it passes or you have made a genuine \
         effort. Do not claim success without verifying.\n\n\
         PROVE IT WORKS: for a web app or site, don't stop at \"it builds\" — start_app the \
         server, then check_page(url, expect=[...]) to render the page in a real browser and \
         confirm the expected content is actually there. Treat a check_page FAIL like a build \
         failure: read the output, fix it, restart, and re-check until it passes.\n\n\
         When you are finished, stop calling tools and reply with a short summary of what you \
         built and what verification showed.\n\n\
         {}{}The current project root is: {}",
        memory_prompt(root),
        multi_repo_prompt(root),
        root.display()
    )
}

/// If the project has learned memory, fold it into the prompt so every run starts
/// knowing it; otherwise contribute nothing.
fn memory_prompt(root: &Path) -> String {
    let notes = crate::memory::load_memory(root);
    let rendered = crate::memory::render_memory(&notes);
    if rendered.is_empty() {
        return String::new();
    }
    format!(
        "What you've already learned about THIS project (persistent memory — trust it, and keep \
         it current with remember(note, category) when you learn something durable):\n{rendered}\n"
    )
}

/// If the project is linked to other repositories, tell the agent how to reason
/// across them; otherwise contribute nothing.
fn multi_repo_prompt(root: &Path) -> String {
    let ws = crate::repos::load_workspace(root);
    if ws.repos.is_empty() {
        return String::new();
    }
    let mut list = String::new();
    for r in &ws.repos {
        list.push_str(&format!("\"{}\" ({}), ", r.name, r.path));
    }
    let list = list.trim_end_matches(", ");
    format!(
        "This project is part of a MULTI-REPOSITORY workspace. Linked repositories: {list}. Call \
         list_repos() to see them all with their paths. To reason across repos, search a linked \
         repo with search(query, repo=\"name\"), and read a file from one with read_file using \
         its absolute path. Writes still go only to the primary project.\n\n"
    )
}

/// The tools the agent may call.
pub fn builtin_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "update_plan".to_string(),
            description: "Create or update your task plan — the checklist you work through. Call \
                          this FIRST for any non-trivial task to break the goal into concrete \
                          steps, then call it again as you go to mark a step \"done\" and set the \
                          next one \"active\". Pass the FULL list each time (it replaces the plan). \
                          The plan is shown to the user live."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "A short restatement of the goal." },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "status": { "type": "string", "enum": ["todo", "active", "done"] },
                            },
                            "required": ["title", "status"],
                        },
                    },
                },
                "required": ["steps"],
            }),
        },
        ToolSpec {
            name: "remember".to_string(),
            description: "Save a durable fact about THIS project to persistent memory, so future \
                          runs start knowing it — a convention, the exact build/run/test command, \
                          an architecture note, a gotcha, or a decision. Keep each note short and \
                          specific. Don't remember transient details."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "note": { "type": "string" },
                    "category": {
                        "type": "string",
                        "enum": ["command", "convention", "architecture", "gotcha", "decision", "note"],
                    },
                },
                "required": ["note"],
            }),
        },
        ToolSpec {
            name: "spawn_subagent".to_string(),
            description: "Delegate a focused, self-contained sub-task to a fresh sub-agent with \
                          its own clean context (it can read/search/write/run/verify on the same \
                          project). Use it to isolate a big chunk of work — e.g. \"implement and \
                          test the auth module\" — so your own context stays lean. Returns the \
                          sub-agent's result summary. Sub-agents cannot spawn sub-agents."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "task": { "type": "string" } },
                "required": ["task"],
            }),
        },
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
            name: "read_doc".to_string(),
            description: "Read a real document's text: Word (.docx/.doc/.rtf/.odt), PDF, or Excel \
                          (.xlsx/.xls, returned as TSV per sheet). Use this instead of read_file \
                          for anything that isn't plain text — read_file only handles UTF-8 and \
                          will produce garbage for these formats."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolSpec {
            name: "check_doc".to_string(),
            description: "ACCEPTANCE CHECK for a document you produced: re-open it (.docx/.xlsx/\
                          .pdf/.md/.csv) and verify the expected content is really there. Use it \
                          before telling the user a document is finished — it catches a missing \
                          section, a dropped figure, or a file that didn't save."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "expect": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Substrings that must appear in the document.",
                    },
                },
                "required": ["path"],
            }),
        },
        ToolSpec {
            name: "write_doc".to_string(),
            description: "Write a real Word document (.docx) from Markdown. Supports headings \
                          (#/##/###), paragraphs, **bold**, *italic*, `code`, bullet and numbered \
                          lists, > quotes, and | tables |. Use this when the user wants a Word \
                          document — no conversion step needed."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Must end in .docx" },
                    "markdown": { "type": "string" },
                },
                "required": ["path", "markdown"],
            }),
        },
        ToolSpec {
            name: "write_sheet".to_string(),
            description: "Write a real Excel workbook (.xlsx). The first row of each sheet is a \
                          bold, frozen header; numeric values are stored as numbers; a cell \
                          starting with `=` becomes a LIVE formula (e.g. \"=SUM(B2:B9)\" or \
                          \"=SUM('Data'!B2:B9)\" across sheets). Pass `data` for one sheet, or \
                          `sheets` for several. Add `chart` to draw a real bar/line/pie chart of \
                          the first sheet (labels from column A)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Must end in .xlsx" },
                    "data": { "type": "string", "description": "CSV or TSV, one row per line." },
                    "sheet": { "type": "string", "description": "Sheet name for `data`." },
                    "sheets": {
                        "type": "array",
                        "description": "Several sheets, in tab order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "data": { "type": "string" },
                            },
                            "required": ["name", "data"],
                        },
                    },
                    "chart": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["bar", "line", "pie"] },
                            "title": { "type": "string" },
                            "value_column": {
                                "type": "integer",
                                "description": "1-based column of the values (default 2 = B).",
                            },
                        },
                        "required": ["type"],
                    },
                },
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
            name: "web_search".to_string(),
            description: "Search the web and get back titles, URLs, and snippets. Use it to \
                          discover the CURRENT docs/API for an unfamiliar framework or library \
                          before writing code, then http_get the best result. Prefer this over \
                          guessing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
            }),
        },
        ToolSpec {
            name: "check_page".to_string(),
            description: "ACCEPTANCE CHECK for a running web app: render a URL in a real headless \
                          browser (post-JavaScript DOM) and verify the expected text/content is \
                          actually present. Use it to PROVE a feature works before you claim done \
                          — start_app the server first, then check_page. Falls back to the raw \
                          HTTP body if no browser is installed."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "expect": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Substrings that must appear on the rendered page.",
                    },
                },
                "required": ["url"],
            }),
        },
        ToolSpec {
            name: "definition".to_string(),
            description: "Jump to where a symbol is defined: returns the file, line, kind, and \
                          signature for every definition of `name` in the project. Precise \
                          (tree-sitter), not grep."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            }),
        },
        ToolSpec {
            name: "references".to_string(),
            description: "Find every whole-word use of `name` across the project (returns \
                          path:line: text). Use it before changing or removing a symbol to see \
                          what depends on it."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            }),
        },
        ToolSpec {
            name: "outline".to_string(),
            description: "List the symbols (functions, types, methods…) declared in a file, with \
                          their lines — a quick structural map before you read or edit it."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolSpec {
            name: "rename_symbol".to_string(),
            description: "Rename a symbol across the WHOLE project: replaces every whole-word \
                          occurrence of `old` with `new` in all text files (so `user` never \
                          matches `username`). Verify the build afterward."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                },
                "required": ["old", "new"],
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
            name: "edit_file".to_string(),
            description: "Make a targeted edit to an existing file: replace the exact text `old` \
                          with `new`. `old` must appear EXACTLY ONCE. Use this for small changes \
                          instead of rewriting the whole file — it saves tokens."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                },
                "required": ["path", "old", "new"],
            }),
        },
        ToolSpec {
            name: "search".to_string(),
            description: "Search text files for a query string (case-insensitive) and return \
                          matching `path:line: text` results. Optionally scope to a sub-path. To \
                          search a linked repository instead of the primary project, pass its name \
                          as `repo` (see list_repos)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                    "repo": { "type": "string" },
                },
                "required": ["query"],
            }),
        },
        ToolSpec {
            name: "list_repos".to_string(),
            description: "List the repositories in this workspace — the primary project plus any \
                          linked repositories — with their names and absolute paths. Use it to \
                          reason across repos: search a linked repo with search(repo=\"name\"), or \
                          read a file from one with read_file using its absolute path."
                .to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
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
        ToolSpec {
            name: "open_url".to_string(),
            description: "Open a URL (or local file://) in the user's default browser — use it to \
                          preview a running app or a built page."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"],
            }),
        },
        ToolSpec {
            name: "start_app".to_string(),
            description: "Start a long-running app (e.g. `npm run dev`, `php artisan serve`) in \
                          the background and track it, so it keeps running. Returns its pid."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            }),
        },
        ToolSpec {
            name: "app_logs".to_string(),
            description: "Read the recent stdout/stderr of a background app started by start_app, \
                          by its pid. Use it to debug a server (errors, crashes, requests)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "pid": { "type": "integer" } },
                "required": ["pid"],
            }),
        },
        ToolSpec {
            name: "list_apps".to_string(),
            description: "List the background apps Kestrel started that are still running."
                .to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "stop_app".to_string(),
            description: "Stop a background app started by start_app, by its pid.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "pid": { "type": "integer" } },
                "required": ["pid"],
            }),
        },
        ToolSpec {
            name: "http_check".to_string(),
            description: "Poll a URL until it responds (or times out) and report the HTTP status \
                          — use it after start_app to confirm a server is actually up."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"],
            }),
        },
        ToolSpec {
            name: "screenshot".to_string(),
            description:
                "Capture the screen to a PNG under the project's .kestrel/screenshots for \
                          visual review (Windows)."
                    .to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "install_tool".to_string(),
            description: "Check whether a command-line tool is installed and, if not, install it \
                          via winget (Windows). Use before building a project whose toolchain may \
                          be missing (e.g. `composer` for Laravel, `node`, `php`, `python`). \
                          Optionally pass a winget package id."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "package": { "type": "string" },
                },
                "required": ["command"],
            }),
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
        "update_plan" => "🗺 Updating the plan".to_string(),
        "remember" => "🧠 Remembering a project fact".to_string(),
        "spawn_subagent" => format!("🤝 Delegating: {}", arg("task")),
        "read_file" => format!("📖 Reading {}", arg("path")),
        "read_doc" => format!("📄 Reading document {}", arg("path")),
        "write_doc" => format!("📝 Writing Word document {}", arg("path")),
        "write_sheet" => format!("📊 Writing spreadsheet {}", arg("path")),
        "check_doc" => format!("🧪 Checking document {}", arg("path")),
        "list_dir" => format!("📁 Listing {}", arg("path")),
        "http_get" => format!("🌐 Fetching {}", arg("url")),
        "web_search" => format!("🔍 Searching the web: \"{}\"", arg("query")),
        "check_page" => format!("🧪 Checking page {}", arg("url")),
        "definition" => format!("📍 Finding definition of {}", arg("name")),
        "references" => format!("🔗 Finding references to {}", arg("name")),
        "outline" => format!("🧭 Outlining {}", arg("path")),
        "rename_symbol" => format!("✒ Renaming {} → {}", arg("old"), arg("new")),
        "search" => {
            let repo = arg("repo");
            if repo.is_empty() {
                format!("🔎 Searching \"{}\"", arg("query"))
            } else {
                format!("🔎 Searching \"{}\" in {}", arg("query"), repo)
            }
        }
        "list_repos" => "🗂 Listing repositories".to_string(),
        "write_file" => format!("✍ Writing {}", arg("path")),
        "edit_file" => format!("✏ Editing {}", arg("path")),
        "run_command" => format!("▶ Running: {}", arg("command")),
        "git" => format!("🔀 git {}", arg("args")),
        "verify" => "✅ Verifying".to_string(),
        "open_url" => format!("🖥 Opening {}", arg("url")),
        "http_check" => format!("🩺 Checking {}", arg("url")),
        "start_app" => format!("🚀 Starting: {}", arg("command")),
        "app_logs" => format!("📋 Reading logs (pid {})", arg("pid")),
        "list_apps" => "📋 Listing running apps".to_string(),
        "stop_app" => format!("🛑 Stopping app {}", arg("pid")),
        "screenshot" => "📸 Taking a screenshot".to_string(),
        "install_tool" => format!("📦 Installing {}", arg("command")),
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
        "read_doc" => {
            let path = resolve_read_path(root, &arg("path"));
            match crate::office::read_document(&path) {
                Ok((text, kind)) => {
                    let via = match kind {
                        crate::office::DocKind::Text => "plain text",
                        crate::office::DocKind::Word => "Word document",
                        crate::office::DocKind::Excel => "spreadsheet",
                        crate::office::DocKind::Pdf => "PDF",
                        crate::office::DocKind::Rtf => "RTF",
                    };
                    if text.trim().is_empty() {
                        format!(
                            "({} read via {via}, but it contains no text)",
                            path.display()
                        )
                    } else {
                        cap(format!("[read via {via}]\n{text}"))
                    }
                }
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
        "remember" => {
            let note = arg("note");
            let category = arg("category");
            match crate::memory::remember(root, &category, &note) {
                Ok(true) => format!("Remembered ({}).", crate::memory::load_memory(root).len()),
                Ok(false) => "Already known (or empty) — not added.".to_string(),
                Err(err) => format!("error: {err}"),
            }
        }
        "http_get" => http_get(&arg("url")),
        "check_page" => {
            let url = arg("url");
            if url.trim().is_empty() {
                return "error: url is required".to_string();
            }
            let expects: Vec<String> = call
                .input
                .get("expect")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .or_else(|| {
                    call.input
                        .get("expect")
                        .and_then(|v| v.as_str())
                        .map(|s| vec![s.to_string()])
                })
                .unwrap_or_default();
            // Render with a real browser (post-JS); fall back to raw HTML.
            let (body, how) = match crate::browser::render_dom(&url) {
                Ok(dom) => (dom, "rendered (headless browser)"),
                Err(_) => (http_get(&url), "raw HTML (no browser installed)"),
            };
            if body.starts_with("error:") {
                return format!("Could not load {url}: {body}");
            }
            let missing = crate::browser::missing_content(&body, &expects);
            let mut out = if expects.is_empty() {
                format!("Loaded {url} — {how}, {} bytes.\n", body.len())
            } else if missing.is_empty() {
                format!(
                    "✅ PASS — {url} ({how}): all {} expected item(s) present.\n",
                    expects.len()
                )
            } else {
                format!(
                    "❌ FAIL — {url} ({how}): missing {}.\n",
                    missing
                        .iter()
                        .map(|m| format!("\"{m}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            // A little context to help the agent debug a failure.
            out.push_str(&tail(&strip_html_lite(&body), 800));
            cap(out)
        }
        "web_search" => match crate::websearch::web_search(&arg("query"), 8) {
            Ok(results) if results.is_empty() => {
                "No results found. Try a different query, or http_get a docs URL directly."
                    .to_string()
            }
            Ok(results) => {
                let mut out = String::new();
                for r in results {
                    out.push_str(&format!("{}\n{}\n", r.title, r.url));
                    if !r.snippet.is_empty() {
                        out.push_str(&format!("  {}\n", r.snippet));
                    }
                    out.push('\n');
                }
                cap(out)
            }
            Err(err) => format!("error: {err}"),
        },
        "definition" => {
            let name = arg("name");
            match crate::codenav::find_definitions(root, &name) {
                Ok(hits) if hits.is_empty() => {
                    format!("No definition found for \"{name}\".")
                }
                Ok(hits) => {
                    let mut out = String::new();
                    for h in hits {
                        out.push_str(&format!(
                            "{}:{}  ({})  {}\n",
                            h.path, h.line, h.kind, h.signature
                        ));
                    }
                    cap(out)
                }
                Err(err) => format!("error: {err}"),
            }
        }
        "references" => {
            let name = arg("name");
            match crate::codenav::find_references(root, &name, 200) {
                Ok(hits) if hits.is_empty() => format!("No references found for \"{name}\"."),
                Ok(hits) => {
                    let mut out = format!("{} reference(s):\n", hits.len());
                    for h in hits {
                        out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
                    }
                    cap(out)
                }
                Err(err) => format!("error: {err}"),
            }
        }
        "outline" => {
            let path = arg("path");
            match crate::codenav::outline(root, &path) {
                Ok(symbols) if symbols.is_empty() => {
                    format!("No symbols found in {path} (unsupported language or empty).")
                }
                Ok(symbols) => {
                    let mut out = String::new();
                    for s in symbols {
                        let container = s
                            .container
                            .as_deref()
                            .map(|c| format!(" in {c}"))
                            .unwrap_or_default();
                        out.push_str(&format!(
                            "{}:  {} {}{}\n",
                            s.line,
                            s.kind.as_str(),
                            s.name,
                            container
                        ));
                    }
                    cap(out)
                }
                Err(err) => format!("error: {err}"),
            }
        }
        "rename_symbol" => {
            let old = arg("old");
            let new = arg("new");
            if old.trim().is_empty() || new.trim().is_empty() {
                "error: both `old` and `new` are required".to_string()
            } else {
                match crate::codenav::rename_symbol(root, &old, &new) {
                    Ok(r) if r.occurrences == 0 => {
                        format!("No whole-word occurrences of \"{old}\" found — nothing renamed.")
                    }
                    Ok(r) => format!(
                        "Renamed \"{old}\" → \"{new}\": {} occurrence(s) across {} file(s):\n{}",
                        r.occurrences,
                        r.files_changed,
                        r.changed_paths.join("\n")
                    ),
                    Err(err) => format!("error: {err}"),
                }
            }
        }
        "search" => {
            let query = arg("query");
            if query.trim().is_empty() {
                "error: empty query".to_string()
            } else {
                let repo = arg("repo");
                let base = if repo.trim().is_empty() {
                    root.to_path_buf()
                } else {
                    match crate::repos::resolve_repo(root, &repo) {
                        Some(p) => p,
                        None => {
                            return format!(
                                "error: no repository named \"{repo}\" in this workspace (use \
                                 list_repos to see the available repositories)"
                            )
                        }
                    }
                };
                let scope = arg("path");
                let scope = if scope.trim().is_empty() {
                    None
                } else {
                    Some(scope)
                };
                search_project(&base, &query, scope.as_deref(), 200)
            }
        }
        "list_repos" => {
            let ws = crate::repos::load_workspace(root);
            let mut out = format!(
                "primary \"{}\": {}\n",
                repo_display_name(root),
                root.display()
            );
            for r in &ws.repos {
                out.push_str(&format!("repo \"{}\": {}\n", r.name, r.path));
            }
            if ws.repos.is_empty() {
                out.push_str(
                    "(no linked repositories — the user can link more in the Explorer, then \
                     search(repo=\"name\") reaches them)\n",
                );
            }
            out
        }
        "run_command" => {
            let command = arg("command");
            if command.trim().is_empty() {
                "error: empty command".to_string()
            } else if crate::syscap::is_long_running(&command) {
                format!(
                    "This looks like a long-running server/watcher (\"{command}\"). Not running \
                     it with run_command — that would block. Use start_app(\"{command}\") to run \
                     it in the background, then app_logs(pid) to read its output and http_get / \
                     open_url to check it."
                )
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
        "open_url" => crate::syscap::open_url(&arg("url")),
        "http_check" => crate::syscap::http_check(&arg("url"), 30),
        "start_app" => crate::syscap::start_app(root, &arg("command")),
        "app_logs" => match call.input.get("pid").and_then(|v| v.as_u64()) {
            Some(pid) => crate::syscap::app_logs(root, pid as u32),
            None => "error: pid must be an integer".to_string(),
        },
        "list_apps" => crate::syscap::list_apps(root),
        "stop_app" => match call.input.get("pid").and_then(|v| v.as_u64()) {
            Some(pid) => crate::syscap::stop_app(root, pid as u32),
            None => "error: pid must be an integer".to_string(),
        },
        "screenshot" => crate::syscap::take_screenshot(root),
        "install_tool" => {
            let package = arg("package");
            let package = if package.trim().is_empty() {
                None
            } else {
                Some(package)
            };
            crate::syscap::ensure_tool(&arg("command"), package.as_deref())
        }
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
        "edit_file" => match safe_join(root, &arg("path")) {
            Ok(full) => edit_file(&full, &arg("old"), &arg("new")),
            Err(err) => format!("error: {err}"),
        },
        "check_doc" => {
            let path = resolve_read_path(root, &arg("path"));
            let expects: Vec<String> = call
                .input
                .get("expect")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            match crate::office::read_document(&path) {
                Err(err) => format!("❌ FAIL — could not open {}: {err}", path.display()),
                Ok((text, _)) => {
                    let missing = crate::browser::missing_content(&text, &expects);
                    let words = text.split_whitespace().count();
                    if expects.is_empty() {
                        format!("Opened {} — {words} words.", path.display())
                    } else if missing.is_empty() {
                        format!(
                            "✅ PASS — {} ({words} words): all {} expected item(s) present.",
                            path.display(),
                            expects.len()
                        )
                    } else {
                        format!(
                            "❌ FAIL — {} ({words} words): missing {}. Fix the document and \
                             re-check.",
                            path.display(),
                            missing
                                .iter()
                                .map(|m| format!("\"{m}\""))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                }
            }
        }
        "write_doc" => match safe_join(root, &arg("path")) {
            Ok(full) => match crate::docwrite::write_docx(&full, &arg("markdown")) {
                Ok(()) => format!(
                    "wrote {} ({} bytes) — a real Word document",
                    full.display(),
                    std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0)
                ),
                Err(err) => format!("error: {err}"),
            },
            Err(err) => format!("error: {err}"),
        },
        "write_sheet" => match safe_join(root, &arg("path")) {
            Ok(full) => {
                // Either a list of sheets, or the single-sheet `data` form.
                let sheets: Vec<crate::docwrite::Sheet> = call
                    .input
                    .get("sheets")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .map(|s| crate::docwrite::Sheet {
                                name: s
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                data: s
                                    .get("data")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        vec![crate::docwrite::Sheet {
                            name: arg("sheet"),
                            data: arg("data"),
                        }]
                    });
                // An optional chart of the first sheet.
                let chart = call.input.get("chart").and_then(|c| {
                    let kind = crate::docwrite::ChartKind::parse(
                        c.get("type").and_then(|v| v.as_str()).unwrap_or("bar"),
                    )?;
                    Some(crate::docwrite::Chart {
                        kind,
                        title: c
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        // The model counts columns from 1; we index from 0.
                        value_column: c
                            .get("value_column")
                            .and_then(|v| v.as_u64())
                            .map(|n| n.saturating_sub(1) as usize)
                            .unwrap_or(1),
                    })
                });
                match crate::docwrite::write_workbook(&full, &sheets, chart.as_ref()) {
                    Ok(()) => format!(
                        "wrote {} ({} bytes) — a real Excel workbook with {} sheet(s){}",
                        full.display(),
                        std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0),
                        sheets.len(),
                        if chart.is_some() { " and a chart" } else { "" }
                    ),
                    Err(err) => format!("error: {err}"),
                }
            }
            Err(err) => format!("error: {err}"),
        },
        other => format!("error: unknown tool {other}"),
    }
}

/// Replace the unique occurrence of `old` with `new` in a file. Fails if `old`
/// is empty, missing, or appears more than once (so an edit is never ambiguous).
fn edit_file(path: &Path, old: &str, new: &str) -> String {
    if old.is_empty() {
        return "error: `old` text is empty".to_string();
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return format!("error: {err}"),
    };
    match text.matches(old).count() {
        0 => format!("error: `old` text was not found in {}", path.display()),
        1 => {
            let updated = text.replacen(old, new, 1);
            match std::fs::write(path, updated) {
                Ok(()) => format!("edited {}", path.display()),
                Err(err) => format!("error: {err}"),
            }
        }
        n => format!(
            "error: `old` text appears {n} times in {}; include more context so it is unique",
            path.display()
        ),
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

/// Strip HTML tags and collapse whitespace, for a readable text preview of a
/// fetched/rendered page.
fn strip_html_lite(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut last_ws = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !last_ws {
                    out.push(' ');
                    last_ws = true;
                }
            }
            c => {
                out.push(c);
                last_ws = false;
            }
        }
    }
    out.trim().to_string()
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

/// Run `command` in the project root, capturing its output and exit code
/// (killed after `timeout_secs`). Public entry point for UI-driven commands
/// such as running the affected tests.
pub fn run_shell_command(root: &Path, command: &str, timeout_secs: u64) -> String {
    run_shell(root, command, Duration::from_secs(timeout_secs))
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
/// The folder name of a repo root, for display.
fn repo_display_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string())
}

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

/// A review of the project's uncommitted changes, for the desktop Diff view.
#[derive(Debug, Default, Clone)]
pub struct GitReview {
    /// Whether the project is a git repository at all.
    pub is_repo: bool,
    /// Whether it has at least one commit (so a hard revert is possible).
    pub has_head: bool,
    /// A one-line summary, e.g. "3 file(s) changed".
    pub summary: String,
    /// The changed entries (`git status --porcelain` lines).
    pub files: Vec<String>,
    /// The changed file paths (project-relative), parsed from `files`.
    pub paths: Vec<String>,
    /// The unified working-tree diff (untracked files included).
    pub diff: String,
    /// Likely secrets found in the changed files.
    pub secrets: Vec<crate::secrets::SecretFinding>,
}

/// Run a git command in `root`, returning (success, stdout, stderr).
fn git_output(root: &Path, args: &[&str]) -> (bool, String, String) {
    match Command::new("git").args(args).current_dir(root).output() {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Err(err) => (false, String::new(), err.to_string()),
    }
}

/// Review the project's uncommitted changes (what the agent just did), as a
/// unified diff plus a changed-file list. Untracked files are included via an
/// intent-to-add so new files show up as additions.
pub fn git_review(root: &Path) -> GitReview {
    let (is_repo, _, _) = git_output(root, &["rev-parse", "--is-inside-work-tree"]);
    if !is_repo {
        return GitReview::default();
    }
    let (has_head, _, _) = git_output(root, &["rev-parse", "--verify", "HEAD"]);
    let _ = git_output(root, &["add", "-N", "--", "."]);
    let (_, diff, _) = git_output(root, &["--no-pager", "-c", "core.quotepath=false", "diff"]);
    let (_, status, _) = git_output(
        root,
        &["-c", "core.quotepath=false", "status", "--porcelain"],
    );
    let files: Vec<String> = status
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let summary = if files.is_empty() {
        "No uncommitted changes.".to_string()
    } else {
        format!("{} file(s) changed", files.len())
    };
    let paths: Vec<String> = files
        .iter()
        .map(|l| porcelain_path(l))
        .filter(|p| !p.is_empty())
        .collect();
    let secrets = crate::secrets::scan_secrets(root, &paths);
    GitReview {
        is_repo: true,
        has_head,
        summary,
        files,
        paths,
        diff,
        secrets,
    }
}

/// Extract the current (possibly partial) value of a string field from an
/// **incomplete** JSON object being streamed — e.g. pull the growing `contents`
/// out of a half-arrived `write_file` tool-call argument blob, so the UI can
/// show a file as it's written. Returns the decoded string so far, or `None` if
/// the field/opening quote hasn't streamed in yet.
pub fn partial_json_string_field(buf: &str, field: &str) -> Option<String> {
    json_string_field_state(buf, field).map(|(value, _)| value)
}

/// Like [`partial_json_string_field`], but also reports whether the value is
/// **final** — its closing quote has arrived. Callers that key off a value (a
/// filename, say) must wait for `true`, or a half-streamed value will be treated
/// as a real one. Models order JSON keys freely, so a field can still be
/// mid-flight after a later field has appeared.
pub fn json_string_field_state(buf: &str, field: &str) -> Option<(String, bool)> {
    let key = format!("\"{field}\"");
    let bytes = buf.as_bytes();
    let mut idx = buf.find(&key)? + key.len();
    // Skip whitespace, then the ':' , then whitespace, then the opening quote.
    while idx < bytes.len() && (bytes[idx] as char).is_whitespace() {
        idx += 1;
    }
    if idx >= bytes.len() || bytes[idx] != b':' {
        return None;
    }
    idx += 1;
    while idx < bytes.len() && (bytes[idx] as char).is_whitespace() {
        idx += 1;
    }
    if idx >= bytes.len() || bytes[idx] != b'"' {
        return None;
    }
    idx += 1;

    let chars: Vec<char> = buf[idx..].chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            return Some((out, true)); // closing quote reached — value is final
        }
        if c == '\\' {
            let Some(&e) = chars.get(i + 1) else {
                break; // dangling backslash at the stream's edge
            };
            match e {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0C}'),
                'u' => {
                    if i + 5 < chars.len() {
                        let hex: String = chars[i + 2..i + 6].iter().collect();
                        if let Ok(n) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(n) {
                                out.push(ch);
                            }
                        }
                        i += 6;
                        continue;
                    }
                    break; // incomplete \uXXXX at the edge
                }
                other => out.push(other),
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Some((out, false)) // still streaming: no closing quote yet
}

/// The added/removed line counts of a unified diff, ignoring the `+++`/`---`
/// file headers (so only real content lines are counted).
pub fn diff_line_stats(diff: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// Per-file added/removed line counts, keyed by the file's (project-relative)
/// path, parsed from the `diff --git a/… b/…` section headers.
pub fn diff_stats_by_file(diff: &str) -> std::collections::HashMap<String, (usize, usize)> {
    let mut stats: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    let mut current: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/path b/path" — take the b/ side (survives renames/deletes).
            current = rest
                .split(" b/")
                .nth(1)
                .map(|p| p.trim().trim_matches('"').to_string());
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(path) = &current {
            let entry = stats.entry(path.clone()).or_insert((0, 0));
            if line.starts_with('+') {
                entry.0 += 1;
            } else if line.starts_with('-') {
                entry.1 += 1;
            }
        }
    }
    stats
}

/// Extract the file path from a `git status --porcelain` line.
pub fn porcelain_path(line: &str) -> String {
    let rest = line.get(3..).unwrap_or("").trim();
    if let Some(idx) = rest.find("-> ") {
        rest[idx + 3..].trim().trim_matches('"').to_string()
    } else {
        rest.trim_matches('"').to_string()
    }
}

/// Initialize a git repository in `root`.
pub fn git_init(root: &Path) -> Result<(), String> {
    let (ok, _, err) = git_output(root, &["init"]);
    if ok {
        Ok(())
    } else {
        Err(err.trim().to_string())
    }
}

/// Stage everything and commit it (accept the agent's changes), using a
/// fallback identity so it works even when git has none configured.
pub fn git_commit_all(root: &Path, message: &str) -> Result<String, String> {
    let (ok, _, err) = git_output(root, &["add", "-A"]);
    if !ok {
        return Err(err.trim().to_string());
    }
    let (ok, out, err) = git_output(
        root,
        &[
            "-c",
            "user.name=Kestrel",
            "-c",
            "user.email=kestrel@local",
            "commit",
            "-m",
            message,
        ],
    );
    if ok {
        Ok(out.trim().to_string())
    } else {
        let msg = if err.trim().is_empty() { out } else { err };
        Err(msg.trim().to_string())
    }
}

/// A restore point in the project's history (a git commit).
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub id: String,
    pub summary: String,
    pub when: String,
}

/// Commit the current working tree as a checkpoint before an agent run, so the
/// run's changes are isolated and the whole run can be rolled back. Returns
/// `Ok(true)` if a checkpoint was made, `Ok(false)` if there was nothing to
/// checkpoint (clean tree, or not a git repo).
pub fn git_checkpoint(root: &Path, label: &str) -> Result<bool, String> {
    let (is_repo, _, _) = git_output(root, &["rev-parse", "--is-inside-work-tree"]);
    if !is_repo {
        return Ok(false);
    }
    let (_, status, _) = git_output(root, &["status", "--porcelain"]);
    if status.trim().is_empty() {
        return Ok(false);
    }
    let (ok, _, err) = git_output(root, &["add", "-A"]);
    if !ok {
        return Err(err.trim().to_string());
    }
    let message = format!("Kestrel checkpoint: {label}");
    let (ok, out, err) = git_output(
        root,
        &[
            "-c",
            "user.name=Kestrel",
            "-c",
            "user.email=kestrel@local",
            "commit",
            "-m",
            &message,
        ],
    );
    if ok {
        Ok(true)
    } else {
        let msg = if err.trim().is_empty() { out } else { err };
        Err(msg.trim().to_string())
    }
}

/// The most recent commits, as restore points for the Diff view.
pub fn git_log(root: &Path, limit: usize) -> Vec<Checkpoint> {
    let (ok, out, _) = git_output(
        root,
        &[
            "--no-pager",
            "log",
            "--format=%h\x1f%s\x1f%cr",
            &format!("-n{limit}"),
        ],
    );
    if !ok {
        return Vec::new();
    }
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some(Checkpoint {
                id: parts.next()?.to_string(),
                summary: parts.next()?.to_string(),
                when: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Reset the working tree to a specific commit and remove new files, rolling
/// back to that restore point.
pub fn git_restore(root: &Path, id: &str) -> Result<String, String> {
    let (ok, _, err) = git_output(root, &["reset", "--hard", id]);
    if !ok {
        return Err(err.trim().to_string());
    }
    let (ok, _, err) = git_output(root, &["clean", "-fd"]);
    if ok {
        Ok(format!("Restored to {id}."))
    } else {
        Err(err.trim().to_string())
    }
}

/// Discard all uncommitted changes, reverting the working tree to HEAD and
/// removing new files (reject the agent's changes). Requires a commit to exist.
pub fn git_revert_all(root: &Path) -> Result<String, String> {
    let (ok, _, err) = git_output(root, &["reset", "--hard", "HEAD"]);
    if !ok {
        return Err(err.trim().to_string());
    }
    let (ok, _, err) = git_output(root, &["clean", "-fd"]);
    if ok {
        Ok("Reverted to the last commit.".to_string())
    } else {
        Err(err.trim().to_string())
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

/// The result of an agent run plus the full conversation, so the caller can
/// keep it and let a follow-up prompt refine the same project.
pub struct AgentOutcome {
    pub result: Result<String, String>,
    pub history: Vec<AgentMessage>,
    pub usage: Usage,
    /// The agent paused without truly finishing — it hit the step budget or the
    /// user stopped it — and can be **continued**. This is a normal, resumable
    /// state, not a failure, so the UI offers "Continue" instead of an error.
    pub incomplete: bool,
}

/// A project's saved agent state: the tool-using conversation (so a follow-up
/// resumes it) and the chat transcript (so the UI restores what was said). Kept
/// per-project under `.kestrel/agent-session.json` so reopening a project days
/// later picks up exactly where it left off.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    #[serde(default)]
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub transcript: Vec<ChatMessage>,
    /// Project-relative paths the agent created/changed, so the UI can restore
    /// its file-preview history (contents are re-read from disk on load).
    #[serde(default)]
    pub created_files: Vec<String>,
}

/// The path to a project's saved agent session.
pub fn agent_session_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("agent-session.json")
}

/// Load a project's saved agent session, or a default (empty) one if absent.
pub fn load_agent_session(root: &Path) -> AgentSession {
    match std::fs::read_to_string(agent_session_path(root)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => AgentSession::default(),
    }
}

/// Persist a project's agent session under its `.kestrel/` directory.
pub fn save_agent_session(root: &Path, session: &AgentSession) -> std::io::Result<()> {
    let path = agent_session_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// The path to a project's agent audit log.
pub fn audit_log_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("audit.log")
}

/// Append one timestamped line to the project's audit log (best-effort).
fn append_audit(root: &Path, entry: &str) {
    let path = audit_log_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}  {entry}", utc_timestamp());
    }
}

/// A `YYYY-MM-DD HH:MM:SSZ` UTC timestamp, computed without a date dependency.
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}Z")
}

/// Convert a count of days since the Unix epoch to a civil (Y, M, D) date.
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// The first line of a tool result, capped, for the audit log.
fn audit_line(content: &str) -> String {
    content
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(140)
        .collect()
}

/// The approximate byte size of one conversation message.
fn message_bytes(message: &AgentMessage) -> usize {
    match message {
        AgentMessage::User(text) => text.len(),
        AgentMessage::Assistant { text, calls } => {
            text.len()
                + calls
                    .iter()
                    .map(|c| c.name.len() + c.input.to_string().len())
                    .sum::<usize>()
        }
        AgentMessage::ToolResults(results) => results.iter().map(|r| r.content.len()).sum(),
    }
}

fn history_bytes(history: &[AgentMessage]) -> usize {
    history.iter().map(message_bytes).sum()
}

/// A rough token estimate for a whole agent conversation (for the context gauge).
pub fn history_tokens(history: &[AgentMessage]) -> usize {
    history_bytes(history) / 4
}

/// Compact a conversation to roughly `target` bytes by dropping the *middle*
/// while keeping the first message (the original request) and the most recent
/// turns. Cuts only on whole "rounds" (an assistant tool-call message plus its
/// tool results stay together) so the tool_use/tool_result pairing the APIs
/// require is never broken. The project files on disk remain the source of
/// truth, so the agent can always re-read what it forgot.
fn compact_history(history: Vec<AgentMessage>, target: usize) -> Vec<AgentMessage> {
    // Group into rounds: an assistant-with-calls message pairs with the
    // following tool-results message; everything else is its own round.
    let mut rounds: Vec<Vec<AgentMessage>> = Vec::new();
    let mut it = history.into_iter().peekable();
    while let Some(message) = it.next() {
        let has_calls =
            matches!(&message, AgentMessage::Assistant { calls, .. } if !calls.is_empty());
        let mut round = vec![message];
        if has_calls && matches!(it.peek(), Some(AgentMessage::ToolResults(_))) {
            round.push(it.next().unwrap());
        }
        rounds.push(round);
    }
    if rounds.is_empty() {
        return Vec::new();
    }

    let first = rounds.remove(0);
    let mut total: usize = first.iter().map(message_bytes).sum();
    let mut kept_recent: Vec<Vec<AgentMessage>> = Vec::new();
    for round in rounds.into_iter().rev() {
        let size: usize = round.iter().map(message_bytes).sum();
        if total + size > target && !kept_recent.is_empty() {
            break;
        }
        total += size;
        kept_recent.push(round);
    }
    kept_recent.reverse();

    let mut out = first;
    for round in kept_recent {
        out.extend(round);
    }
    out
}

/// The review instruction injected once the agent first thinks it is done.
fn review_prompt(request: &str) -> String {
    format!(
        "Now do a rigorous self-review before you finish. Re-check everything you just did \
         against the ORIGINAL request:\n\n\"{request}\"\n\n\
         Use read_file, search, and `git diff` to inspect your own changes, and run the build \
         (run_command) or verify() to catch errors. Look specifically for: compile/type errors, \
         missing or broken imports, unmet requirements, inconsistent data, and any unrelated \
         code you may have damaged. Fix every problem you find with write_file, then re-verify. \
         If everything is already correct and builds cleanly, briefly confirm that — do not make \
         needless changes."
    )
}

/// Run the tool-using agent loop until the model stops calling tools or the
/// step limit is hit. `history` is the prior conversation (empty for a fresh
/// build; carried across builds so follow-ups refine the same project); the
/// updated conversation is returned in the [`AgentOutcome`]. When `review` is
/// set, the first time the model believes it is done it is asked to critique
/// and fix its own work before finishing. `on_event` reports progress live.
#[allow(clippy::too_many_arguments)]
/// Run one agent turn with streaming, translating live tool-argument deltas into
/// [`AgentEvent::Writing`] so the UI shows files as they're typed. Falls back to a
/// buffered [`run_turn`] if the provider can't stream (transport error or an
/// empty stream), so no provider is worse off than before.
fn streamed_turn(
    config: &ProviderConfig,
    model: &str,
    system: &str,
    history: &[AgentMessage],
    tools: &[ToolSpec],
    on_event: &mut dyn FnMut(AgentEvent),
) -> std::io::Result<Result<TurnResult, String>> {
    // Throttle live previews: only re-emit once a file's streamed contents have
    // grown by a chunk, so we don't flood the channel with tiny deltas.
    let mut last_len: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let stream = run_turn_streaming(config, model, 8192, Some(system), history, tools, |ev| {
        let TurnEvent::ToolProgress { name, args } = ev else {
            return;
        };
        let field = match name {
            "write_file" => "contents",
            "edit_file" => "new",
            _ => return,
        };
        // Only preview once the body field has started streaming…
        let Some(contents) = partial_json_string_field(args, field) else {
            return;
        };
        // …and only once `path` is FINAL. Models order keys freely — several
        // stream `contents` before `path` — so a still-streaming filename would
        // otherwise register as its own phantom file ("EC", "ECAD", "ECADEL"…).
        let Some((path, path_final)) = json_string_field_state(args, "path") else {
            return;
        };
        if !path_final || path.is_empty() {
            return;
        }
        let entry = last_len.entry(path.clone()).or_insert(0);
        if contents.len() >= *entry + 24 || contents.len() < *entry {
            *entry = contents.len();
            on_event(AgentEvent::Writing { path, contents });
        }
    });
    match stream {
        // A stream that produced nothing usable → fall back to a buffered turn.
        Ok(Ok(turn))
            if turn.calls.is_empty()
                && turn.text.trim().is_empty()
                && turn.stop_reason.is_empty() =>
        {
            run_turn(config, model, 8192, Some(system), history, tools)
        }
        Ok(result) => Ok(result),
        // Transport failure after retries → fall back to the buffered path.
        Err(_) => run_turn(config, model, 8192, Some(system), history, tools),
    }
}

/// Run the tool-using agent loop for `user_prompt`. Thin public wrapper over
/// [`run_agent_inner`] at depth 0 (sub-agents recurse at deeper levels).
#[allow(clippy::too_many_arguments)]
pub fn run_agent(
    config: &ProviderConfig,
    model: &str,
    user_prompt: &str,
    root: &Path,
    max_steps: usize,
    review: bool,
    policy: &crate::policy::Policy,
    cancel: &std::sync::atomic::AtomicBool,
    profile: Profile,
    history: Vec<AgentMessage>,
    mut on_event: impl FnMut(AgentEvent),
    mut approve: impl FnMut(&ToolCall) -> bool,
) -> AgentOutcome {
    run_agent_inner(
        0,
        config,
        model,
        user_prompt,
        root,
        max_steps,
        review,
        policy,
        cancel,
        profile,
        history,
        &mut on_event,
        &mut approve,
    )
}

/// The agent loop. `depth` is the sub-agent nesting level (0 = top). Closures are
/// `&mut dyn` (not generic) so the loop can recurse for sub-agents without
/// infinite monomorphization.
#[allow(clippy::too_many_arguments)]
fn run_agent_inner(
    depth: usize,
    config: &ProviderConfig,
    model: &str,
    user_prompt: &str,
    root: &Path,
    max_steps: usize,
    review: bool,
    policy: &crate::policy::Policy,
    cancel: &std::sync::atomic::AtomicBool,
    profile: Profile,
    mut history: Vec<AgentMessage>,
    on_event: &mut dyn FnMut(AgentEvent),
    approve: &mut dyn FnMut(&ToolCall) -> bool,
) -> AgentOutcome {
    use std::sync::atomic::Ordering;
    let system = system_prompt_for(profile, root);
    let tools = tools_for(profile);
    history.push(AgentMessage::User(user_prompt.to_string()));
    let mut reviewed = !review;
    let mut total_usage = Usage::default();
    // The task plan — only the top-level agent owns the shared ledger; sub-agents
    // keep a private, in-memory plan so they can't clobber it.
    let mut plan = if depth == 0 {
        crate::plan::load_plan(root)
    } else {
        crate::plan::Plan::default()
    };
    if depth == 0 && !plan.steps.is_empty() {
        on_event(AgentEvent::Plan(plan.clone()));
    }
    let mut plan_reflected = false;
    // Stall detection: the previous turn's tool signature, how many times it has
    // repeated, and a bounded nudge budget so we never loop on nudging itself.
    let mut last_sig: Option<u64> = None;
    let mut repeats = 0usize;
    let mut nudges = 0usize;
    // Token-aware compaction: trigger at ~70% of the model's context window
    // (≈4 bytes/token), compacting back to ~40%.
    let window = crate::model_context_window(model) as usize;
    let compact_trigger = window * 4 * 7 / 10;
    let compact_target = window * 4 * 4 / 10;
    append_audit(
        root,
        &format!("RUN  {}", user_prompt.chars().take(140).collect::<String>()),
    );

    for _ in 0..max_steps {
        // Cooperative cancellation: the user stopped the run. Return the work so
        // far as a resumable pause, not a failure.
        if cancel.load(Ordering::Relaxed) {
            append_audit(root, "END  stopped by user");
            return AgentOutcome {
                result: Ok(
                    "⏹ Stopped at your request. Everything written so far is on disk — \
                            click Continue to pick up where this left off."
                        .to_string(),
                ),
                history,
                usage: total_usage,
                incomplete: true,
            };
        }
        // Keep the conversation affordable: drop the middle of a long history
        // before sending it, preserving the request and the recent turns.
        if history_bytes(&history) > compact_trigger {
            history = compact_history(std::mem::take(&mut history), compact_target);
        }
        // Stream the turn so the UI can show files being written in real time.
        // Falls back to a plain (buffered) turn if the provider can't stream.
        let turn: TurnResult =
            match streamed_turn(config, model, &system, &history, &tools, on_event) {
                Ok(Ok(turn)) => turn,
                Ok(Err(msg)) => {
                    return AgentOutcome {
                        result: Err(msg),
                        history,
                        usage: total_usage,
                        incomplete: false,
                    }
                }
                Err(err) => {
                    return AgentOutcome {
                        result: Err(err.to_string()),
                        history,
                        usage: total_usage,
                        incomplete: false,
                    }
                }
            };
        total_usage.add(&turn.usage);
        on_event(AgentEvent::Usage(turn.usage));
        if !turn.text.trim().is_empty() {
            on_event(AgentEvent::Assistant(turn.text.clone()));
        }

        if turn.calls.is_empty() {
            if !reviewed {
                reviewed = true;
                let final_text = if turn.text.trim().is_empty() {
                    "Done.".to_string()
                } else {
                    turn.text.clone()
                };
                history.push(AgentMessage::Assistant {
                    text: final_text,
                    calls: Vec::new(),
                });
                on_event(AgentEvent::Assistant(
                    "— reviewing my work against the request —".to_string(),
                ));
                history.push(AgentMessage::User(review_prompt(user_prompt)));
                continue;
            }
            // Plan-aware reflect: don't declare victory while the plan still has
            // outstanding steps — send the agent back to finish or justify them.
            if !plan_reflected && !plan.outstanding().is_empty() {
                plan_reflected = true;
                let outstanding = plan.outstanding().join("; ");
                history.push(AgentMessage::Assistant {
                    text: turn.text.clone(),
                    calls: Vec::new(),
                });
                on_event(AgentEvent::Assistant(
                    "— checking the plan before finishing —".to_string(),
                ));
                history.push(AgentMessage::User(format!(
                    "Before you finish: your plan still lists these steps as not done: \
                     {outstanding}. If they are actually needed, do them now. If they are already \
                     done or genuinely unnecessary, call update_plan to reflect that and briefly \
                     say why. Only then wrap up."
                )));
                continue;
            }
            append_audit(root, "END  finished");
            return AgentOutcome {
                result: Ok(turn.text),
                history,
                usage: total_usage,
                incomplete: false,
            };
        }

        // Stall detection: a signature of this turn's tool calls. If the agent
        // repeats the exact same calls turn after turn, it's stuck.
        let sig = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for c in &turn.calls {
                c.name.hash(&mut h);
                c.input.to_string().hash(&mut h);
            }
            h.finish()
        };
        let stalled = if Some(sig) == last_sig {
            repeats += 1;
            repeats >= 2 && nudges < 3
        } else {
            repeats = 0;
            false
        };
        last_sig = Some(sig);

        history.push(AgentMessage::Assistant {
            text: turn.text.clone(),
            calls: turn.calls.clone(),
        });
        let mut results = Vec::new();
        for call in &turn.calls {
            // The plan tool is handled here (not in execute_tool) so it can update
            // the live ledger and persist. It is always safe — no policy/approval.
            // Sub-agents keep a private plan (no persist/emit) to avoid clobbering.
            let content = if call.name == "update_plan" {
                plan = crate::plan::plan_from_tool_input(&call.input);
                let (done, total) = plan.progress();
                if depth == 0 {
                    let _ = crate::plan::save_plan(root, &plan);
                    on_event(AgentEvent::Plan(plan.clone()));
                    append_audit(root, &format!("PLAN  {done}/{total} steps done"));
                }
                format!("Plan updated ({done}/{total} done).\n{}", plan.render())
            }
            // Delegate a focused sub-task to a fresh sub-agent with its own clean
            // context. Only the top-level agent may spawn (no unbounded nesting).
            else if call.name == "spawn_subagent" {
                let task = call
                    .input
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if depth >= 1 {
                    "error: a sub-agent cannot spawn further sub-agents. Do this work directly."
                        .to_string()
                } else if task.is_empty() {
                    "error: provide a `task` describing the self-contained work to delegate."
                        .to_string()
                } else {
                    on_event(AgentEvent::Tool(format!("🤝 Delegating: {task}")));
                    append_audit(root, &format!("SUBAGENT  {task}"));
                    let sub = run_agent_inner(
                        depth + 1,
                        config,
                        model,
                        &task,
                        root,
                        60,
                        false,
                        policy,
                        cancel,
                        profile,
                        Vec::new(),
                        &mut |ev| match ev {
                            AgentEvent::Assistant(t) => {
                                on_event(AgentEvent::Assistant(format!("  ↳ {t}")))
                            }
                            other => on_event(other),
                        },
                        approve,
                    );
                    total_usage.add(&sub.usage);
                    match sub.result {
                        Ok(summary) => format!("Sub-agent finished:\n{summary}"),
                        Err(err) => format!("Sub-agent could not finish: {err}"),
                    }
                }
            }
            // Policy gate: a denied call is not executed; the model sees why.
            else if let Err(reason) = policy.check(call) {
                on_event(AgentEvent::Tool(format!(
                    "⛔ Blocked: {}",
                    describe_call(call)
                )));
                append_audit(
                    root,
                    &format!("BLOCKED  {}  ({reason})", describe_call(call)),
                );
                format!("error: {reason}")
            } else if !approve(call) {
                // Permission gate: the user declined this action at the prompt.
                // Feed that back so the model adapts instead of the run dying.
                on_event(AgentEvent::Tool(format!(
                    "🚫 Declined: {}",
                    describe_call(call)
                )));
                append_audit(root, &format!("DECLINED  {}", describe_call(call)));
                "error: the user declined to allow this action. Do not retry it; find another \
                 approach or ask them what to do."
                    .to_string()
            } else if call.name == "write_file" || call.name == "edit_file" {
                let path = call
                    .input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let out = execute_tool(root, call);
                // Reflect the file as it now is on disk in the live preview
                // (write_file and edit_file both change it).
                let contents = safe_join(root, &path)
                    .ok()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                on_event(AgentEvent::Wrote { path, contents });
                out
            } else {
                on_event(AgentEvent::Tool(describe_call(call)));
                let out = execute_tool(root, call);
                // Surface build/verify results in the transcript, not just to
                // the model, so the user can watch it check its own work.
                if matches!(
                    call.name.as_str(),
                    "run_command"
                        | "verify"
                        | "git"
                        | "install_tool"
                        | "start_app"
                        | "app_logs"
                        | "list_apps"
                        | "stop_app"
                        | "open_url"
                        | "http_check"
                        | "screenshot"
                ) {
                    on_event(AgentEvent::Assistant(tail(&out, 1200)));
                }
                out
            };
            append_audit(
                root,
                &format!("{}  →  {}", describe_call(call), audit_line(&content)),
            );
            results.push(ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                content,
            });
        }
        history.push(AgentMessage::ToolResults(results));

        // Stalled: the agent repeated the same actions with no progress. Nudge it
        // to change tack (bounded, so nudging can't itself loop).
        if stalled {
            nudges += 1;
            repeats = 0;
            append_audit(root, "STALL  nudged to change approach");
            on_event(AgentEvent::Assistant(
                "⚠ You seem to be repeating yourself — trying a different approach.".to_string(),
            ));
            history.push(AgentMessage::User(
                "You've repeated the same action several times without making progress. Stop \
                 repeating it. Re-read your plan, try a DIFFERENT approach to the current step \
                 (inspect the actual error, read the relevant file, or revise the plan). If you \
                 are genuinely blocked, stop and explain clearly what is blocking you."
                    .to_string(),
            ));
        }
    }
    append_audit(root, "END  step limit reached");
    AgentOutcome {
        result: Ok(format!(
            "⏸ I've paused after {max_steps} steps — this is a big task and I haven't fully \
             finished. Everything written so far is on disk and verified as far as I got. Click \
             **Continue** to keep going from exactly here, or start a New chat."
        )),
        history,
        usage: total_usage,
        incomplete: true,
    }
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
    fn checkpoint_log_and_restore_round_trip() {
        let dir = std::env::temp_dir().join(format!("kestrel-ckpt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A throwaway repo.
        let init = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !init(&["init"]) {
            return; // git not available; skip
        }
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        // First checkpoint captures the initial state.
        assert!(git_checkpoint(&dir, "first").unwrap());
        let first = git_log(&dir, 5);
        assert!(!first.is_empty());
        let first_id = first[0].id.clone();

        // Change and checkpoint again.
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        assert!(git_checkpoint(&dir, "second").unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two");

        // Restore the first checkpoint.
        git_restore(&dir, &first_id).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one");

        // A clean tree needs no checkpoint.
        assert!(!git_checkpoint(&dir, "noop").unwrap());
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
    fn edit_file_replaces_unique_text_and_rejects_ambiguity() {
        let dir = std::env::temp_dir().join(format!("kestrel-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello world").unwrap();

        let ok = execute_tool(
            &dir,
            &ToolCall {
                id: "1".to_string(),
                name: "edit_file".to_string(),
                input: serde_json::json!({"path": "a.txt", "old": "world", "new": "kestrel"}),
            },
        );
        assert!(ok.starts_with("edited"), "got: {ok}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "hello kestrel"
        );

        std::fs::write(dir.join("b.txt"), "x x x").unwrap();
        let ambiguous = execute_tool(
            &dir,
            &ToolCall {
                id: "2".to_string(),
                name: "edit_file".to_string(),
                input: serde_json::json!({"path": "b.txt", "old": "x", "new": "y"}),
            },
        );
        assert!(ambiguous.contains("appears 3 times"), "got: {ambiguous}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_keeps_first_and_recent_and_shrinks() {
        let mut history = vec![AgentMessage::User("original request".to_string())];
        for i in 0..40 {
            history.push(AgentMessage::Assistant {
                text: format!("turn {i}"),
                calls: vec![ToolCall {
                    id: i.to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": format!("f{i}"), "contents": "x".repeat(20_000) }),
                }],
            });
            history.push(AgentMessage::ToolResults(vec![ToolResult {
                id: i.to_string(),
                name: "write_file".to_string(),
                content: "wrote".to_string(),
            }]));
        }
        let before = history_bytes(&history);
        let compact = compact_history(history, 100_000);
        let after = history_bytes(&compact);
        assert!(after < before);
        assert!(after <= 130_000, "after was {after}");
        assert!(
            matches!(&compact[0], AgentMessage::User(s) if s == "original request"),
            "first message must be the original request"
        );
    }

    #[test]
    fn agent_session_round_trips() {
        let dir = std::env::temp_dir().join(format!("kestrel-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = AgentSession {
            messages: vec![AgentMessage::User("build it".to_string())],
            transcript: vec![ChatMessage::user("build it")],
            created_files: vec!["src/main.rs".to_string()],
        };
        save_agent_session(&dir, &session).unwrap();
        let loaded = load_agent_session(&dir);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.transcript.len(), 1);
        assert_eq!(loaded.created_files, vec!["src/main.rs".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_repo_search_and_list() {
        let base = std::env::temp_dir().join(format!("kestrel-mr-{}", std::process::id()));
        let primary = base.join("app");
        let lib = base.join("shared-lib");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("token.rs"), "fn mint_UNIQUEXYZ_token() {}").unwrap();
        crate::repos::link_repo(&primary, &lib).unwrap();

        // list_repos names both the primary and the linked repo.
        let listed = execute_tool(
            &primary,
            &ToolCall {
                id: "1".into(),
                name: "list_repos".into(),
                input: serde_json::json!({}),
            },
        );
        assert!(listed.contains("shared-lib"), "list_repos: {listed}");
        assert!(listed.contains("app"), "list_repos: {listed}");

        // A repo-scoped search finds text that lives only in the linked repo.
        let hit = execute_tool(
            &primary,
            &ToolCall {
                id: "2".into(),
                name: "search".into(),
                input: serde_json::json!({ "query": "UNIQUEXYZ", "repo": "shared-lib" }),
            },
        );
        assert!(hit.contains("token.rs"), "cross-repo search: {hit}");

        // The same search scoped to the primary finds nothing there.
        let miss = execute_tool(
            &primary,
            &ToolCall {
                id: "3".into(),
                name: "search".into(),
                input: serde_json::json!({ "query": "UNIQUEXYZ" }),
            },
        );
        assert!(!miss.contains("token.rs"), "primary search: {miss}");

        // An unknown repo name is an error, not a silent empty search.
        let err = execute_tool(
            &primary,
            &ToolCall {
                id: "4".into(),
                name: "search".into(),
                input: serde_json::json!({ "query": "x", "repo": "ghost" }),
            },
        );
        assert!(err.starts_with("error:"), "unknown repo: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn work_profile_gets_its_own_tool_pack() {
        let build: Vec<String> = tools_for(Profile::Build)
            .into_iter()
            .map(|t| t.name)
            .collect();
        let work: Vec<String> = tools_for(Profile::Work)
            .into_iter()
            .map(|t| t.name)
            .collect();
        // Build keeps everything, including the code-specific tools.
        assert!(build.contains(&"git".to_string()));
        assert!(build.contains(&"rename_symbol".to_string()));
        assert!(build.len() > work.len());
        // Work keeps autonomy, files, and research…
        for expected in [
            "update_plan",
            "remember",
            "write_file",
            "web_search",
            "check_page",
        ] {
            assert!(
                work.contains(&expected.to_string()),
                "work missing {expected}"
            );
        }
        // …but not the coding toolchain.
        for absent in [
            "git",
            "verify",
            "install_tool",
            "start_app",
            "rename_symbol",
        ] {
            assert!(
                !work.contains(&absent.to_string()),
                "work should not have {absent}"
            );
        }
    }

    #[test]
    fn work_prompt_is_work_shaped_not_code_shaped() {
        let dir = std::env::temp_dir();
        let work = work_system_prompt(&dir);
        assert!(work.contains("Kestrel Work"));
        assert!(work.contains("update_plan"));
        // It must insist on real files and sourced claims.
        assert!(work.to_lowercase().contains("write_file"));
        assert!(work.to_lowercase().contains("cite") || work.to_lowercase().contains("sourced"));
        // And it is not the coding prompt.
        assert!(!work.contains("cargo test"));
    }

    #[test]
    fn partial_json_field_extracts_streaming_contents() {
        // Complete value.
        assert_eq!(
            partial_json_string_field(r#"{"path":"a.txt","contents":"hello"}"#, "contents"),
            Some("hello".to_string())
        );
        // Partial value (stream cut off before the closing quote).
        assert_eq!(
            partial_json_string_field(r#"{"path":"a.txt","contents":"line1\nline"#, "contents"),
            Some("line1\nline".to_string())
        );
        // Escapes decode; a dangling backslash at the edge is dropped.
        assert_eq!(
            partial_json_string_field(r#"{"contents":"a\tb\"c\"#, "contents"),
            Some("a\tb\"c".to_string())
        );
        // Path streams in first and stays stable.
        assert_eq!(
            partial_json_string_field(r#"{"path":"src/main.rs","cont"#, "path"),
            Some("src/main.rs".to_string())
        );
        // Field not present yet.
        assert_eq!(partial_json_string_field(r#"{"path":"#, "contents"), None);
    }

    #[test]
    fn a_half_streamed_path_is_reported_as_not_final() {
        // Regression: some models stream `contents` BEFORE `path`. A partial
        // filename must never be treated as a real file, or the UI fills with
        // phantoms ("EC", "ECAD", "ECADEL"…).
        let mid = r##"{"contents":"# Report\n\nbody","path":"ECADEL"##;
        let (value, is_final) = json_string_field_state(mid, "path").unwrap();
        assert_eq!(value, "ECADEL");
        assert!(!is_final, "a path still streaming must not be final");

        let done = r##"{"contents":"# Report","path":"ECADEL_Profile.md"}"##;
        let (value, is_final) = json_string_field_state(done, "path").unwrap();
        assert_eq!(value, "ECADEL_Profile.md");
        assert!(is_final, "a closed path must be final");
    }

    #[test]
    fn diff_stats_count_added_and_removed() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n\
                    index 1..2 100644\n\
                    --- a/src/lib.rs\n\
                    +++ b/src/lib.rs\n\
                    @@ -1,2 +1,3 @@\n\
                    -old line\n\
                    +new line\n\
                    +another new line\n\
                    diff --git a/README.md b/README.md\n\
                    --- a/README.md\n\
                    +++ b/README.md\n\
                    @@ -0,0 +1 @@\n\
                    +hello\n";
        assert_eq!(diff_line_stats(diff), (3, 1));
        let by_file = diff_stats_by_file(diff);
        assert_eq!(by_file.get("src/lib.rs"), Some(&(2, 1)));
        assert_eq!(by_file.get("README.md"), Some(&(1, 0)));
    }

    #[test]
    fn describe_call_is_human_readable() {
        let call = ToolCall {
            id: "1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "src/main.rs"}),
        };
        assert_eq!(describe_call(&call), "📖 Reading src/main.rs");
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
