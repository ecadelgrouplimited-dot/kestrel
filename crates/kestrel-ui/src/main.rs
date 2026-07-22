//! Kestrel native desktop shell.
//!
//! An all-Rust GUI over `kestrel-core`. A left-hand **file explorer** browses
//! the project's directory tree and creates, renames, and deletes files and
//! folders; the central pane is a **source editor** (with save and rustfmt
//! formatting) or the **output** of a local analysis. The action bar runs the
//! analyses (inspect, graph, query-seeded context), the verification ladder,
//! and environment discovery. A **Chat** view talks to your configured model
//! provider, and **Settings** manages providers and your details. Slow work
//! (indexing, verification, model calls) runs on a background thread so the
//! window never freezes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use kestrel_core::Symbol;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([760.0, 460.0])
            .with_title("Kestrel"),
        ..Default::default()
    };
    eframe::run_native(
        "Kestrel",
        native_options,
        Box::new(|_cc| Ok(Box::<KestrelApp>::default())),
    )
}

/// A node in the project's directory tree, loaded eagerly on Open/Refresh.
#[derive(Clone)]
struct TreeNode {
    name: String,
    path: PathBuf,
    is_dir: bool,
    children: Vec<TreeNode>,
}

/// The result a background job sends back to the UI thread.
enum JobOutcome {
    /// Free-text output for the Output tab, plus a status line.
    Text { output: String, status: String },
    /// A freshly loaded project directory tree.
    Tree { root: TreeNode, status: String },
    /// Diagnostics from a checker run.
    Diagnostics {
        items: Vec<kestrel_core::Diagnostic>,
        status: String,
    },
}

/// A live update streamed from a running agent loop.
enum AgentUpdate {
    /// The model's narration (also shown in the transcript).
    Line(String),
    /// A tool action in progress (shown in the transcript and as live status).
    Activity(String),
    /// A file the agent wrote, with its full contents for live preview.
    Wrote { path: String, contents: String },
    /// A file being streamed *right now* (partial contents), for real-time
    /// preview before the write lands on disk.
    Writing { path: String, contents: String },
    /// Token usage from a completed turn, for the live meter.
    Usage(kestrel_core::Usage),
    /// The agent's task plan changed — the live TODO ledger.
    Plan(kestrel_core::Plan),
    /// The agent finished; carries the final summary and the full conversation
    /// so a follow-up prompt can refine the same project.
    Done {
        summary: String,
        history: Vec<kestrel_core::AgentMessage>,
    },
    /// The agent paused without finishing (step budget or Stop) — resumable via
    /// Continue. Not an error.
    Incomplete {
        summary: String,
        history: Vec<kestrel_core::AgentMessage>,
    },
    /// The agent failed; still returns the conversation so far.
    Failed {
        err: String,
        history: Vec<kestrel_core::AgentMessage>,
    },
    /// The agent is waiting for the user to permit a tool action.
    ApprovalRequest(String),
}

/// The user's decision on a permission prompt.
enum ApprovalDecision {
    Allow,
    /// Allow this and everything else for the rest of this run.
    AllowAll,
    Deny,
}

/// A file produced during an agent run, kept for the created-files history and
/// its live preview.
struct AgentFile {
    path: String,
    contents: String,
}

/// A streamed update from a plain (non-agent) chat request.
enum ChatUpdate {
    /// A text delta to append to the in-progress reply.
    Token(String),
    /// The reply finished, with its token usage.
    Done(kestrel_core::Usage),
    /// The request failed.
    Failed(String),
}

#[derive(PartialEq, Eq)]
enum AppView {
    Main,
    Settings,
    Chat,
    Usage,
    Workflows,
    /// Kestrel Work — the everyday knowledge-work surface (research, documents,
    /// data) in its own scoped folder, separate from the coding project.
    Work,
}

/// Which pane the central area shows in the Main view.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CentralView {
    Editor,
    Output,
    Diff,
    Run,
    Problems,
}

/// A pending create/rename operation driving the entry modal.
#[derive(PartialEq, Eq, Clone, Copy)]
enum EntryOp {
    NewFile,
    NewFolder,
    Rename,
}

/// An action requested while rendering the (immutably borrowed) file tree,
/// applied after rendering so the tree walk needn't borrow `self` mutably.
enum TreeAction {
    Open(PathBuf),
    Select(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    NewIn(PathBuf, bool),
}

/// A workflow being authored or edited in the marketplace editor.
#[derive(Default, Clone)]
struct WorkflowDraft {
    /// Empty when authoring a brand-new workflow; set when editing an existing one.
    id: String,
    name: String,
    description: String,
    prompt: String,
    /// Comma-separated `{param}` names.
    params: String,
    status: String,
    /// Template files carried through an edit (skill packs), preserved as-is.
    resources: Vec<kestrel_core::WorkflowResource>,
}

struct KestrelApp {
    view: AppView,
    dark_mode: bool,
    path: String,
    query: String,
    output: String,
    status: String,
    job: Option<Receiver<JobOutcome>>,
    // File explorer + editor state.
    tree: Option<TreeNode>,
    selected_path: Option<PathBuf>,
    central: CentralView,
    editor_path: Option<PathBuf>,
    editor_text: String,
    editor_original: String,
    editor_symbols: Vec<Symbol>,
    editor_status: String,
    diagnostics: Vec<kestrel_core::Diagnostic>,
    // Create/rename modal.
    entry_op: Option<EntryOp>,
    entry_target: PathBuf,
    entry_name: String,
    entry_status: String,
    // Delete confirmation.
    delete_target: Option<PathBuf>,
    // Diff review state.
    diff_review: Option<kestrel_core::GitReview>,
    diff_status: String,
    confirm_revert: bool,
    checkpoints: Vec<kestrel_core::Checkpoint>,
    restore_target: Option<String>,
    // Run tab state.
    run_command_input: String,
    run_url: String,
    run_apps: Vec<kestrel_core::RunningApp>,
    run_log: String,
    run_selected_pid: Option<u32>,
    run_status: String,
    run_shots: Vec<PathBuf>,
    // Settings state.
    settings: kestrel_core::Settings,
    user_name: String,
    user_email: String,
    budget_session: String,
    budget_daily: String,
    policy_patterns: String,
    /// Total cost logged today (UTC), for the daily budget check.
    today_cost: f64,
    new_provider_name: String,
    new_provider_preset: String,
    settings_status: String,
    // New-project modal state.
    new_project_open: bool,
    new_project_parent: String,
    new_project_name: String,
    new_project_status: String,
    // Chat state.
    chat_input: String,
    chat_history: Vec<kestrel_core::ChatMessage>,
    chat_include_context: bool,
    chat_agent_mode: bool,
    chat_pending: bool,
    chat_error: String,
    chat_job: Option<Receiver<ChatUpdate>>,
    agent_job: Option<Receiver<AgentUpdate>>,
    /// Files the current/last agent run produced, in creation order.
    agent_files: Vec<AgentFile>,
    /// Index into `agent_files` currently shown in the build preview.
    agent_preview: Option<usize>,
    /// The running agent conversation, carried across builds so follow-up
    /// prompts refine the same project instead of starting from scratch.
    agent_messages: Vec<kestrel_core::AgentMessage>,
    /// The agent's current activity, shown live while it works.
    agent_activity: String,
    /// The agent's live task plan (TODO ledger), if any.
    agent_plan: Option<kestrel_core::Plan>,
    /// Set when a running agent should stop; the worker checks it each step.
    agent_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The last run paused (step budget or Stop) and can be continued.
    agent_incomplete: bool,
    /// A tool action awaiting the user's permission, if any.
    pending_approval: Option<String>,
    /// Channel back to the paused worker to deliver a permission decision.
    agent_decision: Option<Sender<ApprovalDecision>>,
    /// Whether to prompt before the agent runs commands/installs/git.
    ask_permission: bool,
    /// Kestrel Work's scoped workspace folder (documents live here, not code).
    work_folder: String,
    /// Actual token usage this conversation, for the live meter.
    session_usage: kestrel_core::Usage,
    session_cost: f64,
    /// The most recent request's usage (for the "last turn" readout).
    last_usage: kestrel_core::Usage,
    /// Loaded usage records for the Usage dashboard.
    usage_records: Vec<kestrel_core::UsageRecord>,
    /// Available workflows and the parameter values being filled in.
    workflows: Vec<kestrel_core::Workflow>,
    workflow_params: std::collections::HashMap<String, String>,
    /// Whether the marketplace catalog gallery is expanded.
    show_catalog: bool,
    /// The workflow editor (author/edit), open when `Some`.
    wf_editor: Option<WorkflowDraft>,
    /// Repositories linked to the current project (the multi-repo workspace).
    workspace_repos: Vec<kestrel_core::Repo>,
}

impl Default for KestrelApp {
    fn default() -> Self {
        let path = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let settings = kestrel_core::load_settings();
        let user_name = settings.user.name.clone().unwrap_or_default();
        let user_email = settings.user.email.clone().unwrap_or_default();
        let budget_session = settings
            .budget
            .session_limit
            .map(|v| v.to_string())
            .unwrap_or_default();
        let budget_daily = settings
            .budget
            .daily_limit
            .map(|v| v.to_string())
            .unwrap_or_default();
        let policy_patterns = settings.policy.denied_patterns.join("\n");
        let ask_permission = settings.ask_permission;
        // Work defaults to the user's Documents folder — a sane, non-code place.
        let work_folder = settings.work_folder.clone().unwrap_or_else(|| {
            std::env::var("USERPROFILE")
                .map(|home| format!("{home}\\Documents"))
                .unwrap_or_else(|_| path.clone())
        });
        let workspace_repos = kestrel_core::load_workspace(std::path::Path::new(&path)).repos;
        Self {
            view: AppView::Main,
            dark_mode: true,
            path,
            query: String::new(),
            output: "Open a project to browse its files. Click a file to view and edit it; use \
                     + File / + Folder to create one. The action bar runs Inspect, Graph, a \
                     Context query, Verify, and Env — their results appear on the Output tab."
                .to_string(),
            status: String::new(),
            job: None,
            tree: None,
            selected_path: None,
            central: CentralView::Editor,
            editor_path: None,
            editor_text: String::new(),
            editor_original: String::new(),
            editor_symbols: Vec::new(),
            editor_status: String::new(),
            diagnostics: Vec::new(),
            entry_op: None,
            entry_target: PathBuf::new(),
            entry_name: String::new(),
            entry_status: String::new(),
            delete_target: None,
            diff_review: None,
            diff_status: String::new(),
            confirm_revert: false,
            checkpoints: Vec::new(),
            restore_target: None,
            run_command_input: String::new(),
            run_url: String::new(),
            run_apps: Vec::new(),
            run_log: String::new(),
            run_selected_pid: None,
            run_status: String::new(),
            run_shots: Vec::new(),
            settings,
            user_name,
            user_email,
            budget_session,
            budget_daily,
            policy_patterns,
            today_cost: 0.0,
            new_provider_name: String::new(),
            new_provider_preset: "anthropic".to_string(),
            settings_status: String::new(),
            new_project_open: false,
            new_project_parent: String::new(),
            new_project_name: String::new(),
            new_project_status: String::new(),
            chat_input: String::new(),
            chat_history: Vec::new(),
            chat_include_context: false,
            chat_agent_mode: false,
            chat_pending: false,
            chat_error: String::new(),
            chat_job: None,
            agent_job: None,
            agent_files: Vec::new(),
            agent_preview: None,
            agent_messages: Vec::new(),
            agent_activity: String::new(),
            agent_plan: None,
            agent_cancel: None,
            agent_incomplete: false,
            pending_approval: None,
            agent_decision: None,
            ask_permission,
            work_folder,
            session_usage: kestrel_core::Usage::default(),
            session_cost: 0.0,
            last_usage: kestrel_core::Usage::default(),
            usage_records: Vec::new(),
            workflows: kestrel_core::all_workflows(),
            workflow_params: std::collections::HashMap::new(),
            show_catalog: false,
            wf_editor: None,
            workspace_repos,
        }
    }
}

impl KestrelApp {
    /// Spawn `work` on a background thread; its result is applied on a later
    /// frame. Ignored if a job is already running.
    fn spawn(&mut self, work: impl FnOnce() -> JobOutcome + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.job = Some(rx);
        self.status = "Working…".to_string();
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.job else { return };
        match rx.try_recv() {
            Ok(JobOutcome::Text { output, status }) => {
                self.output = output;
                self.status = status;
                self.central = CentralView::Output;
                self.job = None;
            }
            Ok(JobOutcome::Tree { root, status }) => {
                self.tree = Some(root);
                self.status = status;
                self.job = None;
            }
            Ok(JobOutcome::Diagnostics { items, status }) => {
                self.diagnostics = items;
                self.status = status;
                self.central = CentralView::Problems;
                self.job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => {
                self.status = "The background job stopped unexpectedly.".to_string();
                self.job = None;
            }
        }
    }

    /// Poll the in-flight chat request, if any, and append the reply.
    fn poll_chat(&mut self, ctx: &egui::Context) {
        if self.chat_job.is_none() {
            return;
        }
        loop {
            let message = self.chat_job.as_ref().unwrap().try_recv();
            match message {
                Ok(ChatUpdate::Token(token)) => {
                    if let Some(last) = self.chat_history.last_mut() {
                        if last.role == "assistant" {
                            last.content.push_str(&token);
                        }
                    }
                    ctx.request_repaint();
                }
                Ok(ChatUpdate::Done(usage)) => {
                    self.add_usage(&usage);
                    self.chat_pending = false;
                    self.chat_job = None;
                    self.save_session();
                    break;
                }
                Ok(ChatUpdate::Failed(err)) => {
                    // Drop the empty placeholder reply we added when sending.
                    if self
                        .chat_history
                        .last()
                        .is_some_and(|m| m.role == "assistant" && m.content.is_empty())
                    {
                        self.chat_history.pop();
                    }
                    self.chat_error = err;
                    self.chat_pending = false;
                    self.chat_job = None;
                    break;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint();
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    self.chat_pending = false;
                    self.chat_job = None;
                    break;
                }
            }
        }
    }

    /// The permission prompt: when the agent wants to run a command/install/git
    /// and "ask permission" is on, it blocks here until the user decides.
    fn approval_modal(&mut self, ctx: &egui::Context) {
        let Some(desc) = self.pending_approval.clone() else {
            return;
        };
        let mut decision: Option<ApprovalDecision> = None;
        egui::Window::new("🔐 Permission needed")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label("The agent wants to:");
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&desc).strong().monospace());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("✓ Allow").clicked() {
                        decision = Some(ApprovalDecision::Allow);
                    }
                    if ui
                        .button("✓✓ Allow all this run")
                        .on_hover_text("Don't ask again until this run ends")
                        .clicked()
                    {
                        decision = Some(ApprovalDecision::AllowAll);
                    }
                    if ui.button("✕ Deny").clicked() {
                        decision = Some(ApprovalDecision::Deny);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Deny lets the agent try another approach — the run won't die.",
                    )
                    .weak()
                    .small(),
                );
            });
        if let Some(d) = decision {
            if let Some(tx) = &self.agent_decision {
                let _ = tx.send(d);
            }
            self.pending_approval = None;
            self.agent_activity = "💭 Thinking…".to_string();
            ctx.request_repaint();
        }
    }

    /// Tear down a finished or stopped agent run: signal the worker to halt (so
    /// it stops spending after a budget stop or app-side stop), drop the channels,
    /// and clear the live status.
    fn end_agent_run(&mut self) {
        if let Some(cancel) = &self.agent_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.chat_pending = false;
        self.agent_activity.clear();
        self.agent_job = None;
        self.agent_cancel = None;
        self.agent_decision = None;
        self.pending_approval = None;
    }

    /// Ask the running agent to stop; it pauses at the next step and returns its
    /// progress as a resumable Incomplete outcome.
    fn stop_agent(&mut self) {
        if let Some(cancel) = &self.agent_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.agent_activity = "⏹ Stopping…".to_string();
            self.status = "Stopping the agent…".to_string();
        }
        // If it's blocked on a permission prompt, unblock it so it can wind down.
        if self.pending_approval.is_some() {
            if let Some(tx) = &self.agent_decision {
                let _ = tx.send(ApprovalDecision::Deny);
            }
            self.pending_approval = None;
        }
    }

    /// Drain live updates from a running agent loop into the transcript.
    fn poll_agent(&mut self, ctx: &egui::Context) {
        if self.agent_job.is_none() {
            return;
        }
        // Drain everything queued this frame, coalescing file writes into a
        // single tree refresh so the explorer updates live but not wastefully.
        let mut last_written: Option<PathBuf> = None;
        let mut refresh = false;
        loop {
            let message = {
                let rx = self.agent_job.as_ref().unwrap();
                rx.try_recv()
            };
            match message {
                Ok(AgentUpdate::Line(line)) => {
                    self.agent_activity = "💭 Thinking…".to_string();
                    self.chat_history
                        .push(kestrel_core::ChatMessage::assistant(line));
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Activity(line)) => {
                    self.agent_activity = line.clone();
                    self.chat_history
                        .push(kestrel_core::ChatMessage::assistant(line));
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Usage(usage)) => {
                    self.add_usage(&usage);
                    // Hard-stop the run if a budget cap was crossed mid-run.
                    if let Some(reason) = self.budget_blocked() {
                        self.end_agent_run();
                        self.chat_history
                            .push(kestrel_core::ChatMessage::assistant(format!(
                                "⛔ Stopped — {reason}."
                            )));
                        self.status = "Stopped: over budget.".to_string();
                        self.save_session();
                        break;
                    }
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Plan(plan)) => {
                    let (done, total) = plan.progress();
                    self.agent_plan = Some(plan);
                    self.agent_activity = format!("🗺 Plan: {done}/{total} steps done");
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Writing { path, contents }) => {
                    // Live, token-by-token preview as the model types the file —
                    // no disk write and no tree refresh yet; that happens on Wrote.
                    self.agent_activity = format!("✍ Writing {path}…");
                    if let Some(idx) = self.agent_files.iter().position(|f| f.path == path) {
                        self.agent_files[idx].contents = contents;
                        self.agent_preview = Some(idx);
                    } else {
                        self.agent_files.push(AgentFile { path, contents });
                        self.agent_preview = Some(self.agent_files.len() - 1);
                    }
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Wrote { path, contents }) => {
                    self.agent_activity = format!("✍ Writing {path}");
                    if let Some(idx) = self.agent_files.iter().position(|f| f.path == path) {
                        self.agent_files[idx].contents = contents;
                        self.agent_preview = Some(idx);
                    } else {
                        self.agent_files.push(AgentFile {
                            path: path.clone(),
                            contents,
                        });
                        self.agent_preview = Some(self.agent_files.len() - 1);
                    }
                    last_written = Some(self.agent_root().join(&path));
                    refresh = true;
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Done { summary, history }) => {
                    if !summary.trim().is_empty() {
                        self.chat_history
                            .push(kestrel_core::ChatMessage::assistant(summary));
                    }
                    self.agent_messages = history;
                    self.end_agent_run();
                    self.status = "Agent finished — review changes in the Diff tab.".to_string();
                    self.save_session();
                    self.diff_review = None;
                    refresh = true;
                    break;
                }
                Ok(AgentUpdate::Incomplete { summary, history }) => {
                    if !summary.trim().is_empty() {
                        self.chat_history
                            .push(kestrel_core::ChatMessage::assistant(summary));
                    }
                    self.agent_messages = history;
                    self.end_agent_run();
                    self.agent_incomplete = true;
                    self.status = "Agent paused — click Continue to keep going.".to_string();
                    self.save_session();
                    self.diff_review = None;
                    refresh = true;
                    break;
                }
                Ok(AgentUpdate::ApprovalRequest(description)) => {
                    // The worker is blocked awaiting the user's decision.
                    self.pending_approval = Some(description);
                    self.agent_activity = "⏸ Waiting for your permission…".to_string();
                    ctx.request_repaint();
                    break;
                }
                Ok(AgentUpdate::Failed { err, history }) => {
                    self.chat_error = err;
                    self.agent_messages = history;
                    self.end_agent_run();
                    self.save_session();
                    self.diff_review = None;
                    // Show whatever the agent managed to write before stopping.
                    refresh = true;
                    break;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint();
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    self.end_agent_run();
                    break;
                }
            }
        }

        if refresh {
            self.refresh_tree_now();
        }
        // If the agent just rewrote the file open in the editor, and it has no
        // unsaved edits, reload it so the editor mirrors disk live.
        if let Some(path) = last_written {
            let clean = self.editor_text == self.editor_original;
            if clean && self.editor_path.as_deref() == Some(path.as_path()) {
                if let Ok(text) = kestrel_core::read_text_file(&path) {
                    self.editor_text = text.clone();
                    self.editor_original = text;
                }
            }
        }
    }

    /// Rebuild the project tree synchronously (fast for typical projects), used
    /// for live refreshes during an agent run without waiting on the job queue.
    fn refresh_tree_now(&mut self) {
        let path = self.project_path();
        if let JobOutcome::Tree { root, .. } = load_tree(&path) {
            self.tree = Some(root);
        }
    }
}

