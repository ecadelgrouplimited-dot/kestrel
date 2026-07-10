//! Persistent, incremental index cache — the first step from a re-derived
//! context engine toward a *living* one.
//!
//! Parsing every file on every command is fine for a demo and ruinous for a
//! real repository. This module stores each file's extracted structure keyed
//! by its path, validated against the file's size and modification time, in
//! `<project-root>/.kestrel/index.json`. On the next run, unchanged files are
//! served from the cache and only changed files are re-parsed. That is the
//! seed of the Living System Model's defining property: the model persists and
//! updates incrementally rather than being rebuilt from scratch.

use crate::symbols::{Import, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::Metadata;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Bump when the cached representation changes in an incompatible way; a
/// mismatch causes the whole cache to be discarded and rebuilt.
const CACHE_VERSION: u32 = 1;
const CACHE_DIR: &str = ".kestrel";
const CACHE_FILE: &str = "index.json";

/// One file's cached extraction result plus the stamp used to validate it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFile {
    pub size: u64,
    pub mtime_ns: u128,
    pub language: String,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub references: Vec<String>,
    pub source_bytes: usize,
}

/// The on-disk index: a version stamp and per-file cached extractions keyed by
/// normalized relative path.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexCache {
    pub version: u32,
    pub files: BTreeMap<String, CachedFile>,
}

impl Default for IndexCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            files: BTreeMap::new(),
        }
    }
}

impl IndexCache {
    /// Load the cache for `root`, returning an empty cache if it is missing,
    /// unreadable, corrupt, or from an incompatible version.
    pub fn load(root: &Path) -> Self {
        let path = root.join(CACHE_DIR).join(CACHE_FILE);
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<IndexCache>(&text) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            _ => Self::default(),
        }
    }

    /// Write the cache under `<root>/.kestrel/`. Best-effort; callers typically
    /// ignore the result so a read-only tree still works.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let dir = root.join(CACHE_DIR);
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join(CACHE_FILE), text)
    }

    /// Return the cached entry for `key` only if its stamp still matches the
    /// file's current size and modification time.
    pub fn get_fresh(&self, key: &str, size: u64, mtime_ns: u128) -> Option<&CachedFile> {
        self.files
            .get(key)
            .filter(|c| c.size == size && c.mtime_ns == mtime_ns)
    }

    pub fn insert(&mut self, key: String, entry: CachedFile) {
        self.files.insert(key, entry);
    }
}

/// The (size, modification-time-in-nanoseconds) stamp used to detect changes.
pub fn file_signature(meta: &Metadata) -> (u64, u128) {
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (meta.len(), mtime_ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::SymbolKind;

    fn cached() -> CachedFile {
        CachedFile {
            size: 10,
            mtime_ns: 12345,
            language: "Rust".to_string(),
            symbols: vec![Symbol {
                name: "foo".to_string(),
                kind: SymbolKind::Function,
                line: 1,
                container: None,
                exported: true,
                signature: "pub fn foo()".to_string(),
            }],
            imports: vec![Import {
                module: "std::io".to_string(),
                names: vec!["Write".to_string()],
                line: 1,
            }],
            references: vec!["bar".to_string()],
            source_bytes: 10,
        }
    }

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kestrel-cache-test-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_cache_loads_empty() {
        let root = temp_root("missing");
        let cache = IndexCache::load(&root);
        assert!(cache.files.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_then_load_round_trips() {
        let root = temp_root("roundtrip");
        let mut cache = IndexCache::default();
        cache.insert("src/lib.rs".to_string(), cached());
        cache.save(&root).unwrap();

        let loaded = IndexCache::load(&root);
        let entry = loaded.files.get("src/lib.rs").expect("entry present");
        assert_eq!(entry.symbols[0].name, "foo");
        assert_eq!(entry.imports[0].module, "std::io");
        assert_eq!(entry.references, vec!["bar"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn freshness_depends_on_stamp() {
        let mut cache = IndexCache::default();
        cache.insert("a.rs".to_string(), cached());
        assert!(cache.get_fresh("a.rs", 10, 12345).is_some());
        assert!(cache.get_fresh("a.rs", 11, 12345).is_none()); // size changed
        assert!(cache.get_fresh("a.rs", 10, 99999).is_none()); // mtime changed
        assert!(cache.get_fresh("missing.rs", 10, 12345).is_none());
    }

    #[test]
    fn version_mismatch_discards_cache() {
        let root = temp_root("version");
        std::fs::create_dir_all(root.join(CACHE_DIR)).unwrap();
        std::fs::write(
            root.join(CACHE_DIR).join(CACHE_FILE),
            r#"{"version":999,"files":{}}"#,
        )
        .unwrap();
        let cache = IndexCache::load(&root);
        assert_eq!(cache.version, CACHE_VERSION);
        let _ = std::fs::remove_dir_all(&root);
    }
}
