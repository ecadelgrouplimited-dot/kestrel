//! Multi-repository workspaces.
//!
//! A project can be *linked* to other repositories, forming a workspace, so the
//! agent can reason across them — search a sibling service, read a shared
//! library, trace a call that crosses repo boundaries. The links are stored in
//! the primary project's `.kestrel/workspace.json`, and the agent reaches them
//! through `list_repos` and a `repo`-scoped `search` (reads already accept
//! absolute paths anywhere).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One linked repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub name: String,
    pub path: String,
}

/// The set of repositories linked to a project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub repos: Vec<Repo>,
}

/// The path to a project's workspace file.
pub fn workspace_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("workspace.json")
}

/// Load a project's linked repositories (empty if none).
pub fn load_workspace(root: &Path) -> Workspace {
    std::fs::read_to_string(workspace_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist a project's linked repositories.
pub fn save_workspace(root: &Path, workspace: &Workspace) -> std::io::Result<()> {
    let path = workspace_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(workspace)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// Link `repo_path` to the project at `primary`, named after its folder.
/// De-duplicates by path and refuses to link a project to itself.
pub fn link_repo(primary: &Path, repo_path: &Path) -> std::io::Result<Workspace> {
    let mut ws = load_workspace(primary);
    let path = repo_path.display().to_string();
    if repo_path != primary && !ws.repos.iter().any(|r| r.path == path) {
        let name = repo_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        ws.repos.push(Repo { name, path });
    }
    save_workspace(primary, &ws)?;
    Ok(ws)
}

/// Remove a linked repository by path.
pub fn unlink_repo(primary: &Path, repo_path: &str) -> std::io::Result<Workspace> {
    let mut ws = load_workspace(primary);
    ws.repos.retain(|r| r.path != repo_path);
    save_workspace(primary, &ws)?;
    Ok(ws)
}

/// Resolve a repo name (or `"primary"`, or a raw path) to its root directory,
/// for a repo-scoped tool.
pub fn resolve_repo(primary: &Path, name: &str) -> Option<PathBuf> {
    if name.eq_ignore_ascii_case("primary") || name.trim().is_empty() {
        return Some(primary.to_path_buf());
    }
    load_workspace(primary)
        .repos
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name) || r.path == name)
        .map(|r| PathBuf::from(r.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_unlink_and_resolve() {
        let dir = std::env::temp_dir().join(format!("kestrel-ws-{}", std::process::id()));
        let primary = dir.join("primary");
        let other = dir.join("service-b");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let ws = link_repo(&primary, &other).unwrap();
        assert_eq!(ws.repos.len(), 1);
        assert_eq!(ws.repos[0].name, "service-b");
        // Idempotent + self-link refused.
        link_repo(&primary, &other).unwrap();
        link_repo(&primary, &primary).unwrap();
        assert_eq!(load_workspace(&primary).repos.len(), 1);

        assert_eq!(resolve_repo(&primary, "service-b"), Some(other.clone()));
        assert_eq!(resolve_repo(&primary, "primary"), Some(primary.clone()));
        assert!(resolve_repo(&primary, "nope").is_none());

        let ws = unlink_repo(&primary, &other.display().to_string()).unwrap();
        assert!(ws.repos.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
