//! Terminal output: colours, and a status line that updates in place.
//!
//! The agent emits events continuously — reasoning tokens, tool calls, results.
//! Printing every one as its own line would bury the useful output in noise, so
//! transient state (what it's thinking, how long it's been) lives on a single
//! line rewritten with `\r`, while anything worth keeping is printed *above* it.
//!
//! Colour is raw ANSI — no crate needed. `NO_COLOR` and a non-terminal stdout
//! both disable it, per the informal standard.

use std::io::Write;

/// ANSI styles, resolved once against whether colour is wanted.
#[derive(Clone, Copy)]
pub struct Style {
    pub enabled: bool,
    /// Whether the terminal can do 24-bit colour. When it can't, the accent
    /// falls back to the nearest xterm-256 index instead of vanishing.
    pub truecolor: bool,
}

impl Style {
    pub fn detect() -> Self {
        use std::io::IsTerminal;
        // https://no-color.org — any value disables colour. Piped output gets
        // none either, so `kestrel … | grep` stays clean.
        let disabled = std::env::var_os("NO_COLOR").is_some()
            || std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false)
            || !std::io::stdout().is_terminal();
        // …unless something downstream can handle ANSI anyway: a recorder, a CI
        // log viewer, or a test capturing the escape sequences. NO_COLOR wins.
        let forced = std::env::var_os("NO_COLOR").is_none()
            && (std::env::var_os("FORCE_COLOR").is_some()
                || std::env::var_os("CLICOLOR_FORCE").is_some());
        Self {
            enabled: forced || !disabled,
            truecolor: supports_truecolor(),
        }
    }
    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    /// The falcon's gold, exactly as sampled from the logo where the terminal
    /// can render it.
    pub fn accent(&self, text: &str) -> String {
        if self.truecolor {
            self.wrap(
                &kestrel_core::brand_ansi_fg(kestrel_core::brand::GOLD),
                text,
            )
        } else {
            self.wrap(&format!("38;5;{}", kestrel_core::brand::GOLD_256), text)
        }
    }
    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }
    /// Success, failure and warning, warmed to sit beside the gold. The plain
    /// SGR codes remain the fallback so a 16-colour terminal still reads.
    pub fn green(&self, text: &str) -> String {
        self.brand_or(kestrel_core::brand::GREEN, "32", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.brand_or(kestrel_core::brand::RED, "31", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.brand_or(kestrel_core::brand::AMBER, "33", text)
    }
    fn brand_or(&self, colour: kestrel_core::brand::Rgb, fallback: &str, text: &str) -> String {
        if self.truecolor {
            self.wrap(&kestrel_core::brand_ansi_fg(colour), text)
        } else {
            self.wrap(fallback, text)
        }
    }
    pub fn cyan(&self, text: &str) -> String {
        self.wrap("36", text)
    }
}

/// Owns the transient status line so permanent output never collides with it.
pub struct Term {
    pub style: Style,
    /// Whether a status line is currently drawn and needs clearing.
    status_shown: bool,
}

impl Term {
    pub fn new() -> Self {
        Self {
            style: Style::detect(),
            status_shown: false,
        }
    }

    /// Erase the status line if one is drawn, so a permanent line can be
    /// printed cleanly over it.
    fn clear_status(&mut self) {
        if self.status_shown {
            // Carriage return, then erase to end of line.
            print!("\r\x1b[2K");
            self.status_shown = false;
        }
    }

    /// Print a permanent line above the status line.
    pub fn line(&mut self, text: &str) {
        self.clear_status();
        println!("{text}");
        let _ = std::io::stdout().flush();
    }

    /// Draw (or redraw) the transient status line.
    pub fn status(&mut self, text: &str) {
        if !self.style.enabled {
            return; // Without ANSI, in-place rewriting just makes a mess.
        }
        self.clear_status();
        // Keep it to one terminal row so the rewrite always lands.
        let width = terminal_width().saturating_sub(1);
        let shown = truncate(text, width);
        print!("\r\x1b[2K{shown}");
        let _ = std::io::stdout().flush();
        self.status_shown = true;
    }

    /// Remove the status line for good (end of a run).
    pub fn finish_status(&mut self) {
        self.clear_status();
        let _ = std::io::stdout().flush();
    }
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the terminal advertises 24-bit colour.
///
/// Windows Terminal, VS Code and modern conhost all handle truecolour but none
/// of them set `COLORTERM`, so on Windows the default is yes and the env vars
/// only confirm it. Elsewhere `COLORTERM` is the usual signal.
pub fn supports_truecolor() -> bool {
    if let Ok(v) = std::env::var("COLORTERM") {
        if v.contains("truecolor") || v.contains("24bit") {
            return true;
        }
    }
    if std::env::var_os("WT_SESSION").is_some() {
        return true;
    }
    cfg!(windows)
}

/// Best-effort terminal width; 100 columns when it can't be determined.
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .filter(|c| *c > 20)
        .unwrap_or(100)
}

/// Cut `text` to `max` display characters, with an ellipsis when shortened.
pub fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(1);
    let cut: String = text.chars().take(keep).collect();
    format!("{cut}…")
}

/// A spinner frame for the given tick, so a long wait visibly moves.
pub fn spinner(tick: usize) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[tick % FRAMES.len()]
}

/// A compact duration: `4.2s`, `1m 12s`.
pub fn duration(seconds: f32) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        format!(
            "{}m {:02}s",
            (seconds / 60.0) as u32,
            (seconds % 60.0) as u32
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_within_the_budget() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("hello world", 8).chars().count(), 8);
        // Multi-byte characters are counted, not bytes.
        assert_eq!(truncate("🦅🦅🦅🦅", 3), "🦅🦅…");
    }

    #[test]
    fn styles_are_inert_when_colour_is_off() {
        let plain = Style {
            enabled: false,
            truecolor: true,
        };
        assert_eq!(plain.green("ok"), "ok");
        assert_eq!(plain.bold("hi"), "hi");
        assert_eq!(plain.accent("k"), "k");
        let colour = Style {
            enabled: true,
            truecolor: false,
        };
        assert!(colour.green("ok").contains("\x1b[32m"));
        assert!(colour.green("ok").ends_with("\x1b[0m"));
    }

    #[test]
    fn the_accent_is_the_logo_gold() {
        let full = Style {
            enabled: true,
            truecolor: true,
        };
        // Truecolour reproduces the sampled gold exactly.
        assert!(full.accent("k").starts_with("\x1b[38;2;220;141;31m"));
        // A 256-colour terminal gets the nearest index rather than no colour.
        let indexed = Style {
            enabled: true,
            truecolor: false,
        };
        assert!(indexed.accent("k").starts_with("\x1b[38;5;172m"));
        assert!(indexed.accent("k").ends_with("\x1b[0m"));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(4.25), "4.2s");
        assert_eq!(duration(72.0), "1m 12s");
    }

    #[test]
    fn spinner_cycles() {
        assert_eq!(spinner(0), spinner(10));
        assert_ne!(spinner(0), spinner(1));
    }
}
