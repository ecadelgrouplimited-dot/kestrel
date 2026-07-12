//! User settings: who you are, and which model providers Kestrel may use.
//!
//! Unlike `kestrel.toml` (per-project, committed to the repo), settings are
//! per-user and hold secrets, so they live in the user config directory
//! (`%APPDATA%\kestrel\settings.toml` on Windows, `~/.config/kestrel/…`
//! elsewhere) — never in the project. The settings UI reads and writes this
//! file; the presets give sensible defaults for each supported provider.

use crate::providers::{ProviderConfig, ProviderKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The whole user configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub user: UserInfo,
    /// Name of the provider entry to use by default.
    pub active_provider: Option<String>,
    /// Configured providers, keyed by a user-chosen name.
    pub providers: BTreeMap<String, ProviderSettings>,
    /// Recently opened/created project paths, most recent first.
    #[serde(default)]
    pub recent_projects: Vec<String>,
    /// Cost caps that warn/stop the agent when reached.
    #[serde(default)]
    pub budget: Budget,
    /// Allow/deny rules gating the agent's tools.
    #[serde(default)]
    pub policy: crate::policy::Policy,
}

/// Spend caps in USD. `None`/`0` means no limit.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Cap for the current conversation.
    pub session_limit: Option<f64>,
    /// Cap for all requests today (UTC).
    pub daily_limit: Option<f64>,
}

/// Optional identifying details for the developer.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserInfo {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// One configured provider: its API shape, endpoint, key, and default model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub model: String,
}

impl ProviderSettings {
    /// Turn this into a `ProviderConfig` for making a request.
    pub fn to_config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: self.kind,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
        }
    }
}

impl Settings {
    /// The active provider's settings, if one is selected and present.
    pub fn active(&self) -> Option<&ProviderSettings> {
        self.active_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
    }
}

/// A starter provider configuration for one of the built-in presets:
/// `"anthropic"`, `"openai"`, `"deepseek"`, or `"kimi"`. The default model is
/// the first (latest) suggestion for that provider.
pub fn provider_preset(name: &str) -> Option<ProviderSettings> {
    let (kind, base_url) = match name {
        "anthropic" => (ProviderKind::Anthropic, "https://api.anthropic.com"),
        "openai" => (ProviderKind::Openai, "https://api.openai.com/v1"),
        "deepseek" => (ProviderKind::Openai, "https://api.deepseek.com/v1"),
        "kimi" => (ProviderKind::Openai, "https://api.moonshot.ai/v1"),
        "zai" => (ProviderKind::Openai, "https://api.z.ai/api/paas/v4"),
        _ => return None,
    };
    let model = model_suggestions(name)
        .first()
        .copied()
        .unwrap_or("")
        .to_string();
    Some(ProviderSettings {
        kind,
        base_url: base_url.to_string(),
        api_key: String::new(),
        model,
    })
}

/// Suggested model IDs for a preset name, latest/best first. These are only
/// suggestions — the model field is free text, so a user can enter any ID the
/// provider currently supports.
pub fn model_suggestions(name: &str) -> &'static [&'static str] {
    match name {
        "anthropic" => &[
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-haiku-4-5",
        ],
        "openai" => &["gpt-5", "gpt-5-mini", "gpt-4.1", "gpt-4o", "o3"],
        "deepseek" => &[
            "deepseek-v4-pro",
            "deepseek-v4-preview",
            "deepseek-reasoner",
        ],
        "kimi" => &["kimi-k2", "kimi-k2-turbo", "moonshot-v1-128k"],
        "zai" => &[
            "glm-5.2",
            "glm-5.1",
            "glm-5",
            "glm-5-turbo",
            "glm-4.7",
            "glm-4.6",
        ],
        _ => &[],
    }
}

/// Suggested model IDs for an already-configured provider, inferred from its
/// base URL (falling back to its API kind).
pub fn model_suggestions_for(provider: &ProviderSettings) -> &'static [&'static str] {
    let base = provider.base_url.to_lowercase();
    if base.contains("anthropic") {
        model_suggestions("anthropic")
    } else if base.contains("deepseek") {
        model_suggestions("deepseek")
    } else if base.contains("moonshot") {
        model_suggestions("kimi")
    } else if base.contains("z.ai") {
        model_suggestions("zai")
    } else if base.contains("openai") {
        model_suggestions("openai")
    } else {
        match provider.kind {
            ProviderKind::Anthropic => model_suggestions("anthropic"),
            ProviderKind::Openai => model_suggestions("openai"),
        }
    }
}

