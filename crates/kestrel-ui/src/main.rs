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
    /// Whether the in-flight request is a build-agent request (writes files).
    chat_agent_pending: bool,
    chat_error: String,
    chat_job: Option<Receiver<Result<String, String>>>,
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
            chat_agent_pending: false,
            chat_error: String::new(),
            chat_job: None,
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
        let Some(rx) = &self.chat_job else { return };
        match rx.try_recv() {
            Ok(Ok(reply)) => {
                let message = if self.chat_agent_pending {
                    self.apply_agent_reply(&reply)
                } else {
                    reply
                };
                self.chat_history
                    .push(kestrel_core::ChatMessage::assistant(message));
                self.chat_pending = false;
                self.chat_agent_pending = false;
                self.chat_job = None;
            }
            Ok(Err(err)) => {
                self.chat_error = err;
                self.chat_pending = false;
                self.chat_agent_pending = false;
                self.chat_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => {
                self.chat_error = "The chat request stopped unexpectedly.".to_string();
                self.chat_pending = false;
                self.chat_agent_pending = false;
                self.chat_job = None;
            }
        }
    }
}

impl eframe::App for KestrelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job(ctx);
        self.poll_chat(ctx);
        let busy = self.job.is_some();
        self.new_project_modal(ctx);
        self.entry_modal(ctx);
        self.delete_modal(ctx);

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Kestrel");
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
                        if ui.button("Open…").clicked() {
                            if let Some(dir) = rfd::FileDialog::new()
                                .set_title("Open project folder")
                                .pick_folder()
                            {
                                self.open_project_path(dir);
                            }
                        }
                        if ui.button("New project…").clicked() {
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
                        if ui.button("Inspect").clicked() {
                            self.run_text(inspect);
                        }
                        if ui.button("Graph").clicked() {
                            self.run_text(graph);
                        }
                        if ui.button("Verify").clicked() {
                            self.run_text(verify);
                        }
                        if ui.button("Env").clicked() {
                            self.spawn(environment);
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
            }
        });
    }
}

impl KestrelApp {
    fn project_path(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
    }

    /// Make `path` the active project: record it, remember it in the recent
    /// list (persisted), return to the main view, and load its file tree.
    fn open_project_path(&mut self, path: PathBuf) {
        self.path = path.display().to_string();
        kestrel_core::push_recent(&mut self.settings.recent_projects, &path);
        let _ = kestrel_core::save_settings(&self.settings);
        self.view = AppView::Main;
        self.reload_tree();
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
                response.request_focus();
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

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.editor_text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(28),
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
                ("You", egui::Color32::from_rgb(120, 170, 255))
            } else {
                ("Kestrel", egui::Color32::from_rgb(150, 210, 150))
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
                ui.label(egui::RichText::new("Kestrel is thinking…").weak());
            });
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
                }
                if self.chat_pending && ui.button("Stop").clicked() {
                    self.chat_job = None;
                    self.chat_pending = false;
                    self.chat_agent_pending = false;
                    self.chat_error = "Cancelled.".to_string();
                }
            });
        });

        if self.chat_agent_mode {
            ui.colored_label(
                egui::Color32::from_rgb(150, 200, 150),
                format!(
                    "Agent mode: files will be written into {}",
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
        let agent = self.chat_agent_mode;
        self.chat_agent_pending = agent;

        let config = provider.to_config();
        let model = provider.model.clone();
        let messages = self.chat_history.clone();
        let include = self.chat_include_context;
        let project = self.project_path();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let system = if agent {
                agent_system_prompt(include, &project, &text)
            } else {
                chat_system_prompt(include, &project, &text)
            };
            // Agent replies must fit a whole project's worth of files.
            let max_tokens = if agent { 8192 } else { 2048 };
            let request = kestrel_core::ChatRequest {
                model,
                max_tokens,
                system: Some(system),
                messages,
            };
            let result = match kestrel_core::chat(&config, &request) {
                Ok(inner) => inner,
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send(result);
        });
        self.chat_job = Some(rx);
    }

    /// Apply a build-agent reply: write its files under the project root, reload
    /// the tree, and return a human summary to show in the transcript.
    fn apply_agent_reply(&mut self, reply: &str) -> String {
        let root = self.project_path();
        let edits = kestrel_core::parse_file_edits(reply);
        if edits.is_empty() {
            return "The agent produced no file blocks. Its raw reply:\n\n".to_string() + reply;
        }
        let applied = kestrel_core::apply_file_edits(&root, &edits);
        let ok = applied.iter().filter(|a| a.is_ok()).count();
        let mut summary = format!(
            "Wrote {ok}/{} file(s) to {}:\n",
            applied.len(),
            root.display()
        );
        for entry in &applied {
            match &entry.outcome {
                Ok(_) => summary.push_str(&format!("  ✓ {}\n", entry.path)),
                Err(err) => summary.push_str(&format!("  ✗ {} — {err}\n", entry.path)),
            }
        }
        self.reload_tree();
        self.status = format!("Agent wrote {ok} file(s) to {}.", root.display());
        summary
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

/// The system prompt for a build-agent turn: the file-manifest protocol, plus
/// optional project context so the agent can extend an existing codebase.
fn agent_system_prompt(include: bool, project: &Path, query: &str) -> String {
    let mut prompt = kestrel_core::agent_system_prompt();
    if include {
        if let Ok(graph) = kestrel_core::build_project_graph(project) {
            let pack = kestrel_core::build_context_pack_for_query(&graph, query, 6000);
            if !pack.entries.is_empty() {
                let context = kestrel_core::assemble_context_prompt(&graph.root, &pack);
                prompt.push_str(
                    "\n\nThe project already contains these files. Extend or modify them as \
                     needed, emitting the full new contents of any file you change.\n\n",
                );
                prompt.push_str(&context);
            }
        }
    }
    prompt
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