impl eframe::App for KestrelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        configure_style(ctx, self.dark_mode);
        self.poll_job(ctx);
        self.poll_chat(ctx);
        self.poll_agent(ctx);
        let busy = self.job.is_some();
        self.new_project_modal(ctx);
        self.entry_modal(ctx);
        self.delete_modal(ctx);
        self.revert_modal(ctx);
        self.restore_modal(ctx);
        self.workflow_editor(ctx);
        self.approval_modal(ctx);

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("🦅 Kestrel").color(ACCENT));
                ui.separator();
                ui.label("Project:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .desired_width(380.0)
                        .hint_text("path to a repository"),
                );
                if ui.add_enabled(!busy, egui::Button::new("Load")).clicked() {
                    self.open_project_path(self.project_path());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let theme_icon = if self.dark_mode { "🌙" } else { "☀" };
                    if ui
                        .button(theme_icon)
                        .on_hover_text("Toggle light / dark theme")
                        .clicked()
                    {
                        self.dark_mode = !self.dark_mode;
                    }
                    if self.view == AppView::Main {
                        if ui.button("⚙ Settings").clicked() {
                            self.view = AppView::Settings;
                        }
                        if ui.button("📊 Usage").clicked() {
                            self.usage_records =
                                kestrel_core::load_usage_records(&self.project_path());
                            self.view = AppView::Usage;
                        }
                        if ui.button("⚡ Workflows").clicked() {
                            self.workflows = kestrel_core::all_workflows();
                            self.view = AppView::Workflows;
                        }
                        if ui.button("💬 Chat").clicked() {
                            self.view = AppView::Chat;
                        }
                        if ui
                            .button("💼 Work")
                            .on_hover_text(
                                "Kestrel Work — research, documents and data in your own \
                                 workspace folder",
                            )
                            .clicked()
                        {
                            self.enter_work_mode();
                        }
                    } else if ui.button("← Back").clicked() {
                        if self.view == AppView::Work {
                            self.exit_work_mode();
                        }
                        self.view = AppView::Main;
                    }
                });
            });
            if self.view == AppView::Main {
                ui.add_space(4.0);
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("📂 Open…").clicked() {
                            if let Some(dir) = rfd::FileDialog::new()
                                .set_title("Open project folder")
                                .pick_folder()
                            {
                                self.open_project_path(dir);
                            }
                        }
                        if ui.button("✨ New project…").clicked() {
                            if self.new_project_parent.trim().is_empty() {
                                self.new_project_parent = self.path.clone();
                            }
                            self.new_project_status.clear();
                            self.new_project_open = true;
                        }
                        let mut chosen: Option<String> = None;
                        ui.menu_button("Recent ▾", |ui| {
                            if self.settings.recent_projects.is_empty() {
                                ui.label(egui::RichText::new("(none yet)").weak());
                            }
                            for recent in &self.settings.recent_projects {
                                if ui.button(recent).clicked() {
                                    chosen = Some(recent.clone());
                                    ui.close_menu();
                                }
                            }
                        });
                        if let Some(recent) = chosen {
                            self.open_project_path(PathBuf::from(recent));
                        }
                        ui.separator();
                        if ui.button("🔍 Inspect").clicked() {
                            self.run_text(inspect);
                        }
                        if ui.button("🕸 Graph").clicked() {
                            self.run_text(graph);
                        }
                        if ui.button("✅ Verify").clicked() {
                            self.run_text(verify);
                        }
                        if ui.button("⚠ Check").clicked() {
                            self.run_diagnostics();
                        }
                        if ui.button("🖥 Env").clicked() {
                            self.spawn(environment);
                        }
                        if ui.button("📜 Audit").clicked() {
                            self.show_audit_log();
                        }
                        ui.separator();
                        ui.label("Query:");
                        let enter = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.query)
                                    .desired_width(220.0)
                                    .hint_text("e.g. dependency graph edges"),
                            )
                            .lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Context").clicked() || enter {
                            let query = self.query.clone();
                            self.run_text(move |path| context(path, &query));
                        }
                    });
                });
            }
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.label(if self.status.is_empty() {
                "Ready.".to_string()
            } else {
                self.status.clone()
            });
            ui.add_space(2.0);
        });

        if self.view == AppView::Settings {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.settings_ui(ui);
                    });
            });
            return;
        }

        if self.view == AppView::Usage {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.usage_ui(ui);
                    });
            });
            return;
        }

        if self.view == AppView::Workflows {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.workflows_ui(ui);
                    });
            });
            return;
        }

        if self.view == AppView::Work {
            egui::TopBottomPanel::bottom("work-compose").show(ctx, |ui| {
                self.chat_compose_ui(ui);
            });
            // The workspace: which folder Work is scoped to, and what's in it.
            egui::SidePanel::left("work-files")
                .resizable(true)
                .default_width(260.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.work_panel_ui(ui);
                });
            // The artifact pane: the plan ledger plus the documents produced.
            egui::SidePanel::right("work-artifacts")
                .resizable(true)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.build_preview_ui(ui);
                });
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        self.chat_history_ui(ui);
                    });
            });
            return;
        }

        if self.view == AppView::Chat {
            egui::TopBottomPanel::bottom("chat-compose").show(ctx, |ui| {
                self.chat_compose_ui(ui);
            });
            // Keep the explorer visible so files created by the agent appear live.
            egui::SidePanel::left("files")
                .resizable(true)
                .default_width(240.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.tree_ui(ui);
                });
            // A live preview of the files the agent is creating.
            egui::SidePanel::right("build-preview")
                .resizable(true)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.build_preview_ui(ui);
                });
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        self.chat_history_ui(ui);
                    });
            });
            return;
        }

        egui::SidePanel::left("files")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                self.tree_ui(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.central, CentralView::Editor, "Editor");
                ui.selectable_value(&mut self.central, CentralView::Output, "Output");
                ui.selectable_value(&mut self.central, CentralView::Diff, "Diff");
                if ui
                    .selectable_value(&mut self.central, CentralView::Run, "▶ Run")
                    .clicked()
                {
                    self.refresh_apps();
                }
                let problems_label = if self.diagnostics.is_empty() {
                    "⚠ Problems".to_string()
                } else {
                    format!("⚠ Problems ({})", self.diagnostics.len())
                };
                ui.selectable_value(&mut self.central, CentralView::Problems, problems_label);
            });
            ui.separator();
            match self.central {
                CentralView::Editor => self.editor_ui(ui),
                CentralView::Output => {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&self.output).monospace())
                                    .selectable(true),
                            );
                        });
                }
                CentralView::Diff => self.diff_ui(ui),
                CentralView::Run => self.run_ui(ui),
                CentralView::Problems => self.problems_ui(ui),
            }
        });
    }
}

impl KestrelApp {
    fn project_path(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
    }

    /// Which product surface is active: Kestrel **Work** in the Work view, else
    /// Kestrel **Build**. This picks the agent's tool pack and system prompt.
    fn profile(&self) -> kestrel_core::Profile {
        if self.view == AppView::Work {
            kestrel_core::Profile::Work
        } else {
            kestrel_core::Profile::Build
        }
    }

    /// The folder the agent is scoped to: Work's own workspace, or the project.
    fn agent_root(&self) -> PathBuf {
        if self.view == AppView::Work {
            PathBuf::from(self.work_folder.trim())
        } else {
            self.project_path()
        }
    }

    /// Make `path` the active project: record it, remember it in the recent
    /// list (persisted), restore its saved agent session, return to the main
    /// view, and load its file tree.
    fn open_project_path(&mut self, path: PathBuf) {
        self.path = path.display().to_string();
        kestrel_core::push_recent(&mut self.settings.recent_projects, &path);
        let _ = kestrel_core::save_settings(&self.settings);
        // Resume this project's agent conversation and transcript, if any, and
        // rebuild the file-preview history by re-reading the files from disk.
        let session = kestrel_core::load_agent_session(&path);
        self.agent_messages = session.messages;
        self.chat_history = session.transcript;
        self.agent_files = session
            .created_files
            .iter()
            .filter_map(|rel| {
                std::fs::read_to_string(path.join(rel))
                    .ok()
                    .map(|contents| AgentFile {
                        path: rel.clone(),
                        contents,
                    })
            })
            .collect();
        self.agent_preview = self.agent_files.len().checked_sub(1);
        self.chat_error.clear();
        self.session_usage = kestrel_core::Usage::default();
        self.session_cost = 0.0;
        self.today_cost = kestrel_core::cost_today(&kestrel_core::load_usage_records(&path));
        self.workspace_repos = kestrel_core::load_workspace(&path).repos;
        let plan = kestrel_core::load_plan(&path);
        self.agent_plan = (!plan.steps.is_empty()).then_some(plan);
        self.view = AppView::Main;
        self.reload_tree();
    }

    /// Persist the current surface's agent conversation, transcript, and the
    /// list of files it created so returning to it resumes where this left off.
    /// Scoped to `agent_root`, so Build and Work keep separate sessions.
    fn save_session(&self) {
        let session = kestrel_core::AgentSession {
            messages: self.agent_messages.clone(),
            transcript: self.chat_history.clone(),
            created_files: self.agent_files.iter().map(|f| f.path.clone()).collect(),
        };
        let _ = kestrel_core::save_agent_session(&self.agent_root(), &session);
    }

