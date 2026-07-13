//! Structural symbol extraction: the seed of the Ghost Context Engine.
//!
//! This module turns source text into a list of [`Symbol`]s — the top-level
//! declarations (functions, types, classes, and so on) that a model needs in
//! order to reason about a file without reading every line of it.
//!
//! Extraction runs behind the [`SymbolExtractor`] trait. The default backend is
//! now **tree-sitter** (see [`crate::treesitter`]) — a real parser, so symbols
//! are precise. The heuristic scanners in this module remain as the tree-sitter
//! backend's fallback (if a grammar fails to load) and still supply the
//! import/reference edges the dependency graph is built from. That the MVP
//! heuristic could be swapped for the real parser with no caller change — the
//! trait was the seam all along — is the point: the component is the literal
//! substrate of its horizon successor (the Living System Model), not scaffolding.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// The category of a declaration. This is an intentionally small, mostly
/// cross-language set; language-specific concepts map onto the nearest kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    Interface,
    Class,
    Impl,
    Module,
    Constant,
    TypeAlias,
    Macro,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Union => "union",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Class => "class",
            SymbolKind::Impl => "impl",
            SymbolKind::Module => "module",
            SymbolKind::Constant => "constant",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Macro => "macro",
        }
    }
}

/// A single declaration discovered in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based line where the declaration begins.
    pub line: usize,
    /// The enclosing declaration name (e.g. the `impl`/`class` a method lives
    /// in), when one could be determined.
    pub container: Option<String>,
    /// Whether the declaration is publicly visible (`pub` / `export`).
    pub exported: bool,
    /// A trimmed, length-capped copy of the declaring source line.
    pub signature: String,
}

/// A dependency brought into a file: `use`/`import`/`from … import` and
/// friends. This is the raw edge material for the project dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The module path or specifier as written, e.g. `std::collections`,
    /// `./service`, or `os.path`.
    pub module: String,
    /// The named items pulled in, when the syntax lists them (empty for
    /// whole-module or side-effect imports).
    pub names: Vec<String>,
    /// 1-based line where the import statement begins.
    pub line: usize,
}

/// All symbols extracted from one file, tagged with the language used.
#[derive(Debug, Clone)]
pub struct FileSymbols {
    pub language: String,
    pub symbols: Vec<Symbol>,
}

/// A language-specific structural extractor. Implementations must be cheap,
/// panic-free on arbitrary input, and stable across refactors of unrelated
/// code — a future tree-sitter backend is expected to implement this trait.
pub trait SymbolExtractor {
    fn language(&self) -> &'static str;
    fn extract(&self, source: &str) -> Vec<Symbol>;

    /// Extract the file's imports/dependencies. Default: none.
    fn imports(&self, source: &str) -> Vec<Import> {
        let _ = source;
        Vec::new()
    }

    /// Collect the distinct identifiers referenced in the file, with comments
    /// and string literals excluded. Used to infer cross-file usage edges.
    /// Default: none.
    fn referenced_identifiers(&self, source: &str) -> Vec<String> {
        let _ = source;
        Vec::new()
    }
}

/// Return an extractor for a `language_for_path`-style display name. Uses the
/// tree-sitter backend, which falls back to the heuristic scanner internally.
pub fn extractor_for_language(language: &str) -> Option<Box<dyn SymbolExtractor>> {
    use crate::treesitter::{TreeSitterExtractor, TsLang};
    let (lang, display) = match language {
        "Rust" => (TsLang::Rust, "Rust"),
        "TypeScript" | "JavaScript" | "TypeScript/JavaScript" => {
            (TsLang::TypeScript, "TypeScript/JavaScript")
        }
        "Python" => (TsLang::Python, "Python"),
        _ => return None,
    };
    Some(Box::new(TreeSitterExtractor::new(lang, display)))
}

/// Return an extractor based on a file's extension, or `None` if unsupported.
pub fn extractor_for_path(path: &Path) -> Option<Box<dyn SymbolExtractor>> {
    use crate::treesitter::{TreeSitterExtractor, TsLang};
    let ext = path.extension().and_then(|e| e.to_str())?;
    let (lang, display) = match ext {
        "rs" => (TsLang::Rust, "Rust"),
        "tsx" => (TsLang::Tsx, "TypeScript/JavaScript"),
        "ts" | "mts" | "cts" => (TsLang::TypeScript, "TypeScript/JavaScript"),
        "js" | "jsx" | "mjs" | "cjs" => (TsLang::JavaScript, "TypeScript/JavaScript"),
        "py" | "pyw" => (TsLang::Python, "Python"),
        _ => return None,
    };
    Some(Box::new(TreeSitterExtractor::new(lang, display)))
}

/// The heuristic (dependency-free) extractor for a display language, used as the
/// tree-sitter backend's fallback and for imports/reference extraction.
pub(crate) fn heuristic_for_language(language: &str) -> Option<Box<dyn SymbolExtractor>> {
    match language {
        "Rust" => Some(Box::new(RustExtractor)),
        "TypeScript" | "JavaScript" | "TypeScript/JavaScript" => {
            Some(Box::new(TypeScriptExtractor))
        }
        "Python" => Some(Box::new(PythonExtractor)),
        _ => None,
    }
}

/// Read a file and extract its symbols, returning `None` for unsupported
/// extensions. IO and decoding failures surface as `Err`.
pub fn symbols_for_file(path: &Path) -> std::io::Result<Option<FileSymbols>> {
    let Some(extractor) = extractor_for_path(path) else {
        return Ok(None);
    };
    let source = std::fs::read_to_string(path)?;
    Ok(Some(FileSymbols {
        language: extractor.language().to_string(),
        symbols: extractor.extract(&source),
    }))
}

// ---------------------------------------------------------------------------
// Shared lexical helpers
// ---------------------------------------------------------------------------

const MAX_SIGNATURE_LEN: usize = 160;

