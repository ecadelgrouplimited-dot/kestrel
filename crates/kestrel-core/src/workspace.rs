//! Workspace filesystem operations for the desktop file explorer.
//!
//! The native app needs to browse a project's directory tree and create,
//! rename, and delete files and folders. That logic lives here — free of any
//! UI toolkit and validated against a real temp directory in the tests — so the
//! UI layer only has to render a tree and wire buttons.

use std::io;
use std::path::{Path, PathBuf};

/// Directory names hidden from the explorer: build output and VCS internals
/// that would bury the actual source (and, for `target`, be enormous).
const HIDDEN_DIRS: [&str; 5] = [".git", "target", "node_modules", ".kestrel", "dist"];

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// List the immediate children of `dir`, folders first then files, each group
/// alphabetical (case-insensitive). Build-output and VCS directories are
/// hidden. Returns an error if `dir` cannot be read.
pub fn read_dir_entries(dir: &Path) -> io::Result<Vec<WorkspaceEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && HIDDEN_DIRS.contains(&name.as_str()) {
            continue;
        }
        entries.push(WorkspaceEntry {
            name,
            path: entry.path(),
            is_dir,
        });
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// Validate a single path component (a file or folder name, not a path).
pub fn validate_entry_name(name: &str) -> Result<&str, String> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err("name is empty".to_string());
    }
    if clean == "." || clean == ".." {
        return Err("that name is reserved".to_string());
    }
    if clean.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("name contains a path separator or reserved character".to_string());
    }
    Ok(clean)
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Create a new empty folder `name` inside `parent`. Fails if it already exists.
pub fn create_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let clean = validate_entry_name(name).map_err(invalid_input)?;
    let path = parent.join(clean);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        ));
    }
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// Create a new empty file `name` inside `parent`. Fails if it already exists.
pub fn create_file(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let clean = validate_entry_name(name).map_err(invalid_input)?;
    let path = parent.join(clean);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, "")?;
    Ok(path)
}

/// Rename the file or folder at `path` to `new_name` (keeping it in the same
/// parent directory). Returns the new path. Fails if the target already exists.
pub fn rename_entry(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let clean = validate_entry_name(new_name).map_err(invalid_input)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid_input("cannot rename the root".to_string()))?;
    let target = parent.join(clean);
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", target.display()),
        ));
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Delete the file or folder at `path` (folders are removed recursively).
pub fn delete_entry(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Read a file as UTF-8 text.
pub fn read_text_file(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

/// Write UTF-8 text to a file, creating parent directories as needed.
pub fn write_text_file(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kestrel-workspace-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_dir_lists_folders_first_and_hides_noise() {
        let dir = temp_dir("list");
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "x").unwrap();

        let entries = read_dir_entries(&dir).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "Cargo.toml", "README.md"]);
        assert!(entries[0].is_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_and_rename_and_delete_roundtrip() {
        let dir = temp_dir("crud");
        let file = create_file(&dir, "notes.txt").unwrap();
        assert!(file.is_file());
        assert!(create_file(&dir, "notes.txt").is_err()); // no overwrite

        let folder = create_dir(&dir, "docs").unwrap();
        assert!(folder.is_dir());

        let renamed = rename_entry(&file, "readme.txt").unwrap();
        assert!(renamed.is_file());
        assert!(!file.exists());

        write_text_file(&renamed, "hello").unwrap();
        assert_eq!(read_text_file(&renamed).unwrap(), "hello");

        delete_entry(&renamed).unwrap();
        assert!(!renamed.exists());
        delete_entry(&folder).unwrap();
        assert!(!folder.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_entry_name("  ").is_err());
        assert!(validate_entry_name("a/b").is_err());
        assert!(validate_entry_name("..").is_err());
        assert!(validate_entry_name("good_name.rs").is_ok());
    }
}
