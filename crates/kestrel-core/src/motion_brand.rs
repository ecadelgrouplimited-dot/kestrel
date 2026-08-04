//! Kestrel Motion — brand kits (§13).
//!
//! A brand kit is a small, reusable set of visual defaults — colours, a
//! typeface, a caption style, a watermark — that a project applies so every
//! video in a brand looks like it belongs together. "Apply the Smart Business
//! Book brand kit" should recolour and re-typeset the video *without touching
//! its message*, which is why the kit informs **rendering** rather than
//! rewriting scenes: the renderer fills in a text colour, a font, a themed
//! background only where a scene hasn't chosen one itself. Re-branding is then a
//! one-line change (the project's `theme`), not an edit to every element.
//!
//! The kit lives at `theme/brand-theme.json` (§12).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A reusable brand kit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrandKit {
    /// The kit's name — matches a project's `theme` reference.
    pub name: String,
    /// The ground a themed scene sits on.
    pub background: String,
    /// Default colour for readable text that hasn't set its own.
    pub text: String,
    /// The primary brand colour (headings, key marks).
    pub primary: String,
    /// The accent (calls to action, emphasis).
    pub accent: String,
    /// The font stack applied to all text in the video.
    pub font_family: String,
    /// Caption band colours.
    pub caption_background: String,
    pub caption_text: String,
    /// An optional watermark drawn small in the corner of every scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<String>,
}

impl Default for BrandKit {
    /// Kestrel's own palette, as a sensible starting kit.
    fn default() -> Self {
        BrandKit {
            name: "kestrel".to_string(),
            background: "#0A0A0B".to_string(),
            text: "#F2EFE9".to_string(),
            primary: "#DC8D1F".to_string(),
            accent: "#F2B04A".to_string(),
            font_family: "Segoe UI, Arial, sans-serif".to_string(),
            caption_background: "rgba(0,0,0,.62)".to_string(),
            caption_text: "#ffffff".to_string(),
            watermark: None,
        }
    }
}

impl BrandKit {
    /// The default text colour for an element of `kind`, when it hasn't set its
    /// own `color`. Calls-to-action take the accent; everything else the body
    /// text colour. Titles stay on the readable text colour rather than the
    /// primary, so a coloured heading is a deliberate choice, not a default.
    pub fn text_color_for(&self, kind: &str) -> &str {
        match kind {
            "cta" => &self.accent,
            _ => &self.text,
        }
    }
}

/// Where a project's brand kit lives (§12).
pub fn brand_path(root: &Path) -> PathBuf {
    root.join("theme").join("brand-theme.json")
}

/// Load a project's brand kit, if one has been saved.
pub fn load_brand(root: &Path) -> Option<BrandKit> {
    std::fs::read_to_string(brand_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Persist a brand kit to `theme/brand-theme.json`.
pub fn save_brand(root: &Path, kit: &BrandKit) -> std::io::Result<()> {
    let path = brand_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(kit)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kit_is_the_kestrel_palette() {
        let kit = BrandKit::default();
        assert_eq!(kit.primary, "#DC8D1F");
        // A CTA defaults to the accent; body text to the text colour.
        assert_eq!(kit.text_color_for("cta"), "#F2B04A");
        assert_eq!(kit.text_color_for("title"), kit.text);
        assert_eq!(kit.text_color_for("caption"), kit.text);
    }

    #[test]
    fn round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("kestrel-brand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_brand(&dir).is_none());

        let kit = BrandKit {
            name: "sbb".into(),
            background: "#0e1b2a".into(),
            text: "#eef3f8".into(),
            primary: "#1e88e5".into(),
            accent: "#ffb300".into(),
            font_family: "Inter, sans-serif".into(),
            caption_background: "rgba(0,0,0,.7)".into(),
            caption_text: "#ffffff".into(),
            watermark: Some("Smart Business Book".into()),
        };
        save_brand(&dir, &kit).unwrap();
        assert!(brand_path(&dir).exists());
        assert_eq!(load_brand(&dir), Some(kit));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