/// Strip a leading UTF-8 byte-order mark. Windows and .NET toolchains emit BOMs
/// routinely, and `char::is_whitespace` does not treat U+FEFF as whitespace, so
/// without this the first declaration in a BOM-prefixed file would be missed.
fn strip_bom(source: &str) -> &str {
    source.strip_prefix('\u{feff}').unwrap_or(source)
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Read a leading identifier from `s` (after skipping leading whitespace),
/// returning the identifier and the remaining slice.
fn read_ident(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let mut end = first.len_utf8();
    for (i, c) in chars {
        if is_ident_char(c) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    Some((&s[..end], &s[end..]))
}

/// If `s` (trimmed) begins with the whole word `kw`, return the remainder.
fn after_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let s = s.trim_start();
    let rest = s.strip_prefix(kw)?;
    match rest.chars().next() {
        Some(c) if is_ident_char(c) => None,
        _ => Some(rest),
    }
}

fn signature_of(raw_line: &str) -> String {
    let trimmed = raw_line.trim();
    if trimmed.chars().count() > MAX_SIGNATURE_LEN {
        let capped: String = trimmed.chars().take(MAX_SIGNATURE_LEN).collect();
        format!("{capped}…")
    } else {
        trimmed.to_string()
    }
}

fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4,
            _ => break,
        }
    }
    width
}

/// Syntax knobs describing how to strip comments and string literals when
/// producing a "code only" view of a line for brace-depth tracking.
#[derive(Clone, Copy)]
struct LineSyntax {
    /// `true` for Rust, where `'a` is a lifetime, not a char literal.
    rust_char_literals: bool,
    /// `true` for JS/TS template literals delimited by backticks.
    backtick_strings: bool,
}

/// Cross-line scanner state, so that block comments, multi-line strings, and
/// Rust raw strings are not misread as code on the lines they span.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Normal,
    BlockComment,
    /// Inside a normal string/template literal opened with this delimiter.
    Str(char),
    /// Inside a Rust raw string with this many `#` hashes.
    RawStr(usize),
}

/// If `chars[i..]` begins a Rust raw string (`r"…"`, `r#"…"#`, `br"…"`, …),
/// return `(chars_consumed_through_opening_quote, hash_count)`.
fn raw_string_start(chars: &[char], i: usize) -> Option<(usize, usize)> {
    // Must begin a token, so we do not treat the `r` in `for` as a prefix.
    if i > 0 && is_ident_char(chars[i - 1]) {
        return None;
    }
    let n = chars.len();
    let mut j = i;
    if j < n && chars[j] == 'b' {
        j += 1;
    }
    if j < n && chars[j] == 'r' {
        j += 1;
    } else {
        return None;
    }
    let hash_start = j;
    while j < n && chars[j] == '#' {
        j += 1;
    }
    let hashes = j - hash_start;
    if j < n && chars[j] == '"' {
        Some((j - i + 1, hashes))
    } else {
        None
    }
}

/// Whether `chars[start..]` begins with exactly `hashes` `#` characters (the
/// closing fence of a raw string).
fn closes_raw_string(chars: &[char], start: usize, hashes: usize) -> bool {
    start + hashes <= chars.len() && chars[start..start + hashes].iter().all(|&c| c == '#')
}

