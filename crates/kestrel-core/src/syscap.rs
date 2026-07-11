//! System capabilities for the agent: open a browser/preview, run and stop
//! long-lived apps (dev servers), take a screenshot, and detect-or-install
//! missing tooling. These turn Kestrel from a coding agent into one that can
//! actually stand a project up and drive it on the machine.
//!
//! Long-running apps are tracked in `<project>/.kestrel/processes.json` so they
//! can be listed and stopped across tool calls and sessions.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Open a URL (or local file) in the default browser.
pub fn open_url(url: &str) -> String {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")) {
        return "error: only http(s) or file URLs can be opened".to_string();
    }
    let spawned = if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    match spawned {
        Ok(_) => format!("opened {url} in the default browser"),
        Err(err) => format!("error: could not open browser: {err}"),
    }
}

/// Whether a command is available on `PATH`.
pub fn command_exists(command: &str) -> bool {
    let probe = if cfg!(windows) {
        Command::new("where").arg(command).output()
    } else {
        Command::new("sh")
            .args(["-c", &format!("command -v {command}")])
            .output()
    };
    probe.map(|o| o.status.success()).unwrap_or(false)
}

/// The winget package id for a well-known command, if we know it.
fn known_winget_id(command: &str) -> Option<&'static str> {
    Some(match command.to_lowercase().as_str() {
        "node" | "npm" | "npx" => "OpenJS.NodeJS.LTS",
        "git" => "Git.Git",
        "python" | "python3" | "pip" => "Python.Python.3.12",
        "php" => "PHP.PHP",
        "composer" => "Composer.Composer",
        "docker" => "Docker.DockerDesktop",
        "gh" => "GitHub.cli",
        "rustup" | "cargo" => "Rustlang.Rustup",
        "go" => "GoLang.Go",
        "dotnet" => "Microsoft.DotNet.SDK.8",
        "yarn" => "Yarn.Yarn",
        "pnpm" => "pnpm.pnpm",
        _ => return None,
    })
}

/// Ensure a command is available: if missing, install it via winget (Windows).
/// `package` overrides the guessed winget id. Best-effort — winget installs can
/// take minutes and may require elevation.
pub fn ensure_tool(command: &str, package: Option<&str>) -> String {
    if command_exists(command) {
        return format!("{command} is already installed");
    }
    if !cfg!(windows) {
        return format!(
            "{command} is not installed; automatic install is currently wired for Windows (winget) only"
        );
    }
    let id = package
        .map(str::to_string)
        .or_else(|| known_winget_id(command).map(str::to_string))
        .unwrap_or_else(|| command.to_string());

    let output = Command::new("winget")
        .args([
            "install",
            "--id",
            &id,
            "-e",
            "--silent",
            "--accept-source-agreements",
            "--accept-package-agreements",
        ])
        .output();
    match output {
        Ok(out) => {
            let tail = String::from_utf8_lossy(&out.stdout);
            let tail = tail.lines().rev().take(3).collect::<Vec<_>>();
            let tail: Vec<_> = tail.into_iter().rev().collect();
            if command_exists(command) || out.status.success() {
                format!("installed {command} via winget ({id}). {}", tail.join(" "))
            } else {
                format!(
                    "winget install of {id} did not confirm {command}. {}",
                    tail.join(" ")
                )
            }
        }
        Err(err) => format!("error: winget is not available ({err})"),
    }
}

/// Capture the screen to `<project>/.kestrel/screenshots/` (Windows).
pub fn take_screenshot(root: &Path) -> String {
    if !cfg!(windows) {
        return "error: screenshots are currently Windows-only".to_string();
    }
    let dir = root.join(".kestrel").join("screenshots");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return format!("error: {err}");
    }
    let file = dir.join(format!("shot-{}.png", epoch_secs()));
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
         $b=[System.Windows.Forms.SystemInformation]::VirtualScreen; \
         $bmp=New-Object System.Drawing.Bitmap($b.Width,$b.Height); \
         $g=[System.Drawing.Graphics]::FromImage($bmp); \
         $g.CopyFromScreen($b.X,$b.Y,0,0,$bmp.Size); \
         $bmp.Save('{}'); $g.Dispose(); $bmp.Dispose()",
        file.display()
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    match output {
        Ok(out) if out.status.success() => format!("saved screenshot to {}", file.display()),
        Ok(out) => format!("error: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(err) => format!("error: {err}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrackedProcess {
    pid: u32,
    command: String,
    started: String,
}

fn registry_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("processes.json")
}

fn load_registry(root: &Path) -> Vec<TrackedProcess> {
    std::fs::read_to_string(registry_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_registry(root: &Path, procs: &[TrackedProcess]) {
    let path = registry_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(procs) {
        let _ = std::fs::write(path, text);
    }
}

fn is_alive(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Start a long-running app (e.g. a dev server) in the background and track it.
pub fn start_app(root: &Path, command: &str) -> String {
    if command.trim().is_empty() {
        return "error: empty command".to_string();
    }
    let spawned = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(root)
            .spawn()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(root)
            .spawn()
    };
    match spawned {
        Ok(child) => {
            let pid = child.id();
            // Detach: dropping the handle does not kill the process.
            let mut procs = load_registry(root);
            procs.push(TrackedProcess {
                pid,
                command: command.to_string(),
                started: epoch_secs().to_string(),
            });
            save_registry(root, &procs);
            format!("started (pid {pid}): {command} — running in the background")
        }
        Err(err) => format!("error: could not start: {err}"),
    }
}

/// List the tracked background apps, dropping any that have exited.
pub fn list_apps(root: &Path) -> String {
    let procs = load_registry(root);
    let alive: Vec<TrackedProcess> = procs.into_iter().filter(|p| is_alive(p.pid)).collect();
    save_registry(root, &alive);
    if alive.is_empty() {
        return "no background apps are running".to_string();
    }
    let mut out = String::from("running apps:\n");
    for p in &alive {
        out.push_str(&format!("  pid {} — {}\n", p.pid, p.command));
    }
    out
}

/// Stop a tracked background app by pid (kills the whole process tree).
pub fn stop_app(root: &Path, pid: u32) -> String {
    let result = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).output()
    };
    let mut procs = load_registry(root);
    procs.retain(|p| p.pid != pid);
    save_registry(root, &procs);
    match result {
        Ok(out) if out.status.success() => format!("stopped app (pid {pid})"),
        Ok(out) => format!(
            "could not stop pid {pid}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => format!("error: {err}"),
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_rejects_bad_schemes() {
        assert!(open_url("javascript:alert(1)").starts_with("error:"));
        assert!(open_url("/etc/passwd").starts_with("error:"));
    }

    #[test]
    fn known_ids_cover_common_tools() {
        assert_eq!(known_winget_id("node"), Some("OpenJS.NodeJS.LTS"));
        assert_eq!(known_winget_id("composer"), Some("Composer.Composer"));
        assert_eq!(known_winget_id("php"), Some("PHP.PHP"));
        assert!(known_winget_id("totally-unknown-xyz").is_none());
    }

    #[test]
    fn command_exists_detects_git() {
        // git is required for the repo workflow and present in CI/dev.
        assert!(command_exists("git"));
        assert!(!command_exists("kestrel-nonexistent-command-xyz"));
    }
}
