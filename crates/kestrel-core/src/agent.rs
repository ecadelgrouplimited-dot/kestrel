//! The build agent: turn a model reply into real files on disk.
//!
//! Kestrel's chat can *answer*; the agent can *act*. Given a request like "build
//! me a portfolio site", the model is instructed to emit a file manifest in a
//! strict, fence-free protocol, which this module parses into [`FileEdit`]s and
//! writes under the project root — refusing any path that escapes it. This is
//! the single-shot wedge of the agentic loop: propose a complete set of files,
//! apply them, and show the result. (A multi-turn tool loop that reads, edits,
//! and verifies iteratively is the next step up from here.)

use std::path::{Component, Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
