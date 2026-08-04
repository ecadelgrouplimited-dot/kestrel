//! Kestrel Motion — the structured scene language and its verification.
//!
//! Motion treats a video the way Build treats a program: not one fragile blob of
//! rendering code, but a structured project of independently editable, testable
//! scenes. This module is that project format's backbone — the versioned schema
//! (§4 of the Motion directive), the on-disk project layout (§12), and the
//! deterministic verification engine (§5-F, §17) that turns "is this video
//! broken?" into a concrete, machine-checkable list of issues the agent can
//! repair.
//!
//! Deliberately renderer-agnostic. Nothing here knows about Remotion, FFmpeg, or
//! canvas — the directive is explicit that the project format must not bind to a
//! renderer, and everything the agent needs to *plan, author, revise, and verify*
//! a video is decidable from the schema alone. The renderer consumes this; it
//! doesn't define it.
//!
//! The element model is a strongly-typed envelope (`id`, `type`, `position`,
//! `size`, `animation`) around a flexible property bag. That's a considered
//! choice: the directive lists ~30 component types and demands schema migration
//! and forward compatibility, so the universal invariants that verification
//! actually checks — unique ids, resolved references, safe areas, timing that
//! fits the scene — are typed and enforced, while the long tail of
//! component-specific props stays open for the component library to define
//! without a schema change per component.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The schema version this build writes. Bumped on a breaking schema change;
/// [`MotionProject::migrate`] carries older projects forward.
pub const SCHEMA_VERSION: &str = "1.0";

/// The kind of video, which drives sane defaults and the agent's scene choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectType {
    SketchExplainer,
    ProductTutorial,
    SocialShort,
    PresentationVideo,
}

/// The frame's aspect, which pins its canonical pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// 1080×1920 — shorts, stories, reels.
    Vertical,
    /// 1920×1080 — the classic landscape frame.
    Horizontal,
    /// 1080×1080 — feed squares.
    Square,
}

impl Format {
    /// The canonical (width, height) for this aspect.
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Format::Vertical => (1080, 1920),
            Format::Horizontal => (1920, 1080),
            Format::Square => (1080, 1080),
        }
    }
}

/// A point on the canvas, in pixels from the top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A box size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// A scene's backdrop. Kept small and typed; richer fills can be added as
/// variants without disturbing existing projects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Background {
    /// A flat colour, `#RRGGBB`.
    Solid { color: String },
    /// A linear gradient between two colours.
    Gradient {
        from: String,
        to: String,
        #[serde(default)]
        angle: f32,
    },
    /// An image asset, by project-relative path.
    Image { asset: String },
    /// Defer to the applied brand kit's background (§13). Lets a scene inherit
    /// the brand's ground without hard-coding a colour, so re-branding a project
    /// doesn't touch every scene.
    Theme,
}

impl Default for Background {
    fn default() -> Self {
        Background::Solid {
            color: "#0A0A0B".to_string(),
        }
    }
}

/// How an element enters, moves, or draws itself. `start` and `duration` are in
/// seconds, relative to the start of the element's scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Animation {
    /// The animation kind — `handwrite`, `draw`, `fade`, `slide`, … Open-ended
    /// so the component library owns the catalogue.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub start: f32,
    #[serde(default)]
    pub duration: f32,
    /// Kind-specific parameters (easing, direction, …).
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One thing on a scene — text, a sketch shape, a character, an image, a chart.
///
/// The typed envelope carries the invariants verification enforces; `extra`
/// holds everything the specific component type needs. Reference-bearing props
/// an arrow's `from`/`to`, an image's `asset` — live in `extra` and are read
/// back by name during verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: String,
    /// The component type: `text`, `sketch-arrow`, `sketch-character`, `image`,
    /// `chart`, `caption`, … Matched against the component library at render
    /// time; verification only relies on a known set of reference fields.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub position: Option<Point>,
    #[serde(default)]
    pub size: Option<Size>,
    #[serde(default)]
    pub animation: Option<Animation>,
    /// Stacking order; higher draws later (on top).
    #[serde(default)]
    pub layer: i32,
    /// Component-specific properties: `content`, `character`, `pose`, `from`,
    /// `to`, `asset`, `data`, …
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Element {
    /// A string-valued property from the open bag, if present.
    pub fn prop_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(|v| v.as_str())
    }

    /// The element's text content, for the several kinds that carry text.
    pub fn text_content(&self) -> Option<&str> {
        self.prop_str("content").or_else(|| self.prop_str("text"))
    }

    /// Whether this element renders text the viewer reads (and so must not be
    /// empty or overflow the safe area).
    pub fn is_textual(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "text" | "title" | "caption" | "callout" | "speech-bubble" | "cta"
        )
    }
}

