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

/// A numeric assertion about something the page reports — "FPS at least 55".
///
/// Structural checks ("the FPS counter exists") pass even when the number is
/// bad, which is how "60fps" quietly ships at 46. This turns a target into a
/// measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricCheck {
    /// The on-screen label preceding the value, e.g. `FPS`.
    pub label: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// Find the number that follows `label` in rendered text — "FPS: 46", "FPS 46",
/// "FPS = 46.5" all yield `46`/`46.5`. Case-insensitive.
pub fn find_metric(text: &str, label: &str) -> Option<f64> {
    let hay = text.to_ascii_lowercase();
    let needle = label.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut from = 0;
    while let Some(pos) = hay[from..].find(&needle) {
        let after = from + pos + needle.len();
        // Skip the separators that normally sit between a label and its value.
        let rest = text[after..].trim_start_matches(|c: char| {
            c.is_whitespace() || c == ':' || c == '=' || c == '\u{a0}'
        });
        let number: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        if let Ok(value) = number.trim_end_matches('.').parse::<f64>() {
            return Some(value);
        }
        from = after;
    }
    None
}

/// Evaluate numeric assertions, returning a human-readable line per failure.
/// An empty result means every measurement passed.
pub fn failed_metrics(text: &str, checks: &[MetricCheck]) -> Vec<String> {
    let mut failures = Vec::new();
    for check in checks {
        let Some(value) = find_metric(text, &check.label) else {
            failures.push(format!("{} not found on the page", check.label));
            continue;
        };
        if let Some(min) = check.min {
            if value < min {
                failures.push(format!("{} is {value} (needs at least {min})", check.label));
                continue;
            }
        }
        if let Some(max) = check.max {
            if value > max {
                failures.push(format!("{} is {value} (needs at most {max})", check.label));
            }
        }
    }
    failures
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

    #[test]
    fn metrics_are_read_from_rendered_text() {
        let page = "<div>FPS: 46</div><span>Score = 91.5</span><p>Errors 0</p>";
        assert_eq!(find_metric(page, "FPS"), Some(46.0));
        assert_eq!(find_metric(page, "score"), Some(91.5));
        assert_eq!(find_metric(page, "Errors"), Some(0.0));
        assert_eq!(find_metric(page, "Latency"), None);
        // A label with no number after it keeps looking for a later match.
        assert_eq!(find_metric("FPS counter\nFPS 60", "FPS"), Some(60.0));
    }

    #[test]
    fn a_target_that_is_merely_implemented_still_fails() {
        // The exact case that shipped "60fps" at 46.
        let page = "smooth rendering enabled | FPS: 46";
        let checks = vec![MetricCheck {
            label: "FPS".into(),
            min: Some(55.0),
            max: None,
        }];
        let failures = failed_metrics(page, &checks);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("46") && failures[0].contains("55"));

        // And passes once it genuinely hits the target.
        assert!(failed_metrics("FPS: 60", &checks).is_empty());
    }

    #[test]
    fn missing_and_out_of_range_metrics_are_reported() {
        let checks = vec![
            MetricCheck {
                label: "Load".into(),
                min: None,
                max: Some(2.0),
            },
            MetricCheck {
                label: "Absent".into(),
                min: Some(1.0),
                max: None,
            },
        ];
        let failures = failed_metrics("Load: 3.4", &checks);
        assert_eq!(failures.len(), 2);
        assert!(failures[0].contains("at most 2"));
        assert!(failures[1].contains("not found"));
    }
}