/// Produce a copy of `line` with comments removed and the contents of string,
/// char, and raw-string literals blanked, so that brace counting and symbol
/// detection are not confused by braces or keywords appearing inside them.
/// `state` carries block-comment and multi-line-string context across lines.
fn code_view(line: &str, state: &mut ScanState, syntax: LineSyntax) -> String {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;

    while i < n {
        match *state {
            ScanState::BlockComment => {
                if chars[i] == '*' && i + 1 < n && chars[i + 1] == '/' {
                    *state = ScanState::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ScanState::Str(quote) => {
                if chars[i] == '\\' {
                    i += 2;
                } else if chars[i] == quote {
                    *state = ScanState::Normal;
                    i += 1;
                } else {
                    i += 1;
                }
            }
            ScanState::RawStr(hashes) => {
                if chars[i] == '"' && closes_raw_string(&chars, i + 1, hashes) {
                    *state = ScanState::Normal;
                    i += 1 + hashes;
                } else {
                    i += 1;
                }
            }
            ScanState::Normal => {
                let c = chars[i];

                // Line comment: ignore the rest of the line.
                if c == '/' && i + 1 < n && chars[i + 1] == '/' {
                    break;
                }
                if c == '/' && i + 1 < n && chars[i + 1] == '*' {
                    *state = ScanState::BlockComment;
                    i += 2;
                    continue;
                }

                // Rust raw strings, which have no escapes and may span lines.
                if syntax.rust_char_literals {
                    if let Some((consumed, hashes)) = raw_string_start(&chars, i) {
                        out.push(' ');
                        i += consumed;
                        *state = ScanState::RawStr(hashes);
                        continue;
                    }
                }

                // Rust char literal vs lifetime disambiguation.
                if c == '\'' && syntax.rust_char_literals {
                    let is_char_literal =
                        (i + 1 < n && chars[i + 1] == '\\') || (i + 2 < n && chars[i + 2] == '\'');
                    if is_char_literal {
                        i = skip_string(&chars, i + 1, '\'');
                        out.push(' ');
                    } else {
                        // A lifetime tick; treat as ordinary punctuation.
                        i += 1;
                    }
                    continue;
                }

                let is_quote = c == '"'
                    || (!syntax.rust_char_literals && c == '\'')
                    || (syntax.backtick_strings && c == '`');
                if is_quote {
                    // Consume the string body; if it does not close on this
                    // line, carry the open string into the next line.
                    let mut j = i + 1;
                    let mut closed = false;
                    while j < n {
                        if chars[j] == '\\' {
                            j += 2;
                            continue;
                        }
                        if chars[j] == c {
                            closed = true;
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    out.push(' ');
                    if !closed {
                        *state = ScanState::Str(c);
                    }
                    i = j;
                    continue;
                }

                out.push(c);
                i += 1;
            }
        }
    }

    out
}

/// Advance past a string body that started just after an opening `quote`,
/// honoring backslash escapes. Returns the index just past the closing quote
/// (or the end of line for an unterminated literal).
fn skip_string(chars: &[char], mut i: usize, quote: char) -> usize {
    let n = chars.len();
    while i < n {
        let c = chars[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if c == quote {
            return i + 1;
        }
        i += 1;
    }
    n
}

fn net_braces(code: &str) -> i32 {
    let mut delta = 0i32;
    for c in code.chars() {
        match c {
            '{' => delta += 1,
            '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// A container currently open on the brace-depth stack (an `impl`/`trait`/
/// `class`/`module` whose body we are inside).
#[derive(Clone)]
struct Container {
    name: String,
    kind: SymbolKind,
    base_depth: i32,
}

/// Scan `code` (already stripped of comments/strings) and insert every
/// identifier token into `set`. Numeric literals are skipped by construction
/// because identifiers may not start with a digit.
fn push_identifiers(code: &str, set: &mut BTreeSet<String>) {
    let chars: Vec<char> = code.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if is_ident_start(chars[i]) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            set.insert(chars[start..i].iter().collect());
        } else {
            i += 1;
        }
    }
}

/// Collect identifiers from a brace-language source (Rust/TS/JS), honoring the
/// language's comment and string rules.
fn collect_brace_identifiers(source: &str, syntax: LineSyntax) -> Vec<String> {
    let source = strip_bom(source);
    let mut state = ScanState::Normal;
    let mut set = BTreeSet::new();
    for raw in source.lines() {
        let code = code_view(raw, &mut state, syntax);
        push_identifiers(&code, &mut set);
    }
    set.into_iter().collect()
}

/// Blank the contents of Python string literals on a single line and drop any
/// trailing `#` comment, so identifier scanning is not fooled by them. Triple-
/// quoted spans are handled by the caller.
fn python_code_view(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '#' {
            break;
        }
        if c == '"' || c == '\'' {
            i = skip_string(&chars, i + 1, c);
            out.push(' ');
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Return the content of the first single/double/back-quoted string in `s`.
/// Used to read module specifiers out of import statements.
fn first_quoted(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '"' || c == '\'' || c == '`' {
            let end = skip_string(&chars, i + 1, c);
            let content: String = chars[i + 1..end.saturating_sub(1).max(i + 1)]
                .iter()
                .collect();
            return Some(content);
        }
        i += 1;
    }
    None
}

/// Parse a comma-separated import name list such as `A, B as C, default as D`,
/// returning the locally bound leaf names (`A`, `C`, `D`).
fn parse_import_names(group: &str) -> Vec<String> {
    let mut names = Vec::new();
    for item in group.split(',') {
        let item = item.trim();
        if item.is_empty() || item == "*" {
            continue;
        }
        // `x as y` binds `y`; otherwise the leaf of a `::`/`.` path.
        let bound = if let Some(pos) = find_keyword(item, "as") {
            item[pos + 2..].trim()
        } else {
            item
        };
        let leaf = bound
            .rsplit(['.', ':'])
            .next()
            .unwrap_or(bound)
            .trim()
            .trim_matches('*');
        if let Some((ident, _)) = read_ident(leaf) {
            names.push(ident.to_string());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

pub struct RustExtractor;

impl SymbolExtractor for RustExtractor {
    fn language(&self) -> &'static str {
        "Rust"
    }

    fn extract(&self, source: &str) -> Vec<Symbol> {
        let syntax = LineSyntax {
            rust_char_literals: true,
            backtick_strings: false,
        };
        let source = strip_bom(source);
        let mut symbols = Vec::new();
        let mut stack: Vec<Container> = Vec::new();
        let mut state = ScanState::Normal;
        let mut depth = 0i32;

        for (idx, raw) in source.lines().enumerate() {
            let line_no = idx + 1;
            let code = code_view(raw, &mut state, syntax);
            let depth_before = depth;

            // Leave containers whose body has closed.
            while let Some(top) = stack.last() {
                if depth_before <= top.base_depth {
                    stack.pop();
                } else {
                    break;
                }
            }

            let container = stack.last().map(|c| c.name.clone());
            let (exported, rest) = strip_rust_visibility(&code);

            if let Some((name, kind)) = classify_rust(rest, stack.last().map(|c| c.kind)) {
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind,
                    line: line_no,
                    container: container.clone(),
                    exported,
                    signature: signature_of(raw),
                });

                // Types with bodies become containers for the symbols inside.
                if matches!(
                    kind,
                    SymbolKind::Impl | SymbolKind::Trait | SymbolKind::Module
                ) {
                    stack.push(Container {
                        name: name.to_string(),
                        kind,
                        base_depth: depth_before,
                    });
                }
            }

            depth += net_braces(&code);
        }

        symbols
    }

    fn imports(&self, source: &str) -> Vec<Import> {
        rust_imports(source)
    }

    fn referenced_identifiers(&self, source: &str) -> Vec<String> {
        collect_brace_identifiers(
            source,
            LineSyntax {
                rust_char_literals: true,
                backtick_strings: false,
            },
        )
    }
}

/// Parse Rust `use` and `extern crate` statements, joining multi-line `use`
/// groups until the terminating `;`.
fn rust_imports(source: &str) -> Vec<Import> {
    let syntax = LineSyntax {
        rust_char_literals: true,
        backtick_strings: false,
    };
    let source = strip_bom(source);
    let mut state = ScanState::Normal;
    let mut out = Vec::new();
    let mut buffer = String::new();
    let mut start_line = 0usize;
    let mut accumulating = false;

    for (idx, raw) in source.lines().enumerate() {
        let code = code_view(raw, &mut state, syntax);

        if !accumulating {
            let (_, rest) = strip_rust_visibility(&code);
            let rest = rest.trim_start();
            if let Some(after) = after_keyword(rest, "use") {
                accumulating = true;
                start_line = idx + 1;
                buffer.clear();
                buffer.push_str(after);
            } else if let Some(after) = after_keyword(rest, "extern") {
                if let Some(after) = after_keyword(after.trim_start(), "crate") {
                    if let Some((name, _)) = read_ident(after) {
                        out.push(Import {
                            module: name.to_string(),
                            names: Vec::new(),
                            line: idx + 1,
                        });
                    }
                }
                continue;
            } else {
                continue;
            }
        } else {
            buffer.push(' ');
            buffer.push_str(&code);
        }

        if let Some(semi) = buffer.find(';') {
            let statement = buffer[..semi].trim().to_string();
            parse_rust_use(&statement, start_line, &mut out);
            accumulating = false;
            buffer.clear();
        }
    }

    out
}

fn parse_rust_use(statement: &str, line: usize, out: &mut Vec<Import>) {
    let statement = statement.trim();
    if let Some(brace) = statement.find('{') {
        let prefix = statement[..brace].trim().trim_end_matches("::").trim();
        let inner = statement[brace + 1..]
            .rsplit_once('}')
            .map(|(g, _)| g)
            .unwrap_or(&statement[brace + 1..]);
        out.push(Import {
            module: prefix.to_string(),
            names: parse_import_names(inner),
            line,
        });
        return;
    }

    // No brace group: `a::b::c`, `a::b::c as d`, or `a::b::*`.
    let (path, alias) = match find_keyword(statement, "as") {
        Some(pos) => (statement[..pos].trim(), Some(statement[pos + 2..].trim())),
        None => (statement, None),
    };
    let mut segments: Vec<&str> = path.split("::").map(str::trim).collect();
    let leaf = segments.pop().unwrap_or("");
    let module = segments.join("::");
    let names = if leaf == "*" || leaf.is_empty() {
        Vec::new()
    } else if let Some(alias) = alias {
        read_ident(alias)
            .map(|(n, _)| vec![n.to_string()])
            .unwrap_or_default()
    } else {
        read_ident(leaf)
            .map(|(n, _)| vec![n.to_string()])
            .unwrap_or_default()
    };
    out.push(Import {
        module: if module.is_empty() {
            leaf.to_string()
        } else {
            module
        },
        names,
        line,
    });
}

/// Strip a leading Rust visibility modifier, returning whether the item is
/// public and the remaining code slice.
fn strip_rust_visibility(code: &str) -> (bool, &str) {
    let trimmed = code.trim_start();
    if let Some(rest) = trimmed.strip_prefix("pub") {
        let boundary = matches!(
            rest.chars().next(),
            None | Some(' ') | Some('\t') | Some('(')
        );
        if boundary {
            let rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix('(') {
                if let Some(close) = after.find(')') {
                    return (true, after[close + 1..].trim_start());
                }
            }
            return (true, rest);
        }
    }
    (false, trimmed)
}

/// Identify a Rust item at the start of `code`. `parent` is the kind of the
/// enclosing container, used to decide function-vs-method.
fn classify_rust(code: &str, parent: Option<SymbolKind>) -> Option<(&str, SymbolKind)> {
    // Skip leading item modifiers to reach the item keyword.
    let mut rest = code.trim_start();
    loop {
        let mut advanced = false;
        for kw in ["async", "unsafe", "default", "move"] {
            if let Some(after) = after_keyword(rest, kw) {
                rest = after.trim_start();
                advanced = true;
            }
        }
        if let Some(after) = after_keyword(rest, "extern") {
            // Optionally followed by an ABI string, already blanked by code_view.
            rest = after.trim_start();
            advanced = true;
        }
        if !advanced {
            break;
        }
    }

    // `const fn` — a function, not a constant.
    if let Some(after) = after_keyword(rest, "const") {
        let after = after.trim_start();
        if let Some(fn_rest) = after_keyword(after, "fn") {
            let (name, _) = read_ident(fn_rest)?;
            let kind = method_or_function(parent);
            return Some((name, kind));
        }
        let (name, _) = read_ident(after)?;
        return Some((name, SymbolKind::Constant));
    }

    if let Some(after) = after_keyword(rest, "fn") {
        let (name, _) = read_ident(after)?;
        return Some((name, method_or_function(parent)));
    }
    if let Some(after) = after_keyword(rest, "struct") {
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Struct));
    }
    if let Some(after) = after_keyword(rest, "enum") {
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Enum));
    }
    if let Some(after) = after_keyword(rest, "union") {
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Union));
    }
    if let Some(after) = after_keyword(rest, "trait") {
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Trait));
    }
    if let Some(after) = after_keyword(rest, "mod") {
        // `mod foo;` (declaration only) still names a module.
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Module));
    }
    if let Some(after) = after_keyword(rest, "type") {
        return read_ident(after).map(|(n, _)| (n, SymbolKind::TypeAlias));
    }
    if let Some(after) = after_keyword(rest, "static") {
        let after = after_keyword(after.trim_start(), "mut")
            .map(|s| s.trim_start())
            .unwrap_or(after);
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Constant));
    }
    if let Some(after) = after_keyword(rest, "impl") {
        return Some((rust_impl_target(after), SymbolKind::Impl));
    }
    if rest.trim_start().starts_with("macro_rules!") {
        let after = &rest.trim_start()["macro_rules!".len()..];
        return read_ident(after).map(|(n, _)| (n, SymbolKind::Macro));
    }

    None
}