/// The known preset names, for a settings UI dropdown.
pub const PROVIDER_PRESETS: [&str; 5] = ["anthropic", "openai", "deepseek", "kimi", "zai"];

/// The path to the settings file (`<config-dir>/kestrel/settings.toml`).
pub fn settings_path() -> PathBuf {
    config_dir().join("kestrel").join("settings.toml")
}

/// The platform user config directory.
fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }
    PathBuf::from(".")
}

/// Load settings from the user config file, or defaults if missing/invalid.
pub fn load_settings() -> Settings {
    load_settings_from(&settings_path())
}

/// Load settings from a specific path (used by tests).
pub fn load_settings_from(path: &std::path::Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

/// Persist settings to the user config file, creating directories as needed.
pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    save_settings_to(&settings_path(), settings)
}

/// Persist settings to a specific path (used by tests).
pub fn save_settings_to(path: &std::path::Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_the_four_providers() {
        for name in PROVIDER_PRESETS {
            assert!(provider_preset(name).is_some(), "missing preset {name}");
        }
        assert!(provider_preset("unknown").is_none());
        assert_eq!(
            provider_preset("deepseek").unwrap().kind,
            ProviderKind::Openai
        );
        assert_eq!(
            provider_preset("anthropic").unwrap().kind,
            ProviderKind::Anthropic
        );
        // Preset default model is the latest suggestion, not "deepseek-chat".
        assert_eq!(
            provider_preset("deepseek").unwrap().model,
            "deepseek-v4-pro"
        );
        assert_eq!(provider_preset("openai").unwrap().model, "gpt-5");
        // z.ai GLM is OpenAI-compatible and defaults to the flagship glm-5.2.
        let zai = provider_preset("zai").unwrap();
        assert_eq!(zai.kind, ProviderKind::Openai);
        assert_eq!(zai.base_url, "https://api.z.ai/api/paas/v4");
        assert_eq!(zai.model, "glm-5.2");
        assert_eq!(model_suggestions_for(&zai), model_suggestions("zai"));
    }

    #[test]
    fn model_suggestions_infer_from_base_url() {
        let ds = provider_preset("deepseek").unwrap();
        assert_eq!(model_suggestions_for(&ds), model_suggestions("deepseek"));
        let oa = provider_preset("openai").unwrap();
        assert_eq!(model_suggestions_for(&oa), model_suggestions("openai"));
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let mut settings = Settings {
            user: UserInfo {
                name: Some("Ada".to_string()),
                email: Some("ada@example.com".to_string()),
            },
            active_provider: Some("work".to_string()),
            providers: BTreeMap::new(),
            recent_projects: vec!["E:/demo".to_string()],
            budget: Budget::default(),
            policy: crate::policy::Policy::default(),
        };
        let mut provider = provider_preset("deepseek").unwrap();
        provider.api_key = "sk-secret".to_string();
        settings.providers.insert("work".to_string(), provider);

        let dir = std::env::temp_dir().join(format!("kestrel-settings-{}", std::process::id()));
        let path = dir.join("settings.toml");
        save_settings_to(&path, &settings).unwrap();

        let loaded = load_settings_from(&path);
        assert_eq!(loaded.user.name.as_deref(), Some("Ada"));
        assert_eq!(loaded.active_provider.as_deref(), Some("work"));
        let active = loaded.active().unwrap();
        assert_eq!(active.kind, ProviderKind::Openai);
        assert_eq!(active.api_key, "sk-secret");
        assert_eq!(active.model, "deepseek-v4-pro");
        assert_eq!(loaded.recent_projects, vec!["E:/demo".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_settings_file_is_default() {
        let path = std::env::temp_dir().join("kestrel-nonexistent-settings-xyz.toml");
        let settings = load_settings_from(&path);
        assert!(settings.providers.is_empty());
        assert!(settings.active().is_none());
    }
}
