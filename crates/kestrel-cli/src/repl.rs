//! The interactive session — `kestrel` with no arguments.
//!
//! This is the same engine the desktop app drives: `run_agent`, with its event
//! stream rendered to the terminal, its cancel token wired to Ctrl-C, and its
//! permission gate asking on stdin. Nothing about the agent is GUI-specific,
//! so the CLI is a renderer and a loop rather than a second implementation.

use crate::render::Renderer;
use crate::term::Term;
use kestrel_core::{AgentMessage, Profile, ToolCall};
use rustyline::error::ReadlineError;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A live session: where it is working, which profile, and its conversation.
pub struct Session {
    root: PathBuf,
    profile: Profile,
    history: Vec<AgentMessage>,
    settings: kestrel_core::Settings,
    /// Set when the last run paused and can be continued.
    incomplete: bool,
    /// Skip permission prompts for the rest of the session.
    allow_all: bool,
    cost: f64,
}

/// Start the interactive session in `root`.
pub fn run(root: PathBuf) -> std::io::Result<()> {
    let settings = kestrel_core::load_settings();
    let mut session = Session {
        history: kestrel_core::load_agent_session(&root).messages,
        root,
        profile: Profile::Build,
        settings,
        incomplete: false,
        allow_all: false,
        cost: 0.0,
    };

    let mut term = Term::new();
    banner(&mut term, &session);

    let mut editor = match rustyline::DefaultEditor::new() {
        Ok(editor) => editor,
        Err(err) => {
            eprintln!("could not start the interactive prompt: {err}");
            return Ok(());
        }
    };
    let history_file = history_path();
    if let Some(parent) = history_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = editor.load_history(&history_file);

    loop {
        let prompt = if session.profile == Profile::Work {
            "work › "
        } else {
            "› "
        };
        match editor.readline(prompt) {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line.as_str());
                if line.starts_with('/') {
                    if command(&mut session, &mut term, &line) == Flow::Quit {
                        break;
                    }
                    continue;
                }
                turn(&mut session, &line);
            }
            // Ctrl-C at the prompt clears the line; Ctrl-D exits.
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("input error: {err}");
                break;
            }
        }
    }
    let _ = editor.save_history(&history_file);
    save(&session);
    println!("bye.");
    Ok(())
}

/// Whether the loop should keep going.
#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// Run one agent turn for `prompt`, rendering as it goes.
fn turn(session: &mut Session, prompt: &str) {
    let Some(provider) = session.settings.active().cloned() else {
        eprintln!("No active provider. Configure one in the desktop app's Settings first.");
        return;
    };
    if provider.api_key.trim().is_empty() {
        eprintln!("The active provider has no API key.");
        return;
    }

    // Ctrl-C during a run cancels the agent rather than killing the process.
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    // A handler may already be installed from an earlier turn; that's fine.
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed));

    let policy = kestrel_core::effective_policy(&session.root, &session.settings.policy);
    // Both callbacks need the renderer, and the agent calls them one at a time,
    // so a RefCell is the honest way to share it.
    let renderer = std::cell::RefCell::new(Renderer::new());
    renderer.borrow_mut().redraw_status();

    let root = session.root.clone();
    let profile = session.profile;
    let price = kestrel_core::model_price(&provider.model);
    let mut turn_cost = 0.0f64;
    let mut allow_all = session.allow_all;

    let outcome = kestrel_core::run_agent(
        &provider.to_config(),
        &provider.model,
        prompt,
        &root,
        250,
        true,
        &policy,
        &cancel,
        profile,
        Vec::new(),
        std::mem::take(&mut session.history),
        |event| {
            if let kestrel_core::AgentEvent::Usage(usage) = &event {
                // An unpriced model still runs; the meter just stays at zero
                // rather than inventing a number.
                if let Some(price) = price {
                    turn_cost += kestrel_core::cost_of_usage(price, usage);
                }
            }
            renderer.borrow_mut().event(event);
        },
        |call| approve(&mut renderer.borrow_mut(), call, &mut allow_all),
    );

    session.allow_all = allow_all;
    session.cost += turn_cost;
    session.history = outcome.history;
    session.incomplete = outcome.incomplete;
    let mut renderer = renderer.into_inner();
    match &outcome.result {
        Ok(text) => {
            renderer.summary(true, outcome.incomplete, turn_cost);
            if outcome.incomplete && !text.is_empty() {
                // The pause message is the useful part; the rest already showed.
                println!("  {text}");
            }
        }
        Err(err) => {
            renderer.summary(false, false, turn_cost);
            eprintln!("  {err}");
        }
    }
    save(session);
}