    /// Load the conversation, files, and plan belonging to the *current* surface
    /// (Build project or Work folder). Used when switching between them.
    fn load_session_for_current_root(&mut self) {
        let root = self.agent_root();
        let session = kestrel_core::load_agent_session(&root);
        self.agent_messages = session.messages;
        self.chat_history = session.transcript;
        self.agent_files = session
            .created_files
            .iter()
            .filter_map(|rel| {
                std::fs::read_to_string(root.join(rel))
                    .ok()
                    .map(|contents| AgentFile {
                        path: rel.clone(),
                        contents,
                    })
            })
            .collect();
        self.agent_preview = self.agent_files.len().checked_sub(1);
        let plan = kestrel_core::load_plan(&root);
        self.agent_plan = (!plan.steps.is_empty()).then_some(plan);
        self.chat_error.clear();
        self.session_usage = kestrel_core::Usage::default();
        self.session_cost = 0.0;
        self.agent_incomplete = false;
    }

    /// Switch into Kestrel Work: park the Build conversation and pick up the
    /// Work folder's own.
    fn enter_work_mode(&mut self) {
        if self.view == AppView::Work {
            return;
        }
        self.save_session();
        self.view = AppView::Work;
        // Work is agentic by nature — it acts on files, it doesn't just chat.
        self.chat_agent_mode = true;
        let root = self.agent_root();
        if !root.is_dir() {
            self.status = format!(
                "Work folder {} doesn't exist yet — choose one.",
                root.display()
            );
        }
        self.load_session_for_current_root();
    }

    /// Point Kestrel Work at `dir`: park the current conversation, remember the
    /// folder in the recents list, and pick up that folder's own session.
    fn set_work_folder(&mut self, dir: String) {
        if dir.trim().is_empty() || dir == self.work_folder {
            return;
        }
        self.save_session();
        self.work_folder = dir.clone();
        self.settings.work_folder = Some(dir.clone());
        kestrel_core::push_recent(&mut self.settings.work_recents, std::path::Path::new(&dir));
        let _ = kestrel_core::save_settings(&self.settings);
        self.load_session_for_current_root();
        self.status = format!("Work folder: {dir}");
    }

    /// Leave Kestrel Work, restoring the Build project's conversation.
    fn exit_work_mode(&mut self) {
        self.save_session();
        self.view = AppView::Main;
        self.load_session_for_current_root();
    }

    /// (Re)load the current project's directory tree on a worker thread.
    fn reload_tree(&mut self) {
        let path = self.project_path();
        self.spawn(move || load_tree(&path));
    }

    // --- File explorer ---------------------------------------------------

