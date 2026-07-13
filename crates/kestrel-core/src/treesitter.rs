//! Tree-sitter symbol extraction: the real-parser backend behind the
//! [`SymbolExtractor`](crate::symbols::SymbolExtractor) trait.
//!
//! Where the heuristic scanners approximate structure, this walks an actual
//! syntax tree, so declarations are found precisely — nested methods, exported
//! arrow functions, decorated Python defs, and so on. It implements the same
//! trait, so every caller (inspect, the graph, context packing, the editor
//! outline) benefits with no changes. If a grammar ever fails to load, symbol
//! extraction falls back to the heuristic extractor, and imports/references are
//! always delegated to it (that lexical evidence still feeds the graph).

use crate::symbols::{Import, Symbol, SymbolExtractor, SymbolKind};
use tree_sitter::{Node, Parser};

const MAX_SIGNATURE_LEN: usize = 160;

/// A tree-sitter grammar Kestrel can parse.
#[derive(Debug, Clone, Copy)]
pub enum TsLang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
}

fn language(lang: TsLang) -> tree_sitter::Language {
    match lang {
        TsLang::Rust => tree_sitter_rust::language(),
        TsLang::TypeScript => tree_sitter_typescript::language_typescript(),
        TsLang::Tsx => tree_sitter_typescript::language_tsx(),
        TsLang::JavaScript => tree_sitter_javascript::language(),
        TsLang::Python => tree_sitter_python::language(),
    }
}

fn parse(source: &str, lang: TsLang) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language(lang)).ok()?;
    parser.parse(source, None)
}

/// The `SymbolExtractor` implementation backed by tree-sitter. Symbol extraction
/// is precise; imports/references delegate to the heuristic extractor.
pub struct TreeSitterExtractor {
    lang: TsLang,
    display: &'static str,
}

impl TreeSitterExtractor {
    pub fn new(lang: TsLang, display: &'static str) -> Self {
        Self { lang, display }
    }

    fn heuristic(&self) -> Option<Box<dyn SymbolExtractor>> {
        crate::symbols::heuristic_for_language(self.display)
    }
}

impl SymbolExtractor for TreeSitterExtractor {
    fn language(&self) -> &'static str {
        self.display
    }

    fn extract(&self, source: &str) -> Vec<Symbol> {
        match ts_symbols(source, self.lang) {
            Some(symbols) => symbols,
            None => self
                .heuristic()
                .map(|e| e.extract(source))
                .unwrap_or_default(),
        }
    }

    fn imports(&self, source: &str) -> Vec<Import> {
        self.heuristic()
            .map(|e| e.imports(source))
            .unwrap_or_default()
    }

    fn referenced_identifiers(&self, source: &str) -> Vec<String> {
        self.heuristic()
            .map(|e| e.referenced_identifiers(source))
            .unwrap_or_default()
    }
}

/// Parse `source` and extract its symbols, or `None` if the grammar won't load.
pub fn ts_symbols(source: &str, lang: TsLang) -> Option<Vec<Symbol>> {
    let tree = parse(source, lang)?;
    let root = tree.root_node();
    let src = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        dispatch(child, src, lang, None, &mut out);
    }
    Some(out)
}

fn dispatch(node: Node, src: &[u8], lang: TsLang, container: Option<&str>, out: &mut Vec<Symbol>) {
    match lang {
        TsLang::Rust => handle_rust(node, src, container, out),
        TsLang::Python => handle_python(node, src, container, out),
        _ => handle_ts(node, src, container, out),
    }
}

// --- shared helpers --------------------------------------------------------

fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

fn field_text(node: Node, field: &str, src: &[u8]) -> String {
    node.child_by_field_name(field)
        .map(|n| node_text(n, src).to_string())
        .unwrap_or_default()
}

