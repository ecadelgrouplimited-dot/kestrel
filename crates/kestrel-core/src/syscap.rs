//! System capabilities for the agent: open a browser/preview, run and stop
//! long-lived apps (dev servers), take a screenshot, and detect-or-install
//! missing tooling. These turn Kestrel from a coding agent into one that can
//! actually stand a project up and drive it on the machine.
//!
//! Long-running apps are tracked in `<project>/.kestrel/processes.json` so they
//! can be listed and stopped across tool calls and sessions.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Command fragments that indicate a long-running process (a server/watcher),
/// which must be started in the background rather than run to completion.
const LONG_RUNNING_SIGNALS: &[&str] = &[
    "npm run dev",
    "npm start",
    "yarn dev",
    "yarn start",
    "pnpm dev",
    "pnpm start",
    "artisan serve",
    "manage.py runserver",
    "rails server",
    "rails s",
    "flask run",
    "next dev",
    "nodemon",
    "http-server",
    "php -s",
    "webpack serve",
    "ng serve",
    "gatsby develop",
    "vite",
    "uvicorn",
    "gunicorn",
    "server.js",
    "app.js",
    "serve -",
];

/// Whether a command looks like a long-running server/watcher (so it should be
/// started in the background, not run to completion where it would block).
pub fn is_long_running(command: &str) -> bool {
    let c = command.to_lowercase();
    LONG_RUNNING_SIGNALS.iter().any(|s| c.contains(s))
}

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
    #[serde(default)]
    log: String,
}

/// The last `max_bytes` of a file as text, or empty if it can't be read.
fn read_tail(path: &Path, max_bytes: usize) -> String {
    let mut text = match File::open(path) {
        Ok(mut f) => {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            s
        }
        Err(_) => return String::new(),
    };
    if text.len() > max_bytes {
        let start = text.len() - max_bytes;
        let start = (start..text.len())
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(text.len());
        text = format!("…\n{}", &text[start..]);
    }
    text
}

/// Kill a process tree by pid.
fn kill_pid(pid: u32) -> bool {
    let result = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).output()
    };
    result.map(|o| o.status.success()).unwrap_or(false)
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

/// A background app Kestrel is tracking.
#[derive(Debug, Clone)]
pub struct RunningApp {
    pub pid: u32,
    pub command: String,
    pub log: String,
}

/// The background apps that are still running (prunes any that have exited).
pub fn running_apps(root: &Path) -> Vec<RunningApp> {
    let alive: Vec<TrackedProcess> = load_registry(root)
        .into_iter()
        .filter(|p| is_alive(p.pid))
        .collect();
    save_registry(root, &alive);
    alive
        .into_iter()
        .map(|p| RunningApp {
            pid: p.pid,
            command: p.command,
            log: p.log,
        })
        .collect()
}

/// Poll a URL until it responds (any HTTP status) or `timeout_secs` elapses.
pub fn http_check(url: &str, timeout_secs: u64) -> String {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return "error: only http(s) URLs".to_string();
    }
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let start = Instant::now();
    loop {
        if let Ok(out) = Command::new("curl")
            .args(["-sS", "-o", null, "-m", "5", "-w", "%{http_code}", url])
            .output()
        {
            let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !code.is_empty() && code != "000" {
                return format!("{url} responded with HTTP {code}");
            }
        }
        if start.elapsed().as_secs() >= timeout_secs {
            return format!(
                "{url} did not respond within {timeout_secs}s — is the server running? Check app_logs."
            );
        }
        std::thread::sleep(Duration::from_millis(600));
    }
}

/// Start a long-running app (e.g. a dev server) in the background, capturing its
/// output to a log, and track it. Stops any previous instance of the same
/// command first (so re-running is clean), then does a brief health check so a
/// server that crashes on startup is reported immediately with its output.
pub fn start_app(root: &Path, command: &str) -> String {
    start_app_inner(root, command, true)
}

/// Like [`start_app`] but returns immediately without the health-check wait,
/// for a snappy UI Start button.
pub fn start_app_detached(root: &Path, command: &str) -> String {
    start_app_inner(root, command, false)
}

