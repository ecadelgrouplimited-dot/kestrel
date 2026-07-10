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

struct KestrelApp {
    path: String,
    query: String,
    output: String,
    status: String,
    files: Vec<(PathBuf, FileSymbols)>,
    selected: Option<usize>,
    job: Option<Receiver<JobOutcome>>,
}

impl Default for KestrelApp {
    fn default() -> Self {
        let path = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
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
}

impl eframe::App for KestrelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job(ctx);
        let busy = self.job.is_some();

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Kestrel");
                ui.separator();
                ui.label("Project:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .desired_width(440.0)
                        .hint_text("path to a repository"),
                );
            });
            ui.add_space(4.0);
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open").clicked() {
                        let path = self.project_path();
                        self.spawn(move || load_files(&path));
                    }
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