/// One scene: a self-contained, independently editable, testable unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Seconds on screen.
    pub duration: f32,
    /// The voice-over line for this scene, if any. Present from the start so
    /// audio synchronisation (§8) has somewhere to attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narration: Option<String>,
    /// A voice-over audio clip for this scene, by project-relative path (usually
    /// under `assets/audio/`). When set, the scene can be timed to the clip and
    /// the clip is mixed into the exported MP4 (§8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub elements: Vec<Element>,
}

/// Project-level metadata (the `project` block of the schema).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub title: String,
    #[serde(rename = "type")]
    pub kind: ProjectType,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// The brand theme id to apply (§13). A bare string reference; the theme
    /// itself lives in `theme/`.
    #[serde(default)]
    pub theme: String,
}

/// A whole Motion project: schema version, metadata, and scenes.
///
/// This is the in-memory form. On disk it may be a single `motion.project.json`
/// with inline scenes (the schema example in §4) or the split layout in §12 with
/// one file per scene; [`load_project`] reconciles both into this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionProject {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub project: ProjectMeta,
    #[serde(default)]
    pub scenes: Vec<Scene>,
}

impl MotionProject {
    /// A new, empty project of the given shape, with canonical dimensions and
    /// the current schema version.
    pub fn new(title: impl Into<String>, kind: ProjectType, format: Format) -> Self {
        let (width, height) = format.dimensions();
        MotionProject {
            schema_version: SCHEMA_VERSION.to_string(),
            project: ProjectMeta {
                title: title.into(),
                kind,
                format,
                width,
                height,
                fps: 30,
                theme: String::new(),
            },
            scenes: Vec::new(),
        }
    }

    /// Total runtime across all scenes, in seconds.
    pub fn total_duration(&self) -> f32 {
        self.scenes.iter().map(|s| s.duration).sum()
    }

    /// Carry an older project forward to the current schema version.
    ///
    /// A no-op today — 1.0 is the first version — but the seam is here so that
    /// the moment the schema changes, every reader migrates on load rather than
    /// scattering version checks through the code. Returns whether anything
    /// changed.
    pub fn migrate(&mut self) -> bool {
        if self.schema_version == SCHEMA_VERSION {
            return false;
        }
        // Future: match on self.schema_version and transform in place.
        self.schema_version = SCHEMA_VERSION.to_string();
        true
    }

    /// Parse a project from the single-file JSON form, migrating if needed.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut project: MotionProject =
            serde_json::from_str(text).map_err(|e| format!("invalid motion project: {e}"))?;
        project.migrate();
        Ok(project)
    }

    /// Serialise to pretty JSON (the single-file form).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Verification — the deterministic QA the agent repairs against.
// ---------------------------------------------------------------------------

/// How serious a verification finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The project will not render correctly (or at all) as-is.
    Error,
    /// Renders, but a viewer will notice — text off-safe, timing that spills.
    Warning,
    /// Worth knowing, no action forced.
    Info,
}

impl Severity {
    pub fn glyph(self) -> &'static str {
        match self {
            Severity::Error => "✘",
            Severity::Warning => "⚠",
            Severity::Info => "ℹ",
        }
    }
}

/// One verification finding, addressed so the agent can act on exactly the
/// offending scene/element and knows what "fixed" would mean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionIssue {
    pub severity: Severity,
    /// A stable machine code, e.g. `text-overflow`, `broken-reference`.
    pub code: String,
    /// The scene this concerns, if scene-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// The element this concerns, if element-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// What's wrong, in plain language.
    pub message: String,
    /// A concrete suggestion the agent can turn into an edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// The result of verifying a project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerificationReport {
    pub issues: Vec<MotionIssue>,
}

