//! Kestrel Motion — rendering, behind an adapter.
//!
//! The directive is emphatic on two points (§5-E): the project format must not
//! bind to any one renderer, and the first implementation should stay light. So
//! rendering lives behind [`Renderer`], and the first implementation,
//! [`SvgRenderer`], is pure Rust with zero dependencies — it turns the scene
//! schema into SVG, and a whole project into a self-contained HTML preview that
//! plays in any browser Kestrel already knows how to open.
//!
//! Why SVG rather than Remotion or a canvas framework: it's resolution-
//! independent (which §6 wants for sketch art), it animates deterministically in
//! the browser via baked CSS keyframes — the same project renders identically
//! every time, as §15 demands — and it adds not one line to the dependency tree
//! on a machine where disk is tight. MP4 export ([`export_mp4`]) rides the same
//! trait: it screenshots a settled still per caption segment with the headless
//! browser and stitches them with FFmpeg — a real H.264/AAC file using only
//! tools already present. In-scene entry motion stays in the live preview until
//! a per-frame rasteriser (gated behind the directive's tooling review) replaces
//! the still capture; that's a backend swap, not a format change.
//!
//! The coordinate model matches the verifier exactly: `position` is the
//! element's top-left in pixels, `size` its extent. One model, so what verifies
//! clean is what renders.

use crate::motion::{Animation, Background, Element, MotionProject, Scene};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A rendering backend. Swappable per §5-E — today SVG, tomorrow whatever a
/// licensing and performance review settles on, without touching the schema.
pub trait Renderer {
    /// A human name for the backend, for logs and reports.
    fn name(&self) -> &'static str;

    /// One scene as a standalone, animated SVG document.
    fn render_scene_svg(&self, project: &MotionProject, scene: &Scene) -> String;

    /// The whole project as a self-contained HTML preview player.
    fn render_preview_html(&self, project: &MotionProject) -> String;

    /// Encode the project to a video file. Returns the path on success, or an
    /// explanation when the backend can't (yet) encode.
    fn export(
        &self,
        project: &MotionProject,
        root: &Path,
        opts: &ExportOptions,
    ) -> Result<PathBuf, String>;
}

/// Encode settings for a final render.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Output file name within the project's `output/` directory.
    pub file_name: String,
    /// Frames per second for the encode (falls back to the project's fps).
    pub fps: Option<u32>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            file_name: "final-video.mp4".to_string(),
            fps: None,
        }
    }
}

/// The pure-Rust SVG renderer.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvgRenderer;

impl Renderer for SvgRenderer {
    fn name(&self) -> &'static str {
        "svg"
    }

    fn render_scene_svg(&self, project: &MotionProject, scene: &Scene) -> String {
        render_scene(project, scene, false, None)
    }

    fn render_preview_html(&self, project: &MotionProject) -> String {
        preview_html(
            project,
            &crate::motion_caption::CaptionTrack::default(),
            None,
        )
    }

    fn export(
        &self,
        project: &MotionProject,
        root: &Path,
        opts: &ExportOptions,
    ) -> Result<PathBuf, String> {
        export_mp4(project, root, opts)
    }
}

/// A timeline segment for export: a still frame held for `duration` seconds,
/// with the caption to burn into it.
struct Segment<'a> {
    scene: &'a Scene,
    caption: Option<String>,
    duration: f32,
}

/// Split the project into export segments. A narrated scene's caption cues
/// become one segment each (so captions change on time in the MP4); a scene
/// with no cues is a single captionless segment for its whole duration.
fn export_segments<'a>(
    project: &'a MotionProject,
    captions: &crate::motion_caption::CaptionTrack,
) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut clock = 0.0f32;
    for scene in &project.scenes {
        let (start, end) = (clock, clock + scene.duration);
        clock = end;
        let cues: Vec<&crate::motion_caption::Caption> = captions
            .cues
            .iter()
            .filter(|c| c.start >= start - 1e-3 && c.start < end - 1e-3)
            .collect();
        if cues.is_empty() {
            segments.push(Segment {
                scene,
                caption: None,
                duration: scene.duration.max(0.1),
            });
        } else {
            // Cover from the scene start through each cue, and any tail after the
            // last cue, so the segment durations sum to the scene duration.
            let mut cursor = start;
            for (i, cue) in cues.iter().enumerate() {
                // A gap before the first cue shows the frame without a caption.
                if i == 0 && cue.start > cursor + 1e-3 {
                    segments.push(Segment {
                        scene,
                        caption: None,
                        duration: cue.start - cursor,
                    });
                    cursor = cue.start;
                }
                let seg_end = if i == cues.len() - 1 {
                    end
                } else {
                    cues[i + 1].start
                };
                segments.push(Segment {
                    scene,
                    caption: Some(cue.text.replace('\n', " ")),
                    duration: (seg_end - cursor).max(0.1),
                });
                cursor = seg_end;
            }
        }
    }
    segments
}

