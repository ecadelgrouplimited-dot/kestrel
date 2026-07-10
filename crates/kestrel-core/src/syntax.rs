//! Lightweight, dependency-free syntax highlighting.
//!
//! A full tree-sitter/syntect stack is too heavy for this build (disk and
//! dependency budget), so this is a compact, language-parameterised tokenizer:
//! it splits source into coloured spans — keywords, types, functions, strings,
//! comments, numbers — good enough to make code readable at a glance. It emits
//! byte-offset [`Span`]s and stays UI-agnostic; the desktop app maps span kinds
//! to theme colours. Anything not worth colouring is simply left as a gap the
//! UI fills with the default text colour.

/// The kind of token a span represents, for colour mapping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    String,
    Comment,
    Number,
}

/// A coloured span of source, by byte offset (`source[start..end]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// A source language Kestrel can highlight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Json,
    Css,
    Toml,
    PlainText,
}

/// Pick a language from a file extension (case-insensitive, no leading dot).
pub fn language_from_extension(ext: &str) -> Language {
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Language::Rust,
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Language::TypeScript,
        "py" | "pyi" => Language::Python,
        "json" | "jsonc" => Language::Json,
        "css" | "scss" | "sass" | "less" => Language::Css,
        "toml" => Language::Toml,
        _ => Language::PlainText,
    }
}

struct Spec {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    strings: &'static [char],
    keywords: &'static [&'static str],
    rust_char: bool,
    hash_hex: bool,
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "box",
];

const TS_KW: &[&str] = &[
    "abstract",
    "any",
    "as",
    "async",
    "await",
    "boolean",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "keyof",
    "let",
    "namespace",
    "new",
    "null",
    "number",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "set",
    "static",
    "string",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "void",
    "while",
    "yield",
];

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try",
    "while", "with", "yield",
];

const JSON_KW: &[&str] = &["true", "false", "null"];
const TOML_KW: &[&str] = &["true", "false"];

fn spec_for(language: Language) -> Spec {
    match language {
        Language::Rust => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: &['"'],
            keywords: RUST_KW,
            rust_char: true,
            hash_hex: false,
        },
        Language::TypeScript => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: &['"', '\'', '`'],
            keywords: TS_KW,
            rust_char: false,
            hash_hex: false,
        },
        Language::Python => Spec {
            line_comments: &["#"],
            block_comment: None,
            strings: &['"', '\''],
            keywords: PY_KW,
            rust_char: false,
            hash_hex: false,
        },
        Language::Json => Spec {
            line_comments: &[],
            block_comment: None,
            strings: &['"'],
            keywords: JSON_KW,
            rust_char: false,
            hash_hex: false,
        },
        Language::Css => Spec {
            line_comments: &[],
            block_comment: Some(("/*", "*/")),
            strings: &['"', '\''],
            keywords: &[],
            rust_char: false,
            hash_hex: true,
        },
        Language::Toml => Spec {
            line_comments: &["#"],
            block_comment: None,
            strings: &['"', '\''],
            keywords: TOML_KW,
            rust_char: false,
            hash_hex: false,
        },
        Language::PlainText => Spec {
            line_comments: &[],
            block_comment: None,
            strings: &[],
            keywords: &[],
            rust_char: false,
            hash_hex: false,
        },
    }
}

/// Tokenize `source` into coloured spans for the given language. Spans are
/// sorted by start offset and never overlap; gaps are ordinary text.
pub fn highlight(source: &str, language: Language) -> Vec<Span> {
    let spec = spec_for(language);
    let cs: Vec<(usize, char)> = source.char_indices().collect();
    let n = cs.len();
    let byte_at = |i: usize| if i < n { cs[i].0 } else { source.len() };
    let mut spans = Vec::new();
    let mut i = 0;

    while i < n {
        let c = cs[i].1;

        // Line comment.
        if spec.line_comments.iter().any(|t| starts_with(&cs, i, t)) {
            let start = i;
            while i < n && cs[i].1 != '\n' {
                i += 1;
            }
            spans.push(span(byte_at(start), byte_at(i), TokenKind::Comment));
            continue;
        }

        // Block comment.
        if let Some((open, close)) = spec.block_comment {
            if starts_with(&cs, i, open) {
                let start = i;
                i += open.chars().count();
                while i < n && !starts_with(&cs, i, close) {
                    i += 1;
                }
                if i < n {
                    i += close.chars().count();
                }
                spans.push(span(byte_at(start), byte_at(i), TokenKind::Comment));
                continue;
            }
        }

        // Rust char literal vs lifetime.
        if spec.rust_char && c == '\'' {
            if let Some(end) = rust_char_end(&cs, i, n) {
                spans.push(span(byte_at(i), byte_at(end), TokenKind::String));
                i = end;
                continue;
            }
            // Otherwise a lifetime; fall through and treat `'` as punctuation.
        }

        // String literal.
        if spec.strings.contains(&c) {
            let quote = c;
            let start = i;
            i += 1;
            while i < n {
                let ch = cs[i].1;
                if ch == '\\' {
                    i += 2;
                    continue;
                }
                if ch == quote {
                    i += 1;
                    break;
                }
                if quote != '`' && ch == '\n' {
                    break;
                }
                i += 1;
            }
            spans.push(span(byte_at(start), byte_at(i.min(n)), TokenKind::String));
            continue;
        }

        // CSS-style hex colour (#fff / #1e1e1e).
        if spec.hash_hex && c == '#' {
            let start = i;
            i += 1;
            while i < n && cs[i].1.is_ascii_hexdigit() {
                i += 1;
            }
            if i > start + 1 {
                spans.push(span(byte_at(start), byte_at(i), TokenKind::Number));
            }
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < n && (cs[i].1.is_ascii_alphanumeric() || cs[i].1 == '.' || cs[i].1 == '_') {
                i += 1;
            }
            spans.push(span(byte_at(start), byte_at(i), TokenKind::Number));
            continue;
        }

        // Identifier / keyword / type / function.
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < n && (cs[i].1.is_alphanumeric() || cs[i].1 == '_' || cs[i].1 == '$') {
                i += 1;
            }
            let word: String = cs[start..i].iter().map(|(_, ch)| *ch).collect();
            let kind = if spec.keywords.contains(&word.as_str()) {
                Some(TokenKind::Keyword)
            } else if word.chars().next().is_some_and(|c| c.is_uppercase()) {
                Some(TokenKind::Type)
            } else if next_nonspace_is(&cs, i, n, '(') {
                Some(TokenKind::Function)
            } else {
                None
            };
            if let Some(kind) = kind {
                spans.push(span(byte_at(start), byte_at(i), kind));
            }
            continue;
        }

        // Anything else (whitespace, punctuation) stays uncoloured.
        i += 1;
    }

    spans
}