fn start_app_inner(root: &Path, command: &str, wait: bool) -> String {
    if command.trim().is_empty() {
        return "error: empty command".to_string();
    }
    // Clean re-run: stop and drop any previous instance of the same command.
    let mut procs = load_registry(root);
    for p in procs.iter().filter(|p| p.command == command) {
        if is_alive(p.pid) {
            kill_pid(p.pid);
        }
    }
    procs.retain(|p| p.command != command);

    let logs_dir = root.join(".kestrel").join("logs");
    if let Err(err) = std::fs::create_dir_all(&logs_dir) {
        return format!("error: {err}");
    }
    let log_path = logs_dir.join(format!("app-{}.log", epoch_millis()));
    let out_file = match File::create(&log_path) {
        Ok(file) => file,
        Err(err) => return format!("error: {err}"),
    };
    let err_file = match out_file.try_clone() {
        Ok(file) => file,
        Err(err) => return format!("error: {err}"),
    };

    let spawned = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(out_file)
            .stderr(err_file)
            .spawn()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(out_file)
            .stderr(err_file)
            .spawn()
    };
    match spawned {
        Ok(child) => {
            let pid = child.id();
            // Detach: dropping the handle does not kill the process.
            procs.push(TrackedProcess {
                pid,
                command: command.to_string(),
                started: epoch_secs().to_string(),
                log: log_path.display().to_string(),
            });
            save_registry(root, &procs);

            if !wait {
                return format!(
                    "started (pid {pid}) in the background: {command}\nlog: {}",
                    log_path.display()
                );
            }
            // Give it a moment to bind/crash, then report.
            std::thread::sleep(Duration::from_millis(1500));
            let tail = read_tail(&log_path, 1500);
            if is_alive(pid) {
                format!(
                    "started (pid {pid}): {command} — running in the background.\nInitial output:\n{}\n(Use app_logs({pid}) for more, stop_app({pid}) to stop.)",
                    if tail.trim().is_empty() { "(no output yet)".to_string() } else { tail }
                )
            } else {
                format!(
                    "the app exited immediately (pid {pid}) — it likely crashed on startup. Output:\n{}",
                    if tail.trim().is_empty() { "(no output)".to_string() } else { tail }
                )
            }
        }
        Err(err) => format!("error: could not start: {err}"),
    }
}

/// Read the recent output of a background app by pid.
pub fn app_logs(root: &Path, pid: u32) -> String {
    let procs = load_registry(root);
    match procs.iter().find(|p| p.pid == pid) {
        Some(p) => {
            let tail = read_tail(Path::new(&p.log), 6000);
            format!(
                "logs for pid {pid} ({}):\n{}",
                p.command,
                if tail.trim().is_empty() {
                    "(no output yet)".to_string()
                } else {
                    tail
                }
            )
        }
        None => format!("no tracked app with pid {pid} (use list_apps to see running apps)"),
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
    let killed = kill_pid(pid);
    let mut procs = load_registry(root);
    procs.retain(|p| p.pid != pid);
    save_registry(root, &procs);
    if killed {
        format!("stopped app (pid {pid})")
    } else {
        format!("could not stop pid {pid} (it may have already exited)")
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
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
    fn detects_long_running_commands() {
        assert!(is_long_running("node server.js"));
        assert!(is_long_running("npm run dev"));
        assert!(is_long_running("php artisan serve"));
        assert!(is_long_running("npx vite"));
        assert!(!is_long_running("npm run build"));
        assert!(!is_long_running("cargo test"));
        assert!(!is_long_running("node scripts/migrate.js"));
    }

    #[test]
    fn start_app_captures_output_to_a_log() {
        let dir = std::env::temp_dir().join(format!("kestrel-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A command that prints and exits — exercises log capture + health check.
        let out = start_app(&dir, "echo kestrel_app_ok");
        assert!(out.contains("kestrel_app_ok"), "got: {out}");
        // The tracked process's log holds the output.
        let procs = load_registry(&dir);
        assert_eq!(procs.len(), 1);
        assert!(read_tail(Path::new(&procs[0].log), 4000).contains("kestrel_app_ok"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_exists_detects_git() {
        // git is required for the repo workflow and present in CI/dev.
        assert!(command_exists("git"));
        assert!(!command_exists("kestrel-nonexistent-command-xyz"));
    }
}