    /// The left-hand explorer: a toolbar plus the project's directory tree.
    fn tree_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Explorer");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⟳").on_hover_text("Refresh").clicked() {
                    self.reload_tree();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("+ File").clicked() {
                self.begin_new_entry(false);
            }
            if ui.button("+ Folder").clicked() {
                self.begin_new_entry(true);
            }
            if ui
                .button("🔗 Repo")
                .on_hover_text("Link another repository so the agent can reason across them")
                .clicked()
            {
                self.link_repo();
            }
        });
        ui.separator();

        self.workspace_repos_ui(ui);

        let mut actions: Vec<TreeAction> = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(root) = &self.tree {
                    if root.children.is_empty() {
                        ui.label(egui::RichText::new("(empty project)").weak());
                    }
                    for child in &root.children {
                        render_tree(ui, child, &self.selected_path, &mut actions);
                    }
                } else {
                    ui.label(egui::RichText::new("Open a project to see its files.").weak());
                }
            });

        for action in actions {
            match action {
                TreeAction::Open(path) => {
                    self.selected_path = Some(path.clone());
                    self.open_file(&path);
                }
                TreeAction::Select(path) => self.selected_path = Some(path),
                TreeAction::Rename(path) => self.begin_rename(path),
                TreeAction::Delete(path) => self.delete_target = Some(path),
                TreeAction::NewIn(dir, is_dir) => {
                    self.entry_target = dir;
                    self.entry_name.clear();
                    self.entry_status.clear();
                    self.entry_op = Some(if is_dir {
                        EntryOp::NewFolder
                    } else {
                        EntryOp::NewFile
                    });
                }
            }
        }
    }

    /// Pick a folder and link it to the current project as a workspace repo.
    fn link_repo(&mut self) {
        let root = self.project_path();
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Link a repository")
            .set_directory(&root)
            .pick_folder()
        {
            match kestrel_core::link_repo(&root, &dir) {
                Ok(ws) => {
                    self.workspace_repos = ws.repos;
                    self.status = format!("Linked {}", dir.display());
                }
                Err(err) => self.status = format!("Could not link repo: {err}"),
            }
        }
    }

    /// The linked-repositories section of the explorer: each repo can be opened
    /// as the primary project or unlinked. The agent reaches them via list_repos.
    fn workspace_repos_ui(&mut self, ui: &mut egui::Ui) {
        if self.workspace_repos.is_empty() {
            return;
        }
        enum RepoAction {
            Open(PathBuf),
            Unlink(String),
        }
        let mut action: Option<RepoAction> = None;
        egui::CollapsingHeader::new(format!("🔗 Linked repos ({})", self.workspace_repos.len()))
            .default_open(true)
            .show(ui, |ui| {
                for repo in &self.workspace_repos {
                    ui.horizontal(|ui| {
                        if ui
                            .button("↗")
                            .on_hover_text(format!("Open {} as the project", repo.path))
                            .clicked()
                        {
                            action = Some(RepoAction::Open(PathBuf::from(&repo.path)));
                        }
                        if ui.button("✕").on_hover_text("Unlink").clicked() {
                            action = Some(RepoAction::Unlink(repo.path.clone()));
                        }
                        ui.label(&repo.name).on_hover_text(&repo.path);
                    });
                }
            });
        ui.separator();

        match action {
            Some(RepoAction::Open(path)) => {
                if path.is_dir() {
                    self.open_project_path(path);
                } else {
                    self.status = "That repository folder no longer exists.".to_string();
                }
            }
            Some(RepoAction::Unlink(path)) => {
                let root = self.project_path();
                if let Ok(ws) = kestrel_core::unlink_repo(&root, &path) {
                    self.workspace_repos = ws.repos;
                }
            }
            None => {}
        }
    }

    /// The directory a new entry should be created in: the selected folder, the
    /// selected file's parent, or the project root.
    fn new_entry_parent_dir(&self) -> PathBuf {
        if let Some(selected) = &self.selected_path {
            if selected.is_dir() {
                return selected.clone();
            }
            if let Some(parent) = selected.parent() {
                return parent.to_path_buf();
            }
        }
        self.project_path()
    }

    fn begin_new_entry(&mut self, is_dir: bool) {
        self.entry_target = self.new_entry_parent_dir();
        self.entry_name.clear();
        self.entry_status.clear();
        self.entry_op = Some(if is_dir {
            EntryOp::NewFolder
        } else {
            EntryOp::NewFile
        });
    }

    fn begin_rename(&mut self, path: PathBuf) {
        self.entry_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.entry_target = path;
        self.entry_status.clear();
        self.entry_op = Some(EntryOp::Rename);
    }

    /// The create-file / create-folder / rename modal.
    fn entry_modal(&mut self, ctx: &egui::Context) {
        let Some(op) = self.entry_op else { return };
        let title = match op {
            EntryOp::NewFile => "New file",
            EntryOp::NewFolder => "New folder",
            EntryOp::Rename => "Rename",
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let hint = match op {
                    EntryOp::NewFolder => "folder name",
                    _ => "file name (e.g. main.rs)",
                };
                let context_line = match op {
                    EntryOp::Rename => format!("Renaming {}", self.entry_target.display()),
                    _ => format!("In {}", self.entry_target.display()),
                };
                ui.label(egui::RichText::new(context_line).weak());
                ui.add_space(4.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.entry_name)
                        .desired_width(320.0)
                        .hint_text(hint),
                );
                // Focus the field once, when nothing else is focused (i.e. on
                // open); requesting every frame would defeat Enter detection.
                if ui.memory(|m| m.focused().is_none()) {
                    response.request_focus();
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                if !self.entry_status.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), &self.entry_status);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(if op == EntryOp::Rename {
                            "Rename"
                        } else {
                            "Create"
                        })
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel || !open {
            self.entry_op = None;
            self.entry_status.clear();
            return;
        }
        if confirm {
            let result = match op {
                EntryOp::NewFile => kestrel_core::create_file(&self.entry_target, &self.entry_name),
                EntryOp::NewFolder => {
                    kestrel_core::create_dir(&self.entry_target, &self.entry_name)
                }
                EntryOp::Rename => kestrel_core::rename_entry(&self.entry_target, &self.entry_name),
            };
            match result {
                Ok(new_path) => {
                    self.entry_op = None;
                    self.entry_status.clear();
                    self.reload_tree();
                    self.selected_path = Some(new_path.clone());
                    match op {
                        EntryOp::NewFile => self.open_file(&new_path),
                        EntryOp::Rename
                            if self.editor_path.as_deref() == Some(&self.entry_target) =>
                        {
                            self.editor_path = Some(new_path);
                        }
                        _ => {}
                    }
                    self.status = "Done.".to_string();
                }
                Err(err) => self.entry_status = err.to_string(),
            }
        }
    }

    /// The delete-confirmation modal.
    fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some(target) = self.delete_target.clone() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Delete")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let kind = if target.is_dir() { "folder" } else { "file" };
                ui.label(format!("Delete this {kind}?"));
                ui.add_space(2.0);
                ui.strong(target.display().to_string());
                if target.is_dir() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 150, 80),
                        "The folder and everything inside it will be removed.",
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel || !open {
            self.delete_target = None;
            return;
        }
        if confirm {
            match kestrel_core::delete_entry(&target) {
                Ok(()) => {
                    if self.editor_path.as_ref() == Some(&target)
                        || self
                            .editor_path
                            .as_ref()
                            .is_some_and(|p| p.starts_with(&target))
                    {
                        self.editor_path = None;
                        self.editor_text.clear();
                        self.editor_original.clear();
                        self.editor_symbols.clear();
                    }
                    if self.selected_path.as_ref() == Some(&target) {
                        self.selected_path = None;
                    }
                    self.status = format!("Deleted {}.", target.display());
                    self.reload_tree();
                }
                Err(err) => self.status = format!("Delete failed: {err}"),
            }
            self.delete_target = None;
        }
    }

    // --- Editor ----------------------------------------------------------

    fn open_file(&mut self, path: &Path) {
        self.central = CentralView::Editor;
        self.editor_path = Some(path.to_path_buf());
        self.editor_status.clear();
        match kestrel_core::read_text_file(path) {
            Ok(text) => {
                self.editor_text = text.clone();
                self.editor_original = text;
                self.editor_symbols = kestrel_core::symbols_for_file(path)
                    .ok()
                    .flatten()
                    .map(|f| f.symbols)
                    .unwrap_or_default();
                self.status = format!("Opened {}.", path.display());
            }
            Err(err) => {
                self.editor_text.clear();
                self.editor_original.clear();
                self.editor_symbols.clear();
                self.editor_status = format!(
                    "Cannot open as UTF-8 text ({err}). Binary files aren't editable here."
                );
                self.status = "Open failed.".to_string();
            }
        }
    }

    fn save_file(&mut self) {
        let Some(path) = self.editor_path.clone() else {
            return;
        };
        match kestrel_core::write_text_file(&path, &self.editor_text) {
            Ok(()) => {
                self.editor_original = self.editor_text.clone();
                self.editor_symbols = kestrel_core::symbols_for_file(&path)
                    .ok()
                    .flatten()
                    .map(|f| f.symbols)
                    .unwrap_or_default();
                self.editor_status = "Saved.".to_string();
                self.status = format!("Saved {}.", path.display());
            }
            Err(err) => self.editor_status = format!("Save failed: {err}"),
        }
    }

    fn format_current_file(&mut self) {
        let Some(path) = self.editor_path.clone() else {
            return;
        };
        let filename = path.to_string_lossy();
        let Some(formatter) = kestrel_core::formatter_for(&filename) else {
            self.editor_status =
                "No formatter is configured for this file type (Rust, Go, Python, JS/TS, CSS, \
                 HTML, JSON, Markdown, YAML, …)."
                    .to_string();
            return;
        };
        let label = formatter.label;
        match kestrel_core::format_source(&filename, &self.editor_text) {
            Ok(formatted) => {
                self.editor_text = formatted;
                self.editor_status = format!("Formatted with {label}.");
            }
            Err(err) => self.editor_status = err,
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.editor_path.clone() else {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Select a file in the explorer to view and edit it, or create one with \
                     + File.",
                )
                .weak(),
            );
            return;
        };

        let dirty = self.editor_text != self.editor_original;
        let mut do_save = false;
        let mut do_format = false;
        ui.horizontal(|ui| {
            ui.strong(path.display().to_string());
            if dirty {
                ui.colored_label(egui::Color32::from_rgb(220, 150, 80), "● unsaved");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_fmt = kestrel_core::can_format(&path.to_string_lossy());
                if ui
                    .add_enabled(can_fmt, egui::Button::new("Format"))
                    .on_hover_text(
                        "Format with the language's formatter (rustfmt, prettier, black, gofmt, …)",
                    )
                    .clicked()
                {
                    do_format = true;
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("💾 Save"))
                    .on_hover_text("Ctrl+S")
                    .clicked()
                {
                    do_save = true;
                }
            });
        });
        if !self.editor_status.is_empty() {
            ui.label(egui::RichText::new(&self.editor_status).weak());
        }

        if !self.editor_symbols.is_empty() {
            egui::CollapsingHeader::new(format!("Outline ({} symbols)", self.editor_symbols.len()))
                .id_source("editor-outline")
                .show(ui, |ui| {
                    for symbol in &self.editor_symbols {
                        let vis = if symbol.exported { "+" } else { "-" };
                        ui.label(
                            egui::RichText::new(format!(
                                "{vis} {:<9} {}  @{}",
                                symbol.kind.as_str(),
                                symbol.name,
                                symbol.line
                            ))
                            .monospace(),
                        );
                    }
                });
        }

        // Inline diagnostics for this file, if a check has been run.
        let here: Vec<&kestrel_core::Diagnostic> = self
            .diagnostics
            .iter()
            .filter(|d| self.project_path().join(&d.file) == path)
            .collect();
        if !here.is_empty() {
            egui::CollapsingHeader::new(format!("⚠ Problems in this file ({})", here.len()))
                .id_source("editor-diagnostics")
                .default_open(true)
                .show(ui, |ui| {
                    for d in &here {
                        let color = match d.severity {
                            kestrel_core::Severity::Error => egui::Color32::from_rgb(220, 100, 100),
                            kestrel_core::Severity::Warning => {
                                egui::Color32::from_rgb(220, 170, 90)
                            }
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "{} line {}: {}",
                                d.severity.icon(),
                                d.line,
                                d.message
                            ))
                            .color(color),
                        );
                    }
                });
        }
        ui.separator();

        let language = language_for_path(&path);
        let dark = ui.visuals().dark_mode;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
            let mut job = code_layout(text, language, dark, font.clone());
            job.wrap.max_width = wrap_width;
            ui.fonts(|f| f.layout_job(job))
        };
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.editor_text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(28)
                        .layouter(&mut layouter),
                );
            });

        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            do_save = true;
        }
        if do_save {
            self.save_file();
        }
        if do_format {
            self.format_current_file();
        }
    }

    /// The Diff review: a git-diff of everything the agent changed since the
    /// last commit, with Keep (commit) and Revert (discard) actions.
    fn diff_ui(&mut self, ui: &mut egui::Ui) {
        if self.diff_review.is_none() {
            let root = self.project_path();
            self.diff_review = Some(kestrel_core::git_review(&root));
            self.checkpoints = kestrel_core::git_log(&root, 15);
        }
        let mut refresh = false;
        let mut commit = false;
        let mut revert = false;
        let mut init = false;
        let mut test = false;
        let mut restore: Option<String> = None;

        {
            let review = self.diff_review.as_ref().unwrap();
            if !review.is_repo {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "This project isn't a git repository. Initialize one to review and \
                         snapshot the agent's changes at a glance.",
                    )
                    .weak(),
                );
                ui.add_space(4.0);
                if ui.button("git init").clicked() {
                    init = true;
                }
            } else {
                ui.horizontal(|ui| {
                    ui.strong(&review.summary);
                    if !review.files.is_empty() {
                        let (added, removed) = kestrel_core::diff_line_stats(&review.diff);
                        ui.label(
                            egui::RichText::new(format!("+{added}"))
                                .monospace()
                                .color(DIFF_ADD),
                        );
                        ui.label(
                            egui::RichText::new(format!("−{removed}"))
                                .monospace()
                                .color(DIFF_DEL),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⟳ Refresh").clicked() {
                            refresh = true;
                        }
                        let has_changes = !review.files.is_empty();
                        if ui
                            .add_enabled(has_changes, egui::Button::new("🧪 Test changes"))
                            .on_hover_text("run only the tests affected by these changes")
                            .clicked()
                        {
                            test = true;
                        }
                        if ui
                            .add_enabled(has_changes, egui::Button::new("✓ Keep (commit)"))
                            .on_hover_text("git add -A && commit")
                            .clicked()
                        {
                            commit = true;
                        }
                        if review.has_head
                            && ui
                                .add_enabled(has_changes, egui::Button::new("⟲ Revert all"))
                                .on_hover_text("discard all changes since the last commit")
                                .clicked()
                        {
                            revert = true;
                        }
                    });
                });
                if !self.diff_status.is_empty() {
                    ui.label(egui::RichText::new(&self.diff_status).weak());
                }

                if !review.secrets.is_empty() {
                    ui.add_space(2.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 90, 90),
                        format!(
                            "⚠ {} possible secret(s) in these changes — review before committing:",
                            review.secrets.len()
                        ),
                    );
                    for finding in &review.secrets {
                        ui.label(
                            egui::RichText::new(format!(
                                "   {}:{} — {}",
                                finding.path, finding.line, finding.kind
                            ))
                            .monospace()
                            .color(egui::Color32::from_rgb(220, 120, 120)),
                        );
                    }
                }

                if !self.checkpoints.is_empty() {
                    egui::CollapsingHeader::new(format!(
                        "Checkpoints ({}) — roll back a run",
                        self.checkpoints.len()
                    ))
                    .id_source("checkpoints")
                    .show(ui, |ui| {
                        for cp in &self.checkpoints {
                            ui.horizontal(|ui| {
                                if ui.small_button("Restore").clicked() {
                                    restore = Some(cp.id.clone());
                                }
                                ui.label(
                                    egui::RichText::new(format!("{} · {}", cp.id, cp.when)).weak(),
                                );
                                ui.label(&cp.summary);
                            });
                        }
                    });
                }
                ui.separator();

                if review.files.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "No changes since the last commit — nothing to review.",
                        )
                        .weak(),
                    );
                } else {
                    let text_color = ui.visuals().text_color();
                    let per_file = kestrel_core::diff_stats_by_file(&review.diff);
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for entry in &review.files {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(entry).monospace().weak());
                                    if let Some((added, removed)) =
                                        per_file.get(&kestrel_core::porcelain_path(entry))
                                    {
                                        ui.label(
                                            egui::RichText::new(format!("+{added}"))
                                                .monospace()
                                                .small()
                                                .color(DIFF_ADD),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!("−{removed}"))
                                                .monospace()
                                                .small()
                                                .color(DIFF_DEL),
                                        );
                                    }
                                });
                            }
                            ui.separator();
                            for line in review.diff.lines() {
                                let color = diff_line_color(line, text_color);
                                ui.label(egui::RichText::new(line).monospace().color(color));
                            }
                        });
                }
            }
        }

        if init {
            match kestrel_core::git_init(&self.project_path()) {
                Ok(()) => self.diff_status = "Initialized a git repository.".to_string(),
                Err(err) => self.diff_status = format!("git init failed: {err}"),
            }
            self.diff_review = None;
        }
        if commit {
            match kestrel_core::git_commit_all(&self.project_path(), "Kestrel: snapshot changes") {
                Ok(_) => self.diff_status = "Committed — changes kept.".to_string(),
                Err(err) => self.diff_status = format!("Commit failed: {err}"),
            }
            self.diff_review = None;
        }
        if revert {
            self.confirm_revert = true;
        }
        if let Some(id) = restore {
            self.restore_target = Some(id);
        }
        if test {
            self.run_affected_tests();
        }
        if refresh {
            self.diff_status.clear();
            self.diff_review = None;
        }
    }

    /// Select and run only the tests affected by the current changes, showing
    /// the command and its output on the Output tab.
    fn run_affected_tests(&mut self) {
        let root = self.project_path();
        let changed = self
            .diff_review
            .as_ref()
            .map(|r| r.paths.clone())
            .unwrap_or_default();
        self.central = CentralView::Output;
        self.spawn(move || {
            let selection = kestrel_core::select_tests(&root, &changed);
            match selection.command {
                Some(command) => {
                    let output = kestrel_core::run_shell_command(&root, &command, 300);
                    JobOutcome::Text {
                        output: format!("$ {command}\n\n{output}"),
                        status: selection.note,
                    }
                }
                None => {
                    let files = if selection.test_files.is_empty() {
                        "(none)".to_string()
                    } else {
                        selection
                            .test_files
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    JobOutcome::Text {
                        output: format!(
                            "{}\n\nAffected test files:\n{files}\n\n(No runner command could be \
                             built automatically — run these yourself.)",
                            selection.note
                        ),
                        status: selection.note,
                    }
                }
            }
        });
    }

    /// Load this project's agent audit log into the Output tab.
    fn show_audit_log(&mut self) {
        let path = kestrel_core::audit_log_path(&self.project_path());
        self.output = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| "No audit log yet for this project.".to_string());
        self.central = CentralView::Output;
        self.status = format!("Audit log: {}", path.display());
    }

    /// Confirm before discarding the agent's changes.
    fn revert_modal(&mut self, ctx: &egui::Context) {
        if !self.confirm_revert {
            return;
        }
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Revert all changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Discard every uncommitted change and remove new files?");
                ui.colored_label(
                    egui::Color32::from_rgb(220, 150, 80),
                    "This resets the project to the last commit and cannot be undone.",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Revert").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel || !open {
            self.confirm_revert = false;
            return;
        }
        if confirm {
            match kestrel_core::git_revert_all(&self.project_path()) {
                Ok(msg) => self.diff_status = msg,
                Err(err) => self.diff_status = format!("Revert failed: {err}"),
            }
            self.confirm_revert = false;
            self.diff_review = None;
            self.reload_tree();
        }
    }

    /// Accumulate real token usage into the session total (and its cost), and
    /// append a record to the project's usage log for the dashboard.
    fn add_usage(&mut self, usage: &kestrel_core::Usage) {
        self.session_usage.add(usage);
        self.last_usage = *usage;
        let (provider_name, model) = match self.settings.active() {
            Some(p) => (
                self.settings.active_provider.clone().unwrap_or_default(),
                p.model.clone(),
            ),
            None => (String::new(), String::new()),
        };
        let req_cost = kestrel_core::model_price(&model)
            .map(|price| kestrel_core::cost_of_usage(price, usage))
            .unwrap_or(0.0);
        self.session_cost += req_cost;
        if usage.total_input() > 0 || usage.output_tokens > 0 {
            self.today_cost += req_cost;
            let record = kestrel_core::UsageRecord {
                ts: kestrel_core::now_epoch(),
                provider: provider_name,
                model,
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                cost: req_cost,
            };
            kestrel_core::append_usage_record(&self.project_path(), &record);
        }
    }

    /// If a budget cap has been reached, the reason to stop; else `None`.
    fn budget_blocked(&self) -> Option<String> {
        if let Some(limit) = self.settings.budget.session_limit {
            if limit > 0.0 && self.session_cost >= limit {
                return Some(format!(
                    "session budget of ${:.2} reached (${:.2} spent) — raise it in Settings or \
                     start a New chat",
                    limit, self.session_cost
                ));
            }
        }
        if let Some(limit) = self.settings.budget.daily_limit {
            if limit > 0.0 && self.today_cost >= limit {
                return Some(format!(
                    "daily budget of ${:.2} reached (${:.2} today) — raise it in Settings",
                    limit, self.today_cost
                ));
            }
        }
        None
    }

    /// Run the project's checker on a worker thread and show the Problems tab.
    fn run_diagnostics(&mut self) {
        let root = self.project_path();
        if kestrel_core::checker_name(&root).is_none() {
            self.diagnostics.clear();
            self.central = CentralView::Problems;
            self.status =
                "No supported checker (cargo/tsc/ruff) detected for this project.".to_string();
            return;
        }
        self.spawn(move || {
            let items = kestrel_core::run_diagnostics(&root);
            let status = if items.is_empty() {
                "No problems found. ✓".to_string()
            } else {
                format!("{} problem(s) found.", items.len())
            };
            JobOutcome::Diagnostics { items, status }
        });
    }

    /// The Problems tab: the checker's diagnostics; click one to open its file.
    fn problems_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let checker = kestrel_core::checker_name(&self.project_path());
        let mut recheck = false;
        ui.horizontal(|ui| {
            ui.strong(format!("Problems ({})", self.diagnostics.len()));
            if let Some(c) = checker {
                ui.label(egui::RichText::new(format!("· {c}")).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚠ Re-check").clicked() {
                    recheck = true;
                }
            });
        });
        ui.separator();
        if self.diagnostics.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No problems. Run a check with the ⚠ Check button in the action bar.",
                )
                .weak(),
            );
        } else {
            let root = self.project_path();
            let mut open: Option<PathBuf> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for d in &self.diagnostics {
                        let color = match d.severity {
                            kestrel_core::Severity::Error => egui::Color32::from_rgb(220, 100, 100),
                            kestrel_core::Severity::Warning => {
                                egui::Color32::from_rgb(220, 170, 90)
                            }
                        };
                        let label = format!(
                            "{} {}:{}:{}  {}",
                            d.severity.icon(),
                            d.file,
                            d.line,
                            d.col,
                            d.message
                        );
                        if ui
                            .selectable_label(false, egui::RichText::new(label).color(color))
                            .clicked()
                        {
                            open = Some(root.join(&d.file));
                        }
                    }
                });
            if let Some(path) = open {
                self.open_file(&path);
            }
        }
        if recheck {
            self.run_diagnostics();
        }
    }

    fn refresh_apps(&mut self) {
        let root = self.project_path();
        self.run_apps = kestrel_core::running_apps(&root);
        // Auto-fill the preview URL from a server that printed its address.
        if self.run_url.trim().is_empty() {
            if let Some(url) = self.run_apps.iter().find_map(|a| a.url.clone()) {
                self.run_url = url;
            }
        }
        self.run_shots = kestrel_core::list_screenshots(&root);
    }

    /// The Run tab: start/stop the app, watch its logs, and open a preview.
    fn run_ui(&mut self, ui: &mut egui::Ui) {
        if self.run_command_input.is_empty() {
            self.run_command_input = detect_run_command(&self.project_path());
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Command:");
            ui.add(
                egui::TextEdit::singleline(&mut self.run_command_input)
                    .desired_width(340.0)
                    .hint_text("npm run dev"),
            );
            if ui.button("▶ Start").clicked() {
                self.run_status =
                    kestrel_core::start_app_detached(&self.project_path(), &self.run_command_input);
                self.refresh_apps();
            }
            if ui.button("⟳ Refresh").clicked() {
                self.refresh_apps();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Preview:");
            ui.add(
                egui::TextEdit::singleline(&mut self.run_url)
                    .desired_width(260.0)
                    .hint_text("http://localhost:3000"),
            );
            if ui.button("🖥 Open in browser").clicked() {
                let url = if self.run_url.trim().is_empty() {
                    "http://localhost:3000".to_string()
                } else {
                    self.run_url.clone()
                };
                self.run_status = kestrel_core::open_url(&url);
            }
        });
        if !self.run_status.is_empty() {
            ui.label(egui::RichText::new(&self.run_status).weak());
        }
        ui.separator();

        ui.strong(format!("Running apps ({})", self.run_apps.len()));
        if self.run_apps.is_empty() {
            ui.label(
                egui::RichText::new("Nothing running. Start the app above — or the agent will.")
                    .weak(),
            );
        }
        let mut view_log: Option<u32> = None;
        let mut stop: Option<u32> = None;
        let mut open: Option<String> = None;
        for app in &self.run_apps {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("pid {}", app.pid)).monospace());
                ui.label(&app.command);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Stop").clicked() {
                        stop = Some(app.pid);
                    }
                    if ui.small_button("Logs").clicked() {
                        view_log = Some(app.pid);
                    }
                    if let Some(url) = &app.url {
                        if ui.small_button("Open").on_hover_text(url).clicked() {
                            open = Some(url.clone());
                        }
                    }
                });
            });
        }
        if let Some(pid) = view_log {
            self.run_log = kestrel_core::app_logs(&self.project_path(), pid);
            self.run_selected_pid = Some(pid);
        }
        if let Some(url) = open {
            self.run_status = kestrel_core::open_url(&url);
        }
        if let Some(pid) = stop {
            self.run_status = kestrel_core::stop_app(&self.project_path(), pid);
            if self.run_selected_pid == Some(pid) {
                self.run_selected_pid = None;
                self.run_log.clear();
            }
            self.refresh_apps();
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!("Screenshots ({})", self.run_shots.len()));
            if ui.button("📸 Capture").clicked() {
                self.run_status = kestrel_core::take_screenshot(&self.project_path());
                self.run_shots = kestrel_core::list_screenshots(&self.project_path());
            }
        });
        let mut open_shot: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_source("screenshots")
            .max_height(120.0)
            .show(ui, |ui| {
                for shot in &self.run_shots {
                    ui.horizontal(|ui| {
                        if ui.small_button("Open").clicked() {
                            open_shot = Some(shot.display().to_string());
                        }
                        let name = shot
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        ui.label(egui::RichText::new(name).monospace().weak());
                    });
                }
            });
        if let Some(path) = open_shot {
            self.run_status = kestrel_core::open_path(&path);
        }

        if let Some(pid) = self.run_selected_pid {
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(format!("Logs — pid {pid}"));
                if ui.small_button("⟳").clicked() {
                    self.run_log = kestrel_core::app_logs(&self.project_path(), pid);
                }
            });
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&self.run_log).monospace())
                            .selectable(true),
                    );
                });
        }
    }

    /// Confirm before rolling the project back to an earlier checkpoint.
    fn restore_modal(&mut self, ctx: &egui::Context) {
        let Some(target) = self.restore_target.clone() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Restore checkpoint")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Roll the project back to checkpoint {target}?"));
                ui.colored_label(
                    egui::Color32::from_rgb(220, 150, 80),
                    "Every change after that point is discarded and new files are removed.",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Restore").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel || !open {
            self.restore_target = None;
            return;
        }
        if confirm {
            match kestrel_core::git_restore(&self.project_path(), &target) {
                Ok(msg) => self.diff_status = msg,
                Err(err) => self.diff_status = format!("Restore failed: {err}"),
            }
            self.restore_target = None;
            self.diff_review = None;
            self.reload_tree();
        }
    }

    /// The "New project" modal: choose a parent folder and a name, scaffold a
    /// Kestrel-ready project, then open it.
    fn new_project_modal(&mut self, ctx: &egui::Context) {
        if !self.new_project_open {
            return;
        }
        let mut open = self.new_project_open;
        let mut create = false;
        let mut cancel = false;
        egui::Window::new("New project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("new-project-grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Parent folder");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_project_parent)
                                    .desired_width(300.0)
                                    .hint_text("where to create the project"),
                            );
                            if ui.button("Browse…").clicked() {
                                if let Some(dir) = rfd::FileDialog::new()
                                    .set_title("Choose a parent folder")
                                    .pick_folder()
                                {
                                    self.new_project_parent = dir.display().to_string();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Project name");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_project_name)
                                .desired_width(300.0)
                                .hint_text("new-folder-name"),
                        );
                        ui.end_row();
                    });
                ui.add_space(6.0);
                if !self.new_project_status.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 90, 90),
                        &self.new_project_status,
                    );
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if create {
            let parent = PathBuf::from(self.new_project_parent.trim());
            match kestrel_core::create_project(&parent, &self.new_project_name) {
                Ok(project) => {
                    let git = if project.git_initialized {
                        " (git initialized)"
                    } else {
                        ""
                    };
                    self.new_project_open = false;
                    self.new_project_name.clear();
                    self.new_project_status.clear();
                    let root = project.root.clone();
                    self.open_project_path(root);
                    self.status = format!("Created {}{git}.", project.root.display());
                }
                Err(err) => self.new_project_status = format!("Could not create project: {err}"),
            }
            return;
        }
        if cancel {
            self.new_project_open = false;
            self.new_project_status.clear();
            return;
        }
        self.new_project_open = open;
    }

    /// Render the scrollable chat transcript.
    fn chat_history_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        if self.chat_history.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Ask about your project, or anything else. Turn on “Include project \
                     context” below to attach the most relevant files to your question.",
                )
                .weak(),
            );
        }
        for message in &self.chat_history {
            let (who, color) = if message.role == "user" {
                ("🧑 You", egui::Color32::from_rgb(120, 170, 255))
            } else {
                ("🦅 Kestrel", ACCENT)
            };
            ui.add_space(6.0);
            ui.label(egui::RichText::new(who).strong().color(color));
            ui.add(
                egui::Label::new(egui::RichText::new(&message.content).monospace())
                    .selectable(true),
            );
        }
        if self.chat_pending {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                let activity = if self.agent_activity.is_empty() {
                    "Kestrel is thinking…"
                } else {
                    self.agent_activity.as_str()
                };
                ui.label(egui::RichText::new(activity).strong());
            });
        }
    }

    /// The Usage dashboard: this conversation's tokens + cost, and all-time
    /// totals with the per-model breakdown and prompt-cache savings.
    fn usage_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading("📊 Usage & cost");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.usage_records = kestrel_core::load_usage_records(&self.project_path());
                }
                if ui
                    .add_enabled(
                        !self.usage_records.is_empty(),
                        egui::Button::new("⬇ Export CSV"),
                    )
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("kestrel-usage.csv")
                        .save_file()
                    {
                        let csv = kestrel_core::usage_csv(&self.usage_records);
                        match std::fs::write(&path, csv) {
                            Ok(()) => self.status = format!("Exported usage to {}", path.display()),
                            Err(err) => self.status = format!("Export failed: {err}"),
                        }
                    }
                }
            });
        });
        ui.label(
            egui::RichText::new(
                "Real tokens billed by the provider, logged per request to \
                 .kestrel/usage.jsonl.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        // This conversation (in-memory).
        let s = self.session_usage;
        let saved_session = kestrel_core::model_price(
            self.settings
                .active()
                .map(|p| p.model.as_str())
                .unwrap_or(""),
        )
        .map(|p| s.cache_read as f64 * p.input_per_million * 0.9 / 1_000_000.0)
        .unwrap_or(0.0);
        ui.group(|ui| {
            ui.strong("This conversation");
            ui.add_space(2.0);
            ui.label(format!(
                "{} input · {} output · {} cached",
                human_tokens(s.total_input() as usize),
                human_tokens(s.output_tokens as usize),
                human_tokens(s.cache_read as usize),
            ));
            ui.label(format!("Cost: ${:.4}", self.session_cost));
            if saved_session > 0.0 {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 190, 120),
                    format!(
                        "Prompt caching saved ~${saved_session:.4} ({} tokens read from cache)",
                        human_tokens(s.cache_read as usize)
                    ),
                );
            }
        });
        ui.add_space(10.0);

        // All-time (from the log).
        let summary = kestrel_core::summarize_usage(&self.usage_records);
        let t = &summary.totals;
        ui.group(|ui| {
            ui.strong(format!("All time · {} requests", t.requests));
            ui.add_space(2.0);
            ui.label(format!(
                "{} input · {} output · {} cached",
                human_tokens(t.input as usize),
                human_tokens(t.output as usize),
                human_tokens(t.cache_read as usize),
            ));
            ui.label(format!("Cost: ${:.4}", t.cost));
            if summary.saved_cost > 0.0 {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 190, 120),
                    format!(
                        "Prompt caching saved ~${:.4} ({} tokens) — that's off your bill.",
                        summary.saved_cost,
                        human_tokens(summary.saved_tokens as usize)
                    ),
                );
            }
        });
        ui.add_space(10.0);

        if summary.by_model.is_empty() {
            ui.label(egui::RichText::new("No usage recorded yet for this project.").weak());
            return;
        }
        ui.strong("By model");
        ui.add_space(4.0);
        egui::Grid::new("usage-by-model")
            .num_columns(5)
            .striped(true)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Model");
                ui.strong("Requests");
                ui.strong("Input");
                ui.strong("Output");
                ui.strong("Cost");
                ui.end_row();
                for (model, totals) in &summary.by_model {
                    ui.label(model);
                    ui.label(totals.requests.to_string());
                    ui.label(human_tokens((totals.input + totals.cache_read) as usize));
                    ui.label(human_tokens(totals.output as usize));
                    ui.label(format!("${:.4}", totals.cost));
                    ui.end_row();
                }
            });
    }

    /// The Workflows view: pick a recipe, fill any parameters, and run it as a
    /// verified agent run on the current project.
    fn workflows_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading("⚡ Workflows");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("⭱ Export…")
                    .on_hover_text("Save your workflows to a shareable file")
                    .clicked()
                {
                    self.export_workflows();
                }
                if ui
                    .button("⭳ Import…")
                    .on_hover_text("Import workflows someone shared with you")
                    .clicked()
                {
                    self.import_workflows();
                }
                if ui
                    .button("＋ New")
                    .on_hover_text("Author a workflow")
                    .clicked()
                {
                    self.wf_editor = Some(WorkflowDraft::default());
                }
                ui.toggle_value(&mut self.show_catalog, "🛍 Catalog");
            });
        });
        ui.label(
            egui::RichText::new(
                "Named, verified agent recipes. Running one starts the agent on the current \
                 project — with checkpoints, verification, policy, and your budget applied.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        let user_ids: std::collections::HashSet<String> = kestrel_core::load_user_workflows()
            .into_iter()
            .map(|w| w.id)
            .collect();

        enum WfAction {
            Run(
                kestrel_core::Workflow,
                std::collections::BTreeMap<String, String>,
            ),
            Edit(kestrel_core::Workflow),
            Remove(String),
            Install(kestrel_core::Workflow),
        }
        let mut action: Option<WfAction> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.show_catalog {
                    self.catalog_ui(ui, &mut |a| action = Some(a), &|wf| WfAction::Install(wf));
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.strong("Your workflows");
                    ui.add_space(6.0);
                }

                let workflows = self.workflows.clone();
                for wf in &workflows {
                    let is_user = user_ids.contains(&wf.id);
                    let is_builtin = kestrel_core::is_builtin_workflow(&wf.id);
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(&wf.name);
                            if is_user && is_builtin {
                                ui.label(egui::RichText::new("· customized").weak().small());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("▶ Run").clicked() {
                                        let mut values = std::collections::BTreeMap::new();
                                        for p in &wf.params {
                                            let key = format!("{}::{}", wf.id, p);
                                            values.insert(
                                                p.clone(),
                                                self.workflow_params
                                                    .get(&key)
                                                    .cloned()
                                                    .unwrap_or_default(),
                                            );
                                        }
                                        action = Some(WfAction::Run(wf.clone(), values));
                                    }
                                    if ui.button("✎").on_hover_text("Edit").clicked() {
                                        action = Some(WfAction::Edit(wf.clone()));
                                    }
                                    // User workflows can be removed; a customized
                                    // built-in reverts to its default; pure
                                    // built-ins have nothing to remove.
                                    if is_user {
                                        let hint = if is_builtin {
                                            "Reset to the built-in default"
                                        } else {
                                            "Delete this workflow"
                                        };
                                        if ui.button("🗑").on_hover_text(hint).clicked() {
                                            action = Some(WfAction::Remove(wf.id.clone()));
                                        }
                                    }
                                },
                            );
                        });
                        ui.label(egui::RichText::new(&wf.description).weak());
                        if !wf.params.is_empty() {
                            ui.add_space(2.0);
                            egui::Grid::new(format!("wf-params-{}", wf.id))
                                .num_columns(2)
                                .spacing([10.0, 4.0])
                                .show(ui, |ui| {
                                    for p in &wf.params {
                                        ui.label(p);
                                        let key = format!("{}::{}", wf.id, p);
                                        let entry = self.workflow_params.entry(key).or_default();
                                        ui.add(
                                            egui::TextEdit::singleline(entry)
                                                .desired_width(360.0)
                                                .hint_text(format!("{p}…")),
                                        );
                                        ui.end_row();
                                    }
                                });
                        }
                    });
                    ui.add_space(8.0);
                }
            });

        match action {
            Some(WfAction::Run(wf, values)) => self.run_workflow(&wf, &values),
            Some(WfAction::Edit(wf)) => self.wf_editor = Some(draft_from(&wf)),
            Some(WfAction::Remove(id)) => {
                if let Err(err) = kestrel_core::remove_user_workflow(&id) {
                    self.status = format!("Could not remove workflow: {err}");
                } else {
                    self.reload_workflows();
                    self.status = "Workflow removed.".to_string();
                }
            }
            Some(WfAction::Install(wf)) => {
                if let Err(err) = kestrel_core::install_workflow(&wf) {
                    self.status = format!("Could not install workflow: {err}");
                } else {
                    self.reload_workflows();
                    self.status = format!("Installed “{}”.", wf.name);
                }
            }
            None => {}
        }
    }

    /// The catalog gallery: ready-made recipes not yet installed, each with an
    /// Install button. `emit` records the chosen action (kept generic so the
    /// caller owns the action enum).
    fn catalog_ui<T>(
        &self,
        ui: &mut egui::Ui,
        emit: &mut dyn FnMut(T),
        install: &dyn Fn(kestrel_core::Workflow) -> T,
    ) {
        ui.strong("🛍 Catalog");
        ui.label(
            egui::RichText::new("Ready-made recipes — install one to add it to your workflows.")
                .weak()
                .small(),
        );
        ui.add_space(4.0);
        let installed: std::collections::HashSet<String> =
            self.workflows.iter().map(|w| w.id.clone()).collect();
        let available: Vec<kestrel_core::Workflow> = kestrel_core::catalog_workflows()
            .into_iter()
            .filter(|w| !installed.contains(&w.id))
            .collect();
        if available.is_empty() {
            ui.label(
                egui::RichText::new("Everything in the catalog is already installed. 🎉").weak(),
            );
            return;
        }
        for wf in available {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(&wf.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⬇ Install").clicked() {
                            emit(install(wf.clone()));
                        }
                    });
                });
                ui.label(egui::RichText::new(&wf.description).weak());
            });
            ui.add_space(6.0);
        }
    }

    /// Reload the merged workflow list (built-ins + user).
    fn reload_workflows(&mut self) {
        self.workflows = kestrel_core::all_workflows();
    }

    /// Import workflows from a shared `.toml` file the user picks.
    fn import_workflows(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Workflow file", &["toml"])
            .set_title("Import workflows")
            .pick_file()
        {
            match kestrel_core::import_workflows_from(&path) {
                Ok(0) => self.status = "No workflows found in that file.".to_string(),
                Ok(n) => {
                    self.reload_workflows();
                    self.status = format!("Imported {n} workflow(s).");
                }
                Err(err) => self.status = format!("Import failed: {err}"),
            }
        }
    }

    /// Export the user's own workflows to a shareable `.toml` file.
    fn export_workflows(&mut self) {
        let user = kestrel_core::load_user_workflows();
        if user.is_empty() {
            self.status =
                "No personal workflows yet — install one from the Catalog or create one first."
                    .to_string();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Workflow file", &["toml"])
            .set_title("Export workflows")
            .set_file_name("kestrel-workflows.toml")
            .save_file()
        {
            match kestrel_core::export_workflows_to(&path, &user) {
                Ok(()) => self.status = format!("Exported {} workflow(s).", user.len()),
                Err(err) => self.status = format!("Export failed: {err}"),
            }
        }
    }

    /// The author/edit workflow window; open when `wf_editor` is `Some`.
    fn workflow_editor(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.wf_editor.take() else {
            return;
        };
        let editing = !draft.id.is_empty();
        let mut open = true;
        let mut close = false;
        egui::Window::new(if editing {
            "Edit workflow"
        } else {
            "New workflow"
        })
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut draft.name);
            ui.add_space(4.0);
            ui.label("Description");
            ui.text_edit_singleline(&mut draft.description);
            ui.add_space(4.0);
            ui.label("Parameters (comma-separated, referenced as {name} in the prompt)");
            ui.text_edit_singleline(&mut draft.params);
            ui.add_space(4.0);
            ui.label("Prompt — the instruction the agent runs");
            ui.add(
                egui::TextEdit::multiline(&mut draft.prompt)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            if !draft.status.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(0xd9, 0x53, 0x4f), &draft.status);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("💾 Save").clicked() {
                    match build_workflow(&draft) {
                        Ok(wf) => {
                            if let Err(err) = kestrel_core::install_workflow(&wf) {
                                draft.status = format!("Could not save: {err}");
                            } else {
                                self.reload_workflows();
                                self.status = format!("Saved “{}”.", wf.name);
                                close = true;
                            }
                        }
                        Err(msg) => draft.status = msg,
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if open && !close {
            self.wf_editor = Some(draft);
        }
    }

    /// Fill a workflow's prompt and run it as an agent build on the project.
    fn run_workflow(
        &mut self,
        wf: &kestrel_core::Workflow,
        values: &std::collections::BTreeMap<String, String>,
    ) {
        for p in &wf.params {
            if values.get(p).map(|v| v.trim().is_empty()).unwrap_or(true) {
                self.status = format!("Fill in '{p}' before running “{}”.", wf.name);
                return;
            }
        }
        // Skill pack: drop the workflow's template files into the project first,
        // so the agent's prompt can reference and adapt them.
        if !wf.resources.is_empty() {
            match kestrel_core::materialize_resources(&self.project_path(), &wf.resources) {
                Ok(written) if !written.is_empty() => {
                    self.chat_history
                        .push(kestrel_core::ChatMessage::assistant(format!(
                            "📦 Added starter file(s): {}",
                            written.join(", ")
                        )));
                }
                Ok(_) => {}
                Err(err) => self.status = format!("Could not add template files: {err}"),
            }
        }
        self.chat_input = wf.fill(values);
        self.chat_agent_mode = true;
        self.view = AppView::Chat;
        self.send_chat();
    }

    /// The build-preview panel: a live, clickable history of the files the
    /// agent is creating, with a preview of the selected one.
    /// Kestrel Work's left panel: which folder it's scoped to, and what's in it.
    /// Files open in their default app (Word, Excel, a PDF viewer, …).
    fn work_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("💼 Kestrel Work");
        });
        ui.label(
            egui::RichText::new(
                "Research, write, and work with your documents and data — in this folder.",
            )
            .weak()
            .small(),
        );
        ui.add_space(6.0);

        let root = PathBuf::from(self.work_folder.trim());
        let mut switch_to: Option<String> = None;
        ui.horizontal(|ui| {
            if ui
                .button("📂 Folder…")
                .on_hover_text("Choose any folder for Kestrel Work to read and write")
                .clicked()
            {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Choose your Work folder")
                    .set_directory(&root)
                    .pick_folder()
                {
                    switch_to = Some(dir.display().to_string());
                }
            }
            if ui.button("↗").on_hover_text("Open in Explorer").clicked() {
                let _ = kestrel_core::open_path(&root.display().to_string());
            }
            // Quick-switch between folders used before.
            if !self.settings.work_recents.is_empty() {
                egui::ComboBox::from_id_source("work-recents")
                    .selected_text("Recent")
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for recent in self.settings.work_recents.clone() {
                            let label = PathBuf::from(&recent)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| recent.clone());
                            if ui
                                .selectable_label(recent == self.work_folder, label)
                                .on_hover_text(&recent)
                                .clicked()
                            {
                                switch_to = Some(recent.clone());
                            }
                        }
                    });
            }
        });
        ui.label(
            egui::RichText::new(self.work_folder.clone())
                .monospace()
                .small()
                .weak(),
        );
        if let Some(dir) = switch_to {
            self.set_work_folder(dir);
        }
        ui.separator();

        if !root.is_dir() {
            ui.label(
                egui::RichText::new("That folder doesn't exist — choose another.")
                    .color(egui::Color32::from_rgb(220, 150, 80)),
            );
            return;
        }

        // A flat listing of the workspace: folders first, then files.
        let mut dirs: Vec<String> = Vec::new();
        let mut files: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if entry.path().is_dir() {
                    dirs.push(name);
                } else {
                    files.push(name);
                }
            }
        }
        dirs.sort_by_key(|d| d.to_lowercase());
        files.sort_by_key(|f| f.to_lowercase());

        let mut open: Option<PathBuf> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if dirs.is_empty() && files.is_empty() {
                    ui.label(
                        egui::RichText::new("This folder is empty. Ask for something below.")
                            .weak(),
                    );
                }
                for d in &dirs {
                    if ui.selectable_label(false, format!("📁 {d}")).clicked() {
                        open = Some(root.join(d));
                    }
                }
                for f in &files {
                    if ui
                        .selectable_label(false, format!("{} {f}", doc_icon(f)))
                        .on_hover_text("Open in its default app")
                        .clicked()
                    {
                        open = Some(root.join(f));
                    }
                }
            });
        if let Some(path) = open {
            let _ = kestrel_core::open_path(&path.display().to_string());
        }
    }

    /// The live task plan (TODO ledger): the agent's checklist with progress,
    /// shown above the created-files list so the user can watch it self-direct.
    fn plan_ui(&self, ui: &mut egui::Ui) {
        let Some(plan) = &self.agent_plan else {
            return;
        };
        if plan.steps.is_empty() {
            return;
        }
        let (done, total) = plan.progress();
        egui::CollapsingHeader::new(format!("🗺 Plan — {done}/{total} done"))
            .default_open(true)
            .id_source("agent-plan")
            .show(ui, |ui| {
                if !plan.goal.trim().is_empty() {
                    ui.label(egui::RichText::new(&plan.goal).weak().italics());
                    ui.add_space(2.0);
                }
                for step in &plan.steps {
                    let (glyph, text) = match step.status {
                        kestrel_core::StepStatus::Done => {
                            ("☑", egui::RichText::new(&step.title).strikethrough().weak())
                        }
                        kestrel_core::StepStatus::Active => {
                            ("▶", egui::RichText::new(&step.title).strong().color(ACCENT))
                        }
                        kestrel_core::StepStatus::Todo => ("☐", egui::RichText::new(&step.title)),
                    };
                    ui.horizontal(|ui| {
                        ui.label(glyph);
                        ui.label(text);
                    });
                }
            });
        ui.add_space(6.0);
        ui.separator();
    }

    fn build_preview_ui(&mut self, ui: &mut egui::Ui) {
        self.plan_ui(ui);
        ui.horizontal(|ui| {
            ui.strong(format!("Files created ({})", self.agent_files.len()));
            if self.chat_pending {
                ui.spinner();
            }
        });
        ui.separator();
        if self.agent_files.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Turn on Agent mode and Build. Every file the agent writes appears here \
                     live — click one to preview exactly what it wrote.",
                )
                .weak(),
            );
            return;
        }

        let mut select: Option<usize> = None;
        let mut open_in_editor: Option<String> = None;

        egui::ScrollArea::vertical()
            .id_source("agent-file-list")
            .max_height(150.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, file) in self.agent_files.iter().enumerate() {
                    if ui
                        .selectable_label(
                            self.agent_preview == Some(i),
                            format!("📄 {}", file.path),
                        )
                        .clicked()
                    {
                        select = Some(i);
                    }
                }
            });

        ui.separator();

        if let Some(idx) = self.agent_preview {
            if let Some(file) = self.agent_files.get(idx) {
                ui.horizontal(|ui| {
                    ui.strong(&file.path);
                    ui.label(
                        egui::RichText::new(format!("· {} lines", file.contents.lines().count()))
                            .weak(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Open in editor").clicked() {
                            open_in_editor = Some(file.path.clone());
                        }
                    });
                });
                let language = language_for(&file.path);
                let dark = ui.visuals().dark_mode;
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let job = code_layout(&file.contents, language, dark, font);
                egui::ScrollArea::both()
                    .id_source("agent-file-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(job).selectable(true));
                    });
            }
        }

        if let Some(i) = select {
            self.agent_preview = Some(i);
        }
        if let Some(path) = open_in_editor {
            let full = self.project_path().join(path);
            self.open_file(&full);
            self.view = AppView::Main;
        }
    }

    /// Render the compose bar: provider status, controls, and the input.
    fn chat_compose_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let mut new_active: Option<String> = None;
        let mut new_model: Option<String> = None;
        ui.horizontal(|ui| {
            if self.settings.providers.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 150, 80),
                    "No provider — add one in Settings.",
                );
            } else {
                // Quick provider switch.
                let active = self.settings.active_provider.clone().unwrap_or_default();
                egui::ComboBox::from_id_source("chat-provider")
                    .selected_text(if active.is_empty() {
                        "(none)".to_string()
                    } else {
                        active.clone()
                    })
                    .show_ui(ui, |ui| {
                        for name in self.settings.providers.keys().cloned().collect::<Vec<_>>() {
                            if ui.selectable_label(active == name, &name).clicked() {
                                new_active = Some(name);
                            }
                        }
                    });
                // Quick model switch for the active provider.
                let model_info = self
                    .settings
                    .active()
                    .map(|p| (kestrel_core::model_suggestions_for(p), p.model.clone()));
                if let Some((suggestions, current)) = model_info {
                    egui::ComboBox::from_id_source("chat-model")
                        .selected_text(current.clone())
                        .show_ui(ui, |ui| {
                            for m in suggestions {
                                if ui.selectable_label(current == *m, *m).clicked() {
                                    new_model = Some(m.to_string());
                                }
                            }
                        });
                }
            }
            ui.separator();
            ui.checkbox(&mut self.chat_include_context, "Include project context");
            ui.checkbox(&mut self.chat_agent_mode, "Agent · write files")
                .on_hover_text(
                    "Turn the request into real files written into the current project.",
                );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("New chat")
                    .on_hover_text("Stop any running agent and start a fresh conversation")
                    .clicked()
                {
                    self.stop_agent();
                    self.chat_job = None;
                    self.end_agent_run();
                    self.chat_history.clear();
                    self.chat_error.clear();
                    self.chat_input.clear();
                    self.agent_files.clear();
                    self.agent_preview = None;
                    self.agent_messages.clear();
                    self.agent_incomplete = false;
                    self.agent_plan = None;
                    kestrel_core::clear_plan(&self.project_path());
                    self.session_usage = kestrel_core::Usage::default();
                    self.session_cost = 0.0;
                    self.save_session();
                }
                if self.chat_pending {
                    if ui
                        .button("⏹ Stop")
                        .on_hover_text("Halt the agent at the next step (you can Continue after)")
                        .clicked()
                    {
                        // A plain chat can't be resumed; the agent can.
                        if self.agent_job.is_some() {
                            self.stop_agent();
                        } else {
                            self.chat_job = None;
                            self.chat_pending = false;
                            self.agent_activity.clear();
                            self.chat_error = "Cancelled.".to_string();
                        }
                    }
                } else if self.agent_incomplete
                    && ui
                        .button("▶ Continue")
                        .on_hover_text("Resume the agent from where it paused")
                        .clicked()
                {
                    self.agent_incomplete = false;
                    self.chat_input = "Continue from where you left off.".to_string();
                    self.chat_agent_mode = true;
                    self.send_chat();
                }
            });
        });
        if let Some(name) = new_active {
            self.settings.active_provider = Some(name);
            let _ = kestrel_core::save_settings(&self.settings);
        }
        if let Some(model) = new_model {
            if let Some(active) = self.settings.active_provider.clone() {
                if let Some(provider) = self.settings.providers.get_mut(&active) {
                    provider.model = model;
                }
                let _ = kestrel_core::save_settings(&self.settings);
            }
        }

        // Real-time context gauge + session token/cost meter.
        if let Some(provider) = self.settings.active() {
            let model = provider.model.clone();
            let window = kestrel_core::model_context_window(&model) as usize;
            let ctx = if !self.agent_messages.is_empty() {
                kestrel_core::history_tokens(&self.agent_messages)
            } else {
                self.chat_history
                    .iter()
                    .map(|m| m.content.len())
                    .sum::<usize>()
                    / 4
            };
            let pct = ctx.saturating_mul(100).checked_div(window).unwrap_or(0);
            let gauge_color = if pct >= 85 {
                egui::Color32::from_rgb(220, 90, 90)
            } else if pct >= 60 {
                egui::Color32::from_rgb(220, 150, 80)
            } else {
                ui.visuals().weak_text_color()
            };
            let usage = self.session_usage;
            let cache_note = if usage.cache_read > 0 {
                format!(" · {} cached", human_tokens(usage.cache_read as usize))
            } else {
                String::new()
            };
            let cost_note = if self.session_cost > 0.0 {
                format!(" · ${:.4}", self.session_cost)
            } else if kestrel_core::model_price(&model).is_none() {
                " · $ n/a".to_string()
            } else {
                String::new()
            };
            let saved = kestrel_core::model_price(&model)
                .map(|p| usage.cache_read as f64 * p.input_per_million * 0.9 / 1_000_000.0)
                .unwrap_or(0.0);
            let last = self.last_usage;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "context {} / {} ({pct}%)",
                        human_tokens(ctx),
                        human_tokens(window)
                    ))
                    .color(gauge_color),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "session {} in · {} out{cache_note}{cost_note}",
                        human_tokens(usage.total_input() as usize),
                        human_tokens(usage.output_tokens as usize),
                    ))
                    .weak(),
                );
                if saved > 0.0 {
                    ui.separator();
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 190, 120),
                        format!("cache saved ~${saved:.4}"),
                    );
                }
                if last.total_input() > 0 || last.output_tokens > 0 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "last {} / {}",
                            human_tokens(last.total_input() as usize),
                            human_tokens(last.output_tokens as usize)
                        ))
                        .weak(),
                    );
                }
            });
        }

        // Budget status, if any cap is set.
        let budget = &self.settings.budget;
        if budget.session_limit.is_some() || budget.daily_limit.is_some() {
            let mut parts = Vec::new();
            if let Some(limit) = budget.session_limit {
                parts.push(format!("session ${:.2} / ${:.2}", self.session_cost, limit));
            }
            if let Some(limit) = budget.daily_limit {
                parts.push(format!("today ${:.2} / ${:.2}", self.today_cost, limit));
            }
            let color = if self.budget_blocked().is_some() {
                egui::Color32::from_rgb(220, 90, 90)
            } else {
                ui.visuals().weak_text_color()
            };
            ui.colored_label(color, format!("Budget: {}", parts.join(" · ")));
        }

        if self.chat_agent_mode {
            let continuing = if self.agent_messages.is_empty() {
                String::new()
            } else {
                "  ·  continuing this project (New chat to start fresh)".to_string()
            };
            ui.colored_label(
                egui::Color32::from_rgb(150, 200, 150),
                format!(
                    "Agent mode: files will be written into {}{continuing}",
                    self.agent_root().display()
                ),
            );
        }
        if !self.chat_error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), &self.chat_error);
        }

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let hint = if self.chat_agent_mode {
                "Describe what to build…  (Enter to send, Shift+Enter for a new line)"
            } else {
                "Message…  (Enter to send, Shift+Enter for a new line)"
            };
            let input = ui.add(
                egui::TextEdit::multiline(&mut self.chat_input)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text(hint),
            );
            // Enter sends; Shift+Enter inserts a newline (handled by the widget).
            let enter_send = input.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            let label = if self.chat_agent_mode {
                "Build"
            } else {
                "Send"
            };
            let clicked = ui
                .add_enabled(!self.chat_pending, egui::Button::new(label))
                .clicked();
            if (enter_send || clicked) && !self.chat_pending {
                self.send_chat();
            }
        });
        ui.add_space(4.0);
    }

    /// Send the composed message to the active provider on a worker thread.
    fn send_chat(&mut self) {
        let text = self.chat_input.trim().to_string();
        if text.is_empty() || self.chat_pending {
            return;
        }
        let provider = match self.settings.active() {
            Some(p) => p.clone(),
            None => {
                self.chat_error = "No active provider — set one in Settings.".to_string();
                return;
            }
        };
        if provider.api_key.trim().is_empty() {
            self.chat_error =
                "The active provider has no API key — add one in Settings.".to_string();
            return;
        }
        if let Some(reason) = self.budget_blocked() {
            self.chat_error = format!("⛔ Over budget — {reason}.");
            return;
        }

        self.chat_error.clear();
        self.chat_input.clear();
        self.chat_history
            .push(kestrel_core::ChatMessage::user(text.clone()));
        self.chat_pending = true;

        if self.chat_agent_mode {
            self.start_agent(text, provider);
            return;
        }

        let config = provider.to_config();
        let model = provider.model.clone();
        // Snapshot the conversation before adding the placeholder reply.
        let messages = self.chat_history.clone();
        let include = self.chat_include_context;
        let project = self.project_path();
        // A placeholder assistant message that streamed tokens append to.
        self.chat_history
            .push(kestrel_core::ChatMessage::assistant(String::new()));

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let system = chat_system_prompt(include, &project, &text);
            let request = kestrel_core::ChatRequest {
                model,
                max_tokens: 2048,
                system: Some(system),
                messages,
            };
            let result = kestrel_core::chat_stream(&config, &request, |token| {
                let _ = tx.send(ChatUpdate::Token(token.to_string()));
            });
            let _ = match result {
                Ok(Ok((_text, usage))) => tx.send(ChatUpdate::Done(usage)),
                Ok(Err(err)) => tx.send(ChatUpdate::Failed(err)),
                Err(err) => tx.send(ChatUpdate::Failed(err.to_string())),
            };
        });
        self.chat_job = Some(rx);
    }

    /// Start the tool-using agent loop for `prompt` on a worker thread, relaying
    /// its progress to the transcript via `agent_job`.
    fn start_agent(&mut self, prompt: String, provider: kestrel_core::ProviderSettings) {
        let config = provider.to_config();
        let model = provider.model.clone();
        // Work runs in its own scoped folder; Build runs in the project.
        let root = self.agent_root();
        let profile = self.profile();
        // Merge the user's policy with any guardrails the project committed to
        // its kestrel.toml (union — most restrictive wins).
        let policy = kestrel_core::effective_policy(&root, &self.settings.policy);
        // Checkpoint the current state so this whole run can be rolled back —
        // and tell the user their uncommitted work was captured first.
        let label: String = prompt.chars().take(60).collect();
        if let Ok(true) = kestrel_core::git_checkpoint(&root, &label) {
            self.chat_history.push(kestrel_core::ChatMessage::assistant(
                "🔖 Checkpointed your current changes before starting — roll back any time from \
                 the Diff tab."
                    .to_string(),
            ));
        }
        self.diff_review = None;
        self.agent_activity = "💭 Planning…".to_string();
        self.agent_incomplete = false;
        // Carry the running conversation so a follow-up refines the same
        // project. The file history keeps accumulating across builds too.
        let history = self.agent_messages.clone();

        // Cancellation token: Stop flips this and the worker halts at the next step.
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.agent_cancel = Some(cancel.clone());
        // Permission channel: the worker blocks on this while a prompt is open.
        let (decision_tx, decision_rx) = std::sync::mpsc::channel::<ApprovalDecision>();
        self.agent_decision = Some(decision_tx);
        let ask_permission = self.ask_permission;

        let (tx, rx) = std::sync::mpsc::channel();
        let events = tx.clone();
        let approve_events = tx.clone();
        std::thread::spawn(move || {
            let mut allow_all = false;
            let outcome = kestrel_core::run_agent(
                &config,
                &model,
                &prompt,
                &root,
                // A generous budget; hitting it now pauses gracefully (Continue),
                // instead of failing.
                250,
                true,
                &policy,
                &cancel,
                profile,
                history,
                |event| {
                    let update = match event {
                        kestrel_core::AgentEvent::Assistant(text) => AgentUpdate::Line(text),
                        kestrel_core::AgentEvent::Tool(call) => AgentUpdate::Activity(call),
                        kestrel_core::AgentEvent::Wrote { path, contents } => {
                            AgentUpdate::Wrote { path, contents }
                        }
                        kestrel_core::AgentEvent::Writing { path, contents } => {
                            AgentUpdate::Writing { path, contents }
                        }
                        kestrel_core::AgentEvent::Usage(usage) => AgentUpdate::Usage(usage),
                        kestrel_core::AgentEvent::Plan(plan) => AgentUpdate::Plan(plan),
                    };
                    let _ = events.send(update);
                },
                |call| {
                    // Permission gate. Auto-allow when the setting is off, once
                    // "Allow all" is chosen, or for anything but system-touching
                    // tools. Otherwise ask the user and block for their answer.
                    // Irreversible outward actions (sending mail) always ask.
                    let always = tool_always_needs_permission(&call.name);
                    if !always && (!ask_permission || allow_all) {
                        return true;
                    }
                    if !tool_needs_permission(&call.name) {
                        return true;
                    }
                    let _ = approve_events.send(AgentUpdate::ApprovalRequest(
                        kestrel_core::describe_call(call),
                    ));
                    match decision_rx.recv() {
                        Ok(ApprovalDecision::Allow) => true,
                        Ok(ApprovalDecision::AllowAll) => {
                            allow_all = true;
                            true
                        }
                        // Denied, or the app/channel went away → decline safely.
                        Ok(ApprovalDecision::Deny) | Err(_) => false,
                    }
                },
            );
            let history = outcome.history;
            let _ = match outcome.result {
                Ok(summary) if outcome.incomplete => {
                    tx.send(AgentUpdate::Incomplete { summary, history })
                }
                Ok(summary) => tx.send(AgentUpdate::Done { summary, history }),
                Err(err) => tx.send(AgentUpdate::Failed { err, history }),
            };
        });
        self.agent_job = Some(rx);
    }

    /// Run a text-producing action against the current path on a worker thread.
    fn run_text(&mut self, action: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) {
        let path = self.project_path();
        self.spawn(move || {
            let start = std::time::Instant::now();
            match action(&path) {
                Ok(output) => JobOutcome::Text {
                    output,
                    status: format!("Done in {} ms.", start.elapsed().as_millis()),
                },
                Err(err) => JobOutcome::Text {
                    output: format!("Error: {err}"),
                    status: "Action failed.".to_string(),
                },
            }
        });
    }

    /// The Settings screen: your details, model providers, and the active one.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Settings");
        ui.label(
            egui::RichText::new(
                "Stored per-user in your config directory (never in the project), \
                 because it holds API keys.",
            )
            .weak(),
        );
        ui.add_space(8.0);

        // --- Your details -------------------------------------------------
        ui.group(|ui| {
            ui.strong("Your details");
            ui.add_space(4.0);
            egui::Grid::new("user-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user_name)
                            .desired_width(320.0)
                            .hint_text("your name"),
                    );
                    ui.end_row();
                    ui.label("Email");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user_email)
                            .desired_width(320.0)
                            .hint_text("you@example.com"),
                    );
                    ui.end_row();
                });
        });
        ui.add_space(10.0);

        // --- Budget -------------------------------------------------------
        ui.group(|ui| {
            ui.strong("Budget (USD)");
            ui.label(
                egui::RichText::new(
                    "Kestrel warns and stops the agent when a cap is reached. Blank = no limit.",
                )
                .weak(),
            );
            ui.add_space(4.0);
            egui::Grid::new("budget-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Per conversation");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.budget_session)
                            .desired_width(120.0)
                            .hint_text("e.g. 1.00"),
                    );
                    ui.end_row();
                    ui.label("Per day");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.budget_daily)
                            .desired_width(120.0)
                            .hint_text("e.g. 10.00"),
                    );
                    ui.end_row();
                });
        });
        ui.add_space(10.0);

        // --- Policy (agent guardrails) ------------------------------------
        ui.group(|ui| {
            ui.strong("Policy — agent guardrails");
            ui.label(
                egui::RichText::new(
                    "Disable tools or block command patterns. A blocked call is refused and the \
                     agent adapts — nothing runs.",
                )
                .weak(),
            );
            ui.add_space(4.0);
            if ui
                .checkbox(
                    &mut self.settings.ask_permission,
                    "Ask permission before running commands / installs / git",
                )
                .on_hover_text(
                    "Pops a prompt for each system-touching action; Deny lets the agent adapt.",
                )
                .changed()
            {
                self.ask_permission = self.settings.ask_permission;
            }
            ui.add_space(4.0);
            ui.label("Disabled tools:");
            ui.horizontal_wrapped(|ui| {
                for tool in POLICY_TOOLS {
                    let mut denied = self.settings.policy.denied_tools.iter().any(|t| t == tool);
                    if ui.checkbox(&mut denied, *tool).changed() {
                        if denied {
                            self.settings.policy.denied_tools.push(tool.to_string());
                        } else {
                            self.settings.policy.denied_tools.retain(|t| t != tool);
                        }
                    }
                }
            });
            ui.add_space(4.0);
            ui.label("Blocked command patterns (one per line):");
            ui.add(
                egui::TextEdit::multiline(&mut self.policy_patterns)
                    .desired_rows(4)
                    .desired_width(360.0),
            );
            if ui.button("Reset to safe defaults").clicked() {
                self.policy_patterns = kestrel_core::default_denied_patterns().join("\n");
            }
        });
        ui.add_space(10.0);

        // --- Add a provider ----------------------------------------------
        ui.group(|ui| {
            ui.strong("Add a provider");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_source("new-provider-preset")
                    .selected_text(&self.new_provider_preset)
                    .show_ui(ui, |ui| {
                        for preset in kestrel_core::PROVIDER_PRESETS {
                            ui.selectable_value(
                                &mut self.new_provider_preset,
                                preset.to_string(),
                                preset,
                            );
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_provider_name)
                        .desired_width(200.0)
                        .hint_text("name (defaults to preset)"),
                );
                if ui.button("Add").clicked() {
                    if let Some(preset) = kestrel_core::provider_preset(&self.new_provider_preset) {
                        let name = if self.new_provider_name.trim().is_empty() {
                            self.new_provider_preset.clone()
                        } else {
                            self.new_provider_name.trim().to_string()
                        };
                        let first = self.settings.providers.is_empty();
                        self.settings.providers.insert(name.clone(), preset);
                        if first {
                            self.settings.active_provider = Some(name);
                        }
                        self.new_provider_name.clear();
                    }
                }
            });
        });
        ui.add_space(10.0);

        // --- Configured providers ----------------------------------------
        ui.strong("Providers");
        ui.add_space(4.0);
        if self.settings.providers.is_empty() {
            ui.label(egui::RichText::new("No providers yet — add one above.").weak());
        }
        let names: Vec<String> = self.settings.providers.keys().cloned().collect();
        let active = self.settings.active_provider.clone();
        let mut make_active: Option<String> = None;
        let mut remove: Option<String> = None;
        for name in &names {
            let is_active = active.as_deref() == Some(name.as_str());
            let Some(provider) = self.settings.providers.get_mut(name) else {
                continue;
            };
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    if is_active {
                        ui.label(egui::RichText::new("● active").strong());
                    } else if ui.button("Set active").clicked() {
                        make_active = Some(name.clone());
                    }
                    ui.strong(name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(name.clone());
                        }
                    });
                });
                ui.add_space(4.0);
                let suggestions = kestrel_core::model_suggestions_for(provider);
                egui::Grid::new(format!("provider-grid-{name}"))
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("API kind");
                        egui::ComboBox::from_id_source(format!("kind-{name}"))
                            .selected_text(kind_label(provider.kind))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut provider.kind,
                                    kestrel_core::ProviderKind::Anthropic,
                                    "Anthropic",
                                );
                                ui.selectable_value(
                                    &mut provider.kind,
                                    kestrel_core::ProviderKind::Openai,
                                    "OpenAI-compatible",
                                );
                            });
                        ui.end_row();

                        ui.label("Base URL");
                        ui.add(
                            egui::TextEdit::singleline(&mut provider.base_url).desired_width(360.0),
                        );
                        ui.end_row();

                        ui.label("API key");
                        ui.add(
                            egui::TextEdit::singleline(&mut provider.api_key)
                                .password(true)
                                .desired_width(360.0)
                                .hint_text("stored locally only"),
                        );
                        ui.end_row();

                        ui.label("Model");
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_source(format!("model-{name}"))
                                .selected_text(if provider.model.is_empty() {
                                    "choose…".to_string()
                                } else {
                                    provider.model.clone()
                                })
                                .show_ui(ui, |ui| {
                                    for model in suggestions {
                                        ui.selectable_value(
                                            &mut provider.model,
                                            model.to_string(),
                                            *model,
                                        );
                                    }
                                });
                            ui.add(
                                egui::TextEdit::singleline(&mut provider.model)
                                    .desired_width(220.0)
                                    .hint_text("or type any model ID"),
                            );
                        });
                        ui.end_row();
                    });
            });
            ui.add_space(6.0);
        }
        if let Some(name) = make_active {
            self.settings.active_provider = Some(name);
        }
        if let Some(name) = remove {
            self.settings.providers.remove(&name);
            if self.settings.active_provider.as_deref() == Some(name.as_str()) {
                self.settings.active_provider = self.settings.providers.keys().next().cloned();
            }
        }

        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("💾 Save").clicked() {
                self.settings.user.name = non_empty(&self.user_name);
                self.settings.user.email = non_empty(&self.user_email);
                self.settings.budget.session_limit = parse_budget(&self.budget_session);
                self.settings.budget.daily_limit = parse_budget(&self.budget_daily);
                self.settings.policy.denied_patterns = self
                    .policy_patterns
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                match kestrel_core::save_settings(&self.settings) {
                    Ok(()) => {
                        self.settings_status = format!(
                            "Saved to {}.",
                            kestrel_core::settings::settings_path().display()
                        );
                    }
                    Err(err) => self.settings_status = format!("Save failed: {err}"),
                }
            }
            if !self.settings_status.is_empty() {
                ui.label(&self.settings_status);
            }
        });
        ui.add_space(8.0);
    }
}

