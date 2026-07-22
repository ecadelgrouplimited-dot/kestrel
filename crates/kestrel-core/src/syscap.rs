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

/// Written to a background task's log by the wrapper script when the command
/// finishes, carrying its exit code.
const EXIT_MARKER: &str = "##KESTREL_EXIT:";

/// How a background task is getting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    /// Finished on its own, with this exit code.
    Finished(i32),
    /// The process is gone but left no exit marker — killed, or crashed hard.
    Stopped,
}

/// Read a task's state out of its log, given whether the process is still alive.
pub fn task_state_from(log: &str, alive: bool) -> TaskState {
    if let Some(rest) = log.rsplit(EXIT_MARKER).next() {
        // Only a real marker split yields something different from the input.
        if rest.len() != log.len() {
            let code: i32 = rest
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .parse()
                .unwrap_or(-1);
            return TaskState::Finished(code);
        }
    }
    if alive {
        TaskState::Running
    } else {
        TaskState::Stopped
    }
}

/// A background task's state plus a tail of its output, for the agent to poll.
pub fn task_status(root: &Path, pid: u32) -> String {
    let Some(task) = load_registry(root).into_iter().find(|p| p.pid == pid) else {
        return format!("error: no background task with pid {pid}");
    };
    let log = read_tail(Path::new(&task.log), 6000);
    let state = task_state_from(&log, is_alive(pid));
    // The marker is bookkeeping, not output — don't show it to the model.
    let shown = log
        .lines()
        .filter(|l| !l.contains(EXIT_MARKER))
        .collect::<Vec<_>>()
        .join("\n");
    let headline = match state {
        TaskState::Running => format!("⏳ still running (pid {pid}): {}", task.command),
        TaskState::Finished(0) => format!("✅ finished successfully (pid {pid}): {}", task.command),
        TaskState::Finished(code) => {
            format!(
                "❌ failed with exit code {code} (pid {pid}): {}",
                task.command
            )
        }
        TaskState::Stopped => format!("⏹ stopped (pid {pid}): {}", task.command),
    };
    if shown.trim().is_empty() {
        format!("{headline}\n(no output yet)")
    } else {
        format!("{headline}\n--- output ---\n{shown}")
    }
}

/// Whether a command looks like a long-running server/watcher (so it should be
/// started in the background, not run to completion where it would block).
///
/// Compound commands are split first: `cd server && npx tsx src/index.ts` is a
/// server, even though no single listed phrase matches the whole string. Missing
/// that is how a run blocks for half an hour on a process that never exits.
pub fn is_long_running(command: &str) -> bool {
    command
        .split(['\n', ';'])
        .flat_map(|part| part.split("&&"))
        .flat_map(|part| part.split("||"))
        .flat_map(|part| part.split('|'))
        .any(segment_is_long_running)
}

/// Whether one segment of a command never returns on its own.
fn segment_is_long_running(segment: &str) -> bool {
    let c = segment.trim().to_lowercase();
    if c.is_empty() {
        return false;
    }
    if LONG_RUNNING_SIGNALS.iter().any(|s| c.contains(s)) {
        return true;
    }
    // Watchers of every flavour.
    if c.contains("--watch") || c.contains(" -w ") || c.ends_with(" -w") || c.contains("watchexec")
    {
        return true;
    }
    // Foreground container/tunnel/log commands.
    if (c.contains("docker compose up") || c.contains("docker-compose up"))
        && !c.contains("-d")
        && !c.contains("--detach")
    {
        return true;
    }
    if c.starts_with("ngrok") || c.contains("port-forward") || c.contains("tail -f") {
        return true;
    }
    // A runtime pointed at an entry-point file is almost always a server:
    // `npx tsx src/index.ts`, `node dist/server.js`, `bun run main.ts`.
    let runtimes = [
        "node ",
        "npx tsx",
        "tsx ",
        "ts-node",
        "bun ",
        "deno run",
        "python -m",
        "python3 -m",
    ];
    if runtimes.iter().any(|r| c.contains(r)) {
        let entrypoints = ["index.", "server.", "main.", "app.", "start."];
        if entrypoints.iter().any(|e| c.contains(e)) {
            return true;
        }
    }
    false
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
    /// A local URL detected in the app's output, if any (for a preview button).
    pub url: Option<String>,
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
        .map(|p| {
            let url = detect_url(&read_tail(Path::new(&p.log), 6000));
            RunningApp {
                pid: p.pid,
                command: p.command,
                log: p.log,
                url,
            }
        })
        .collect()
}