impl VerificationReport {
    /// Whether any issue is an outright error.
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    /// (errors, warnings, infos).
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut e = 0;
        let mut w = 0;
        let mut i = 0;
        for issue in &self.issues {
            match issue.severity {
                Severity::Error => e += 1,
                Severity::Warning => w += 1,
                Severity::Info => i += 1,
            }
        }
        (e, w, i)
    }

    /// Whether the project passes — no errors. Warnings don't block a render.
    pub fn passed(&self) -> bool {
        !self.has_errors()
    }

    /// A compact report for the agent (and the CLI) to read back.
    pub fn render(&self) -> String {
        if self.issues.is_empty() {
            return "Motion verification: passed — no issues.".to_string();
        }
        let (e, w, i) = self.counts();
        let mut out = format!("Motion verification: {e} error(s), {w} warning(s), {i} info.\n");
        for issue in &self.issues {
            let loc = match (&issue.scene, &issue.element) {
                (Some(s), Some(el)) => format!(" [{s}/{el}]"),
                (Some(s), None) => format!(" [{s}]"),
                _ => String::new(),
            };
            out.push_str(&format!(
                "  {} {}{}: {}\n",
                issue.severity.glyph(),
                issue.code,
                loc,
                issue.message
            ));
            if let Some(fix) = &issue.fix {
                out.push_str(&format!("      ↳ {fix}\n"));
            }
        }
        out
    }
}

/// The fraction of the frame inside the title-safe area (margins of 5% a side).
const SAFE_MARGIN: f32 = 0.05;
/// Vertical videos reserve the bottom for platform UI; keep captions above it.
const VERTICAL_BOTTOM_RESERVE: f32 = 0.12;
/// MVP scope targets 30–90s (§16); outside that is worth flagging, not blocking.
const MIN_TARGET_SECS: f32 = 5.0;
const MAX_SCENE_SECS: f32 = 60.0;

/// Verify a project against everything decidable without rendering it.
///
/// `assets_root`, when given, is the project directory, so asset references can
/// be checked for existence on disk. Without it, asset checks are skipped (the
/// scene data is still fully checked).
pub fn verify_project(project: &MotionProject, assets_root: Option<&Path>) -> VerificationReport {
    let mut issues = Vec::new();
    let (cw, ch) = (project.project.width as f32, project.project.height as f32);

    verify_metadata(project, &mut issues);

    let mut seen_scene_ids = BTreeMap::new();
    for scene in &project.scenes {
        // Unique scene ids.
        *seen_scene_ids.entry(scene.id.clone()).or_insert(0) += 1;

        if scene.duration <= 0.0 {
            issues.push(issue(
                Severity::Error,
                "invalid-duration",
                Some(&scene.id),
                None,
                format!("scene '{}' has non-positive duration", scene.id),
                Some("give the scene a positive duration in seconds".into()),
            ));
        } else if scene.duration > MAX_SCENE_SECS {
            issues.push(issue(
                Severity::Warning,
                "long-scene",
                Some(&scene.id),
                None,
                format!(
                    "scene '{}' runs {:.0}s — long enough to lose the viewer",
                    scene.id, scene.duration
                ),
                Some("split it or tighten the narration".into()),
            ));
        }

        if scene.elements.is_empty() {
            issues.push(issue(
                Severity::Warning,
                "empty-scene",
                Some(&scene.id),
                None,
                format!(
                    "scene '{}' has no elements — it will render blank",
                    scene.id
                ),
                Some("add a title, visual, or character, or remove the scene".into()),
            ));
        }

        verify_scene_elements(project, scene, cw, ch, assets_root, &mut issues);
    }

    for (id, count) in seen_scene_ids {
        if count > 1 {
            issues.push(issue(
                Severity::Error,
                "duplicate-scene-id",
                Some(&id),
                None,
                format!("scene id '{id}' is used {count} times — ids must be unique"),
                Some("rename the duplicates".into()),
            ));
        }
    }

    if project.scenes.is_empty() {
        issues.push(issue(
            Severity::Error,
            "no-scenes",
            None,
            None,
            "project has no scenes".into(),
            Some("add at least one scene".into()),
        ));
    } else {
        let total = project.total_duration();
        if total < MIN_TARGET_SECS {
            issues.push(issue(
                Severity::Info,
                "short-video",
                None,
                None,
                format!("total runtime is {total:.1}s — very short"),
                None,
            ));
        }
    }

    VerificationReport { issues }
}