/// Encode the project to an MP4 by screenshotting a still per caption segment
/// and stitching them with FFmpeg at the project's frame rate. Requires a
/// headless browser (already used for acceptance checks) and FFmpeg on PATH;
/// both are reported clearly if missing.
///
/// The stills are settled frames (entry animations resolved), so the MP4 is a
/// correctly-timed, branded, captioned cut of the storyboard. In-scene motion
/// plays in the live HTML preview; bringing it into the encode is the job of a
/// per-frame rasteriser, gated behind the directive's tooling review.
pub fn export_mp4(
    project: &MotionProject,
    root: &Path,
    opts: &ExportOptions,
) -> Result<PathBuf, String> {
    if project.scenes.is_empty() {
        return Err("nothing to export — the project has no scenes".into());
    }
    let browser = crate::browser::find_browser()
        .ok_or("no Chrome/Edge found to render frames for the export")?;
    let ffmpeg = find_ffmpeg().ok_or(
        "FFmpeg isn't installed or on PATH — it encodes the frames into an MP4. Install it \
         (e.g. `winget install Gyan.FFmpeg`) and try again.",
    )?;

    let (w, h) = (project.project.width, project.project.height);
    let brand = crate::motion_brand::load_brand(root);
    let captions = crate::motion_caption::load_captions(root);
    let segments = export_segments(project, &captions);

    let out_dir = root.join("output");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("could not create output/: {e}"))?;
    // Frame files live in output/ so the SVG's `../asset` paths resolve exactly
    // as they do for the preview. They're cleaned up at the end.
    let mut frame_files = Vec::new();
    let mut concat = String::from("ffconcat version 1.0\n");
    for (i, seg) in segments.iter().enumerate() {
        let svg = render_scene_inner(
            project,
            seg.scene,
            false,
            brand.as_ref(),
            true,
            seg.caption.as_deref(),
        );
        let html = format!(
            "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;padding:0;background:#000}}svg{{display:block}}</style>{svg}"
        );
        let html_path = out_dir.join(format!(".mframe-{i:03}.html"));
        let png_name = format!(".mframe-{i:03}.png");
        let png_path = out_dir.join(&png_name);
        std::fs::write(&html_path, html).map_err(|e| format!("frame write failed: {e}"))?;
        screenshot_frame(&browser, &html_path, &png_path, w, h)?;
        frame_files.push(html_path);
        frame_files.push(png_path);
        concat.push_str(&format!(
            "file '{png_name}'\nduration {:.3}\n",
            seg.duration
        ));
    }
    // The concat demuxer ignores the last entry's duration, so repeat the final
    // frame to hold it for its full time.
    if let Some(last) = segments.len().checked_sub(1) {
        concat.push_str(&format!("file '.mframe-{last:03}.png'\n"));
    }
    let list_path = out_dir.join(".mframes.txt");
    std::fs::write(&list_path, concat).map_err(|e| format!("concat list write failed: {e}"))?;

    let fps = opts.fps.unwrap_or(project.project.fps).clamp(1, 60);
    let out_name = if opts.file_name.trim().is_empty() {
        "final-video.mp4".to_string()
    } else {
        opts.file_name.clone()
    };
    let out_path = out_dir.join(&out_name);

    // Silent stereo AAC track so the file carries audio, per §14 (real narration
    // muxing comes with the audio phase).
    let status = std::process::Command::new(&ffmpeg)
        .current_dir(&out_dir)
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            ".mframes.txt",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=stereo:sample_rate=44100",
            "-vf",
            &format!("fps={fps},format=yuv420p"),
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-c:a",
            "aac",
            "-shortest",
            "-movflags",
            "+faststart",
            // Pin the exact runtime: the concat demuxer's trailing repeated
            // frame otherwise overhangs the intended length.
            "-t",
            &format!("{:.3}", project.total_duration().max(0.1)),
        ])
        .arg(&out_name)
        .output();

    // Clean up frames regardless of outcome.
    for f in &frame_files {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(&list_path);

    match status {
        Ok(out) if out.status.success() && out_path.exists() => Ok(out_path),
        Ok(out) => Err(format!(
            "FFmpeg failed to encode the video: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        )),
        Err(e) => Err(format!("could not run FFmpeg: {e}")),
    }
}

/// Locate FFmpeg: on PATH, or the WinGet shim directory it commonly installs to.
fn find_ffmpeg() -> Option<String> {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("ffmpeg".to_string());
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let shim = format!(r"{local}\Microsoft\WinGet\Links\ffmpeg.exe");
        if Path::new(&shim).exists() {
            return Some(shim);
        }
    }
    None
}

/// Screenshot one frame HTML to a PNG at exactly `w`×`h` via headless Chrome.
fn screenshot_frame(browser: &str, html: &Path, png: &Path, w: u32, h: u32) -> Result<(), String> {
    let profile = std::env::temp_dir().join(format!(
        "kestrel-mframe-{}-{}",
        std::process::id(),
        png.file_name().and_then(|n| n.to_str()).unwrap_or("f")
    ));
    let url = format!("file:///{}", html.display().to_string().replace('\\', "/"));
    let out = std::process::Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            "--disable-extensions",
            "--virtual-time-budget=1200",
        ])
        .arg(format!("--window-size={w},{h}"))
        .arg(format!("--screenshot={}", png.display()))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(&url)
        .output()
        .map_err(|e| format!("could not run the browser for a frame: {e}"))?;
    let _ = std::fs::remove_dir_all(&profile);
    if png.exists() {
        Ok(())
    } else {
        Err(format!(
            "the browser did not produce a frame image: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Write the preview player for a project to `output/preview.html` and return
/// its path. This is what the agent's `preview_motion` tool calls.
///
/// If the project has a caption track on disk (`captions/captions.json`), it is
/// overlaid live in the player — captions stay editable data, never baked into
/// the scene SVG (§9).
pub fn write_preview(root: &Path, project: &MotionProject) -> std::io::Result<PathBuf> {
    let dir = root.join("output");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("preview.html");
    let captions = crate::motion_caption::load_captions(root);
    let brand = crate::motion_brand::load_brand(root);
    std::fs::write(&path, preview_html(project, &captions, brand.as_ref()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// SVG generation.
// ---------------------------------------------------------------------------

/// Render one scene to an SVG document. When `overlay_safe_area` is set, the
/// title-safe margin is drawn as a dashed guide (used by the preview toggle).
/// `brand`, when present, supplies the font, default text colour, themed
/// background, and watermark (§13).
fn render_scene(
    project: &MotionProject,
    scene: &Scene,
    overlay_safe_area: bool,
    brand: Option<&crate::motion_brand::BrandKit>,
) -> String {
    render_scene_inner(project, scene, overlay_safe_area, brand, false, None)
}

/// The full scene renderer. `static_frame` drops the entry animations so every
/// element shows at its settled state (for a still frame the export screenshots);
/// `caption`, when set, burns a caption band into the frame — only ever for
/// export, never into the editable project (§9).
fn render_scene_inner(
    project: &MotionProject,
    scene: &Scene,
    overlay_safe_area: bool,
    brand: Option<&crate::motion_brand::BrandKit>,
    static_frame: bool,
    caption: Option<&str>,
) -> String {
    let (w, h) = (project.project.width, project.project.height);
    let font = brand
        .map(|b| b.font_family.as_str())
        .unwrap_or("Segoe UI, Arial, sans-serif");
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" preserveAspectRatio="xMidYMid meet" font-family="{}">"##,
        esc_attr(font)
    );

    // A reusable arrowhead marker for sketch arrows.
    svg.push_str(
        r##"<defs><marker id="arrowhead" markerWidth="12" markerHeight="12" refX="9" refY="4" orient="auto"><path d="M0,0 L10,4 L0,8 z" fill="#333"/></marker></defs>"##,
    );

    render_background(&mut svg, &scene.background, w, h, brand);

    // Elements draw in `layer` order, stable within equal layers.
    let mut ordered: Vec<&Element> = scene.elements.iter().collect();
    ordered.sort_by_key(|e| e.layer);
    for el in ordered {
        render_element(&mut svg, el, scene, brand, static_frame);
    }

    if let Some(text) = caption {
        render_caption_band(&mut svg, text, w, h, project.project.format, brand);
    }

    if let Some(mark) = brand.and_then(|b| b.watermark.as_deref()) {
        // A small, unobtrusive watermark in the bottom-right, inside the safe
        // margin (§13).
        let _ = write!(
            svg,
            r##"<text x="{:.0}" y="{:.0}" font-size="{:.0}" fill="{}" opacity="0.5" text-anchor="end">{}</text>"##,
            w as f32 * 0.95,
            h as f32 * 0.965,
            (w as f32 * 0.022).clamp(16.0, 40.0),
            esc_attr(brand.map(|b| b.text.as_str()).unwrap_or("#888")),
            esc_text(mark)
        );
    }

    if overlay_safe_area {
        let (mx, my) = (w as f32 * 0.05, h as f32 * 0.05);
        let _ = write!(
            svg,
            r##"<rect x="{mx}" y="{my}" width="{}" height="{}" fill="none" stroke="#e0403f" stroke-width="2" stroke-dasharray="12 10" opacity="0.6"/>"##,
            w as f32 - 2.0 * mx,
            h as f32 - 2.0 * my
        );
    }

    svg.push_str("</svg>");
    svg
}

/// Draw the scene backdrop.
fn render_background(
    svg: &mut String,
    bg: &Background,
    w: u32,
    h: u32,
    brand: Option<&crate::motion_brand::BrandKit>,
) {
    match bg {
        Background::Solid { color } => {
            let _ = write!(
                svg,
                r##"<rect x="0" y="0" width="{w}" height="{h}" fill="{}"/>"##,
                esc_attr(color)
            );
        }
        Background::Theme => {
            // Defer to the brand's ground; fall back to a neutral dark if no
            // kit is applied.
            let color = brand.map(|b| b.background.as_str()).unwrap_or("#0A0A0B");
            let _ = write!(
                svg,
                r##"<rect x="0" y="0" width="{w}" height="{h}" fill="{}"/>"##,
                esc_attr(color)
            );
        }
        Background::Gradient { from, to, angle } => {
            // Map the angle to gradient endpoints on the unit square.
            let rad = angle.to_radians();
            let (dx, dy) = (rad.cos(), rad.sin());
            let (x1, y1) = (0.5 - dx * 0.5, 0.5 - dy * 0.5);
            let (x2, y2) = (0.5 + dx * 0.5, 0.5 + dy * 0.5);
            let _ = write!(
                svg,
                r##"<defs><linearGradient id="bg" x1="{x1:.3}" y1="{y1:.3}" x2="{x2:.3}" y2="{y2:.3}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs><rect x="0" y="0" width="{w}" height="{h}" fill="url(#bg)"/>"##,
                esc_attr(from),
                esc_attr(to)
            );
        }
        Background::Image { asset } => {
            // Assets are referenced relative to the project; the preview is
            // written into output/, so step back up one level to reach them.
            let _ = write!(
                svg,
                r##"<image href="../{}" x="0" y="0" width="{w}" height="{h}" preserveAspectRatio="xMidYMid slice"/>"##,
                esc_attr(asset)
            );
        }
    }
}

/// Burn a caption band into a still frame (export only). Centred, with the
/// brand caption colours, sitting above the bottom UI reserve on vertical.
fn render_caption_band(
    svg: &mut String,
    text: &str,
    w: u32,
    h: u32,
    format: crate::motion::Format,
    brand: Option<&crate::motion_brand::BrandKit>,
) {
    if text.trim().is_empty() {
        return;
    }
    let (w, h) = (w as f32, h as f32);
    let bottom_reserve = if format == crate::motion::Format::Vertical {
        0.13
    } else {
        0.07
    };
    let font = (w * 0.032).clamp(20.0, 56.0);
    // A rough pill sized to the text; SVG can't measure, so estimate width from
    // the glyph count and centre it, clamped to the safe width.
    let est_w = (text.chars().count() as f32 * font * 0.55 + font).min(w * 0.9);
    let cx = w / 2.0;
    let baseline = h * (1.0 - bottom_reserve);
    let pad = font * 0.35;
    let bg = brand
        .map(|b| b.caption_background.as_str())
        .unwrap_or("rgba(0,0,0,0.62)");
    let fg = brand.map(|b| b.caption_text.as_str()).unwrap_or("#ffffff");
    let _ = write!(
        svg,
        r##"<rect x="{:.0}" y="{:.0}" width="{:.0}" height="{:.0}" rx="{:.0}" fill="{}"/><text x="{cx:.0}" y="{:.0}" font-size="{font:.0}" font-weight="600" fill="{}" text-anchor="middle">{}</text>"##,
        cx - est_w / 2.0,
        baseline - font - pad,
        est_w,
        font + pad * 2.0,
        font * 0.25,
        esc_attr(bg),
        baseline - pad,
        esc_attr(fg),
        esc_text(text)
    );
}

/// Draw one element, wrapped in a group carrying its entry animation. When
/// `static_frame` is set the animation is dropped, so the element shows settled.
fn render_element(
    svg: &mut String,
    el: &Element,
    scene: &Scene,
    brand: Option<&crate::motion_brand::BrandKit>,
    static_frame: bool,
) {
    let anim_style = if static_frame {
        String::new()
    } else {
        animation_style(el.animation.as_ref())
    };
    let _ = write!(svg, r##"<g style="{anim_style}">"##);

    match el.kind.as_str() {
        "text" | "title" | "caption" | "cta" | "callout" => render_text(svg, el, brand),
        "image" | "screenshot" => render_image(svg, el),
        "sketch-arrow" | "arrow" | "connector" => render_arrow(svg, el, scene),
        "sketch-character" | "character" => render_character(svg, el),
        "rect" | "rectangle" | "highlight" => render_rect(svg, el),
        "circle" | "ellipse" => render_circle(svg, el),
        "line" | "underline" => render_line(svg, el),
        // Anything else still renders as a labelled placeholder so a
        // verification-driven iteration can see it exists and where it sits.
        _ => render_placeholder(svg, el),
    }

    svg.push_str("</g>");
}

fn render_text(svg: &mut String, el: &Element, brand: Option<&crate::motion_brand::BrandKit>) {
    let content = el.text_content().unwrap_or("");
    let pos = el.position.unwrap_or_default_point();
    let size = el.size;
    // Font size: an explicit prop wins. Otherwise derive from the box height,
    // then clamp so the text also fits the box WIDTH — a tall, short box would
    // otherwise size a long headline far past its own right edge (and past the
    // safe area the verifier assumes it respects). ~0.55·fontSize is a decent
    // average glyph advance for a proportional face.
    let explicit = el.extra.get("fontSize").and_then(|v| v.as_f64());
    let font = match explicit {
        Some(f) => f as f32,
        None => {
            let by_height = size.map(|s| s.height * 0.7).unwrap_or(64.0);
            let by_width = size
                .filter(|_| !content.is_empty())
                .map(|s| s.width / (0.55 * content.chars().count().max(1) as f32));
            by_width
                .map(|w| by_height.min(w))
                .unwrap_or(by_height)
                .clamp(16.0, 400.0)
        }
    };
    // An explicit colour on the element wins; otherwise the brand supplies a
    // default for this kind (CTA → accent, else body text). Without a kit, a
    // dark ink keeps text legible on the default light scaffolding.
    let brand_default = brand
        .map(|b| b.text_color_for(&el.kind))
        .unwrap_or("#111111");
    let fill = el.prop_str("color").unwrap_or(brand_default);
    let weight = if matches!(el.kind.as_str(), "title" | "cta") {
        "700"
    } else {
        "400"
    };
    // Baseline sits one font-size below the top-left, so the box top aligns.
    let _ = write!(
        svg,
        r##"<text x="{:.1}" y="{:.1}" font-size="{font:.0}" font-weight="{weight}" fill="{}">{}</text>"##,
        pos.x,
        pos.y + font,
        esc_attr(fill),
        esc_text(content)
    );
}

fn render_image(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((320.0, 320.0));
    if let Some(asset) = el.prop_str("asset") {
        let _ = write!(
            svg,
            r##"<image href="../{}" x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" preserveAspectRatio="xMidYMid meet"/>"##,
            esc_attr(asset),
            pos.x,
            pos.y
        );
    } else {
        // A placeholder box so a not-yet-supplied image is visible and located.
        let _ = write!(
            svg,
            r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" fill="#eceae2" stroke="#b9b3a3" stroke-dasharray="8 8"/><text x="{:.1}" y="{:.1}" font-size="28" fill="#8a8474" text-anchor="middle">{}</text>"##,
            pos.x,
            pos.y,
            pos.x + w / 2.0,
            pos.y + h / 2.0,
            esc_text(&format!("🖼 {}", el.id))
        );
    }
}

/// A hand-drawn arrow from one element's centre to another's, with a slight
/// deterministic curve so it reads as sketched rather than plotted.
fn render_arrow(svg: &mut String, el: &Element, scene: &Scene) {
    let from = el.prop_str("from").and_then(|id| element_center(scene, id));
    let to = el.prop_str("to").and_then(|id| element_center(scene, id));
    let (start, end) = match (from, to) {
        (Some(a), Some(b)) => (a, b),
        // Fall back to explicit position → position+size if refs are absent.
        _ => {
            let p = el.position.unwrap_or_default_point();
            let s = el.size.map(|s| (s.width, s.height)).unwrap_or((200.0, 0.0));
            ((p.x, p.y), (p.x + s.0, p.y + s.1))
        }
    };
    // Curve control point: midpoint nudged perpendicular by a stable amount.
    let (mx, my) = ((start.0 + end.0) / 2.0, (start.1 + end.1) / 2.0);
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let jitter = 0.12 * len * seed_unit(&el.id); // deterministic per element id
    let (nx, ny) = (-dy / len, dx / len);
    let (cx, cy) = (mx + nx * jitter, my + ny * jitter);
    let stroke = el.prop_str("stroke").unwrap_or("#333333");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(6.0);
    // pathLength=1 lets a draw animation run stroke-dashoffset 1→0 without
    // knowing the real length.
    let _ = write!(
        svg,
        r##"<path d="M{:.1},{:.1} Q{cx:.1},{cy:.1} {:.1},{:.1}" fill="none" stroke="{}" stroke-width="{width:.1}" stroke-linecap="round" marker-end="url(#arrowhead)" pathLength="1"/>"##,
        start.0,
        start.1,
        end.0,
        end.1,
        esc_attr(stroke)
    );
}

/// A simple, recognisable stand-in for the vector-character system (§7 proper
/// comes later): head, body, and a label of who and what pose.
fn render_character(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let color = el.prop_str("color").unwrap_or("#d98c1f");
    let name = el.prop_str("character").unwrap_or(&el.id);
    let pose = el.prop_str("pose").unwrap_or("");
    let (cx, top) = (pos.x, pos.y);
    let _ = write!(
        svg,
        r##"<g stroke="#333" stroke-width="4" fill="{c}"><circle cx="{cx:.1}" cy="{:.1}" r="46"/><path d="M{:.1},{:.1} q46,-70 92,0 v96 h-92 z" fill="{c}"/></g><text x="{cx:.1}" y="{:.1}" font-size="26" fill="#333" text-anchor="middle">{}</text>"##,
        top + 46.0,
        cx - 46.0,
        top + 130.0,
        top + 260.0,
        esc_text(&if pose.is_empty() {
            name.to_string()
        } else {
            format!("{name} · {pose}")
        }),
        c = esc_attr(color),
    );
}

fn render_rect(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((200.0, 120.0));
    let fill = el.prop_str("color").unwrap_or("#ffd54a");
    let opacity = if el.kind == "highlight" { "0.5" } else { "1" };
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="6" fill="{}" opacity="{opacity}"/>"##,
        pos.x,
        pos.y,
        esc_attr(fill)
    );
}

fn render_circle(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((160.0, 160.0));
    let fill = el.prop_str("color").unwrap_or("none");
    let stroke = el.prop_str("stroke").unwrap_or("#333333");
    let _ = write!(
        svg,
        r##"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}" fill="{}" stroke="{}" stroke-width="4"/>"##,
        pos.x + w / 2.0,
        pos.y + h / 2.0,
        w / 2.0,
        h / 2.0,
        esc_attr(fill),
        esc_attr(stroke)
    );
}

fn render_line(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or((240.0, 0.0));
    let stroke = el
        .prop_str("color")
        .or(el.prop_str("stroke"))
        .unwrap_or("#333333");
    let _ = write!(
        svg,
        r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="6" stroke-linecap="round" pathLength="1"/>"##,
        pos.x,
        pos.y,
        pos.x + w,
        pos.y + h,
        esc_attr(stroke)
    );
}

fn render_placeholder(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((240.0, 120.0));
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" fill="none" stroke="#999" stroke-dasharray="6 6"/><text x="{:.1}" y="{:.1}" font-size="24" fill="#999" text-anchor="middle">{}</text>"##,
        pos.x,
        pos.y,
        pos.x + w / 2.0,
        pos.y + h / 2.0,
        esc_text(&format!("{} ({})", el.id, el.kind))
    );
}

// ---------------------------------------------------------------------------
// Animation — baked as CSS so the browser plays it deterministically.
// ---------------------------------------------------------------------------

/// The inline style that runs an element's entry animation. Returns the base
/// animation properties; the `@keyframes` themselves are emitted once, globally,
/// by [`ANIM_KEYFRAMES`].
fn animation_style(anim: Option<&Animation>) -> String {
    let Some(anim) = anim else {
        return String::new();
    };
    let name = match anim.kind.as_str() {
        "fade" | "fade-in" | "appear" => "kf-fade",
        "slide" | "slide-up" | "slide-in" => "kf-slide",
        // A left-to-right reveal — the "written on" look — as a pure-CSS
        // clip-path wipe. (SVG's own SMIL <animate> was the obvious tool, but
        // it doesn't advance under a headless virtual-time render, so the text
        // came out clipped to nothing. CSS animations do advance, so the wipe
        // is CSS.)
        "handwrite" | "wipe" | "draw" => "kf-wipe",
        "pop" | "scale" => "kf-pop",
        _ => "kf-fade",
    };
    // fill-mode both so the element holds its pre-start and post-end state.
    // transform-box fill-box makes transforms/clips resolve against the
    // element's own bounds inside the SVG, not the whole canvas.
    format!(
        "animation:{name} {:.3}s ease {:.3}s both;transform-box:fill-box;",
        anim.duration.max(0.0),
        anim.start.max(0.0)
    )
}

/// The global keyframes, emitted once in the preview's `<style>`.
const ANIM_KEYFRAMES: &str = r##"
@keyframes kf-fade { from { opacity: 0 } to { opacity: 1 } }
@keyframes kf-slide { from { opacity: 0; transform: translateY(48px) } to { opacity: 1; transform: none } }
@keyframes kf-pop { from { opacity: 0; transform: scale(0.6) } 70% { transform: scale(1.05) } to { opacity: 1; transform: scale(1) } }
@keyframes kf-wipe { from { clip-path: inset(0 100% 0 0) } to { clip-path: inset(0 0 0 0) } }
"##;

// ---------------------------------------------------------------------------
// The preview player — one self-contained HTML file.
// ---------------------------------------------------------------------------

fn preview_html(
    project: &MotionProject,
    captions: &crate::motion_caption::CaptionTrack,
    brand: Option<&crate::motion_brand::BrandKit>,
) -> String {
    let mut scenes_svg = String::new();
    for scene in &project.scenes {
        // Each scene's SVG is embedded as a data-bearing template the player
        // swaps in; swapping restarts the CSS animations, so each scene plays
        // its entry animations when it appears.
        let svg = render_scene(project, scene, false, brand);
        let _ = write!(
            scenes_svg,
            r##"<template class="scene" data-duration="{}">{}</template>"##,
            scene.duration, svg
        );
    }

    // Captions ride the player as data — an array of {{s,e,t}} the overlay
    // shows by time — never drawn into the scene SVG (§9).
    let captions_json: Vec<String> = captions
        .cues
        .iter()
        .map(|c| {
            format!(
                r#"{{"s":{:.3},"e":{:.3},"t":"{}"}}"#,
                c.start,
                c.end,
                esc_js(&c.text.replace('\n', " "))
            )
        })
        .collect();
    let durations: Vec<String> = project
        .scenes
        .iter()
        .map(|s| format!("{}", s.duration))
        .collect();
    let names: Vec<String> = project
        .scenes
        .iter()
        .map(|s| format!("{:?}", scene_label(s)))
        .collect();

    let title = esc_text(&project.project.title);
    let total = project.total_duration();
    let (w, h) = (project.project.width, project.project.height);

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — Kestrel Motion preview</title>
<style>
:root {{ --gold: #DC8D1F; --ink: #0A0A0B; --raised: #151517; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; background: var(--ink); color: #eee; font: 15px/1.4 "Segoe UI", system-ui, sans-serif; display: flex; flex-direction: column; min-height: 100vh; }}
header {{ padding: 12px 18px; display: flex; align-items: center; gap: 14px; border-bottom: 1px solid #262626; }}
header b {{ color: var(--gold); }}
header .meta {{ color: #9a9a9a; font-size: 13px; }}
main {{ flex: 1; display: grid; place-items: center; padding: 18px; }}
#stage {{ position: relative; max-width: min(94vw, calc(94vh * {w} / {h})); aspect-ratio: {w} / {h}; background: #000; border-radius: 10px; overflow: hidden; box-shadow: 0 10px 40px rgba(0,0,0,.5); }}
#stage svg {{ width: 100%; height: 100%; display: block; }}
#safe {{ position: absolute; inset: 5%; border: 2px dashed rgba(224,64,63,.7); border-radius: 4px; pointer-events: none; display: none; }}
#stage.show-safe #safe {{ display: block; }}
#caption {{ position: absolute; left: 6%; right: 6%; bottom: {cap_bottom}%; text-align: center; pointer-events: none; }}
#caption span {{ display: inline; background: {cap_bg}; color: {cap_fg}; font-weight: 600; font-size: clamp(14px, 2.1vw, 28px); line-height: 1.35; padding: .18em .5em; border-radius: 6px; box-decoration-break: clone; -webkit-box-decoration-break: clone; }}
#caption:empty {{ display: none; }}
footer {{ padding: 12px 18px; border-top: 1px solid #262626; display: flex; align-items: center; gap: 14px; }}
button {{ background: var(--raised); color: #eee; border: 1px solid #333; border-radius: 7px; padding: 7px 14px; cursor: pointer; font-size: 14px; }}
button:hover {{ border-color: var(--gold); }}
button.primary {{ background: var(--gold); color: #1a1206; border-color: var(--gold); font-weight: 600; }}
#track {{ flex: 1; height: 8px; background: #262626; border-radius: 4px; overflow: hidden; }}
#bar {{ height: 100%; width: 0; background: var(--gold); }}
#clock {{ color: #9a9a9a; font-variant-numeric: tabular-nums; min-width: 96px; text-align: right; }}
#label {{ color: var(--gold); min-width: 140px; }}
{keyframes}
</style>
</head>
<body>
<header>
  <b>🦅 Kestrel Motion</b>
  <span>{title}</span>
  <span class="meta">{w}×{h} · {n} scenes · {total:.1}s</span>
</header>
<main>
  <div id="stage"><div id="mount"></div><div id="safe"></div><div id="caption"></div></div>
</main>
<footer>
  <button id="play" class="primary">▶ Play</button>
  <button id="restart">↺ Restart</button>
  <span id="label"></span>
  <div id="track"><div id="bar"></div></div>
  <span id="clock">0.0 / {total:.1}s</span>
  <button id="capbtn">CC</button>
  <button id="safebtn">Safe area</button>
</footer>
<div id="scenes" hidden>{scenes}</div>
<script>
const scenes = [...document.querySelectorAll('#scenes .scene')].map(t => t.innerHTML);
const durations = [{durations}];
const names = [{names}];
const captions = [{captions}];
const total = {total};
const mount = document.getElementById('mount');
const bar = document.getElementById('bar');
const clock = document.getElementById('clock');
const label = document.getElementById('label');
const stage = document.getElementById('stage');
const cap = document.getElementById('caption');
const playBtn = document.getElementById('play');
let shown = -1, playing = false, anchor = 0, base = 0, raf = 0, capsOn = true;

function show(idx) {{
  if (idx === shown) return;
  shown = idx;
  // Reassigning innerHTML restarts the scene's CSS/SMIL animations, so each
  // scene plays its entry animation when it appears.
  mount.innerHTML = scenes[idx] || '';
  label.textContent = names[idx] || '';
}}
function sceneAt(t) {{
  let acc = 0;
  for (let k = 0; k < durations.length; k++) {{
    if (t < acc + durations[k]) return k;
    acc += durations[k];
  }}
  return durations.length - 1;
}}
function activeCaption(t) {{
  for (const c of captions) {{ if (t >= c.s && t < c.e) return c.t; }}
  return '';
}}
function render(t) {{
  const clamped = Math.min(t, total);
  show(sceneAt(clamped));
  bar.style.width = (total ? 100 * clamped / total : 0) + '%';
  clock.textContent = clamped.toFixed(1) + ' / ' + total.toFixed(1) + 's';
  const line = capsOn ? activeCaption(clamped) : '';
  cap.innerHTML = line ? '<span></span>' : '';
  if (line) cap.firstChild.textContent = line;
}}
function tick(now) {{
  if (!playing) return;
  const t = base + (now - anchor) / 1000;
  render(t);
  if (t >= total) {{ pause(); return; }}
  raf = requestAnimationFrame(tick);
}}
function play() {{
  if (base >= total) {{ base = 0; shown = -1; }}
  playing = true; anchor = performance.now();
  playBtn.textContent = '⏸ Pause';
  raf = requestAnimationFrame(tick);
}}
function pause() {{
  if (playing) base += (performance.now() - anchor) / 1000;
  playing = false; cancelAnimationFrame(raf);
  playBtn.textContent = base >= total ? '↻ Replay' : '▶ Play';
}}
playBtn.onclick = () => playing ? pause() : play();
document.getElementById('restart').onclick = () => {{ pause(); base = 0; shown = -1; render(0); playBtn.textContent = '▶ Play'; }};
document.getElementById('safebtn').onclick = () => stage.classList.toggle('show-safe');
document.getElementById('capbtn').onclick = (e) => {{ capsOn = !capsOn; e.target.style.opacity = capsOn ? '1' : '.45'; render(base); }};
render(0);
</script>
</body>
</html>"##,
        keyframes = ANIM_KEYFRAMES,
        scenes = scenes_svg,
        durations = durations.join(","),
        names = names.join(","),
        captions = captions_json.join(","),
        cap_bottom = if project.project.format == crate::motion::Format::Vertical {
            13
        } else {
            7
        },
        cap_bg = brand
            .map(|b| b.caption_background.clone())
            .unwrap_or_else(|| "rgba(0,0,0,.62)".to_string()),
        cap_fg = brand
            .map(|b| b.caption_text.clone())
            .unwrap_or_else(|| "#ffffff".to_string()),
        n = project.scenes.len(),
    )
}

/// Escape a string for embedding inside a JSON/JS double-quoted string.
fn esc_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '<' => out.push_str("\\u003c"), // never let </script> escape the block
            c => out.push(c),
        }
    }
    out
}

/// A display label for a scene (name, or the id when unnamed).
fn scene_label(scene: &Scene) -> String {
    if scene.name.trim().is_empty() {
        scene.id.clone()
    } else {
        scene.name.clone()
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// A deterministic value in [0, 1) derived from a string — for the stable
/// "hand-drawn" wobble the directive requires (§6: same project, same output).
fn seed_unit(key: &str) -> f32 {
    // FNV-1a, then fold to a signed unit so the jitter goes both ways.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let unit = (hash % 10_000) as f32 / 10_000.0; // [0,1)
    unit - 0.5 // [-0.5, 0.5)
}

/// Escape text content for XML/SVG.
fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape an attribute value.
fn esc_attr(s: &str) -> String {
    esc_text(s).replace('"', "&quot;")
}

/// Convenience: a `Point`, defaulting to the origin when an element is unplaced.
trait PointExt {
    fn unwrap_or_default_point(self) -> crate::motion::Point;
}
impl PointExt for Option<crate::motion::Point> {
    fn unwrap_or_default_point(self) -> crate::motion::Point {
        self.unwrap_or(crate::motion::Point { x: 0.0, y: 0.0 })
    }
}

/// The centre of a named element in a scene, for arrow endpoints.
fn element_center(scene: &Scene, id: &str) -> Option<(f32, f32)> {
    let el = scene.elements.iter().find(|e| e.id == id)?;
    let p = el.position?;
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or((0.0, 0.0));
    Some((p.x + w / 2.0, p.y + h / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Format, MotionProject, ProjectType};

    fn sample_project() -> MotionProject {
        let json = r##"{
          "schemaVersion": "1.0",
          "project": { "title": "The Missing Stock", "type": "sketch-explainer",
            "format": "vertical", "width": 1080, "height": 1920, "fps": 30, "theme": "" },
          "scenes": [
            { "id": "scene-01", "name": "Hook", "duration": 6,
              "narration": "Your business may be losing stock.",
              "background": { "type": "solid", "color": "#F7F4EC" },
              "elements": [
                { "id": "title", "type": "title", "content": "Where did the stock go?",
                  "position": { "x": 120, "y": 300 }, "size": { "width": 840, "height": 160 },
                  "animation": { "type": "handwrite", "start": 0.5, "duration": 1.2 } },
                { "id": "owner", "type": "sketch-character", "character": "shop-owner",
                  "pose": "confused", "position": { "x": 300, "y": 1050 } },
                { "id": "shelf", "type": "image", "position": { "x": 640, "y": 1050 },
                  "size": { "width": 240, "height": 240 } },
                { "id": "arrow", "type": "sketch-arrow", "from": "owner", "to": "shelf",
                  "animation": { "type": "draw", "start": 2, "duration": 0.8 } }
              ] } ] }"##;
        MotionProject::from_json(json).unwrap()
    }

    #[test]
    fn scene_svg_has_the_expected_marks() {
        let project = sample_project();
        let svg = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        // The background colour and the title text made it in.
        assert!(svg.contains("#F7F4EC"));
        assert!(svg.contains("Where did the stock go?"));
        // The arrow resolved to a curved path with an arrowhead marker.
        assert!(svg.contains("marker-end=\"url(#arrowhead)\""));
        // The character stand-in and the image placeholder are present.
        assert!(svg.contains("shop-owner"));
        assert!(svg.contains("🖼 shelf") || svg.contains("shelf"));
    }

    #[test]
    fn animations_are_baked_as_css() {
        let project = sample_project();
        let svg = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        // The draw arrow carries an animation with its start delay.
        assert!(svg.contains("animation:"));
        assert!(svg.contains("2.000s")); // draw starts at 2s
                                         // The handwrite title reveals via a CSS clip-path wipe —
                                         // NOT SMIL, which doesn't advance in a headless render.
        assert!(svg.contains("kf-wipe"));
        assert!(!svg.contains("<animate")); // no SMIL anywhere
    }

    #[test]
    fn rendering_is_deterministic() {
        let project = sample_project();
        let a = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        let b = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        assert_eq!(a, b, "same project must render identical SVG (§15)");
        let html1 = SvgRenderer.render_preview_html(&project);
        let html2 = SvgRenderer.render_preview_html(&project);
        assert_eq!(html1, html2);
    }

    #[test]
    fn preview_is_self_contained_html() {
        let project = sample_project();
        let html = SvgRenderer.render_preview_html(&project);
        assert!(html.starts_with("<!doctype html>"));
        // No external resources — the strict thing the directive (§15 network
        // restrictions) and our artifact rules both want. The only permitted
        // URL is the SVG namespace, which is an identifier browsers never fetch.
        for marker in [
            "src=\"http",
            "href=\"http",
            "cdn",
            "googleapis",
            "unpkg",
            "@import",
        ] {
            assert!(
                !html.to_lowercase().contains(marker),
                "external resource: {marker}"
            );
        }
        // Every remaining http(s) occurrence must be the w3.org SVG namespace.
        for piece in html.split("http").skip(1) {
            assert!(
                piece.starts_with("://www.w3.org") || piece.starts_with("s://www.w3.org"),
                "unexpected URL in preview: http{}",
                &piece[..piece.len().min(40)]
            );
        }
        assert!(html.contains("Kestrel Motion"));
        assert!(html.contains("The Missing Stock"));
        // Both scenes' durations reach the player.
        assert!(html.contains("data-duration=\"6\""));
    }

    #[test]
    fn escaping_blocks_injection() {
        let mut project =
            MotionProject::new("A <b> & \"co\"", ProjectType::SocialShort, Format::Square);
        project.scenes.push(crate::motion::Scene {
            id: "s".into(),
            name: String::new(),
            duration: 3.0,
            narration: None,
            background: Background::default(),
            elements: vec![crate::motion::Element {
                id: "t".into(),
                kind: "text".into(),
                position: Some(crate::motion::Point { x: 10.0, y: 10.0 }),
                size: None,
                animation: None,
                layer: 0,
                extra: {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("content".into(), serde_json::json!("<script>x</script>"));
                    m
                },
            }],
        });
        let svg = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        assert!(!svg.contains("<script>x</script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn writes_preview_to_output_dir() {
        let dir =
            std::env::temp_dir().join(format!("kestrel-motion-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project = sample_project();
        let path = write_preview(&dir, &project).unwrap();
        assert!(path.ends_with("output/preview.html") || path.ends_with("output\\preview.html"));
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn static_frame_drops_animation_and_can_burn_a_caption() {
        let project = sample_project();
        // A settled export frame with a burned-in caption.
        let frame = render_scene_inner(
            &project,
            &project.scenes[0],
            false,
            None,
            true,
            Some("Your business may be losing stock."),
        );
        // No CSS animation in a still frame (it would render mid-transition).
        assert!(!frame.contains("animation:"));
        // The caption text is drawn into the frame.
        assert!(frame.contains("Your business may be losing stock."));
        // The live preview frame, by contrast, keeps its animations.
        let live = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        assert!(live.contains("animation:"));
    }

    #[test]
    fn export_segments_tile_the_timeline() {
        let mut project = MotionProject::new("t", ProjectType::SketchExplainer, Format::Vertical);
        project.scenes.push(crate::motion::Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 6.0,
            narration: Some(
                "A fairly long line that will split into two caption cues here.".into(),
            ),
            background: Background::default(),
            elements: vec![],
        });
        project.scenes.push(crate::motion::Scene {
            id: "s2".into(),
            name: String::new(),
            duration: 3.0,
            narration: None, // captionless -> one segment
            background: Background::default(),
            elements: vec![],
        });
        let captions = crate::motion_caption::CaptionTrack::from_project(&project);
        let segments = export_segments(&project, &captions);
        // Segment durations sum to the total runtime.
        let total: f32 = segments.iter().map(|s| s.duration).sum();
        assert!(
            (total - project.total_duration()).abs() < 0.05,
            "sum was {total}"
        );
        // The captionless scene contributes exactly one caption-free segment.
        assert!(segments.iter().any(|s| s.caption.is_none()));
        assert!(segments.iter().any(|s| s.caption.is_some()));
    }

    #[test]
    fn export_needs_scenes() {
        let project = MotionProject::new("empty", ProjectType::SocialShort, Format::Square);
        let err = export_mp4(&project, Path::new("."), &ExportOptions::default()).unwrap_err();
        assert!(err.contains("no scenes"));
    }
}