/// Find the first local server URL (localhost/127.0.0.1/0.0.0.0) printed in
/// `text`, normalising `0.0.0.0` to `localhost`. Used to auto-fill the preview.
pub fn detect_url(text: &str) -> Option<String> {
    let separators = [' ', '\t', '\n', '\r', '"', '\'', '(', ')', '<', '>', '`'];
    for raw in text.split(|c: char| separators.contains(&c)) {
        if raw.starts_with("http://") || raw.starts_with("https://") {
            let low = raw.to_lowercase();
            if low.contains("localhost") || low.contains("127.0.0.1") || low.contains("0.0.0.0") {
                let url = raw.trim_end_matches(['.', ',', ';', '!']);
                return Some(url.replace("0.0.0.0", "localhost"));
            }
        }
    }
    None
}

/// Open a local file (e.g. a screenshot) with the OS default application.
pub fn open_path(path: &str) -> String {
    let spawned = if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", path]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    };
    match spawned {
        Ok(_) => format!("opened {path}"),
        Err(err) => format!("error: {err}"),
    }
}

/// The saved screenshots for a project, newest first.
pub fn list_screenshots(root: &Path) -> Vec<PathBuf> {
    let dir = root.join(".kestrel").join("screenshots");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "png").unwrap_or(false))
        .collect();
    files.sort();
    files.reverse();
    files
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

    // Run through a tiny wrapper script that echoes the exit code when the
    // command finishes. Servers never reach it; a finite task (an install, a
    // build) leaves a marker so we can report success or failure later. A
    // script file avoids cmd's `%ERRORLEVEL%` expansion-time trap entirely.
    let script_path = logs_dir.join(format!(
        "task-{}.{}",
        epoch_millis(),
        if cfg!(windows) { "bat" } else { "sh" }
    ));
    let script = if cfg!(windows) {
        format!("@echo off\r\n{command}\r\necho {EXIT_MARKER}%ERRORLEVEL%\r\n")
    } else {
        format!("{command}\necho \"{EXIT_MARKER}$?\"\n")
    };
    if let Err(err) = std::fs::write(&script_path, script) {
        return format!("error: {err}");
    }
    let spawned = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(&script_path)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(out_file)
            .stderr(err_file)
            .spawn()
    } else {
        Command::new("sh")
            .arg(&script_path)
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
    fn compound_commands_hiding_a_server_are_caught() {
        // The exact command that blocked a run for 23 minutes: no listed phrase
        // matches the whole string, but the last segment never returns.
        assert!(is_long_running(
            r"cd E:\Projects\RealtimeKanban\server && npx tsx src/seed.ts && npx tsx src/index.ts"
        ));
        assert!(is_long_running("cd server; node dist/server.js"));
        assert!(is_long_running("npm run build && npm run dev"));
        assert!(is_long_running("cargo build --release && ./target/app.js"));
    }

    #[test]
    fn watchers_containers_and_tunnels_are_long_running() {
        assert!(is_long_running("tsc --watch"));
        assert!(is_long_running("docker compose up"));
        assert!(is_long_running("ngrok http 3000"));
        assert!(is_long_running("kubectl port-forward svc/api 8080:80"));
        assert!(is_long_running("tail -f server.log"));
        // Detached compose returns immediately, so it's fine to run inline.
        assert!(!is_long_running("docker compose up -d"));
    }

    #[test]
    fn finite_commands_are_not_blocked() {
        // These end on their own — refusing them would be wrong.
        assert!(!is_long_running("npm install"));
        assert!(!is_long_running("cargo test --workspace"));
        assert!(!is_long_running("npx tsc --noEmit"));
        assert!(!is_long_running("git status"));
        assert!(!is_long_running("python scripts/migrate.py"));
    }

    #[test]
    fn task_state_reads_the_exit_marker() {
        let done = format!("installing…\ndone\n{EXIT_MARKER}0\n");
        assert_eq!(task_state_from(&done, false), TaskState::Finished(0));
        let failed = format!("boom\n{EXIT_MARKER}1\n");
        assert_eq!(task_state_from(&failed, false), TaskState::Finished(1));
        // A server has no marker and is still alive.
        assert_eq!(
            task_state_from("listening on :3000", true),
            TaskState::Running
        );
        // Gone without a marker means it was killed.
        assert_eq!(task_state_from("partial output", false), TaskState::Stopped);
    }

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
    fn detects_local_server_url() {
        assert_eq!(
            detect_url("  ➜  Local:   http://localhost:5173/"),
            Some("http://localhost:5173/".to_string())
        );
        assert_eq!(
            detect_url("Server running on http://0.0.0.0:3000."),
            Some("http://localhost:3000".to_string())
        );
        assert_eq!(detect_url("no url here, just building…"), None);
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