fn method_or_function(parent: Option<SymbolKind>) -> SymbolKind {
    match parent {
        Some(SymbolKind::Impl) | Some(SymbolKind::Trait) => SymbolKind::Method,
        _ => SymbolKind::Function,
    }
}

/// Derive a readable name for an `impl` block: the implemented type, or
/// `Trait for Type` when a trait is named.
fn rust_impl_target(after_impl: &str) -> &str {
    // Drop generic parameters directly after `impl`, e.g. `impl<'a, T>`.
    let mut s = after_impl.trim_start();
    if s.starts_with('<') {
        if let Some(close) = match_angle(s) {
            s = s[close + 1..].trim_start();
        }
    }
    // Prefer the type after `for` (the type the impl is *for*).
    if let Some(pos) = find_keyword(s, "for") {
        let target = s[pos + 3..].trim_start();
        return first_type_token(target);
    }
    first_type_token(s)
}

/// Find the byte index of the matching `>` for a leading `<`, accounting for
/// nesting. Returns `None` if unbalanced on this slice.
fn match_angle(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_keyword(s: &str, kw: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(pos) = s[i..].find(kw) {
        let abs = i + pos;
        let before_ok = abs == 0 || !is_ident_char(bytes[abs - 1] as char);
        let after_idx = abs + kw.len();
        let after_ok = after_idx >= bytes.len() || !is_ident_char(bytes[after_idx] as char);
        if before_ok && after_ok {
            return Some(abs);
        }
        i = abs + kw.len();
    }
    None
}

/// Read a type path token like `Foo`, `foo::Bar`, or `Vec` up to the first
/// delimiter, stripping any trailing generic arguments.
fn first_type_token(s: &str) -> &str {
    let s = s.trim_start();
    let end = s.find(['<', '{', ' ', '(', '\t']).unwrap_or(s.len());
    s[..end].trim_end()
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript
// ---------------------------------------------------------------------------

pub struct TypeScriptExtractor;

impl SymbolExtractor for TypeScriptExtractor {
    fn language(&self) -> &'static str {
        "TypeScript/JavaScript"
    }

    fn extract(&self, source: &str) -> Vec<Symbol> {
        let syntax = LineSyntax {
            rust_char_literals: false,
            backtick_strings: true,
        };
        let source = strip_bom(source);
        let mut symbols = Vec::new();
        let mut stack: Vec<Container> = Vec::new();
        let mut state = ScanState::Normal;
        let mut depth = 0i32;

        for (idx, raw) in source.lines().enumerate() {
            let line_no = idx + 1;
            let code = code_view(raw, &mut state, syntax);
            let depth_before = depth;

            while let Some(top) = stack.last() {
                if depth_before <= top.base_depth {
                    stack.pop();
                } else {
                    break;
                }
            }

            let inside_type = matches!(
                stack.last().map(|c| c.kind),
                Some(SymbolKind::Class) | Some(SymbolKind::Interface)
            );
            let container = stack.last().map(|c| c.name.clone());

            if let Some((name, kind, exported, opens_container)) = classify_ts(&code, inside_type) {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind,
                    line: line_no,
                    container: container.clone(),
                    exported,
                    signature: signature_of(raw),
                });
                if opens_container {
                    stack.push(Container {
                        name,
                        kind,
                        base_depth: depth_before,
                    });
                }
            }

            depth += net_braces(&code);
        }

        symbols
    }

    fn imports(&self, source: &str) -> Vec<Import> {
        ts_imports(source)
    }

    fn referenced_identifiers(&self, source: &str) -> Vec<String> {
        collect_brace_identifiers(
            source,
            LineSyntax {
                rust_char_literals: false,
                backtick_strings: true,
            },
        )
    }
}