fn span(start: usize, end: usize, kind: TokenKind) -> Span {
    Span { start, end, kind }
}

fn starts_with(cs: &[(usize, char)], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, nc)| cs.get(i + k).is_some_and(|(_, c)| *c == nc))
}

fn next_nonspace_is(cs: &[(usize, char)], mut i: usize, n: usize, target: char) -> bool {
    while i < n && (cs[i].1 == ' ' || cs[i].1 == '\t') {
        i += 1;
    }
    i < n && cs[i].1 == target
}

/// If `cs[i]` opens a Rust char literal, return the index just past its closing
/// quote; otherwise `None` (it is a lifetime).
fn rust_char_end(cs: &[(usize, char)], i: usize, n: usize) -> Option<usize> {
    let mut j = i + 1;
    if j >= n {
        return None;
    }
    if cs[j].1 == '\\' {
        j += 1;
        while j < n && cs[j].1 != '\'' {
            j += 1;
        }
        if j < n {
            return Some(j + 1);
        }
        return None;
    }
    if j + 1 < n && cs[j + 1].1 == '\'' {
        return Some(j + 2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_of(source: &str, language: Language) -> Vec<(&str, TokenKind)> {
        highlight(source, language)
            .into_iter()
            .map(|s| (&source[s.start..s.end], s.kind))
            .collect()
    }

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(language_from_extension("rs"), Language::Rust);
        assert_eq!(language_from_extension("TSX"), Language::TypeScript);
        assert_eq!(language_from_extension("py"), Language::Python);
        assert_eq!(language_from_extension("md"), Language::PlainText);
    }

    #[test]
    fn highlights_rust_keywords_strings_and_comments() {
        let src = "let x = \"hi\"; // note";
        let spans = kinds_of(src, Language::Rust);
        assert!(spans.contains(&("let", TokenKind::Keyword)));
        assert!(spans.contains(&("\"hi\"", TokenKind::String)));
        assert!(spans
            .iter()
            .any(|(t, k)| *k == TokenKind::Comment && t.contains("note")));
    }

    #[test]
    fn highlights_types_functions_and_numbers() {
        let src = "const n = compute(42); let c: Config;";
        let spans = kinds_of(src, Language::TypeScript);
        assert!(spans.contains(&("compute", TokenKind::Function)));
        assert!(spans.contains(&("42", TokenKind::Number)));
        assert!(spans.contains(&("Config", TokenKind::Type)));
    }

    #[test]
    fn block_comments_and_templates_span_multiple_lines() {
        let src = "/* a\nb */ `x\ny`";
        let spans = kinds_of(src, Language::TypeScript);
        assert!(spans
            .iter()
            .any(|(t, k)| *k == TokenKind::Comment && t.contains('\n')));
        assert!(spans
            .iter()
            .any(|(t, k)| *k == TokenKind::String && t.contains('\n')));
    }

    #[test]
    fn spans_are_sorted_and_non_overlapping() {
        let src = "fn main() { let s = \"str\"; /* c */ }";
        let spans = highlight(src, Language::Rust);
        let mut last = 0;
        for s in spans {
            assert!(s.start >= last, "overlap at {}", s.start);
            assert!(s.end <= src.len());
            last = s.end;
        }
    }

    #[test]
    fn css_hex_colors_are_numbers() {
        let spans = kinds_of("color: #1e1e1e;", Language::Css);
        assert!(spans.contains(&("#1e1e1e", TokenKind::Number)));
    }
}
