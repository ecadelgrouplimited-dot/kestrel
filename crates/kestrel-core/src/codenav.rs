//! Semantic code navigation for the agent.
//!
//! These give the agent LSP-style precision instead of grep-and-hope: jump to a
//! symbol's definition, find every reference, outline a file, and rename a symbol
//! across the whole project. Definitions and outlines reuse the tree-sitter
//! symbol backend; references and rename use whole-word matching (so `user`
//! never matches `username`) over the project's non-ignored text files.

use std::path::Path;

/// A symbol definition site.
#[derive(Debug, Clone)]
pub struct DefHit {
    pub path: String,
    pub line: usize,
    pub kind: String,
    pub signature: String,
}

/// A reference (whole-word use) of a name.
#[derive(Debug, Clone)]
pub struct RefHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// The outcome of a project-wide rename.
#[derive(Debug, Clone, Default)]
pub struct RenameResult {
    pub files_changed: usize,
    pub occurrences: usize,
    pub changed_paths: Vec<String>,
}

/// Max bytes of a file we'll scan for references/rename (skip huge/generated).
const SCAN_FILE_CAP: u64 = 2_000_000;

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte offsets of every whole-word occurrence of `needle` in `haystack` — a
/// match whose neighbouring characters are not identifier characters.
pub fn word_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !haystack[..abs]
                .chars()
                .next_back()
                .map(is_ident_char)
                .unwrap_or(false);
        let after = abs + needle.len();
        let after_ok = after >= haystack.len()
            || !haystack[after..]
                .chars()
                .next()
                .map(is_ident_char)
                .unwrap_or(false);
        if before_ok && after_ok {
            out.push(abs);
        }
        start = abs + needle.len();
    }
    out
}

/// Replace every whole-word occurrence of `old` with `new`, returning the new
/// text and how many replacements were made.
pub fn replace_word(text: &str, old: &str, new: &str) -> (String, usize) {
    let occurrences = word_occurrences(text, old);
    if occurrences.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for start in &occurrences {
        out.push_str(&text[last..*start]);
        out.push_str(new);
        last = start + old.len();
    }
    out.push_str(&text[last..]);
    (out, occurrences.len())
}

/// Find where `name` is defined across the project (exact symbol-name match).
pub fn find_definitions(root: &Path, name: &str) -> std::io::Result<Vec<DefHit>> {
    let mut hits = Vec::new();
    for (rel, file) in crate::inspect::project_symbols(root)? {
        for sym in file.symbols {
            if sym.name == name {
                hits.push(DefHit {
                    path: rel.display().to_string().replace('\\', "/"),
                    line: sym.line,
                    kind: sym.kind.as_str().to_string(),
                    signature: sym.signature.clone(),
                });
            }
        }
    }
    Ok(hits)
}

/// Find whole-word references to `name` across the project's text files, capped
/// at `max` results.
pub fn find_references(root: &Path, name: &str, max: usize) -> std::io::Result<Vec<RefHit>> {
    let (project_root, files) = crate::inspect::walk_project(root)?;
    let mut hits = Vec::new();
    for path in files {
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > SCAN_FILE_CAP {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&project_root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            if !word_occurrences(line, name).is_empty() {
                hits.push(RefHit {
                    path: rel.clone(),
                    line: i + 1,
                    text: line.trim().chars().take(200).collect(),
                });
                if hits.len() >= max {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

/// The symbol outline of a single file (relative to `root`, or absolute).
pub fn outline(root: &Path, path: &str) -> std::io::Result<Vec<crate::symbols::Symbol>> {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    Ok(crate::symbols::symbols_for_file(&full)?
        .map(|f| f.symbols)
        .unwrap_or_default())
}

/// Rename `old` to `new` (whole-word) across every non-ignored text file.
pub fn rename_symbol(root: &Path, old: &str, new: &str) -> std::io::Result<RenameResult> {
    let (_project_root, files) = crate::inspect::walk_project(root)?;
    let mut result = RenameResult::default();
    for path in files {
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > SCAN_FILE_CAP {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (updated, count) = replace_word(&text, old, new);
        if count > 0 {
            std::fs::write(&path, updated)?;
            result.files_changed += 1;
            result.occurrences += count;
            result.changed_paths.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_matching_is_precise() {
        assert_eq!(word_occurrences("user = User(user_id)", "user"), vec![0]);
        // `user` inside `username`/`user_id` is not a whole word.
        assert!(word_occurrences("username user_id", "user").is_empty());
        let (out, n) = replace_word("let user = user; username", "user", "acct");
        assert_eq!(n, 2);
        assert_eq!(out, "let acct = acct; username");
    }

    #[test]
    fn definitions_references_and_rename_over_a_temp_project() {
        let dir = std::env::temp_dir().join(format!("kestrel-nav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lib.rs"),
            "pub fn compute_total(x: i32) -> i32 { x + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.rs"),
            "fn main() { let t = compute_total(2); println!(\"{t}\"); }\n",
        )
        .unwrap();

        let defs = find_definitions(&dir, "compute_total").unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0].path.ends_with("lib.rs"));
        assert_eq!(defs[0].kind, "function");

        let refs = find_references(&dir, "compute_total", 50).unwrap();
        // Definition line + call site.
        assert!(refs.len() >= 2);

        let renamed = rename_symbol(&dir, "compute_total", "total_of").unwrap();
        assert_eq!(renamed.files_changed, 2);
        assert!(renamed.occurrences >= 2);
        let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
        assert!(main.contains("total_of(2)"));
        assert!(!main.contains("compute_total"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
