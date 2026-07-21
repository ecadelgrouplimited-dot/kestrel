//! Persistent project memory.
//!
//! A capable agent shouldn't re-derive a repo every session. As it works, it can
//! `remember` durable facts about *this* project — conventions, the build/run/test
//! commands, architecture notes, gotchas, decisions — stored in
//! `.kestrel/memory.json`. At the start of each run the memory is folded into the
//! system prompt, so every run starts smarter (and cheaper) than the last.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The most notes we keep, so the prompt stays bounded; oldest drop first.
const MAX_NOTES: usize = 60;

/// One learned fact about the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNote {
    /// A short bucket: "convention", "command", "architecture", "gotcha",
    /// "decision", or "note".
    #[serde(default = "default_category")]
    pub category: String,
    pub text: String,
}

fn default_category() -> String {
    "note".to_string()
}

/// Where a project's memory lives.
pub fn memory_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("memory.json")
}

/// Load a project's learned notes (empty if none/invalid).
pub fn load_memory(root: &Path) -> Vec<MemoryNote> {
    std::fs::read_to_string(memory_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist a project's notes.
pub fn save_memory(root: &Path, notes: &[MemoryNote]) -> std::io::Result<()> {
    let path = memory_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(notes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// Normalize a category to one of the known buckets.
fn normalize_category(category: &str) -> String {
    match category.trim().to_ascii_lowercase().as_str() {
        "convention" | "conventions" | "style" => "convention",
        "command" | "commands" | "build" | "run" | "test" => "command",
        "architecture" | "arch" | "structure" | "design" => "architecture",
        "gotcha" | "gotchas" | "warning" | "pitfall" => "gotcha",
        "decision" | "decisions" | "choice" => "decision",
        _ => "note",
    }
    .to_string()
}

/// Remember a durable fact about the project (de-duplicated by text, capped).
/// Returns whether it was newly added.
pub fn remember(root: &Path, category: &str, text: &str) -> std::io::Result<bool> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(false);
    }
    let mut notes = load_memory(root);
    if notes.iter().any(|n| n.text.eq_ignore_ascii_case(text)) {
        return Ok(false);
    }
    notes.push(MemoryNote {
        category: normalize_category(category),
        text: text.to_string(),
    });
    // Keep the most recent MAX_NOTES.
    if notes.len() > MAX_NOTES {
        let overflow = notes.len() - MAX_NOTES;
        notes.drain(0..overflow);
    }
    save_memory(root, &notes)?;
    Ok(true)
}

/// Render the memory as a compact, category-grouped block for the prompt.
/// Empty string when there is nothing remembered.
pub fn render_memory(notes: &[MemoryNote]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    // Stable, readable ordering by category.
    const ORDER: &[&str] = &[
        "command",
        "convention",
        "architecture",
        "gotcha",
        "decision",
        "note",
    ];
    let mut out = String::new();
    for cat in ORDER {
        let mut first = true;
        for note in notes.iter().filter(|n| n.category == *cat) {
            if first {
                out.push_str(&format!("{}:\n", cat));
                first = false;
            }
            out.push_str(&format!("  - {}\n", note.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_dedups_and_normalizes() {
        let dir = std::env::temp_dir().join(format!("kestrel-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(remember(&dir, "build", "cargo test --workspace").unwrap());
        // Same text again → not added.
        assert!(!remember(&dir, "command", "cargo test --workspace").unwrap());
        assert!(remember(&dir, "Gotcha", "the UI exe locks during rebuild").unwrap());

        let notes = load_memory(&dir);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].category, "command"); // "build" normalized
        assert_eq!(notes[1].category, "gotcha");

        let rendered = render_memory(&notes);
        assert!(rendered.contains("command:"));
        assert!(rendered.contains("cargo test --workspace"));
        assert!(rendered.contains("gotcha:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_memory_renders_nothing() {
        assert_eq!(render_memory(&[]), "");
    }
}