/// Recursively render one tree node, pushing any requested actions to `actions`.
fn render_tree(
    ui: &mut egui::Ui,
    node: &TreeNode,
    selected: &Option<PathBuf>,
    actions: &mut Vec<TreeAction>,
) {
    if node.is_dir {
        let response = egui::CollapsingHeader::new(format!("📁 {}", node.name))
            .id_source(&node.path)
            .default_open(false)
            .show(ui, |ui| {
                for child in &node.children {
                    render_tree(ui, child, selected, actions);
                }
            });
        response.header_response.context_menu(|ui| {
            if ui.button("New File…").clicked() {
                actions.push(TreeAction::NewIn(node.path.clone(), false));
                ui.close_menu();
            }
            if ui.button("New Folder…").clicked() {
                actions.push(TreeAction::NewIn(node.path.clone(), true));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Rename…").clicked() {
                actions.push(TreeAction::Rename(node.path.clone()));
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                actions.push(TreeAction::Delete(node.path.clone()));
                ui.close_menu();
            }
        });
        if response.header_response.clicked() {
            actions.push(TreeAction::Select(node.path.clone()));
        }
    } else {
        let is_selected = selected.as_deref() == Some(node.path.as_path());
        let response = ui.selectable_label(is_selected, format!("📄 {}", node.name));
        if response.clicked() {
            actions.push(TreeAction::Open(node.path.clone()));
        }
        response.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                actions.push(TreeAction::Rename(node.path.clone()));
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                actions.push(TreeAction::Delete(node.path.clone()));
                ui.close_menu();
            }
        });
    }
}