/// Metadata-level checks: schema, dimensions, frame rate.
fn verify_metadata(project: &MotionProject, issues: &mut Vec<MotionIssue>) {
    if project.schema_version != SCHEMA_VERSION {
        issues.push(issue(
            Severity::Warning,
            "schema-version",
            None,
            None,
            format!(
                "project is schema {}, this build writes {SCHEMA_VERSION}",
                project.schema_version
            ),
            Some("load and re-save to migrate it".into()),
        ));
    }

    let (want_w, want_h) = project.project.format.dimensions();
    if project.project.width != want_w || project.project.height != want_h {
        issues.push(issue(
            Severity::Warning,
            "dimension-mismatch",
            None,
            None,
            format!(
                "{:?} format is normally {want_w}×{want_h}, but this project is {}×{}",
                project.project.format, project.project.width, project.project.height
            ),
            Some("match the dimensions to the format, or the export will letterbox".into()),
        ));
    }

    if project.project.width == 0 || project.project.height == 0 {
        issues.push(issue(
            Severity::Error,
            "invalid-resolution",
            None,
            None,
            "project has a zero dimension".into(),
            Some("set width and height to positive pixels".into()),
        ));
    }

    if !(1..=120).contains(&project.project.fps) {
        issues.push(issue(
            Severity::Warning,
            "unusual-fps",
            None,
            None,
            format!(
                "frame rate {} is outside the usual 1–120",
                project.project.fps
            ),
            Some("24, 30, or 60 fps cover almost every case".into()),
        ));
    }
}

/// Element-level checks within one scene: unique ids, content, safe area,
/// animation timing, and reference resolution.
fn verify_scene_elements(
    project: &MotionProject,
    scene: &Scene,
    cw: f32,
    ch: f32,
    assets_root: Option<&Path>,
    issues: &mut Vec<MotionIssue>,
) {
    let element_ids: std::collections::BTreeSet<&str> =
        scene.elements.iter().map(|e| e.id.as_str()).collect();
    let mut seen = BTreeMap::new();

    for el in &scene.elements {
        *seen.entry(el.id.clone()).or_insert(0) += 1;

        // Textual elements must actually say something.
        if el.is_textual() {
            match el.text_content() {
                None | Some("") => issues.push(issue(
                    Severity::Error,
                    "empty-text",
                    Some(&scene.id),
                    Some(&el.id),
                    format!("{} element '{}' has no text", el.kind, el.id),
                    Some("set its `content`, or remove it".into()),
                )),
                Some(_) => {}
            }
        }

        verify_placement(scene, el, cw, ch, project.project.format, issues);
        verify_animation(scene, el, issues);
        verify_references(scene, el, &element_ids, assets_root, issues);
    }

    for (id, count) in seen {
        if count > 1 {
            issues.push(issue(
                Severity::Error,
                "duplicate-element-id",
                Some(&scene.id),
                Some(&id),
                format!(
                    "element id '{id}' appears {count} times in scene '{}'",
                    scene.id
                ),
                Some("element ids must be unique within a scene".into()),
            ));
        }
    }
}

/// Safe-area and on-canvas checks for a placed element.
fn verify_placement(
    scene: &Scene,
    el: &Element,
    cw: f32,
    ch: f32,
    format: Format,
    issues: &mut Vec<MotionIssue>,
) {
    let Some(pos) = el.position else {
        return; // Unplaced elements (e.g. full-bleed backgrounds) are fine.
    };
    let size = el.size.unwrap_or(Size {
        width: 0.0,
        height: 0.0,
    });
    let (left, top) = (pos.x, pos.y);
    let (right, bottom) = (pos.x + size.width, pos.y + size.height);

    // Entirely off-canvas is an error; partly off is a warning.
    if right < 0.0 || bottom < 0.0 || left > cw || top > ch {
        issues.push(issue(
            Severity::Error,
            "off-canvas",
            Some(&scene.id),
            Some(&el.id),
            format!(
                "element '{}' is positioned outside the {cw:.0}×{ch:.0} frame",
                el.id
            ),
            Some("move it back inside the canvas".into()),
        ));
        return;
    }
    if left < 0.0 || top < 0.0 || right > cw || bottom > ch {
        issues.push(issue(
            Severity::Warning,
            "clipped",
            Some(&scene.id),
            Some(&el.id),
            format!(
                "element '{}' extends past the frame edge and will be clipped",
                el.id
            ),
            Some("keep the whole element within the canvas".into()),
        ));
    }

    // Title-safe margins matter most for text the viewer must read.
    if el.is_textual() {
        let mx = cw * SAFE_MARGIN;
        let my = ch * SAFE_MARGIN;
        if left < mx || top < my || right > cw - mx || bottom > ch - my {
            issues.push(issue(
                Severity::Warning,
                "outside-safe-area",
                Some(&scene.id),
                Some(&el.id),
                format!(
                    "text '{}' sits in the {:.0}% edge margin",
                    el.id,
                    SAFE_MARGIN * 100.0
                ),
                Some("pull it toward the centre so it isn't cropped on some players".into()),
            ));
        }

        // On vertical, captions crowd into the bottom UI reserve.
        if format == Format::Vertical && el.kind == "caption" {
            let reserve_top = ch * (1.0 - VERTICAL_BOTTOM_RESERVE);
            if bottom > reserve_top {
                issues.push(issue(
                    Severity::Warning,
                    "caption-in-ui-zone",
                    Some(&scene.id),
                    Some(&el.id),
                    format!(
                        "caption '{}' reaches into the bottom {:.0}% where the platform UI sits",
                        el.id,
                        VERTICAL_BOTTOM_RESERVE * 100.0
                    ),
                    Some("raise captions above the bottom UI zone".into()),
                ));
            }
        }
    }
}