/// Parse ES module `import`/`export … from` statements and `require(...)`
/// calls. Import statements are read from raw text (not the comment-stripped
/// view) so that the quoted module specifier survives; they are joined across
/// lines until a specifier is found.
fn ts_imports(source: &str) -> Vec<Import> {
    let source = strip_bom(source);
    let mut out = Vec::new();
    let mut buffer = String::new();
    let mut start_line = 0usize;
    let mut accumulating = false;

    for (idx, raw) in source.lines().enumerate() {
        let trimmed = raw.trim_start();

        if !accumulating {
            let is_import = trimmed.starts_with("import ")
                || trimmed.starts_with("import{")
                || trimmed.starts_with("import(")
                || trimmed.starts_with("import'")
                || trimmed.starts_with("import\"")
                || trimmed.starts_with("export ") && trimmed.contains("from");
            if is_import {
                accumulating = true;
                start_line = idx + 1;
                buffer.clear();
                buffer.push_str(trimmed);
            } else {
                // Detect a `require('...')` anywhere on the line.
                if let Some(pos) = raw.find("require(") {
                    if let Some(module) = first_quoted(&raw[pos + "require(".len()..]) {
                        out.push(Import {
                            module,
                            names: Vec::new(),
                            line: idx + 1,
                        });
                    }
                }
                continue;
            }
        } else {
            buffer.push(' ');
            buffer.push_str(trimmed);
        }

        // A statement is complete once its module specifier is available:
        // after `from` for named/default imports, or directly for a
        // side-effect `import '…'`.
        let has_specifier = first_quoted(&buffer).is_some();
        let complete = buffer.contains(';')
            || (buffer.contains("from") && has_specifier)
            || (buffer.starts_with("import")
                && has_specifier
                && !buffer.contains('{')
                && !buffer.contains("from"));
        if complete {
            parse_ts_import(&buffer, start_line, &mut out);
            accumulating = false;
            buffer.clear();
        }
    }

    if accumulating && !buffer.is_empty() {
        parse_ts_import(&buffer, start_line, &mut out);
    }

    out
}

fn after_from(s: &str) -> &str {
    match find_keyword(s, "from") {
        Some(pos) => &s[pos + 4..],
        None => s,
    }
}

fn parse_ts_import(statement: &str, line: usize, out: &mut Vec<Import>) {
    // Module specifier: prefer the string after `from`, else the first string.
    let module = first_quoted(after_from(statement))
        .or_else(|| first_quoted(statement))
        .unwrap_or_default();
    if module.is_empty() {
        return;
    }

    let mut names = Vec::new();
    if let Some(open) = statement.find('{') {
        if let Some(close) = statement[open..].find('}') {
            names.extend(parse_import_names(&statement[open + 1..open + close]));
        }
    }
    // Default and namespace bindings live between `import` and `from`/`{`.
    let head_end = statement
        .find('{')
        .or_else(|| find_keyword(statement, "from"))
        .unwrap_or(statement.len());
    let head = statement[..head_end]
        .trim_start_matches(|c: char| c.is_alphabetic()) // drop leading `import`/`export`
        .trim();
    for token in head.split(',') {
        let token = token.trim();
        if let Some(after) = find_keyword(token, "as") {
            if let Some((ident, _)) = read_ident(token[after + 2..].trim()) {
                names.push(ident.to_string());
            }
        } else if let Some((ident, _)) = read_ident(token) {
            if !matches!(ident, "type" | "default" | "from" | "import" | "export") {
                names.push(ident.to_string());
            }
        }
    }

    out.push(Import {
        module,
        names,
        line,
    });
}

const TS_METHOD_MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "static",
    "async",
    "readonly",
    "abstract",
    "override",
    "get",
    "set",
];

const TS_NON_METHOD_NAMES: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "return",
    "function",
    "constructor",
    "do",
    "else",
    "new",
];

fn classify_ts(code: &str, inside_type: bool) -> Option<(String, SymbolKind, bool, bool)> {
    let trimmed = code.trim_start();

    // Strip leading `export` / `export default` / `declare`.
    let mut rest = trimmed;
    let mut exported = false;
    if let Some(after) = after_keyword(rest, "export") {
        exported = true;
        rest = after.trim_start();
        if let Some(after_default) = after_keyword(rest, "default") {
            rest = after_default.trim_start();
        }
    }
    if let Some(after) = after_keyword(rest, "declare") {
        rest = after.trim_start();
    }
    // `abstract class` at top level.
    if let Some(after) = after_keyword(rest, "abstract") {
        rest = after.trim_start();
    }

    if let Some(after) = after_keyword(rest, "class") {
        return read_ident(after).map(|(n, _)| (n.to_string(), SymbolKind::Class, exported, true));
    }
    if let Some(after) = after_keyword(rest, "interface") {
        return read_ident(after)
            .map(|(n, _)| (n.to_string(), SymbolKind::Interface, exported, true));
    }
    if let Some(after) = after_keyword(rest, "enum") {
        return read_ident(after).map(|(n, _)| (n.to_string(), SymbolKind::Enum, exported, false));
    }
    if let Some(after) = after_keyword(rest, "type") {
        return read_ident(after)
            .map(|(n, _)| (n.to_string(), SymbolKind::TypeAlias, exported, false));
    }
    // `async function foo` / `function* foo` / `function foo`.
    let fn_rest = after_keyword(rest, "async")
        .map(str::trim_start)
        .unwrap_or(rest);
    if let Some(after) = after_keyword(fn_rest, "function") {
        let after = after.trim_start().trim_start_matches('*').trim_start();
        return read_ident(after)
            .map(|(n, _)| (n.to_string(), SymbolKind::Function, exported, false));
    }

    // `const foo = (…) => …` / `const foo = function` / UPPER_CASE consts.
    for decl in ["const", "let", "var"] {
        if let Some(after) = after_keyword(rest, decl) {
            let after = after.trim_start();
            if let Some((name, tail)) = read_ident(after) {
                let tail = tail.trim_start();
                if let Some(value) = tail.strip_prefix('=') {
                    let value = value.trim_start();
                    let is_fn = value.contains("=>")
                        || value.starts_with("function")
                        || value.starts_with("async");
                    if is_fn {
                        return Some((name.to_string(), SymbolKind::Function, exported, false));
                    }
                    // Capture exported bindings of any case (a module's public
                    // surface), plus module-level SCREAMING_SNAKE constants.
                    if exported || (decl == "const" && is_screaming_snake(name)) {
                        return Some((name.to_string(), SymbolKind::Constant, exported, false));
                    }
                }
            }
            return None;
        }
    }

    // Class/interface members: `name(...)`, `async name(...)`, `get name()`.
    if inside_type {
        return classify_ts_member(rest);
    }

    None
}

