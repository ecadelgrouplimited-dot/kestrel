//! Project management: creating new projects and tracking recent ones.
//!
//! Opening an *existing* project is just picking a folder (the UI does that
//! with a native dialog), but creating a *new* one means scaffolding a minimal,
//! Kestrel-ready layout, and both flows want a "recent projects" list. That
//! logic is here — free of any UI toolkit — so it can be unit-tested against a
//! real temp directory rather than only exercised by clicking buttons.

use std::io;
use std::path::{Path, PathBuf};

/// The outcome of scaffolding a new project.
#[derive(Debug, Clone)]
pub struct NewProject {
    /// The created project root.
    pub root: PathBuf,
    /// Whether a git repository was initialized (false if `git` was absent).
    pub git_initialized: bool,
}

/// Create a new project directory `name` under `parent`, scaffolded with a
/// starter `kestrel.toml`, a `README.md`, and a `.gitignore`, and (if `git` is
/// on `PATH`) an initialized git repository.
///
/// Fails if `name` is empty/invalid or the target directory already exists and
/// is non-empty, so we never scribble into someone else's folder.
pub fn create_project(parent: &Path, name: &str) -> io::Result<NewProject> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project name is empty",
        ));
    }
    if clean.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project name contains a path separator or reserved character",
        ));
    }

    let root = parent.join(clean);
    if root.exists() {
        let non_empty = std::fs::read_dir(&root)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        if non_empty {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists and is not empty", root.display()),
            ));
        }
    }
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(root.join("src"))?;

    write_if_absent(&root.join("kestrel.toml"), &starter_kestrel_toml())?;
    write_if_absent(&root.join("README.md"), &starter_readme(clean))?;
    write_if_absent(&root.join(".gitignore"), STARTER_GITIGNORE)?;

    let git_initialized = git_init(&root);

    Ok(NewProject {
        root,
        git_initialized,
    })
}

/// Write `contents` to `path` only if the file does not already exist.
fn write_if_absent(path: &Path, contents: &str) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, contents)
}

fn starter_kestrel_toml() -> String {
    "# Kestrel project configuration.\n\
     # Everything here is optional; CLI flags override these, which override the\n\
     # built-in defaults.\n\n\
     [defaults]\n\
     # model = \"claude-opus-4-8\"\n\
     # budget = 12000\n\
     # max_tokens = 8192\n\n\
     [verify]\n\
     # Pin the exact checks a change must pass before it lands.\n\
     # steps = [\n\
     #   \"cargo fmt --all -- --check\",\n\
     #   \"cargo test\",\n\
     # ]\n"
        .to_string()
}

fn starter_readme(name: &str) -> String {
    format!(
        "# {name}\n\n\
         A new project scaffolded by Kestrel.\n\n\
         ## Getting started\n\n\
         Open this folder in Kestrel, configure a model provider in Settings, and\n\
         start asking questions or proposing verified edits.\n"
    )
}

const STARTER_GITIGNORE: &str = "/target\n/.kestrel\n";

/// Initialize a git repository in `root`. Returns whether it succeeded (false
/// if `git` is not installed or the command failed — a project without git is
/// still a valid project).
fn git_init(root: &Path) -> bool {
    std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The maximum number of recent projects to remember.
pub const MAX_RECENTS: usize = 12;

/// Add `path` to the front of a recent-projects list, de-duplicated
/// (case-insensitively on the normalized path) and capped at [`MAX_RECENTS`].
pub fn push_recent(recents: &mut Vec<String>, path: &Path) {
    let entry = path.display().to_string();
    let key = entry.to_lowercase();
    recents.retain(|p| p.to_lowercase() != key);
    recents.insert(0, entry);
    recents.truncate(MAX_RECENTS);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kestrel-project-{tag}-{}-{:?}",
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
    fn create_project_scaffolds_expected_files() {
        let parent = temp_dir("create");
        let project = create_project(&parent, "demo").unwrap();
        assert_eq!(project.root, parent.join("demo"));
        assert!(project.root.join("kestrel.toml").is_file());
        assert!(project.root.join("README.md").is_file());
        assert!(project.root.join(".gitignore").is_file());
        assert!(project.root.join("src").is_dir());
        let readme = std::fs::read_to_string(project.root.join("README.md")).unwrap();
        assert!(readme.contains("# demo"));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn create_project_rejects_bad_names() {
        let parent = temp_dir("bad");
        assert!(create_project(&parent, "  ").is_err());
        assert!(create_project(&parent, "a/b").is_err());
        assert!(create_project(&parent, "a:b").is_err());
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn create_project_refuses_non_empty_target() {
        let parent = temp_dir("nonempty");
        let existing = parent.join("taken");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("keep.txt"), "hi").unwrap();
        assert!(create_project(&parent, "taken").is_err());
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn push_recent_dedupes_and_caps() {
        let mut recents = Vec::new();
        push_recent(&mut recents, Path::new("E:/a"));
        push_recent(&mut recents, Path::new("E:/b"));
        push_recent(&mut recents, Path::new("E:/a")); // move to front, no dup
        assert_eq!(recents, vec!["E:/a".to_string(), "E:/b".to_string()]);

        for i in 0..MAX_RECENTS + 5 {
            push_recent(&mut recents, Path::new(&format!("E:/p{i}")));
        }
        assert_eq!(recents.len(), MAX_RECENTS);
        assert_eq!(recents[0], format!("E:/p{}", MAX_RECENTS + 4));
    }
}
