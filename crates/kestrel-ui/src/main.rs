//! Kestrel native desktop shell.
//!
//! An all-Rust GUI over `kestrel-core`. A left-hand file tree lets you browse a
//! project's source files and jump to their symbols; the action bar runs the
//! local analyses (inspect, graph, query-seeded context), the verification
//! ladder, and host environment discovery. Slow work (verification, indexing)
//! runs on a background thread so the window never freezes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use kestrel_core::FileSymbols;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 440.0])
            .with_title("Kestrel"),
        ..Default::default()
    };
    eframe::run_native(
        "Kestrel",
        native_options,
        Box::new(|_cc| Ok(Box::<KestrelApp>::default())),
    )
}

/// The result a background job sends back to the UI thread.
enum JobOutcome {
    /// Free-text output for the main pane, plus a status line.
    Text { output: String, status: String },
    /// A loaded file tree (path + extracted symbols per file).
    Files {
        files: Vec<(PathBuf, FileSymbols)>,
        status: String,
    },
}

#[derive(PartialEq, Eq)]
enum AppView {
    Main,
    Settings,
    Chat,
}

struct KestrelApp {
    view: AppView,
    path: String,
    query: String,
    output: String,
    status: String,
    files: Vec<(PathBuf, FileSymbols)>,
    selected: Option<usize>,
    job: Option<Receiver<JobOutcome>>,
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
    chat_pending: bool,
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
            output: "Set a project folder and press Open to load its files, or use the action \
                     bar:\n\n\
                     • Open     — load the file tree (click a file to see its symbols)\n\
                     • Inspect  — languages, symbols, markers, likely commands\n\
                     • Graph    — the file dependency graph\n\
                     • Context  — files most relevant to the query box\n\
                     • Verify   — run the project's format/test/build ladder\n\
                     • Env      — host shells, toolchains, WSL, Docker"
                .to_string(),
            status: String::new(),
            files: Vec::new(),
            selected: None,
            job: None,
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
            chat_pending: false,
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
                self.job = None;
            }
            Ok(JobOutcome::Files { files, status }) => {
                self.files = files;
                self.selected = None;
                self.status = status;
                self.output = "Select a file on the left to view its symbols.".to_string();
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
                self.chat_history
                    .push(kestrel_core::ChatMessage::assistant(reply));
                self.chat_pending = false;
                self.chat_job = None;
            }
            Ok(Err(err)) => {
                self.chat_error = err;
                self.chat_pending = false;
                self.chat_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => {
                self.chat_error = "The chat request stopped unexpectedly.".to_string();
                self.chat_pending = false;
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

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Kestrel");
                ui.separator();
                ui.label("Project:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .desired_width(400.0)
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
                        if ui.button("New…").clicked() {
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
                                    .desired_width(260.0)
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
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.strong(format!("Files ({})", self.files.len()));
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut clicked = None;
                    for (i, (path, _)) in self.files.iter().enumerate() {
                        let label = path.display().to_string();
                        if ui
                            .selectable_label(self.selected == Some(i), label)
                            .clicked()
                        {
                            clicked = Some(i);
                        }
                    }
                    if let Some(i) = clicked {
                        self.selected = Some(i);
                        let (path, file) = &self.files[i];
                        self.output = format_file_symbols(path, file);
                        self.status =
                            format!("{} — {} symbols", path.display(), file.symbols.len());
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&self.output).monospace())
                            .selectable(true),
                    );
                });
        });
    }
}

impl KestrelApp {
    fn project_path(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
    }

    /// Make `path` the active project: record it as the path, remember it in
    /// the recent list (persisted), return to the main view, and load its files.
    fn open_project_path(&mut self, path: PathBuf) {
        self.path = path.display().to_string();
        kestrel_core::push_recent(&mut self.settings.recent_projects, &path);
        let _ = kestrel_core::save_settings(&self.settings);
        self.view = AppView::Main;
        self.spawn(move || load_files(&path));
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

    /// Render the compose bar: provider status, context toggle, and input.
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.chat_history.clear();
                    self.chat_error.clear();
                }
            });
        });

        if !self.chat_error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), &self.chat_error);
        }

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let hint = if self.chat_include_context {
                "Ask about this project… (Ctrl+Enter to send)"
            } else {
                "Message… (Ctrl+Enter to send)"
            };
            let input = ui.add(
                egui::TextEdit::multiline(&mut self.chat_input)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text(hint),
            );
            let ctrl_enter = input.has_focus()
                && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
            let send = ui
                .add_enabled(!self.chat_pending, egui::Button::new("Send"))
                .clicked();
            if send || ctrl_enter {
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

        let config = provider.to_config();
        let model = provider.model.clone();
        let messages = self.chat_history.clone();
        let include = self.chat_include_context;
        let project = self.project_path();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let system = chat_system_prompt(include, &project, &text);
            let request = kestrel_core::ChatRequest {
                model,
                max_tokens: 2048,
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

fn load_files(path: &Path) -> JobOutcome {
    match kestrel_core::project_symbols(path) {
        Ok(files) => {
            let status = format!("Loaded {} source files.", files.len());
            JobOutcome::Files { files, status }
        }
        Err(err) => JobOutcome::Text {
            output: format!("Error: {err}"),
            status: "Open failed.".to_string(),
        },
    }
}

fn format_file_symbols(path: &Path, file: &FileSymbols) -> String {
    let mut out = format!("{} [{}]\n\n", path.display(), file.language);
    for symbol in &file.symbols {
        let vis = if symbol.exported { "+" } else { "-" };
        let container = symbol
            .container
            .as_deref()
            .map(|c| format!(" (in {c})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {vis} {:<9} {}{}  @{}\n",
            symbol.kind.as_str(),
            symbol.name,
            container,
            symbol.line
        ));
    }
    out
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