fn classify_ts_member(rest: &str) -> Option<(String, SymbolKind, bool, bool)> {
    let mut cursor = rest;
    loop {
        let mut advanced = false;
        for kw in TS_METHOD_MODIFIERS {
            if let Some(after) = after_keyword(cursor, kw) {
                cursor = after.trim_start();
                advanced = true;
            }
        }
        if !advanced {
            break;
        }
    }
    cursor = cursor.trim_start_matches('*').trim_start();

    let (name, tail) = read_ident(cursor)?;
    if TS_NON_METHOD_NAMES.contains(&name) {
        return None;
    }
    // A method is `name(` possibly with generics `name<T>(`.
    let tail = tail.trim_start();
    let looks_like_method = tail.starts_with('(')
        || (tail.starts_with('<')
            && match_angle(tail)
                .is_some_and(|close| tail[close + 1..].trim_start().starts_with('(')));
    if looks_like_method {
        return Some((name.to_string(), SymbolKind::Method, false, false));
    }
    None
}

fn is_screaming_snake(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && name.chars().any(|c| c.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

pub struct PythonExtractor;

impl SymbolExtractor for PythonExtractor {
    fn language(&self) -> &'static str {
        "Python"
    }

    fn extract(&self, source: &str) -> Vec<Symbol> {
        let source = strip_bom(source);
        let mut symbols = Vec::new();
        // Stack of (name, indent) for enclosing `class`/`def` blocks.
        let mut stack: Vec<(String, SymbolKind, usize)> = Vec::new();
        let mut triple: Option<&'static str> = None;

        for (idx, raw) in source.lines().enumerate() {
            let line_no = idx + 1;

            // Skip the interior of triple-quoted strings/docstrings so we do
            // not match `def`/`class` that appear inside them.
            if let Some(delim) = triple {
                if raw.contains(delim) {
                    triple = None;
                }
                continue;
            }
            if let Some(delim) = opens_unclosed_triple(raw) {
                triple = Some(delim);
                continue;
            }

            let trimmed = raw.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = indent_width(raw);

            // Pop blocks we have dedented out of.
            while let Some((_, _, block_indent)) = stack.last() {
                if indent <= *block_indent {
                    stack.pop();
                } else {
                    break;
                }
            }
            let container = stack.last().map(|(name, _, _)| name.clone());
            let parent_kind = stack.last().map(|(_, kind, _)| *kind);

            let def_rest = after_keyword(trimmed, "async")
                .map(str::trim_start)
                .unwrap_or(trimmed);

            if let Some(after) = after_keyword(def_rest, "def") {
                if let Some((name, _)) = read_ident(after) {
                    let kind = if matches!(parent_kind, Some(SymbolKind::Class)) {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind,
                        line: line_no,
                        container,
                        exported: !name.starts_with('_'),
                        signature: signature_of(raw),
                    });
                    stack.push((name.to_string(), kind, indent));
                }
            } else if let Some(after) = after_keyword(trimmed, "class") {
                if let Some((name, _)) = read_ident(after) {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: SymbolKind::Class,
                        line: line_no,
                        container,
                        exported: !name.starts_with('_'),
                        signature: signature_of(raw),
                    });
                    stack.push((name.to_string(), SymbolKind::Class, indent));
                }
            } else if indent == 0 {
                // Module-level SCREAMING_SNAKE constants.
                if let Some((name, tail)) = read_ident(trimmed) {
                    let tail = tail.trim_start();
                    if (tail.starts_with('=') || tail.starts_with(':')) && is_screaming_snake(name)
                    {
                        symbols.push(Symbol {
                            name: name.to_string(),
                            kind: SymbolKind::Constant,
                            line: line_no,
                            container: None,
                            exported: true,
                            signature: signature_of(raw),
                        });
                    }
                }
            }
        }

        symbols
    }

    fn imports(&self, source: &str) -> Vec<Import> {
        python_imports(source)
    }

    fn referenced_identifiers(&self, source: &str) -> Vec<String> {
        let source = strip_bom(source);
        let mut triple: Option<&'static str> = None;
        let mut set = BTreeSet::new();
        for raw in source.lines() {
            if let Some(delim) = triple {
                if raw.contains(delim) {
                    triple = None;
                }
                continue;
            }
            if let Some(delim) = opens_unclosed_triple(raw) {
                triple = Some(delim);
                continue;
            }
            push_identifiers(&python_code_view(raw), &mut set);
        }
        set.into_iter().collect()
    }
}

/// Parse Python `import` and `from … import …` statements, joining
/// parenthesized name lists across lines.
fn python_imports(source: &str) -> Vec<Import> {
    let source = strip_bom(source);
    let mut out = Vec::new();
    let mut buffer = String::new();
    let mut start_line = 0usize;
    let mut accumulating = false;

    for (idx, raw) in source.lines().enumerate() {
        let line = python_code_view(raw);
        let trimmed = line.trim();

        if accumulating {
            buffer.push(' ');
            buffer.push_str(trimmed);
            if buffer.contains(')') {
                parse_python_from(&buffer, start_line, &mut out);
                accumulating = false;
                buffer.clear();
            }
            continue;
        }

        if let Some(rest) = after_keyword(trimmed, "from") {
            // `from module import a, b` — may open a paren group.
            if trimmed.contains('(') && !trimmed.contains(')') {
                accumulating = true;
                start_line = idx + 1;
                buffer.clear();
                buffer.push_str(trimmed);
            } else {
                parse_python_from(trimmed, idx + 1, &mut out);
            }
            let _ = rest;
        } else if let Some(rest) = after_keyword(trimmed, "import") {
            // `import a.b.c as d, e.f`
            for part in rest.split(',') {
                let part = part.trim();
                let module = part.split_whitespace().next().unwrap_or(part).trim();
                if !module.is_empty() {
                    out.push(Import {
                        module: module.to_string(),
                        names: Vec::new(),
                        line: idx + 1,
                    });
                }
            }
        }
    }

    out
}

