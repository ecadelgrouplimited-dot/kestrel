//! Keyless web search for the agent's research.
//!
//! Research shouldn't be limited to URLs the model already knows. `web_search`
//! queries DuckDuckGo's HTML endpoint over the system `curl` (no API key, no
//! bundled HTTP stack) and returns titles, URLs, and snippets the agent can then
//! `http_get`. The HTML parsing is deliberately defensive: if the markup shifts
//! and nothing parses, the caller gets a clear "nothing found" rather than junk.

use std::io::Write;
use std::process::{Command, Stdio};

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Search the web for `query`, returning up to `max` results.
pub fn web_search(query: &str, max: usize) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Err("empty query".to_string());
    }
    let html = fetch(query)?;
    Ok(parse_results(&html, max))
}

/// POST the query to DuckDuckGo's HTML endpoint and return the response body.
fn fetch(query: &str) -> Result<String, String> {
    let body = format!("q={}", percent_encode(query));
    let mut child = Command::new("curl")
        .args(["-sS", "--connect-timeout", "30"])
        .args([
            "-A",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Kestrel/1.0",
        ])
        .arg("https://html.duckduckgo.com/html/")
        .args(["--data-binary", "@-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "search request failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse DuckDuckGo HTML result blocks into structured results.
fn parse_results(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    // Each result title is an <a class="result__a" href="…">Title</a>.
    for chunk in html.split("result__a").skip(1) {
        let Some(href) = attr_after(chunk, "href=\"") else {
            continue;
        };
        let title = inner_text(chunk);
        if title.is_empty() {
            continue;
        }
        let url = clean_url(&href);
        // The snippet follows in a result__snippet anchor, if present.
        let snippet = chunk
            .split_once("result__snippet")
            .map(|(_, rest)| inner_text(rest))
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
        if results.len() >= max {
            break;
        }
    }
    results
}

/// The value of the first `attr="…"` appearing in `s`.
fn attr_after(s: &str, attr: &str) -> Option<String> {
    let start = s.find(attr)? + attr.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The text of the first `>…</a>` after the current position, tag-stripped.
fn inner_text(s: &str) -> String {
    let after_tag = match s.find('>') {
        Some(i) => &s[i + 1..],
        None => s,
    };
    let raw = match after_tag.find("</a>") {
        Some(i) => &after_tag[..i],
        None => after_tag,
    };
    html_decode(&strip_tags(raw)).trim().to_string()
}

/// DuckDuckGo wraps result URLs as `//duckduckgo.com/l/?uddg=<encoded>&…`.
/// Unwrap to the real target when present.
fn clean_url(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let rest = &href[idx + 5..];
        let enc = rest.split('&').next().unwrap_or(rest);
        return percent_decode(enc);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Remove HTML tags from a fragment.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decode the handful of HTML entities that appear in titles/snippets.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Percent-encode a query string (RFC 3986 unreserved kept, rest escaped).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded string (and `+` as space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        assert_eq!(percent_encode("rust async trait"), "rust+async+trait");
        assert_eq!(percent_encode("a/b?c"), "a%2Fb%3Fc");
        assert_eq!(percent_decode("https%3A%2F%2Fx.io%2Fa"), "https://x.io/a");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    #[test]
    fn strips_tags_and_entities() {
        assert_eq!(strip_tags("<b>hi</b> there"), "hi there");
        assert_eq!(html_decode("Tokio &amp; async"), "Tokio & async");
    }

    #[test]
    fn parses_ddg_result_block() {
        let html = r##"
            <div class="result">
              <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio&rut=x">Tokio Docs</a>
              <a class="result__snippet" href="#">An <b>async</b> runtime for Rust.</a>
            </div>
            <div class="result">
              <a class="result__a" href="https://example.com/two">Second</a>
            </div>
        "##;
        let results = parse_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Tokio Docs");
        assert_eq!(results[0].url, "https://docs.rs/tokio");
        assert_eq!(results[0].snippet, "An async runtime for Rust.");
        assert_eq!(results[1].url, "https://example.com/two");
    }
}