/// Load a project's directory tree (on a worker thread).
fn load_tree(path: &Path) -> JobOutcome {
    if !path.exists() {
        return JobOutcome::Text {
            output: format!("Path does not exist: {}", path.display()),
            status: "Open failed.".to_string(),
        };
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let root = build_tree(path, name, path.is_dir(), 0);
    let files = count_files(&root);
    JobOutcome::Tree {
        root,
        status: format!("Loaded {} — {files} files.", path.display()),
    }
}

/// Recursively build a `TreeNode`, capped in depth to avoid pathological trees.
fn build_tree(path: &Path, name: String, is_dir: bool, depth: usize) -> TreeNode {
    let mut children = Vec::new();
    if is_dir && depth < 40 {
        if let Ok(entries) = kestrel_core::read_dir_entries(path) {
            for entry in entries {
                children.push(build_tree(&entry.path, entry.name, entry.is_dir, depth + 1));
            }
        }
    }
    TreeNode {
        name,
        path: path.to_path_buf(),
        is_dir,
        children,
    }
}

/// Count the files (non-directory leaves) in a tree.
fn count_files(node: &TreeNode) -> usize {
    if node.is_dir {
        node.children.iter().map(count_files).sum()
    } else {
        1
    }
}

/// Build the system prompt for a chat turn. When `include` is set and the
/// project graph builds, the most relevant files for `query` are attached as
/// background context. Runs on the chat worker thread (graph building is slow),
/// so the window stays responsive.
fn chat_system_prompt(include: bool, project: &Path, query: &str) -> String {
    let mut prompt = "You are Kestrel, an expert software-engineering assistant embedded in a \
         local coding tool. Be concise, correct, and concrete. When you reference code, cite \
         the file path."
        .to_string();
    if include {
        if let Ok(graph) = kestrel_core::build_project_graph(project) {
            let pack = kestrel_core::build_context_pack_for_query(&graph, query, 6000);
            if !pack.entries.is_empty() {
                let context = kestrel_core::assemble_context_prompt(&graph.root, &pack);
                prompt.push_str(
                    "\n\nThe following files from the user's project are the most relevant to \
                     their message. Use them as ground truth.\n\n",
                );
                prompt.push_str(&context);
            }
        }
    }
    prompt
}

/// Kestrel's amber accent colour (a kestrel is a russet falcon).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0xE8, 0x8A, 0x2E);
/// Green for added lines / counts, red for removed — shared across the Diff view.
const DIFF_ADD: egui::Color32 = egui::Color32::from_rgb(90, 190, 110);
const DIFF_DEL: egui::Color32 = egui::Color32::from_rgb(220, 100, 100);

/// Apply Kestrel's visual style over the base light/dark theme: comfortable
/// spacing, rounded widgets, and an amber accent for selection and links.
fn configure_style(ctx: &egui::Context, dark: bool) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    style.spacing.window_margin = egui::Margin::same(10.0);

    let rounding = egui::Rounding::same(6.0);
    let widgets = &mut style.visuals.widgets;
    for w in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.rounding = rounding;
    }
    style.visuals.window_rounding = egui::Rounding::same(9.0);
    style.visuals.menu_rounding = egui::Rounding::same(7.0);
    style.visuals.selection.bg_fill = ACCENT.linear_multiply(if dark { 0.42 } else { 0.30 });
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.hyperlink_color = ACCENT;
    // A touch of accent on the active widget outline.
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT.linear_multiply(0.6));
    ctx.set_style(style);
}

