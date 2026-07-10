//! Kestrel native desktop shell.
//!
//! A small, all-Rust GUI over `kestrel-core`: point it at a project and run the
//! local, instant capabilities — inspect, symbols, the dependency graph, and
//! query-seeded context packs — with the results in a scrollable pane. This is
//! the first native surface for Kestrel; model-backed actions (ask/edit) and
//! verification live in the CLI for now.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::path::{Path, PathBuf};

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title("Kestrel"),
        ..Default::default()
    };
    eframe::run_native(
        "Kestrel",
        native_options,
        Box::new(|_cc| Ok(Box::<KestrelApp>::default())),
    )
}

struct KestrelApp {
    path: String,
    query: String,
    output: String,
    status: String,
}

impl Default for KestrelApp {
    fn default() -> Self {
        let path = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
            path,
            query: String::new(),
            output: "Choose a project folder, then pick an action above.\n\n\
                     • Inspect  — languages, symbols, markers, likely commands\n\
                     • Symbols  — structural symbols per file\n\
                     • Graph    — the file dependency graph\n\
                     • Context  — files most relevant to the query box"
                .to_string(),
            status: String::new(),
        }
    }
}

impl eframe::App for KestrelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Kestrel");
                ui.separator();
                ui.label("Project:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .desired_width(420.0)
                        .hint_text("path to a repository"),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Inspect").clicked() {
                    self.run(inspect);
                }
                if ui.button("Symbols").clicked() {
                    self.run(symbols);
                }
                if ui.button("Graph").clicked() {
                    self.run(graph);
                }
                ui.separator();
                ui.label("Query:");
                let submit = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.query)
                            .desired_width(300.0)
                            .hint_text("e.g. dependency graph edges"),
                    )
                    .lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Context").clicked() || submit {
                    let query = self.query.clone();
                    self.run(move |path| context(path, &query));
                }
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
    /// Run an action against the current path, capturing timing and result.
    fn run(&mut self, action: impl FnOnce(&Path) -> Result<String, String>) {
        let path = PathBuf::from(self.path.trim());
        let start = std::time::Instant::now();
        match action(&path) {
            Ok(text) => {
                self.output = text;
                self.status = format!("Done in {} ms.", start.elapsed().as_millis());
            }
            Err(err) => {
                self.output = format!("Error: {err}");
                self.status = "Action failed.".to_string();
            }
        }
    }
}

fn inspect(path: &Path) -> Result<String, String> {
    let report = kestrel_core::inspect_project(path).map_err(|e| e.to_string())?;
    let mut out = String::new();
    out.push_str(&format!(
        "Project root: {}\n",
        report.project_root.display()
    ));
    out.push_str(&format!(
        "Files: {}, Bytes: {}\n\n",
        report.inventory.total_files, report.inventory.total_bytes
    ));

    out.push_str("Languages\n");
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

fn symbols(path: &Path) -> Result<String, String> {
    let files = kestrel_core::project_symbols(path).map_err(|e| e.to_string())?;
    if files.is_empty() {
        return Ok("No structural symbols found in supported source files.".to_string());
    }
    let total: usize = files.iter().map(|(_, f)| f.symbols.len()).sum();
    let mut out = format!("{total} symbols across {} files\n\n", files.len());
    for (path, file) in &files {
        out.push_str(&format!("{} [{}]\n", path.display(), file.language));
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
        out.push('\n');
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
    for edge in graph.edges.iter().take(100) {
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
        "Context for query \"{query}\" — {} / {} tokens across {} files\n\n",
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