fn signature(node: Node, src: &[u8]) -> String {
    let first = node_text(node, src).lines().next().unwrap_or("").trim();
    if first.len() > MAX_SIGNATURE_LEN {
        let end = (0..=MAX_SIGNATURE_LEN)
            .rev()
            .find(|&i| first.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}…", &first[..end])
    } else {
        first.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Symbol>,
    name: &str,
    kind: SymbolKind,
    node: Node,
    exported: bool,
    container: Option<&str>,
    src: &[u8],
) {
    if name.is_empty() {
        return;
    }
    out.push(Symbol {
        name: name.to_string(),
        kind,
        line: node.start_position().row + 1,
        container: container.map(|c| c.to_string()),
        exported,
        signature: signature(node, src),
    });
}

// --- Rust ------------------------------------------------------------------

fn rust_pub(node: Node) -> bool {
    (0..node.child_count())
        .filter_map(|i| node.child(i))
        .any(|ch| ch.kind() == "visibility_modifier")
}

fn handle_rust(node: Node, src: &[u8], container: Option<&str>, out: &mut Vec<Symbol>) {
    let name = || field_text(node, "name", src);
    match node.kind() {
        "function_item" => {
            let kind = if container.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push(out, &name(), kind, node, rust_pub(node), container, src);
        }
        "struct_item" => push(
            out,
            &name(),
            SymbolKind::Struct,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "enum_item" => push(
            out,
            &name(),
            SymbolKind::Enum,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "union_item" => push(
            out,
            &name(),
            SymbolKind::Union,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "type_item" => push(
            out,
            &name(),
            SymbolKind::TypeAlias,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "const_item" | "static_item" => push(
            out,
            &name(),
            SymbolKind::Constant,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "macro_definition" => push(
            out,
            &name(),
            SymbolKind::Macro,
            node,
            rust_pub(node),
            container,
            src,
        ),
        "trait_item" => {
            let n = name();
            push(
                out,
                &n,
                SymbolKind::Trait,
                node,
                rust_pub(node),
                container,
                src,
            );
            recurse_rust_body(node, src, &n, out);
        }
        "impl_item" => {
            let ty = field_text(node, "type", src);
            let n = ty.split('<').next().unwrap_or(&ty).trim().to_string();
            push(out, &n, SymbolKind::Impl, node, false, container, src);
            recurse_rust_body(node, src, &n, out);
        }
        "mod_item" => {
            push(
                out,
                &name(),
                SymbolKind::Module,
                node,
                rust_pub(node),
                container,
                src,
            );
            if let Some(body) = node.child_by_field_name("body") {
                let mut c = body.walk();
                for ch in body.named_children(&mut c) {
                    handle_rust(ch, src, None, out);
                }
            }
        }
        _ => {}
    }
}

fn recurse_rust_body(node: Node, src: &[u8], container: &str, out: &mut Vec<Symbol>) {
    if let Some(body) = node.child_by_field_name("body") {
        let mut c = body.walk();
        for ch in body.named_children(&mut c) {
            handle_rust(ch, src, Some(container), out);
        }
    }
}

// --- TypeScript / JavaScript ----------------------------------------------

fn ts_exported(node: Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false)
}

fn handle_ts(node: Node, src: &[u8], container: Option<&str>, out: &mut Vec<Symbol>) {
    match node.kind() {
        "export_statement" => {
            let mut c = node.walk();
            for ch in node.named_children(&mut c) {
                handle_ts(ch, src, container, out);
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            let kind = if container.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push(
                out,
                &field_text(node, "name", src),
                kind,
                node,
                ts_exported(node),
                container,
                src,
            );
        }
        "class_declaration" | "abstract_class_declaration" => {
            let n = field_text(node, "name", src);
            push(
                out,
                &n,
                SymbolKind::Class,
                node,
                ts_exported(node),
                container,
                src,
            );
            if let Some(body) = node.child_by_field_name("body") {
                let mut c = body.walk();
                for ch in body.named_children(&mut c) {
                    handle_ts(ch, src, Some(&n), out);
                }
            }
        }
        "interface_declaration" => push(
            out,
            &field_text(node, "name", src),
            SymbolKind::Interface,
            node,
            ts_exported(node),
            container,
            src,
        ),
        "enum_declaration" => push(
            out,
            &field_text(node, "name", src),
            SymbolKind::Enum,
            node,
            ts_exported(node),
            container,
            src,
        ),
        "type_alias_declaration" => push(
            out,
            &field_text(node, "name", src),
            SymbolKind::TypeAlias,
            node,
            ts_exported(node),
            container,
            src,
        ),
        "method_definition" => push(
            out,
            &field_text(node, "name", src),
            SymbolKind::Method,
            node,
            true,
            container,
            src,
        ),
        "lexical_declaration" | "variable_declaration" => {
            let exported = ts_exported(node);
            let mut c = node.walk();
            for decl in node.named_children(&mut c) {
                if decl.kind() != "variable_declarator" {
                    continue;
                }
                let Some(value) = decl.child_by_field_name("value") else {
                    continue;
                };
                if matches!(
                    value.kind(),
                    "arrow_function" | "function" | "function_expression" | "generator_function"
                ) {
                    let kind = if container.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    push(
                        out,
                        &field_text(decl, "name", src),
                        kind,
                        decl,
                        exported,
                        container,
                        src,
                    );
                }
            }
        }
        _ => {}
    }
}

// --- Python ----------------------------------------------------------------

fn handle_python(node: Node, src: &[u8], container: Option<&str>, out: &mut Vec<Symbol>) {
    match node.kind() {
        "function_definition" => {
            let name = field_text(node, "name", src);
            let exported = !name.starts_with('_');
            let kind = if container.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push(out, &name, kind, node, exported, container, src);
        }
        "class_definition" => {
            let name = field_text(node, "name", src);
            let exported = !name.starts_with('_');
            push(
                out,
                &name,
                SymbolKind::Class,
                node,
                exported,
                container,
                src,
            );
            if let Some(body) = node.child_by_field_name("body") {
                let mut c = body.walk();
                for ch in body.named_children(&mut c) {
                    handle_python(ch, src, Some(&name), out);
                }
            }
        }
        "decorated_definition" => {
            if let Some(def) = node.child_by_field_name("definition") {
                handle_python(def, src, container, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<(&str, SymbolKind, bool)> {
        symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.exported))
            .collect()
    }

    #[test]
    fn rust_symbols_are_precise() {
        let src = "pub fn top() {}\nstruct Bar;\nimpl Bar {\n    pub fn method(&self) {}\n}\n";
        let s = ts_symbols(src, TsLang::Rust).unwrap();
        let n = names(&s);
        assert!(n.contains(&("top", SymbolKind::Function, true)));
        assert!(n.contains(&("Bar", SymbolKind::Struct, false)));
        assert!(n.contains(&("Bar", SymbolKind::Impl, false)));
        let method = s.iter().find(|s| s.name == "method").unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.container.as_deref(), Some("Bar"));
    }

    #[test]
    fn typescript_finds_exports_and_arrow_functions() {
        let src = "export function foo() {}\nexport const bar = () => {};\nclass C { m() {} }\n";
        let s = ts_symbols(src, TsLang::TypeScript).unwrap();
        let n = names(&s);
        assert!(n.contains(&("foo", SymbolKind::Function, true)));
        assert!(n.contains(&("bar", SymbolKind::Function, true)));
        assert!(n
            .iter()
            .any(|(name, k, _)| *name == "C" && *k == SymbolKind::Class));
        let m = s.iter().find(|s| s.name == "m").unwrap();
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.container.as_deref(), Some("C"));
    }

    #[test]
    fn python_methods_have_class_container() {
        let src = "def top():\n    pass\n\nclass C:\n    def m(self):\n        pass\n";
        let s = ts_symbols(src, TsLang::Python).unwrap();
        assert!(s
            .iter()
            .any(|s| s.name == "top" && s.kind == SymbolKind::Function));
        let m = s.iter().find(|s| s.name == "m").unwrap();
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.container.as_deref(), Some("C"));
    }
}
