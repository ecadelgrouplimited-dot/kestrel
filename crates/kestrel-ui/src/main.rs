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
use std::sync::mpsc::{Receiver, TryRecvError};

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
}

/// A live update streamed from a running agent loop.
enum AgentUpdate {
    /// The model's narration (also shown in the transcript).
    Line(String),
    /// A tool action in progress (shown in the transcript and as live status).
    Activity(String),
    /// A file the agent wrote, with its full contents for live preview.
    Wrote { path: String, contents: String },
    /// The agent finished; carries the final summary and the full conversation
    /// so a follow-up prompt can refine the same project.
    Done {
        summary: String,
        history: Vec<kestrel_core::AgentMessage>,
    },
    /// The agent failed; still returns the conversation so far.
    Failed {
        err: String,
        history: Vec<kestrel_core::AgentMessage>,
    },
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
    /// The reply finished.
    Done,
    /// The request failed.
    Failed(String),
}

#[derive(PartialEq, Eq)]
enum AppView {
    Main,
    Settings,
    Chat,
}

/// Which pane the central area shows in the Main view.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CentralView {
    Editor,
    Output,
    Diff,
    Run,
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
}

impl Default for KestrelApp {
    fn default() -> Self {
        let path = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let settings = kestrel_core::load_settings();
        let user_name = settings.user.name.clone().unwrap_or_default();
        let user_email = settings.user.email.clone().unwrap_or_default();
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
                Ok(ChatUpdate::Done) => {
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
                    last_written = Some(self.project_path().join(&path));
                    refresh = true;
                    ctx.request_repaint();
                }
                Ok(AgentUpdate::Done { summary, history }) => {
                    if !summary.trim().is_empty() {
                        self.chat_history
                            .push(kestrel_core::ChatMessage::assistant(summary));
                    }
                    self.agent_messages = history;
                    self.chat_pending = false;
                    self.agent_activity.clear();
                    self.agent_job = None;
                    self.status = "Agent finished — review changes in the Diff tab.".to_string();
                    self.save_session();
                    self.diff_review = None;
                    refresh = true;
                    break;
                }
                Ok(AgentUpdate::Failed { err, history }) => {
                    self.chat_error = err;
                    self.agent_messages = history;
                    self.chat_pending = false;
                    self.agent_activity.clear();
                    self.agent_job = None;
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
                    self.chat_pending = false;
                    self.agent_job = None;
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
                        if ui.button("💬 Chat").clicked() {
                            self.view = AppView::Chat;
                        }
                    } else if ui.button("← Back").clicked() {
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
            }
        });
    }
}

impl KestrelApp {
    fn project_path(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
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
        self.view = AppView::Main;
        self.reload_tree();
    }

