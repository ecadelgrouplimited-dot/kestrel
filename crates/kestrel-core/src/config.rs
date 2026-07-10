//! Project configuration: a `kestrel.toml` at the project root that lets a
//! developer customize Kestrel's defaults without passing flags every time.
//!
//! Everything is optional — a project with no config behaves exactly as before.
//! The config is a thin layer of preferences (default model, context budget,
//! answer length) plus an explicit override for the verification ladder, so a
//! team can pin the exact checks a change must pass.

use serde::Deserialize;
use std::path::Path;

/// The parsed contents of `kestrel.toml`.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub defaults: Defaults,
    pub verify: VerifyConfig,
}

/// Default settings for model-backed commands (`ask`, `edit`).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub model: Option<String>,
    pub budget: Option<usize>,
    pub max_tokens: Option<u64>,
}

/// An explicit verification ladder, overriding auto-detection when non-empty.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerifyConfig {
    /// Shell command lines to run, in order.
    pub steps: Vec<String>,
}

/// The outcome of attempting to load a config file.
#[derive(Debug, Clone)]
pub enum ConfigLoad {
    /// No `kestrel.toml` was found; defaults apply.
    Missing,
    /// A config file was parsed successfully.
    Loaded(Config),
    /// A config file existed but could not be parsed.
    Invalid(String),
}

impl ConfigLoad {
    /// The effective config, treating missing/invalid as empty defaults.
    pub fn config(&self) -> Config {
        match self {
            ConfigLoad::Loaded(config) => config.clone(),
            _ => Config::default(),
        }
    }
}

const CONFIG_NAMES: [&str; 2] = ["kestrel.toml", ".kestrel.toml"];

/// Load `kestrel.toml` (or `.kestrel.toml`) from `root`, if present.
pub fn load_config(root: &Path) -> ConfigLoad {
    for name in CONFIG_NAMES {
        let path = root.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        return match toml::from_str::<Config>(&text) {
            Ok(config) => ConfigLoad::Loaded(config),
            Err(err) => ConfigLoad::Invalid(format!("{}: {err}", path.display())),
        };
    }
    ConfigLoad::Missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let text = r#"
[defaults]
model = "claude-sonnet-5"
budget = 20000
max_tokens = 4096

[verify]
steps = ["cargo fmt --all -- --check", "cargo clippy -- -D warnings", "cargo test"]
"#;
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.defaults.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(config.defaults.budget, Some(20_000));
        assert_eq!(config.defaults.max_tokens, Some(4096));
        assert_eq!(config.verify.steps.len(), 3);
        assert_eq!(config.verify.steps[1], "cargo clippy -- -D warnings");
    }

    #[test]
    fn empty_config_is_all_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.defaults.model.is_none());
        assert!(config.verify.steps.is_empty());
    }

    #[test]
    fn partial_config_only_sets_given_fields() {
        let config: Config = toml::from_str("[defaults]\nmodel = \"x\"\n").unwrap();
        assert_eq!(config.defaults.model.as_deref(), Some("x"));
        assert!(config.defaults.budget.is_none());
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(toml::from_str::<Config>("[defaults]\nmodle = \"x\"\n").is_err());
    }

    #[test]
    fn missing_file_reports_missing() {
        let dir = std::env::temp_dir().join(format!("kestrel-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(load_config(&dir), ConfigLoad::Missing));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