/// Animation timing must start at or after zero and finish within the scene.
fn verify_animation(scene: &Scene, el: &Element, issues: &mut Vec<MotionIssue>) {
    let Some(anim) = &el.animation else {
        return;
    };
    if anim.start < 0.0 {
        issues.push(issue(
            Severity::Warning,
            "negative-start",
            Some(&scene.id),
            Some(&el.id),
            format!(
                "animation on '{}' starts before the scene ({:.2}s)",
                el.id, anim.start
            ),
            Some("set the animation start to 0 or later".into()),
        ));
    }
    if anim.duration < 0.0 {
        issues.push(issue(
            Severity::Error,
            "negative-duration",
            Some(&scene.id),
            Some(&el.id),
            format!("animation on '{}' has negative duration", el.id),
            Some("give the animation a non-negative duration".into()),
        ));
    }
    if anim.start + anim.duration > scene.duration + f32::EPSILON {
        issues.push(issue(
            Severity::Warning,
            "animation-overflows-scene",
            Some(&scene.id),
            Some(&el.id),
            format!(
                "animation on '{}' ends at {:.2}s but scene '{}' is only {:.2}s",
                el.id,
                anim.start + anim.duration,
                scene.id,
                scene.duration
            ),
            Some("shorten the animation or lengthen the scene".into()),
        ));
    }
}

/// Reference-bearing props must resolve: `from`/`to` to sibling element ids,
/// `asset` to a file on disk (when a project root is available).
fn verify_references(
    scene: &Scene,
    el: &Element,
    sibling_ids: &std::collections::BTreeSet<&str>,
    assets_root: Option<&Path>,
    issues: &mut Vec<MotionIssue>,
) {
    for key in ["from", "to", "target"] {
        if let Some(reference) = el.prop_str(key) {
            if !sibling_ids.contains(reference) {
                issues.push(issue(
                    Severity::Error,
                    "broken-reference",
                    Some(&scene.id),
                    Some(&el.id),
                    format!(
                        "element '{}' refers to '{reference}' via `{key}`, which is not in scene '{}'",
                        el.id, scene.id
                    ),
                    Some(format!("point `{key}` at an element id that exists in this scene")),
                ));
            }
        }
    }

    if let Some(asset) = el.prop_str("asset") {
        if let Some(root) = assets_root {
            let path = root.join(asset);
            if !path.exists() {
                issues.push(issue(
                    Severity::Error,
                    "missing-asset",
                    Some(&scene.id),
                    Some(&el.id),
                    format!(
                        "element '{}' references asset '{asset}', which is missing",
                        el.id
                    ),
                    Some("add the asset under assets/, or fix the path".into()),
                ));
            }
        }
    }
}

/// Build one issue. A small helper so the checks above read as intent, not
/// struct literals.
#[allow(clippy::too_many_arguments)]
fn issue(
    severity: Severity,
    code: &str,
    scene: Option<&str>,
    element: Option<&str>,
    message: String,
    fix: Option<String>,
) -> MotionIssue {
    MotionIssue {
        severity,
        code: code.to_string(),
        scene: scene.map(str::to_string),
        element: element.map(str::to_string),
        message,
        fix,
    }
}

// ---------------------------------------------------------------------------
// Project on disk — the §12 layout.
// ---------------------------------------------------------------------------

