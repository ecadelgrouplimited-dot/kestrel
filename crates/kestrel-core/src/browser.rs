//! Browser-driven acceptance checks.
//!
//! "Done" should be *demonstrated*, not asserted. When the agent builds a web app
//! it can `check_page` a running URL: Kestrel renders the page with the machine's
//! own headless Chrome/Edge (post-JavaScript DOM — no bundled browser, no heavy
//! dependency) and verifies the expected content is actually there. If no
//! Chromium browser is installed, the caller falls back to the raw HTTP body.

use std::path::Path;
use std::process::Command;

/// Locate an installed Chromium-family browser for headless rendering.
pub fn find_browser() -> Option<String> {
    let mut candidates = vec![
        r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe".to_string(),
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".to_string(),
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe".to_string(),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(format!(r"{local}\Google\Chrome\Application\chrome.exe"));
    }
    candidates.into_iter().find(|c| Path::new(c).exists())
}

/// Render `url` with a headless browser and return the post-JavaScript DOM.
pub fn render_dom(url: &str) -> Result<String, String> {
    let browser = find_browser().ok_or_else(|| "no Chrome/Edge found for rendering".to_string())?;
    // A throwaway profile forces a fresh instance that exits after dumping,
    // instead of attaching to a running browser and hanging.
    let profile = std::env::temp_dir().join(format!("kestrel-headless-{}", std::process::id()));
    let out = Command::new(&browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--virtual-time-budget=5000",
            "--dump-dom",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .output()
        .map_err(|e| format!("could not run the browser: {e}"))?;
    let _ = std::fs::remove_dir_all(&profile);
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!(
            "headless render failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Check that every expected substring appears in `body` (case-insensitive).
/// Returns the substrings that are missing (empty ⇒ all present).
pub fn missing_content(body: &str, expects: &[String]) -> Vec<String> {
    let hay = body.to_ascii_lowercase();
    expects
        .iter()
        .filter(|e| !e.trim().is_empty())
        .filter(|e| !hay.contains(&e.to_ascii_lowercase()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_checks_are_case_insensitive() {
        let body = "<html><body><h1>Welcome to Kestrel</h1><p>Download now</p></body></html>";
        assert!(missing_content(body, &["kestrel".into(), "download".into()]).is_empty());
        let missing = missing_content(body, &["Kestrel".into(), "Pricing".into()]);
        assert_eq!(missing, vec!["Pricing".to_string()]);
        // Blank expectations are ignored.
        assert!(missing_content(body, &["".into(), "  ".into()]).is_empty());
    }
}