/// The colour for a unified-diff line by its leading marker.
fn diff_line_color(line: &str, default: egui::Color32) -> egui::Color32 {
    if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("diff ")
        || line.starts_with("index ")
    {
        egui::Color32::from_rgb(150, 150, 150)
    } else if line.starts_with("@@") {
        egui::Color32::from_rgb(90, 170, 220)
    } else if line.starts_with('+') {
        DIFF_ADD
    } else if line.starts_with('-') {
        DIFF_DEL
    } else {
        default
    }
}

/// Format a token count compactly (12.3k, 1.2M).
fn human_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// A document-flavoured icon for a file in the Work workspace.
fn doc_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "doc" | "docx" | "odt" | "rtf" => "📄",
        "xls" | "xlsx" | "csv" | "ods" => "📊",
        "ppt" | "pptx" | "odp" => "📽",
        "pdf" => "📕",
        "md" | "markdown" | "txt" => "📝",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "🖼",
        "zip" | "7z" | "rar" => "🗜",
        _ => "📃",
    }
}

/// Whether a tool touches the system in a way worth asking permission for —
/// running commands, installing toolchains, or git operations. File writes stay
/// sandboxed to the project and reads are harmless, so those never prompt.
fn tool_needs_permission(name: &str) -> bool {
    matches!(
        name,
        "run_command" | "install_tool" | "git" | "start_app" | "stop_app"
    ) || tool_always_needs_permission(name)
}