/// Ask on stdin before a system-touching action. Sending mail always asks.
fn approve(renderer: &mut Renderer, call: &ToolCall, allow_all: &mut bool) -> bool {
    let always = call.name == "send_mail";
    let gated = matches!(
        call.name.as_str(),
        "run_command" | "install_tool" | "git" | "start_app" | "stop_app" | "send_mail"
    );
    if !gated || (*allow_all && !always) {
        return true;
    }
    renderer.term.finish_status();
    let s = renderer.term.style;
    println!(
        "\n  {} {}",
        s.yellow("permission"),
        s.bold(&kestrel_core::describe_call(call))
    );
    print!("  {} ", s.dim("allow? [y]es / [n]o / [a]ll ›"));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "" => true,
        "a" | "all" if !always => {
            *allow_all = true;
            true
        }
        "a" | "all" => true,
        _ => false,
    }
}

/// Handle a `/command`.
fn command(session: &mut Session, term: &mut Term, line: &str) -> Flow {
    let mut parts = line.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    let s = term.style;
    match name {
        "/exit" | "/quit" | "/q" => return Flow::Quit,
        "/help" | "/?" => help(term),
        "/clear" | "/new" => {
            session.history.clear();
            session.incomplete = false;
            kestrel_core::clear_plan(&session.root);
            save(session);
            term.line(&s.dim("  conversation cleared"));
        }
        "/work" => {
            session.profile = Profile::Work;
            term.line(&s.accent("  switched to Kestrel Work — documents, data, research"));
        }
        "/build" => {
            session.profile = Profile::Build;
            term.line(&s.accent("  switched to Kestrel Build — coding"));
        }
        "/continue" => {
            if session.incomplete {
                turn(session, "Continue from where you left off.");
            } else {
                term.line(&s.dim("  nothing paused to continue"));
            }
        }
        "/plan" => {
            let plan = kestrel_core::load_plan(&session.root);
            if plan.steps.is_empty() {
                term.line(&s.dim("  no plan yet"));
            } else {
                let (done, total) = plan.progress();
                term.line(&format!(
                    "  {} {} {done}/{total}",
                    s.accent("🗺"),
                    crate::render::progress_bar(done, total)
                ));
                for step in &plan.steps {
                    let (glyph, title) = match step.status {
                        kestrel_core::StepStatus::Done => (s.green("✔"), s.dim(&step.title)),
                        kestrel_core::StepStatus::Active => (s.accent("▶"), s.bold(&step.title)),
                        kestrel_core::StepStatus::Todo => (s.dim("·"), step.title.clone()),
                    };
                    term.line(&format!("    {glyph} {title}"));
                }
            }
        }
        "/cost" => {
            term.line(&format!(
                "  {} this session",
                s.bold(&format!("${:.4}", session.cost))
            ));
        }
        "/model" => {
            if rest.is_empty() {
                let current = session
                    .settings
                    .active()
                    .map(|p| p.model.clone())
                    .unwrap_or_default();
                term.line(&format!("  model: {}", s.bold(&current)));
                if let Some(p) = session.settings.active() {
                    let names = kestrel_core::model_suggestions_for(p).join(", ");
                    term.line(&format!("  {}", s.dim(&names)));
                }
            } else if let Some(active) = session.settings.active_provider.clone() {
                if let Some(p) = session.settings.providers.get_mut(&active) {
                    p.model = rest.to_string();
                    let _ = kestrel_core::save_settings(&session.settings);
                    term.line(&format!("  model → {}", s.bold(rest)));
                }
            }
        }
        "/provider" => {
            if rest.is_empty() {
                for name in session.settings.providers.keys() {
                    let marker = if Some(name) == session.settings.active_provider.as_ref() {
                        s.accent("●")
                    } else {
                        s.dim("○")
                    };
                    term.line(&format!("  {marker} {name}"));
                }
            } else if session.settings.providers.contains_key(rest) {
                session.settings.active_provider = Some(rest.to_string());
                let _ = kestrel_core::save_settings(&session.settings);
                term.line(&format!("  provider → {}", s.bold(rest)));
            } else {
                term.line(&s.red(&format!("  no provider named {rest}")));
            }
        }
        "/diff" => {
            let review = kestrel_core::git_review(&session.root);
            if !review.is_repo {
                term.line(&s.dim("  not a git repository"));
            } else if review.files.is_empty() {
                term.line(&s.dim("  no uncommitted changes"));
            } else {
                let (added, removed) = kestrel_core::diff_line_stats(&review.diff);
                term.line(&format!(
                    "  {}  {}  {}",
                    review.summary,
                    s.green(&format!("+{added}")),
                    s.red(&format!("-{removed}"))
                ));
                for entry in &review.files {
                    term.line(&format!("    {}", s.dim(entry)));
                }
            }
        }
        "/cwd" => term.line(&format!("  {}", session.root.display())),
        other => term.line(&s.red(&format!("  unknown command {other} — try /help"))),
    }
    Flow::Continue
}