/// The single-file project descriptor within a project directory.
pub fn project_file(root: &Path) -> PathBuf {
    root.join("motion.project.json")
}

/// The directories a Motion project is organised into (§12): scripts, scenes,
/// assets, theme, captions, verification, and output stay logically separate.
const PROJECT_DIRS: &[&str] = &[
    "script",
    "storyboard",
    "scenes",
    "components",
    "assets/images",
    "assets/icons",
    "assets/characters",
    "assets/screenshots",
    "assets/video",
    "assets/audio",
    "theme",
    "captions",
    "verification",
    "output",
];

/// Scaffold a new Motion project directory: the §12 folder tree plus a starter
/// `motion.project.json` and a brief. Idempotent — creating over an existing
/// tree just ensures the directories exist and won't clobber a saved project.
pub fn create_project(
    root: &Path,
    title: &str,
    kind: ProjectType,
    format: Format,
) -> std::io::Result<MotionProject> {
    for dir in PROJECT_DIRS {
        std::fs::create_dir_all(root.join(dir))?;
    }
    let project = MotionProject::new(title, kind, format);
    // Don't overwrite an existing project file if the caller re-scaffolds.
    let file = project_file(root);
    if !file.exists() {
        save_project(root, &project)?;
        let brief = root.join("script").join("brief.md");
        if !brief.exists() {
            std::fs::write(
                brief,
                format!("# {title}\n\n_Describe the video's goal, audience, and key message._\n"),
            )?;
        }
    }
    Ok(project)
}

/// Persist a project as the single-file `motion.project.json` with inline
/// scenes. (The split one-file-per-scene layout is a valid *input*; we write the
/// consolidated form, which is simpler to keep consistent.)
pub fn save_project(root: &Path, project: &MotionProject) -> std::io::Result<()> {
    let file = project_file(root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, project.to_json())
}

/// Load a project, reconciling both on-disk forms.
///
/// If `motion.project.json` carries inline scenes, those win. If it has none but
/// a `scenes/` directory exists, every `scene-*.json` there is loaded in name
/// order — the split layout from §12. Migrates the schema version on the way in.
pub fn load_project(root: &Path) -> Result<MotionProject, String> {
    let text = std::fs::read_to_string(project_file(root))
        .map_err(|e| format!("could not read motion.project.json: {e}"))?;
    let mut project = MotionProject::from_json(&text)?;

    if project.scenes.is_empty() {
        let scenes_dir = root.join("scenes");
        if scenes_dir.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&scenes_dir)
                .map_err(|e| format!("could not read scenes/: {e}"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            files.sort();
            for path in files {
                let scene_text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("could not read {}: {e}", path.display()))?;
                let scene: Scene = serde_json::from_str(&scene_text)
                    .map_err(|e| format!("invalid scene {}: {e}", path.display()))?;
                project.scenes.push(scene);
            }
        }
    }
    Ok(project)
}