/// Actions that are irreversible and leave the machine — these are confirmed
/// **every time**, even when the "ask permission" setting is off. Sending mail
/// on someone's behalf can't be undone, so it never happens silently.
fn tool_always_needs_permission(name: &str) -> bool {
    matches!(name, "send_mail")
}

/// Build an editor draft from an existing workflow.
fn draft_from(wf: &kestrel_core::Workflow) -> WorkflowDraft {
    WorkflowDraft {
        id: wf.id.clone(),
        name: wf.name.clone(),
        description: wf.description.clone(),
        prompt: wf.prompt.clone(),
        params: wf.params.join(", "),
        status: String::new(),
        resources: wf.resources.clone(),
    }
}

/// Turn a name into a stable kebab-case id.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Validate an editor draft and build a `Workflow`, or return an error message.
fn build_workflow(draft: &WorkflowDraft) -> Result<kestrel_core::Workflow, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the workflow a name.".to_string());
    }
    if draft.prompt.trim().is_empty() {
        return Err("The prompt can't be empty.".to_string());
    }
    // Keep an existing id when editing; derive one from the name for new ones.
    let id = if draft.id.is_empty() {
        let slug = slugify(name);
        if slug.is_empty() {
            return Err("Use letters or numbers in the name.".to_string());
        }
        slug
    } else {
        draft.id.clone()
    };
    let params: Vec<String> = draft
        .params
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    // Every declared param must actually appear in the prompt.
    for p in &params {
        if !draft.prompt.contains(&format!("{{{p}}}")) {
            return Err(format!(
                "Parameter '{p}' isn't used in the prompt (add {{{p}}})."
            ));
        }
    }
    Ok(kestrel_core::Workflow {
        id,
        name: name.to_string(),
        description: draft.description.trim().to_string(),
        prompt: draft.prompt.trim().to_string(),
        params,
        resources: draft.resources.clone(),
    })
}

/// The default dev/run command for a project (from its markers, else a sane
/// Node fallback), used to prefill the Run tab.
fn detect_run_command(root: &Path) -> String {
    if let Ok(inspection) = kestrel_core::inspect_project(root) {
        if let Some(command) = inspection
            .commands
            .iter()
            .find(|c| matches!(c.kind, kestrel_core::CommandKind::Run))
        {
            return command.command.clone();
        }
    }
    if root.join("package.json").exists() {
        return "npm run dev".to_string();
    }
    String::new()
}

/// The highlighting language for a file path's extension.
fn language_for_path(path: &Path) -> kestrel_core::Language {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    kestrel_core::language_from_extension(ext)
}

/// The highlighting language for a (possibly relative) path string.
fn language_for(path: &str) -> kestrel_core::Language {
    language_for_path(Path::new(path))
}

/// The colour for a token kind under the current theme (VS Code-like palettes).
fn token_color(kind: kestrel_core::TokenKind, dark: bool) -> egui::Color32 {
    use kestrel_core::TokenKind::*;
    if dark {
        match kind {
            Keyword => egui::Color32::from_rgb(0x56, 0x9C, 0xD6),
            Type => egui::Color32::from_rgb(0x4E, 0xC9, 0xB0),
            Function => egui::Color32::from_rgb(0xDC, 0xDC, 0xAA),
            String => egui::Color32::from_rgb(0xCE, 0x91, 0x78),
            Comment => egui::Color32::from_rgb(0x6A, 0x99, 0x55),
            Number => egui::Color32::from_rgb(0xB5, 0xCE, 0xA8),
        }
    } else {
        match kind {
            Keyword => egui::Color32::from_rgb(0x00, 0x00, 0xFF),
            Type => egui::Color32::from_rgb(0x26, 0x7F, 0x99),
            Function => egui::Color32::from_rgb(0x79, 0x5E, 0x26),
            String => egui::Color32::from_rgb(0xA3, 0x15, 0x15),
            Comment => egui::Color32::from_rgb(0x00, 0x80, 0x00),
            Number => egui::Color32::from_rgb(0x09, 0x86, 0x58),
        }
    }
}

/// Build a coloured `LayoutJob` for `source` in `language`, filling the gaps
/// between highlighted spans with the default text colour.
fn code_layout(
    source: &str,
    language: kestrel_core::Language,
    dark: bool,
    font: egui::FontId,
) -> egui::text::LayoutJob {
    let default = if dark {
        egui::Color32::from_rgb(0xD4, 0xD4, 0xD4)
    } else {
        egui::Color32::from_rgb(0x24, 0x29, 0x2E)
    };
    let mut job = egui::text::LayoutJob::default();
    let append = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };
    let mut pos = 0;
    for span in kestrel_core::highlight(source, language) {
        if span.start > pos {
            append(&mut job, &source[pos..span.start], default);
        }
        append(
            &mut job,
            &source[span.start..span.end],
            token_color(span.kind, dark),
        );
        pos = span.end;
    }
    if pos < source.len() {
        append(&mut job, &source[pos..], default);
    }
    job
}

/// A display label for a provider's API kind.
fn kind_label(kind: kestrel_core::ProviderKind) -> &'static str {
    match kind {
        kestrel_core::ProviderKind::Anthropic => "Anthropic",
        kestrel_core::ProviderKind::Openai => "OpenAI-compatible",
    }
}

/// The side-effecting tools a policy can disable (reads are always allowed).
const POLICY_TOOLS: &[&str] = &[
    "run_command",
    "install_tool",
    "git",
    "start_app",
    "stop_app",
    "write_file",
    "edit_file",
    "http_get",
    "open_url",
    "screenshot",
];

/// Parse a budget field ("$1.00", "2", "") into a positive dollar cap.
fn parse_budget(s: &str) -> Option<f64> {
    let t = s.trim().trim_start_matches('$').trim();
    match t.parse::<f64>() {
        Ok(v) if v > 0.0 => Some(v),
        _ => None,
    }
}

/// `Some(trimmed)` if the string has non-whitespace content, else `None`.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn inspect(path: &Path) -> Result<String, String> {
    let report = kestrel_core::inspect_project(path).map_err(|e| e.to_string())?;
    let mut out = format!("Project root: {}\n", report.project_root.display());
    out.push_str(&format!(
        "Files: {}, Bytes: {}\n\nLanguages\n",
        report.inventory.total_files, report.inventory.total_bytes
    ));
    for lang in &report.languages {
        out.push_str(&format!(
            "  {:<14} {} files, {} bytes\n",
            lang.language, lang.files, lang.bytes
        ));
    }
    let symbols = &report.symbols;
    out.push_str(&format!(
        "\nSymbols: {} across {} files\n",
        symbols.total_symbols, symbols.files_with_symbols
    ));
    for (kind, count) in &symbols.kind_counts {
        out.push_str(&format!("  {count:>4} {kind}\n"));
    }
    out.push_str("\nLikely commands\n");
    for command in &report.commands {
        out.push_str(&format!("  {:?}: {}\n", command.kind, command.command));
    }
    Ok(out)
}

fn graph(path: &Path) -> Result<String, String> {
    let graph = kestrel_core::build_project_graph(path).map_err(|e| e.to_string())?;
    let mut out = format!(
        "{} files, {} edges\n\n",
        graph.files.len(),
        graph.edges.len()
    );
    for edge in graph.edges.iter().take(120) {
        let via = edge
            .via
            .iter()
            .chain(edge.imports.iter())
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "{}  ->  {}   [{}] {}\n",
            edge.from.display(),
            edge.to.display(),
            edge.weight(),
            via
        ));
    }
    Ok(out)
}

fn context(path: &Path, query: &str) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("enter a query in the Query box first".to_string());
    }
    let graph = kestrel_core::build_project_graph(path).map_err(|e| e.to_string())?;
    let pack = kestrel_core::build_context_pack_for_query(&graph, query, 12_000);
    if pack.entries.is_empty() {
        return Ok(format!("No files matched the query \"{query}\"."));
    }
    let mut out = format!(
        "Context for \"{query}\" — {} / {} tokens across {} files\n\n",
        pack.used_tokens,
        pack.budget_tokens,
        pack.entries.len()
    );
    for entry in &pack.entries {
        out.push_str(&format!(
            "{}  [{}]  ~{} tok   {}\n",
            entry.path.display(),
            entry.language,
            entry.estimated_tokens,
            entry.reason
        ));
    }
    Ok(out)
}

fn verify(path: &Path) -> Result<String, String> {
    let inspection = kestrel_core::inspect_project(path).map_err(|e| e.to_string())?;
    let configured = kestrel_core::load_config(&inspection.project_root)
        .config()
        .verify
        .steps;
    let steps = if configured.is_empty() {
        kestrel_core::plan_verification(&inspection.markers)
    } else {
        configured
            .iter()
            .map(|c| kestrel_core::VerifyStep {
                label: c.split_whitespace().next().unwrap_or("step").to_string(),
                command: c.clone(),
            })
            .collect()
    };
    if steps.is_empty() {
        return Ok("No verification commands detected for this project.".to_string());
    }
    let report = kestrel_core::run_verification(&inspection.project_root, &steps);
    let mut out = format!(
        "Verification {} — {} step(s)\n\n",
        if report.passed { "PASSED" } else { "FAILED" },
        report.steps.len()
    );
    for step in &report.steps {
        let status = if step.success { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "[{status}] {} — {} ({} ms)\n",
            step.label, step.command, step.duration_ms
        ));
        if !step.success {
            let detail = if step.stderr_tail.is_empty() {
                &step.stdout_tail
            } else {
                &step.stderr_tail
            };
            for line in detail.lines() {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    for step in &report.skipped {
        out.push_str(&format!("[SKIP] {} — {}\n", step.label, step.command));
    }
    Ok(out)
}

fn environment() -> JobOutcome {
    let report = kestrel_core::discover_environment();
    let mut out = format!("Host: {} ({})\n\nShells\n", report.os, report.arch);
    let list = |out: &mut String, tools: &[kestrel_core::ToolInfo]| {
        for tool in tools {
            if tool.found {
                out.push_str(&format!(
                    "  + {:<10} {}\n",
                    tool.name,
                    tool.version.as_deref().unwrap_or("(version unknown)")
                ));
            } else {
                out.push_str(&format!("  - {:<10} not found\n", tool.name));
            }
        }
    };
    list(&mut out, &report.shells);
    out.push_str("\nToolchains\n");
    list(&mut out, &report.toolchains);
    out.push_str("\nCross-boundary\n");
    if report.wsl.available {
        out.push_str(&format!("  + WSL: {}\n", report.wsl.distros.join(", ")));
    } else {
        out.push_str("  - WSL: not installed\n");
    }
    if report.docker.found {
        out.push_str(&format!(
            "  + Docker: {}\n",
            report.docker.version.as_deref().unwrap_or("")
        ));
    } else {
        out.push_str("  - Docker: not found\n");
    }
    JobOutcome::Text {
        output: out,
        status: "Environment probed.".to_string(),
    }
}