fn help(term: &mut Term) {
    let s = term.style;
    let rows = [
        ("/help", "this list"),
        ("/plan", "show the current task plan"),
        ("/continue", "resume a paused run"),
        ("/diff", "uncommitted changes with +/- counts"),
        ("/cost", "spend this session"),
        ("/model [name]", "show or switch the model"),
        ("/provider [name]", "show or switch the provider"),
        ("/work", "switch to Kestrel Work (documents, research)"),
        ("/build", "switch to Kestrel Build (coding)"),
        ("/clear", "start a fresh conversation"),
        ("/cwd", "the folder being worked in"),
        ("/exit", "leave (Ctrl-D also works)"),
    ];
    term.line("");
    for (cmd, what) in rows {
        // Pad the plain text first — padding a coloured string would count the
        // escape codes and wreck the column.
        term.line(&format!(
            "  {} {}",
            s.accent(&format!("{cmd:<18}")),
            s.dim(what)
        ));
    }
    term.line("");
    term.line(&s.dim("  Ctrl-C stops a running agent without leaving Kestrel."));
    term.line("");
}

fn banner(term: &mut Term, session: &Session) {
    let s = term.style;
    let model = session
        .settings
        .active()
        .map(|p| p.model.clone())
        .unwrap_or_else(|| "no provider configured".to_string());
    term.line("");
    term.line(&format!(
        "  {}  {}",
        s.accent("🦅 Kestrel"),
        s.dim(&session.root.display().to_string())
    ));
    term.line(&format!(
        "  {}  {}",
        s.dim(&format!("model {model}")),
        s.dim("· /help for commands")
    ));
    term.line("");
}

fn save(session: &Session) {
    let existing = kestrel_core::load_agent_session(&session.root);
    let _ = kestrel_core::save_agent_session(
        &session.root,
        &kestrel_core::AgentSession {
            messages: session.history.clone(),
            transcript: existing.transcript,
            created_files: existing.created_files,
        },
    );
}

/// Command history lives beside the settings, not in the project.
fn history_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| Path::new(&h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kestrel").join("cli-history.txt")
}