/// Verify the project on disk and write the report to `verification/latest-report.json`
/// (§12), returning it. This is the shape the agent's repair loop reads back.
pub fn verify_on_disk(root: &Path) -> Result<VerificationReport, String> {
    let project = load_project(root)?;
    let report = verify_project(&project, Some(root));
    let dir = root.join("verification");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(dir.join("latest-report.json"), text);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact schema example from the directive (§4) must parse, round-trip,
    /// and — being well-formed — verify with no errors.
    fn directive_example() -> MotionProject {
        let json = r##"{
          "schemaVersion": "1.0",
          "project": {
            "title": "The Missing Stock",
            "type": "sketch-explainer",
            "format": "vertical",
            "width": 1080,
            "height": 1920,
            "fps": 30,
            "theme": "sbb-sketch"
          },
          "scenes": [
            {
              "id": "scene-01",
              "name": "The Hook",
              "duration": 6,
              "narration": "Your business may be losing stock without you noticing.",
              "background": { "type": "solid", "color": "#F7F4EC" },
              "elements": [
                {
                  "id": "title-01",
                  "type": "text",
                  "content": "Where did the stock go?",
                  "position": { "x": 540, "y": 280 },
                  "size": { "width": 420, "height": 120 },
                  "animation": { "type": "handwrite", "start": 0.5, "duration": 1.2 }
                },
                {
                  "id": "character-01",
                  "type": "sketch-character",
                  "character": "shop-owner",
                  "pose": "confused",
                  "position": { "x": 350, "y": 1050 }
                },
                {
                  "id": "empty-shelf-01",
                  "type": "image",
                  "position": { "x": 700, "y": 1050 }
                },
                {
                  "id": "arrow-01",
                  "type": "sketch-arrow",
                  "from": "character-01",
                  "to": "empty-shelf-01",
                  "animation": { "type": "draw", "start": 2, "duration": 0.8 }
                }
              ]
            }
          ]
        }"##;
        MotionProject::from_json(json).expect("directive schema example must parse")
    }

    #[test]
    fn directive_example_parses_and_round_trips() {
        let project = directive_example();
        assert_eq!(project.schema_version, "1.0");
        assert_eq!(project.project.kind, ProjectType::SketchExplainer);
        assert_eq!(project.project.format, Format::Vertical);
        assert_eq!(project.scenes.len(), 1);
        assert_eq!(project.scenes[0].elements.len(), 4);

        // The open property bag preserved the component-specific fields.
        let character = &project.scenes[0].elements[1];
        assert_eq!(character.prop_str("character"), Some("shop-owner"));
        assert_eq!(character.prop_str("pose"), Some("confused"));

        // Round-trip through JSON and back is lossless.
        let reparsed = MotionProject::from_json(&project.to_json()).unwrap();
        assert_eq!(project, reparsed);
    }

    #[test]
    fn a_well_formed_project_verifies_clean() {
        let project = directive_example();
        // No assets_root, so the image reference isn't checked for existence —
        // everything else about this scene is valid.
        let report = verify_project(&project, None);
        assert!(report.passed(), "unexpected: {}", report.render());
        assert!(!report.has_errors());
    }

    #[test]
    fn format_pins_canonical_dimensions() {
        assert_eq!(Format::Vertical.dimensions(), (1080, 1920));
        assert_eq!(Format::Horizontal.dimensions(), (1920, 1080));
        assert_eq!(Format::Square.dimensions(), (1080, 1080));
        let p = MotionProject::new("t", ProjectType::SocialShort, Format::Horizontal);
        assert_eq!((p.project.width, p.project.height), (1920, 1080));
    }

    #[test]
    fn catches_empty_text_and_duplicate_ids() {
        let mut project = MotionProject::new("t", ProjectType::SketchExplainer, Format::Vertical);
        project.scenes.push(Scene {
            id: "s1".into(),
            name: "one".into(),
            duration: 5.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![
                Element {
                    id: "t1".into(),
                    kind: "text".into(),
                    position: Some(Point { x: 200.0, y: 900.0 }),
                    size: Some(Size {
                        width: 400.0,
                        height: 100.0,
                    }),
                    animation: None,
                    layer: 0,
                    extra: BTreeMap::new(), // no content -> empty-text error
                },
                Element {
                    id: "t1".into(), // duplicate id
                    kind: "image".into(),
                    position: Some(Point { x: 100.0, y: 100.0 }),
                    size: None,
                    animation: None,
                    layer: 0,
                    extra: BTreeMap::new(),
                },
            ],
        });
        let report = verify_project(&project, None);
        assert!(report.has_errors());
        let codes: Vec<&str> = report.issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"empty-text"), "{codes:?}");
        assert!(codes.contains(&"duplicate-element-id"), "{codes:?}");
    }

    #[test]
    fn catches_broken_arrow_reference() {
        let mut project = MotionProject::new("t", ProjectType::SketchExplainer, Format::Vertical);
        let mut arrow_props = BTreeMap::new();
        arrow_props.insert("from".to_string(), serde_json::json!("here"));
        arrow_props.insert("to".to_string(), serde_json::json!("nowhere"));
        project.scenes.push(Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 5.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![
                Element {
                    id: "here".into(),
                    kind: "sketch-character".into(),
                    position: Some(Point { x: 300.0, y: 900.0 }),
                    size: None,
                    animation: None,
                    layer: 0,
                    extra: BTreeMap::new(),
                },
                Element {
                    id: "arrow".into(),
                    kind: "sketch-arrow".into(),
                    position: None,
                    size: None,
                    animation: None,
                    layer: 0,
                    extra: arrow_props,
                },
            ],
        });
        let report = verify_project(&project, None);
        let broken: Vec<&MotionIssue> = report
            .issues
            .iter()
            .filter(|i| i.code == "broken-reference")
            .collect();
        // `from: here` resolves, `to: nowhere` does not — exactly one break.
        assert_eq!(broken.len(), 1, "{}", report.render());
        assert_eq!(broken[0].element.as_deref(), Some("arrow"));
    }

    #[test]
    fn catches_animation_overflowing_its_scene() {
        let mut project = MotionProject::new("t", ProjectType::SketchExplainer, Format::Horizontal);
        project.scenes.push(Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 2.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![Element {
                id: "t".into(),
                kind: "text".into(),
                position: Some(Point { x: 400.0, y: 400.0 }),
                size: Some(Size {
                    width: 300.0,
                    height: 80.0,
                }),
                animation: Some(Animation {
                    kind: "fade".into(),
                    start: 1.5,
                    duration: 2.0, // ends at 3.5s in a 2s scene
                    extra: BTreeMap::new(),
                }),
                layer: 0,
                extra: {
                    let mut m = BTreeMap::new();
                    m.insert("content".into(), serde_json::json!("hi"));
                    m
                },
            }],
        });
        let report = verify_project(&project, None);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "animation-overflows-scene"));
    }

    #[test]
    fn flags_text_outside_the_safe_area() {
        let mut project = MotionProject::new("t", ProjectType::SocialShort, Format::Vertical);
        project.scenes.push(Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 5.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![Element {
                id: "t".into(),
                kind: "title".into(),
                position: Some(Point { x: 10.0, y: 10.0 }), // hard against the corner
                size: Some(Size {
                    width: 200.0,
                    height: 80.0,
                }),
                animation: None,
                layer: 0,
                extra: {
                    let mut m = BTreeMap::new();
                    m.insert("content".into(), serde_json::json!("Hook"));
                    m
                },
            }],
        });
        let report = verify_project(&project, None);
        assert!(report.issues.iter().any(|i| i.code == "outside-safe-area"));
    }

    #[test]
    fn scaffolds_and_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("kestrel-motion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let project =
            create_project(&dir, "Demo", ProjectType::SketchExplainer, Format::Vertical).unwrap();
        assert!(dir.join("motion.project.json").exists());
        assert!(dir.join("scenes").is_dir());
        assert!(dir.join("assets/characters").is_dir());
        assert!(dir.join("script/brief.md").exists());

        // Add a scene, save, reload.
        let mut project = project;
        project.scenes.push(Scene {
            id: "scene-01".into(),
            name: "Hook".into(),
            duration: 6.0,
            narration: Some("hi".into()),
            audio: None,
            background: Background::default(),
            elements: vec![Element {
                id: "title".into(),
                kind: "title".into(),
                position: Some(Point { x: 300.0, y: 800.0 }),
                size: Some(Size {
                    width: 480.0,
                    height: 160.0,
                }),
                animation: None,
                layer: 0,
                extra: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "content".into(),
                        serde_json::json!("Where did the stock go?"),
                    );
                    m
                },
            }],
        });
        save_project(&dir, &project).unwrap();
        let loaded = load_project(&dir).unwrap();
        assert_eq!(loaded.scenes.len(), 1);
        assert_eq!(loaded.scenes[0].id, "scene-01");

        // verify_on_disk writes the report where §12 says it lives.
        let report = verify_on_disk(&dir).unwrap();
        assert!(report.passed(), "{}", report.render());
        assert!(dir.join("verification/latest-report.json").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_the_split_scene_layout() {
        let dir = std::env::temp_dir().join(format!("kestrel-motion-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_project(
            &dir,
            "Split",
            ProjectType::PresentationVideo,
            Format::Horizontal,
        )
        .unwrap();

        // Write scenes as separate files (the §12 form) and clear the inline list.
        for (i, name) in ["Intro", "Body"].iter().enumerate() {
            let scene = Scene {
                id: format!("scene-{:02}", i + 1),
                name: name.to_string(),
                duration: 4.0,
                narration: None,
                audio: None,
                background: Background::default(),
                elements: Vec::new(),
            };
            std::fs::write(
                dir.join("scenes").join(format!("scene-{:02}.json", i + 1)),
                serde_json::to_string_pretty(&scene).unwrap(),
            )
            .unwrap();
        }

        let loaded = load_project(&dir).unwrap();
        assert_eq!(loaded.scenes.len(), 2);
        assert_eq!(loaded.scenes[0].id, "scene-01");
        assert_eq!(loaded.scenes[1].name, "Body");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