fn parse_python_from(statement: &str, line: usize, out: &mut Vec<Import>) {
    let rest = match after_keyword(statement.trim_start(), "from") {
        Some(rest) => rest.trim_start(),
        None => return,
    };
    let (module, names_part) = match find_keyword(rest, "import") {
        Some(pos) => (rest[..pos].trim(), &rest[pos + "import".len()..]),
        None => (rest.trim(), ""),
    };
    let names_part = names_part
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    out.push(Import {
        module: module.to_string(),
        names: parse_import_names(names_part),
        line,
    });
}

/// If `line` opens a triple-quoted string that does not also close on the same
/// line, return the delimiter that will close it.
fn opens_unclosed_triple(line: &str) -> Option<&'static str> {
    for delim in ["\"\"\"", "'''"] {
        if let Some(first) = line.find(delim) {
            let after = &line[first + delim.len()..];
            if !after.contains(delim) {
                return Some(delim);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<&str> {
        symbols.iter().map(|s| s.name.as_str()).collect()
    }

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> &'a Symbol {
        symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("symbol `{name}` not found in {:?}", names(symbols)))
    }

    #[test]
    fn rust_top_level_items() {
        let src = r#"
use std::io;

pub struct Widget {
    pub name: String,
}

enum Color { Red, Green }

pub trait Drawable {
    fn draw(&self);
}

pub fn make() -> Widget { Widget { name: "x".into() } }

const MAX: usize = 10;
"#;
        let syms = RustExtractor.extract(src);
        assert_eq!(find(&syms, "Widget").kind, SymbolKind::Struct);
        assert!(find(&syms, "Widget").exported);
        assert_eq!(find(&syms, "Color").kind, SymbolKind::Enum);
        assert!(!find(&syms, "Color").exported);
        assert_eq!(find(&syms, "Drawable").kind, SymbolKind::Trait);
        assert_eq!(find(&syms, "make").kind, SymbolKind::Function);
        assert_eq!(find(&syms, "MAX").kind, SymbolKind::Constant);
    }

    #[test]
    fn rust_methods_belong_to_impl() {
        let src = r#"
struct Server;

impl Server {
    pub fn new() -> Self { Server }
    fn helper(&self) {}
}

impl<'a> Drawable for Server {
    fn draw(&self) {}
}
"#;
        let syms = RustExtractor.extract(src);
        let new = find(&syms, "new");
        assert_eq!(new.kind, SymbolKind::Method);
        assert_eq!(new.container.as_deref(), Some("Server"));
        let draw = find(&syms, "draw");
        assert_eq!(draw.kind, SymbolKind::Method);
        // impl `for Server` resolves its target type name.
        assert_eq!(draw.container.as_deref(), Some("Server"));
    }

    #[test]
    fn rust_ignores_braces_in_strings_and_comments() {
        let src = r#"
impl Thing {
    fn a(&self) {
        let s = "} not a real close {";
        // } also fake {
        println!("{}", s);
    }
    fn b(&self) {}
}
"#;
        let syms = RustExtractor.extract(src);
        // If brace tracking were fooled, `b` would escape the impl container.
        assert_eq!(find(&syms, "a").container.as_deref(), Some("Thing"));
        assert_eq!(find(&syms, "b").container.as_deref(), Some("Thing"));
    }

    #[test]
    fn rust_raw_and_multiline_strings_do_not_leak_symbols() {
        let src = r####"
fn real() {}

const SQL: &str = r#"
    struct FakeStruct { x: i32 }
    fn fake_fn() {}
"#;

const MULTI: &str = "line one {
fn also_fake() {}
";

fn after() {}
"####;
        let syms = RustExtractor.extract(src);
        let n = names(&syms);
        assert!(n.contains(&"real"));
        assert!(n.contains(&"after"));
        assert!(n.contains(&"SQL"));
        assert!(!n.contains(&"FakeStruct"));
        assert!(!n.contains(&"fake_fn"));
        assert!(!n.contains(&"also_fake"));
        // The `after` fn must still be seen as top-level: the fake braces
        // inside the strings must not have corrupted the brace depth.
        assert_eq!(find(&syms, "after").container, None);
    }

    #[test]
    fn rust_const_fn_is_function_not_constant() {
        let src = "pub const fn answer() -> u32 { 42 }";
        let syms = RustExtractor.extract(src);
        assert_eq!(find(&syms, "answer").kind, SymbolKind::Function);
    }

    #[test]
    fn typescript_functions_classes_and_methods() {
        let src = r#"
export function greet(name: string): string {
    return `hi ${name}`;
}

export const add = (a: number, b: number) => a + b;

export class Repo {
    private items: string[] = [];
    async load(): Promise<void> {}
    get size() { return this.items.length; }
}

interface Shape {
    area(): number;
}

export const MAX_SIZE = 100;
"#;
        let syms = TypeScriptExtractor.extract(src);
        assert_eq!(find(&syms, "greet").kind, SymbolKind::Function);
        assert!(find(&syms, "greet").exported);
        assert_eq!(find(&syms, "add").kind, SymbolKind::Function);
        assert_eq!(find(&syms, "Repo").kind, SymbolKind::Class);
        let load = find(&syms, "load");
        assert_eq!(load.kind, SymbolKind::Method);
        assert_eq!(load.container.as_deref(), Some("Repo"));
        assert_eq!(find(&syms, "size").kind, SymbolKind::Method);
        assert_eq!(find(&syms, "Shape").kind, SymbolKind::Interface);
        assert_eq!(find(&syms, "MAX_SIZE").kind, SymbolKind::Constant);
    }

    #[test]
    fn typescript_exported_lowercase_const_is_captured() {
        let src = r#"
export const router = makeRouter();
const localOnly = 5;
export const CONFIG = {};
"#;
        let syms = TypeScriptExtractor.extract(src);
        assert_eq!(find(&syms, "router").kind, SymbolKind::Constant);
        assert!(find(&syms, "router").exported);
        assert_eq!(find(&syms, "CONFIG").kind, SymbolKind::Constant);
        // A non-exported, non-SCREAMING const stays out of the public surface.
        assert!(!names(&syms).contains(&"localOnly"));
    }

    #[test]
    fn typescript_ignores_control_flow_as_methods() {
        let src = r#"
class C {
    run() {
        if (x) { doThing(); }
        for (const i of xs) { work(i); }
    }
}
"#;
        let syms = TypeScriptExtractor.extract(src);
        assert_eq!(names(&syms), vec!["C", "run"]);
    }

    #[test]
    fn python_functions_methods_and_classes() {
        let src = r#"
import os

MAX_RETRIES = 3

def top_level():
    pass

class Service:
    def __init__(self):
        self.ready = False

    async def start(self):
        return True

def _private():
    pass
"#;
        let syms = PythonExtractor.extract(src);
        assert_eq!(find(&syms, "MAX_RETRIES").kind, SymbolKind::Constant);
        assert_eq!(find(&syms, "top_level").kind, SymbolKind::Function);
        assert!(find(&syms, "top_level").exported);
        assert_eq!(find(&syms, "Service").kind, SymbolKind::Class);
        let init = find(&syms, "__init__");
        assert_eq!(init.kind, SymbolKind::Method);
        assert_eq!(init.container.as_deref(), Some("Service"));
        assert!(!init.exported);
        assert_eq!(find(&syms, "start").kind, SymbolKind::Method);
        assert!(!find(&syms, "_private").exported);
    }

    #[test]
    fn python_skips_docstrings() {
        let src = r#"
def real():
    """
    def fake_inside_docstring():
        pass
    """
    return 1
"#;
        let syms = PythonExtractor.extract(src);
        assert_eq!(names(&syms), vec!["real"]);
    }

    #[test]
    fn leading_utf8_bom_is_ignored() {
        // Files written by Windows/.NET tools often begin with a BOM.
        let src = "\u{feff}export class UserService {\n  load() {}\n}\n";
        let syms = TypeScriptExtractor.extract(src);
        assert_eq!(find(&syms, "UserService").kind, SymbolKind::Class);
        assert_eq!(
            find(&syms, "load").container.as_deref(),
            Some("UserService")
        );

        let rust = "\u{feff}pub fn first() {}";
        assert_eq!(
            find(&RustExtractor.extract(rust), "first").kind,
            SymbolKind::Function
        );
    }

    fn modules(imports: &[Import]) -> Vec<&str> {
        imports.iter().map(|i| i.module.as_str()).collect()
    }

    #[test]
    fn rust_imports_parsed() {
        let src = r#"
use std::collections::BTreeMap;
use std::io::{Read, Write};
use crate::graph::ProjectGraph as Graph;
pub use serde::Serialize;
use std::fmt::*;
"#;
        let imports = RustExtractor.imports(src);
        assert!(modules(&imports).contains(&"std::collections"));
        let braces = imports.iter().find(|i| i.module == "std::io").unwrap();
        assert_eq!(braces.names, vec!["Read", "Write"]);
        let aliased = imports.iter().find(|i| i.module == "crate::graph").unwrap();
        assert_eq!(aliased.names, vec!["Graph"]);
        let glob = imports.iter().find(|i| i.module == "std::fmt").unwrap();
        assert!(glob.names.is_empty());
    }

    #[test]
    fn typescript_imports_parsed() {
        let src = r#"
import { UserService, Repo } from './services';
import Default from '../lib/default';
import * as utils from 'utils';
import './side-effect';
const fs = require('fs');
"#;
        let imports = TypeScriptExtractor.imports(src);
        let named = imports.iter().find(|i| i.module == "./services").unwrap();
        assert!(named.names.contains(&"UserService".to_string()));
        assert!(named.names.contains(&"Repo".to_string()));
        let default = imports
            .iter()
            .find(|i| i.module == "../lib/default")
            .unwrap();
        assert_eq!(default.names, vec!["Default"]);
        let ns = imports.iter().find(|i| i.module == "utils").unwrap();
        assert_eq!(ns.names, vec!["utils"]);
        assert!(modules(&imports).contains(&"./side-effect"));
        assert!(modules(&imports).contains(&"fs"));
    }

    #[test]
    fn python_imports_parsed() {
        let src = r#"
import os
import os.path as p
from collections import OrderedDict, defaultdict
from .local import Thing
from typing import (
    List,
    Optional,
)
"#;
        let imports = PythonExtractor.imports(src);
        assert!(modules(&imports).contains(&"os"));
        assert!(modules(&imports).contains(&"os.path"));
        let coll = imports.iter().find(|i| i.module == "collections").unwrap();
        assert_eq!(coll.names, vec!["OrderedDict", "defaultdict"]);
        let local = imports.iter().find(|i| i.module == ".local").unwrap();
        assert_eq!(local.names, vec!["Thing"]);
        let typing = imports.iter().find(|i| i.module == "typing").unwrap();
        assert_eq!(typing.names, vec!["List", "Optional"]);
    }

    #[test]
    fn referenced_identifiers_exclude_comments_and_strings() {
        let rust = r#"
fn use_widget() {
    let w = Widget::new(); // Gadget in a comment
    let s = "Doohickey in a string";
}
"#;
        let refs = RustExtractor.referenced_identifiers(rust);
        assert!(refs.contains(&"Widget".to_string()));
        assert!(!refs.contains(&"Gadget".to_string()));
        assert!(!refs.contains(&"Doohickey".to_string()));
    }

    #[test]
    fn extractor_selection_by_extension() {
        assert!(extractor_for_path(Path::new("a/b.rs")).is_some());
        assert!(extractor_for_path(Path::new("a/b.tsx")).is_some());
        assert!(extractor_for_path(Path::new("a/b.py")).is_some());
        assert!(extractor_for_path(Path::new("a/b.txt")).is_none());
    }

    #[test]
    fn empty_and_garbage_input_do_not_panic() {
        assert!(RustExtractor.extract("").is_empty());
        assert!(TypeScriptExtractor.extract("").is_empty());
        assert!(PythonExtractor.extract("").is_empty());
        let junk = "{{{ ))) ''' \"\"\" ``` <<< >>>";
        RustExtractor.extract(junk);
        TypeScriptExtractor.extract(junk);
        PythonExtractor.extract(junk);
    }
}