    /// Persist the current project's agent conversation, transcript, and the
    /// list of files it created so reopening it resumes where this left off.
    fn save_session(&self) {
        let session = kestrel_core::AgentSession {
            messages: self.agent_messages.clone(),
            transcript: self.chat_history.clone(),
            created_files: self.agent_files.iter().map(|f| f.path.clone()).collect(),
        };
        let _ = kestrel_core::save_agent_session(&self.project_path(), &session);
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
        });
        ui.separator();

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
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "rs" {
            self.editor_status =
                "Formatting is available for Rust files (.rs) via rustfmt.".to_string();
            return;
        }
        match rustfmt_source(&self.editor_text) {
            Ok(formatted) => {
                self.editor_text = formatted;
                self.editor_status = "Formatted with rustfmt.".to_string();
            }
            Err(err) => self.editor_status = format!("rustfmt failed: {err}"),
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
                if ui
                    .button("Format")
                    .on_hover_text("rustfmt (Rust files)")
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
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for entry in &review.files {
                                ui.label(egui::RichText::new(entry).monospace().weak());
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

    /// The build-preview panel: a live, clickable history of the files the
    /// agent is creating, with a preview of the selected one.
    fn build_preview_ui(&mut self, ui: &mut egui::Ui) {
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
        ui.horizontal(|ui| {
            match self.settings.active() {
                Some(p) => {
                    ui.label(egui::RichText::new("Model:").weak());
                    ui.label(
                        egui::RichText::new(format!(
                            "{} ({})",
                            p.model,
                            self.settings.active_provider.as_deref().unwrap_or("?")
                        ))
                        .strong(),
                    );
                    // Cost meter: estimated size (and cost) of the next request.
                    let ctx_chars: usize = self.chat_history.iter().map(|m| m.content.len()).sum();
                    let input_tokens = kestrel_core::estimate_tokens(ctx_chars + 2_000);
                    ui.separator();
                    let meter = match kestrel_core::model_price(&p.model) {
                        Some(price) => format!(
                            "≈ {} tok context · ~${:.4} in · ${:.0}/${:.0} per 1M",
                            input_tokens,
                            kestrel_core::estimate_cost(price, input_tokens, 0),
                            price.input_per_million,
                            price.output_per_million
                        ),
                        None => format!("≈ {input_tokens} tok context"),
                    };
                    ui.label(egui::RichText::new(meter).weak());
                }
                None => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 150, 80),
                        "No active provider — set one in Settings.",
                    );
                }
            }
            ui.separator();
            ui.checkbox(&mut self.chat_include_context, "Include project context");
            ui.checkbox(&mut self.chat_agent_mode, "Agent · write files")
                .on_hover_text(
                    "Turn the request into real files written into the current project.",
                );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("New chat").clicked() {
                    self.chat_history.clear();
                    self.chat_error.clear();
                    self.chat_input.clear();
                    self.agent_files.clear();
                    self.agent_preview = None;
                    self.agent_messages.clear();
                    self.save_session();
                }
                if self.chat_pending && ui.button("Stop").clicked() {
                    self.chat_job = None;
                    self.agent_job = None;
                    self.chat_pending = false;
                    self.agent_activity.clear();
                    self.chat_error = "Cancelled.".to_string();
                }
            });
        });

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
                    self.project_path().display()
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
                Ok(Ok(_)) => tx.send(ChatUpdate::Done),
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
        let root = self.project_path();
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
        // Carry the running conversation so a follow-up refines the same
        // project. The file history keeps accumulating across builds too.
        let history = self.agent_messages.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let events = tx.clone();
        std::thread::spawn(move || {
            let outcome = kestrel_core::run_agent(
                &config,
                &model,
                &prompt,
                &root,
                100,
                true,
                history,
                |event| {
                    let update = match event {
                        kestrel_core::AgentEvent::Assistant(text) => AgentUpdate::Line(text),
                        kestrel_core::AgentEvent::Tool(call) => AgentUpdate::Activity(call),
                        kestrel_core::AgentEvent::Wrote { path, contents } => {
                            AgentUpdate::Wrote { path, contents }
                        }
                    };
                    let _ = events.send(update);
                },
            );
            let history = outcome.history;
            let _ = match outcome.result {
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

/// Format Rust `source` with the system `rustfmt`, returning the formatted text
/// or an error message. Uses a temp file (rustfmt formats files in place).
fn rustfmt_source(source: &str) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!(
        "kestrel-fmt-{}-{}.rs",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, source).map_err(|e| e.to_string())?;
    let output = std::process::Command::new("rustfmt")
        .arg("--edition")
        .arg("2021")
        .arg(&tmp)
        .output();
    let result = match output {
        Ok(out) if out.status.success() => std::fs::read_to_string(&tmp).map_err(|e| e.to_string()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(err) => Err(format!("rustfmt not found on PATH ({err})")),
    };
    let _ = std::fs::remove_file(&tmp);
    result
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
        egui::Color32::from_rgb(90, 190, 110)
    } else if line.starts_with('-') {
        egui::Color32::from_rgb(220, 100, 100)
    } else {
        default
    }
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
