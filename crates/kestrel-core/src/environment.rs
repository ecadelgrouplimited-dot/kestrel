//! Environment discovery: what the developer's machine can actually run.
//!
//! Part of the "Windows Superpower" layer — before Kestrel executes or verifies
//! anything, it should know the ground truth of the host: the OS, which shells
//! exist, whether WSL and Docker are available, and which language toolchains
//! are installed (with versions). Everything here is probed by actually
//! invoking the tools, so the report reflects reality rather than assumptions.

use std::process::{Command, Output};

/// A single tool's presence and version, as reported by running it.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub found: bool,
    pub version: Option<String>,
}

/// Whether WSL is usable and which distributions are installed.
#[derive(Debug, Clone, Default)]
pub struct WslInfo {
    pub available: bool,
    pub distros: Vec<String>,
}

/// A snapshot of the host development environment.
#[derive(Debug, Clone)]
pub struct EnvironmentReport {
    pub os: String,
    pub arch: String,
    pub shells: Vec<ToolInfo>,
    pub toolchains: Vec<ToolInfo>,
    pub wsl: WslInfo,
    pub docker: ToolInfo,
}

/// Probe the host and return a full environment report.
pub fn discover_environment() -> EnvironmentReport {
    // Unquoted `-Command` argument: nested double quotes get mangled by cmd /C,
    // but this is a single token PowerShell evaluates directly.
    let shells = [
        (
            "PowerShell",
            "powershell -NoProfile -Command $PSVersionTable.PSVersion.ToString()",
        ),
        (
            "pwsh",
            "pwsh -NoProfile -Command $PSVersionTable.PSVersion.ToString()",
        ),
        ("bash", "bash --version"),
    ]
    .into_iter()
    .map(|(name, cmd)| probe(name, cmd))
    .collect();

    let toolchains = [
        ("cargo", "cargo --version"),
        ("rustc", "rustc --version"),
        ("node", "node --version"),
        ("npm", "npm --version"),
        ("python", "python --version"),
        ("go", "go version"),
        ("dotnet", "dotnet --version"),
        ("git", "git --version"),
    ]
    .into_iter()
    .map(|(name, cmd)| probe(name, cmd))
    .collect();

    EnvironmentReport {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        shells,
        toolchains,
        wsl: detect_wsl(),
        docker: probe("docker", "docker --version"),
    }
}

/// Run `command` through the platform shell and report whether the tool ran and
/// what version it printed. Success is defined as a zero exit status.
fn probe(name: &str, command: &str) -> ToolInfo {
    match run_shell(command) {
        Some(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let text = if stdout.trim().is_empty() {
                String::from_utf8_lossy(&output.stderr).into_owned()
            } else {
                stdout.into_owned()
            };
            let version = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string);
            ToolInfo {
                name: name.to_string(),
                found: true,
                version,
            }
        }
        _ => ToolInfo {
            name: name.to_string(),
            found: false,
            version: None,
        },
    }
}

/// Detect WSL availability and installed distributions (Windows only). WSL
/// prints its lists as UTF-16LE, which this decodes.
fn detect_wsl() -> WslInfo {
    if !cfg!(windows) {
        return WslInfo::default();
    }
    let output = Command::new("wsl.exe").args(["-l", "-q"]).output();
    match output {
        Ok(out) if out.status.success() => {
            let text = decode_console(&out.stdout);
            let distros: Vec<String> = text
                .lines()
                .map(|line| line.trim().trim_end_matches('\r').to_string())
                .filter(|line| !line.is_empty())
                .collect();
            WslInfo {
                available: !distros.is_empty(),
                distros,
            }
        }
        _ => WslInfo::default(),
    }
}

/// Decode console bytes that may be UTF-16LE (as WSL emits) or UTF-8.
fn decode_console(bytes: &[u8]) -> String {
    // UTF-16LE ASCII text has a zero as every second byte; sample the first few.
    let looks_utf16 = bytes.len() >= 4
        && bytes
            .iter()
            .skip(1)
            .step_by(2)
            .take(8)
            .filter(|&&b| b == 0)
            .count()
            >= 2;
    if looks_utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Run a command line through the platform shell, capturing its output.
fn run_shell(command: &str) -> Option<Output> {
    let result = if cfg!(windows) {
        Command::new("cmd").args(["/C", command]).output()
    } else {
        Command::new("sh").args(["-c", command]).output()
    };
    result.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf16le_console_output() {
        // "Ubuntu\n" as UTF-16LE bytes.
        let text = "Ubuntu\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_console(&bytes).trim(), "Ubuntu");
    }

    #[test]
    fn decodes_plain_utf8_console_output() {
        assert_eq!(decode_console(b"hello\n").trim(), "hello");
    }

    #[test]
    fn probe_reports_missing_tool() {
        let info = probe("nope", "kestrel-nonexistent-tool-xyz --version");
        assert!(!info.found);
        assert!(info.version.is_none());
    }

    #[test]
    fn discover_runs_without_panicking() {
        let report = discover_environment();
        assert!(!report.os.is_empty());
        assert!(!report.arch.is_empty());
        // git is present in essentially every dev environment we target.
        assert!(report.toolchains.iter().any(|t| t.name == "git"));
    }
}
