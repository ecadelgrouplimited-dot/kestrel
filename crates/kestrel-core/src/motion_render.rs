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

    // The soundtrack: a real narration track when any scene has a voice-over
    // clip (§8), otherwise silence so the file still carries an audio stream.
    let narration = out_dir.join(".narration.wav");
    let has_audio = crate::motion_audio::has_voiceover(project)
        && crate::motion_audio::build_narration_track(project, root, &narration).is_ok();

    let total = format!("{:.3}", project.total_duration().max(0.1));
    let vf = format!("fps={fps},format=yuv420p");
    let mut cmd = std::process::Command::new(&ffmpeg);
    cmd.current_dir(&out_dir)
        .args(["-y", "-f", "concat", "-safe", "0", "-i", ".mframes.txt"]);
    if has_audio {
        cmd.args(["-i", ".narration.wav"]);
    } else {
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=stereo:sample_rate=44100",
        ]);
    }
    cmd.args(["-map", "0:v", "-map", "1:a"])
        .args(["-vf", &vf])
        .args(["-c:v", "libx264", "-preset", "medium"])
        .args(["-c:a", "aac", "-shortest", "-movflags", "+faststart"])
        // Pin the exact runtime: the concat demuxer's trailing repeated frame
        // otherwise overhangs the intended length.
        .args(["-t", &total])
        .arg(&out_name);
    let status = cmd.output();

    // Clean up frames and the narration track regardless of outcome.
    for f in &frame_files {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(&list_path);
    let _ = std::fs::remove_file(&narration);

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
        "text" | "title" | "caption" | "cta" => render_text(svg, el, brand),
        "callout" | "speech-bubble" => render_callout(svg, el, brand),
        "chart" => render_chart(svg, el, brand),
        "image" | "screenshot" => render_image(svg, el),
        "browser-frame" | "browser" => render_browser_frame(svg, el),
        "device-frame" | "device" | "phone" => render_device_frame(svg, el),
        "cursor" => render_cursor(svg, el),
        "sketch-arrow" | "arrow" | "connector" => render_arrow(svg, el, scene),
        "sketch-character" | "character" => render_character(svg, el),
        "sketch-rect" | "sketch-box" => render_sketch_rect(svg, el),
        "sketch-circle" | "sketch-ellipse" => render_sketch_ellipse(svg, el),
        "sketch-line" | "sketch-underline" | "underline" => render_sketch_line(svg, el),
        "sketch-highlight" => render_sketch_highlight(svg, el, brand),
        "checkmark" | "check" | "sketch-check" | "tick" => render_checkmark(svg, el),
        "cross" | "sketch-cross" | "x-mark" => render_cross(svg, el),
        "rect" | "rectangle" | "highlight" => render_rect(svg, el),
        "circle" | "ellipse" => render_circle(svg, el),
        "line" => render_line(svg, el),
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

/// A hand-drawn arrow from one element's centre to another's: a rough shaft
/// plus a rough two-stroke arrowhead, deterministic per element id (§6).
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
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let stroke = el
        .prop_str("stroke")
        .or(el.prop_str("color"))
        .unwrap_or("#333333");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(6.0) as f32;
    let mut rng = Rng::new(&el.id);
    let rough = roughness_for(len);

    // A gentle bow in the shaft, so a straight arrow still reads as drawn.
    let (nx, ny) = (-dy / len, dx / len);
    let bow = 0.08 * len * seed_unit(&el.id);
    let mid = (
        (start.0 + end.0) / 2.0 + nx * bow,
        (start.1 + end.1) / 2.0 + ny * bow,
    );
    rough_stroke(
        svg,
        &mut rng,
        &[start, mid, end],
        false,
        stroke,
        width,
        rough,
        1.0,
    );

    // Arrowhead: two short rough strokes back from the tip at ±26°.
    let (ux, uy) = (dx / len, dy / len);
    let head = (len * 0.06).clamp(18.0, 46.0);
    let ang = 0.45_f32; // ~26°
    let (ca, sa) = (ang.cos(), ang.sin());
    let barb = |sign: f32| {
        let rx = -ux * ca + sign * -uy * sa;
        let ry = -uy * ca + sign * ux * sa;
        (end.0 + rx * head, end.1 + ry * head)
    };
    rough_stroke(
        svg,
        &mut rng,
        &[end, barb(1.0)],
        false,
        stroke,
        width,
        rough * 0.5,
        1.0,
    );
    rough_stroke(
        svg,
        &mut rng,
        &[end, barb(-1.0)],
        false,
        stroke,
        width,
        rough * 0.5,
        1.0,
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

/// A default categorical palette for charts, when the brand doesn't drive it.
const CHART_PALETTE: [&str; 6] = [
    "#DC8D1F", "#5ABE6E", "#6AA0DC", "#DC6464", "#B07ADC", "#DCB450",
];

/// Read a chart's `data` as (label, value) pairs from the property bag.
fn chart_data(el: &Element) -> Vec<(String, f64)> {
    el.extra
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let value = item.get("value").and_then(|v| v.as_f64())?;
                    let label = item
                        .get("label")
                        .and_then(|l| l.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((label, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A bar or line chart drawn as SVG from `data` — deterministic, resolution-
/// independent, no plotting dependency. `chartKind` selects bar (default) or
/// line; the primary bar colour comes from the brand.
fn render_chart(svg: &mut String, el: &Element, brand: Option<&crate::motion_brand::BrandKit>) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((640.0, 420.0));
    let data = chart_data(el);
    if data.is_empty() {
        render_placeholder(svg, el);
        return;
    }
    let kind = el.prop_str("chartKind").unwrap_or("bar");
    let axis = brand.map(|b| b.text.as_str()).unwrap_or("#888");
    let primary = el
        .prop_str("color")
        .or_else(|| brand.map(|b| b.primary.as_str()))
        .unwrap_or("#DC8D1F");

    // Plot area: leave room for value labels on top and category labels below.
    let pad_top = h * 0.10;
    let pad_bottom = h * 0.16;
    let pad_left = w * 0.04;
    let plot_w = w - pad_left * 2.0;
    let plot_h = h - pad_top - pad_bottom;
    let base_y = pos.y + pad_top + plot_h;
    let max = data
        .iter()
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max)
        .max(1e-6);
    let label_font = (w * 0.028).clamp(12.0, 30.0);

    // Baseline axis.
    let _ = write!(
        svg,
        r##"<line x1="{:.1}" y1="{base_y:.1}" x2="{:.1}" y2="{base_y:.1}" stroke="{}" stroke-width="2" opacity="0.6"/>"##,
        pos.x + pad_left,
        pos.x + pad_left + plot_w,
        esc_attr(axis)
    );

    let n = data.len();
    let slot = plot_w / n as f32;
    if kind == "line" {
        // A polyline through the points, with a dot at each.
        let mut points = String::new();
        for (i, (_, value)) in data.iter().enumerate() {
            let x = pos.x + pad_left + slot * (i as f32 + 0.5);
            let y = base_y - (*value as f32 / max as f32) * plot_h;
            let _ = write!(points, "{x:.1},{y:.1} ");
        }
        let _ = write!(
            svg,
            r##"<polyline points="{}" fill="none" stroke="{}" stroke-width="4" stroke-linejoin="round" stroke-linecap="round"/>"##,
            points.trim(),
            esc_attr(primary)
        );
        for (i, (_, value)) in data.iter().enumerate() {
            let x = pos.x + pad_left + slot * (i as f32 + 0.5);
            let y = base_y - (*value as f32 / max as f32) * plot_h;
            let _ = write!(
                svg,
                r##"<circle cx="{x:.1}" cy="{y:.1}" r="6" fill="{}"/>"##,
                esc_attr(primary)
            );
        }
    } else {
        // Bars, one per datum, coloured from the palette so a series reads.
        let bar_w = slot * 0.62;
        for (i, (_, value)) in data.iter().enumerate() {
            let bh = (*value as f32 / max as f32) * plot_h;
            let x = pos.x + pad_left + slot * i as f32 + (slot - bar_w) / 2.0;
            let y = base_y - bh;
            let fill = if el.prop_str("color").is_some() {
                primary.to_string()
            } else {
                CHART_PALETTE[i % CHART_PALETTE.len()].to_string()
            };
            let _ = write!(
                svg,
                r##"<rect x="{x:.1}" y="{y:.1}" width="{bar_w:.1}" height="{bh:.1}" rx="4" fill="{}"/>"##,
                esc_attr(&fill)
            );
        }
    }

    // Value labels above each point/bar, category labels below the axis.
    for (i, (label, value)) in data.iter().enumerate() {
        let cx = pos.x + pad_left + slot * (i as f32 + 0.5);
        let top_y = base_y - (*value as f32 / max as f32) * plot_h - label_font * 0.4;
        let val_str = if (value.fract()).abs() < 1e-6 {
            format!("{}", *value as i64)
        } else {
            format!("{value:.1}")
        };
        let _ = write!(
            svg,
            r##"<text x="{cx:.1}" y="{top_y:.1}" font-size="{label_font:.0}" fill="{}" text-anchor="middle">{}</text>"##,
            esc_attr(axis),
            esc_text(&val_str)
        );
        if !label.is_empty() {
            let _ = write!(
                svg,
                r##"<text x="{cx:.1}" y="{:.1}" font-size="{label_font:.0}" fill="{}" text-anchor="middle" opacity="0.85">{}</text>"##,
                base_y + label_font * 1.4,
                esc_attr(axis),
                esc_text(label)
            );
        }
    }
}

/// A browser window frame around an inner screenshot (or a plain content area) —
/// for product tutorials (§10). Traffic-light dots, an address bar, and the
/// inner image.
fn render_browser_frame(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((900.0, 560.0));
    let bar = (h * 0.09).clamp(28.0, 64.0);
    let url = el.prop_str("url").unwrap_or("");
    // Window + chrome bar.
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="12" fill="#ffffff" stroke="#cfcac0" stroke-width="2"/><rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{bar:.1}" rx="12" fill="#ece9e2"/><rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{:.1}" fill="#ece9e2"/>"##,
        pos.x,
        pos.y, // window
        pos.x,
        pos.y, // chrome top (rounded)
        pos.x,
        pos.y + bar / 2.0,
        bar / 2.0, // square off the chrome's bottom corners
    );
    // Traffic lights.
    let r = bar * 0.16;
    let cy = pos.y + bar / 2.0;
    for (i, colour) in ["#ec6a5e", "#f4bf4f", "#61c554"].iter().enumerate() {
        let cx = pos.x + bar * 0.5 + i as f32 * r * 3.0;
        let _ = write!(
            svg,
            r##"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{colour}"/>"##
        );
    }
    // Address pill.
    let pill_x = pos.x + bar * 0.5 + r * 9.0;
    let pill_w = (w - (pill_x - pos.x) - bar * 0.5).max(0.0);
    let _ = write!(
        svg,
        r##"<rect x="{pill_x:.1}" y="{:.1}" width="{pill_w:.1}" height="{:.1}" rx="{:.1}" fill="#ffffff" stroke="#d8d3c8"/><text x="{:.1}" y="{:.1}" font-size="{:.0}" fill="#8a8474">{}</text>"##,
        pos.y + bar * 0.28,
        bar * 0.44,
        bar * 0.22,
        pill_x + bar * 0.4,
        pos.y + bar * 0.62,
        bar * 0.32,
        esc_text(url)
    );
    // Inner content: an image asset, or a light content area.
    let (iy, ih) = (pos.y + bar, h - bar);
    if let Some(asset) = el.prop_str("asset") {
        let _ = write!(
            svg,
            r##"<image href="../{}" x="{:.1}" y="{iy:.1}" width="{w:.1}" height="{ih:.1}" preserveAspectRatio="xMidYMid slice"/>"##,
            esc_attr(asset),
            pos.x
        );
    } else {
        let _ = write!(
            svg,
            r##"<rect x="{:.1}" y="{iy:.1}" width="{w:.1}" height="{ih:.1}" fill="#f7f5f0"/>"##,
            pos.x
        );
    }
}

/// A phone device frame around an inner screenshot — for tutorials and mobile
/// mockups (§10).
fn render_device_frame(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((360.0, 740.0));
    let bezel = (w * 0.05).clamp(10.0, 40.0);
    let radius = w * 0.12;
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="{radius:.1}" fill="#141414"/>"##,
        pos.x, pos.y
    );
    let (ix, iy) = (pos.x + bezel, pos.y + bezel);
    let (iw, ih) = (w - bezel * 2.0, h - bezel * 2.0);
    let inner_r = radius * 0.7;
    if let Some(asset) = el.prop_str("asset") {
        // Clip the screenshot to the rounded screen.
        let clip = format!("dev-{}", el.id);
        let _ = write!(
            svg,
            r##"<clipPath id="{clip}"><rect x="{ix:.1}" y="{iy:.1}" width="{iw:.1}" height="{ih:.1}" rx="{inner_r:.1}"/></clipPath><image href="../{}" x="{ix:.1}" y="{iy:.1}" width="{iw:.1}" height="{ih:.1}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip})"/>"##,
            esc_attr(asset)
        );
    } else {
        let _ = write!(
            svg,
            r##"<rect x="{ix:.1}" y="{iy:.1}" width="{iw:.1}" height="{ih:.1}" rx="{inner_r:.1}" fill="#f7f5f0"/>"##
        );
    }
    // A notch.
    let notch_w = w * 0.34;
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{notch_w:.1}" height="{:.1}" rx="{:.1}" fill="#141414"/>"##,
        pos.x + (w - notch_w) / 2.0,
        pos.y + bezel * 0.5,
        bezel * 0.7,
        bezel * 0.35
    );
}

/// A mouse cursor at a point — for tutorial walkthroughs (§10).
fn render_cursor(svg: &mut String, el: &Element) {
    let pos = el.position.unwrap_or_default_point();
    let scale = el
        .size
        .map(|s| s.width / 24.0)
        .unwrap_or(1.6)
        .clamp(0.8, 6.0);
    let fill = el.prop_str("color").unwrap_or("#111111");
    // A classic arrow pointer, its tip at the element position.
    let _ = write!(
        svg,
        r##"<path transform="translate({:.1},{:.1}) scale({scale:.2})" d="M0,0 L0,20 L5,15 L9,23 L12,22 L8,14 L15,14 Z" fill="{}" stroke="#fff" stroke-width="1.2"/>"##,
        pos.x,
        pos.y,
        esc_attr(fill)
    );
}

/// A callout: a rounded label box with a little pointer, for annotating a
/// tutorial or emphasising a point (§10).
fn render_callout(svg: &mut String, el: &Element, brand: Option<&crate::motion_brand::BrandKit>) {
    let pos = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((420.0, 120.0));
    let text = el.text_content().unwrap_or("");
    let bg = el
        .prop_str("color")
        .or_else(|| brand.map(|b| b.primary.as_str()))
        .unwrap_or("#DC8D1F");
    let fg = el.prop_str("textColor").unwrap_or("#1a1206");
    let font = (h * 0.32).clamp(16.0, 60.0);
    let (cx, cy) = (pos.x + w / 2.0, pos.y + h / 2.0);
    // Box with a downward pointer at bottom-left.
    let _ = write!(
        svg,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="{:.1}" fill="{}"/><path d="M{:.1},{:.1} l{:.1},{:.1} l{:.1},0 Z" fill="{}"/><text x="{cx:.1}" y="{:.1}" font-size="{font:.0}" font-weight="600" fill="{}" text-anchor="middle">{}</text>"##,
        pos.x,
        pos.y,
        (h * 0.16).min(20.0),
        esc_attr(bg),
        pos.x + w * 0.22,
        pos.y + h,
        h * 0.16,
        h * 0.22,
        h * 0.16,
        esc_attr(bg),
        cy + font * 0.35,
        esc_attr(fg),
        esc_text(text)
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

/// A tiny seeded PRNG (xorshift64) for the sketch system's *deterministic*
/// randomness (§6: a sketch element looks identical every render unless
/// regenerated). Seeded from the element id, so each element wobbles its own
/// consistent way.
struct Rng(u64);

impl Rng {
    fn new(seed: &str) -> Self {
        let mut h: u64 = 0xcbf29ce484222325;
        for byte in seed.bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Rng(h | 1) // never zero, or xorshift sticks
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// A value in [-mag, mag).
    fn jitter(&mut self, mag: f32) -> f32 {
        ((self.next() % 100_000) as f32 / 100_000.0 - 0.5) * 2.0 * mag
    }
}

/// A hand-drawn stroke through `pts`, in the rough.js manner: each segment is a
/// cubic with jittered control points, drawn as two slightly different overlaid
/// passes so it reads as sketched rather than plotted. `closed` joins the last
/// point back to the first.
#[allow(clippy::too_many_arguments)]
fn rough_stroke(
    svg: &mut String,
    rng: &mut Rng,
    pts: &[(f32, f32)],
    closed: bool,
    stroke: &str,
    width: f32,
    rough: f32,
    opacity: f32,
) {
    if pts.len() < 2 {
        return;
    }
    let mut segs: Vec<((f32, f32), (f32, f32))> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed {
        segs.push((pts[pts.len() - 1], pts[0]));
    }
    for _pass in 0..2 {
        let (sx, sy) = segs[0].0;
        let mut d = format!(
            "M{:.1},{:.1}",
            sx + rng.jitter(rough),
            sy + rng.jitter(rough)
        );
        for (a, b) in &segs {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let c1 = (
                a.0 + dx * 0.33 + rng.jitter(rough),
                a.1 + dy * 0.33 + rng.jitter(rough),
            );
            let c2 = (
                a.0 + dx * 0.66 + rng.jitter(rough),
                a.1 + dy * 0.66 + rng.jitter(rough),
            );
            let end = (b.0 + rng.jitter(rough * 0.6), b.1 + rng.jitter(rough * 0.6));
            let _ = write!(
                d,
                " C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                c1.0, c1.1, c2.0, c2.1, end.0, end.1
            );
        }
        let _ = write!(
            svg,
            r##"<path d="{d}" fill="none" stroke="{}" stroke-width="{width:.1}" stroke-linecap="round" stroke-linejoin="round" opacity="{opacity}"/>"##,
            esc_attr(stroke)
        );
    }
}

/// A sensible roughness for a stroke of length `len`: enough wobble to read as
/// hand-drawn, never so much it looks broken.
fn roughness_for(len: f32) -> f32 {
    (len * 0.02).clamp(1.5, 9.0)
}

/// The four corners of an element's box, for rough rectangles.
fn box_corners(el: &Element, default: (f32, f32)) -> [(f32, f32); 4] {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or(default);
    [
        (p.x, p.y),
        (p.x + w, p.y),
        (p.x + w, p.y + h),
        (p.x, p.y + h),
    ]
}

fn render_sketch_rect(svg: &mut String, el: &Element) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((300.0, 180.0));
    // An optional flat fill sits behind the hand-drawn outline.
    if let Some(fill) = el.prop_str("fill") {
        let _ = write!(
            svg,
            r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="6" fill="{}" opacity="0.9"/>"##,
            p.x,
            p.y,
            esc_attr(fill)
        );
    }
    let stroke = el
        .prop_str("color")
        .or(el.prop_str("stroke"))
        .unwrap_or("#333333");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(4.0) as f32;
    let mut rng = Rng::new(&el.id);
    let rough = roughness_for((w + h) * 0.5);
    rough_stroke(
        svg,
        &mut rng,
        &box_corners(el, (300.0, 180.0)),
        true,
        stroke,
        width,
        rough,
        1.0,
    );
}

fn render_sketch_ellipse(svg: &mut String, el: &Element) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((200.0, 200.0));
    let (cx, cy, rx, ry) = (p.x + w / 2.0, p.y + h / 2.0, w / 2.0, h / 2.0);
    if let Some(fill) = el.prop_str("fill") {
        let _ = write!(
            svg,
            r##"<ellipse cx="{cx:.1}" cy="{cy:.1}" rx="{rx:.1}" ry="{ry:.1}" fill="{}" opacity="0.9"/>"##,
            esc_attr(fill)
        );
    }
    // Sample points around the ellipse; the cubic-per-segment smoothing in
    // rough_stroke turns the ring into a wobbly hand-drawn oval.
    const N: usize = 12;
    let pts: Vec<(f32, f32)> = (0..N)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / N as f32;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect();
    let stroke = el
        .prop_str("color")
        .or(el.prop_str("stroke"))
        .unwrap_or("#333333");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(4.0) as f32;
    let mut rng = Rng::new(&el.id);
    rough_stroke(
        svg,
        &mut rng,
        &pts,
        true,
        stroke,
        width,
        roughness_for((rx + ry) * 0.5),
        1.0,
    );
}

fn render_sketch_line(svg: &mut String, el: &Element) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or((240.0, 0.0));
    let stroke = el
        .prop_str("color")
        .or(el.prop_str("stroke"))
        .unwrap_or("#333333");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(if el.kind.contains("underline") {
            5.0
        } else {
            4.0
        }) as f32;
    let mut rng = Rng::new(&el.id);
    let len = (w * w + h * h).sqrt();
    rough_stroke(
        svg,
        &mut rng,
        &[(p.x, p.y), (p.x + w, p.y + h)],
        false,
        stroke,
        width,
        roughness_for(len),
        1.0,
    );
}

/// A marker-pen highlight: a thick, translucent hand-drawn band.
fn render_sketch_highlight(
    svg: &mut String,
    el: &Element,
    brand: Option<&crate::motion_brand::BrandKit>,
) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el
        .size
        .map(|s| (s.width, s.height))
        .unwrap_or((300.0, 48.0));
    let colour = el
        .prop_str("color")
        .or_else(|| brand.map(|b| b.accent.as_str()))
        .unwrap_or("#ffd54a");
    let mut rng = Rng::new(&el.id);
    // One thick stroke down the middle, so it reads as a swipe of highlighter.
    let mid = p.y + h / 2.0;
    rough_stroke(
        svg,
        &mut rng,
        &[(p.x, mid), (p.x + w, mid)],
        false,
        colour,
        h * 0.9,
        roughness_for(w) * 0.6,
        0.45,
    );
}

fn render_checkmark(svg: &mut String, el: &Element) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or((80.0, 80.0));
    let stroke = el.prop_str("color").unwrap_or("#5ABE6E");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(8.0) as f32;
    let pts = [
        (p.x, p.y + h * 0.55),
        (p.x + w * 0.38, p.y + h),
        (p.x + w, p.y),
    ];
    let mut rng = Rng::new(&el.id);
    rough_stroke(
        svg,
        &mut rng,
        &pts,
        false,
        stroke,
        width,
        roughness_for(w) * 0.7,
        1.0,
    );
}

fn render_cross(svg: &mut String, el: &Element) {
    let p = el.position.unwrap_or_default_point();
    let (w, h) = el.size.map(|s| (s.width, s.height)).unwrap_or((70.0, 70.0));
    let stroke = el.prop_str("color").unwrap_or("#DC6464");
    let width = el
        .extra
        .get("strokeWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(8.0) as f32;
    let mut rng = Rng::new(&el.id);
    let rough = roughness_for(w) * 0.7;
    rough_stroke(
        svg,
        &mut rng,
        &[(p.x, p.y), (p.x + w, p.y + h)],
        false,
        stroke,
        width,
        rough,
        1.0,
    );
    rough_stroke(
        svg,
        &mut rng,
        &[(p.x + w, p.y), (p.x, p.y + h)],
        false,
        stroke,
        width,
        rough,
        1.0,
    );
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
        // The arrow renders as hand-drawn rough paths (shaft + barbs), not a
        // clean marker-ended line.
        assert!(svg.contains("<path"));
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
            audio: None,
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
            audio: None,
            background: Background::default(),
            elements: vec![],
        });
        project.scenes.push(crate::motion::Scene {
            id: "s2".into(),
            name: String::new(),
            duration: 3.0,
            narration: None, // captionless -> one segment
            audio: None,
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

    /// Render a scene holding one element of `kind` with the given extra props.
    fn render_one(kind: &str, extra: serde_json::Value) -> String {
        let mut project =
            MotionProject::new("t", ProjectType::PresentationVideo, Format::Horizontal);
        let extra_map = extra
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        project.scenes.push(crate::motion::Scene {
            id: "s".into(),
            name: String::new(),
            duration: 4.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![crate::motion::Element {
                id: "e".into(),
                kind: kind.into(),
                position: Some(crate::motion::Point { x: 200.0, y: 200.0 }),
                size: Some(crate::motion::Size {
                    width: 800.0,
                    height: 400.0,
                }),
                animation: None,
                layer: 0,
                extra: extra_map,
            }],
        });
        SvgRenderer.render_scene_svg(&project, &project.scenes[0])
    }

    #[test]
    fn bar_chart_draws_a_bar_per_datum() {
        let svg = render_one(
            "chart",
            serde_json::json!({
                "chartKind": "bar",
                "data": [
                    {"label": "Jan", "value": 10},
                    {"label": "Feb", "value": 25},
                    {"label": "Mar", "value": 15},
                ],
            }),
        );
        // Three bars (rect with rx=4 from the chart), the axis, and the labels.
        assert_eq!(svg.matches(r#"rx="4""#).count(), 3, "expected 3 bars");
        assert!(svg.contains("Jan") && svg.contains("Mar"));
        assert!(svg.contains("25")); // the max value label
        assert!(svg.contains("<line")); // baseline axis
    }

    #[test]
    fn line_chart_draws_a_polyline() {
        let svg = render_one(
            "chart",
            serde_json::json!({
                "chartKind": "line",
                "data": [{"label":"a","value":1},{"label":"b","value":4},{"label":"c","value":2}],
            }),
        );
        assert!(svg.contains("<polyline"));
        // A dot per point.
        assert_eq!(svg.matches("<circle").count(), 3);
    }

    #[test]
    fn empty_chart_falls_back_to_a_placeholder() {
        let svg = render_one("chart", serde_json::json!({ "data": [] }));
        assert!(svg.contains("stroke-dasharray")); // the placeholder box
        assert!(svg.contains("(chart)"));
    }

    #[test]
    fn sketch_primitives_are_rough_and_deterministic() {
        // Each sketch kind renders as overlaid rough paths (two passes each).
        for kind in [
            "sketch-rect",
            "sketch-circle",
            "sketch-line",
            "checkmark",
            "cross",
            "sketch-highlight",
        ] {
            let a = render_one(kind, serde_json::json!({}));
            let b = render_one(kind, serde_json::json!({}));
            assert!(a.contains("<path"), "{kind} should draw paths");
            // Cubic segments are the signature of the rough stroke.
            assert!(a.contains(" C"), "{kind} should use cubic segments");
            // Deterministic: identical input → byte-identical output (§6, §15).
            assert_eq!(a, b, "{kind} must render deterministically");
        }
        // Different element ids wobble differently (the randomness is real).
        let one = render_one("sketch-rect", serde_json::json!({}));
        let mut project = MotionProject::new("t", ProjectType::SketchExplainer, Format::Horizontal);
        project.scenes.push(crate::motion::Scene {
            id: "s".into(),
            name: String::new(),
            duration: 4.0,
            narration: None,
            audio: None,
            background: Background::default(),
            elements: vec![crate::motion::Element {
                id: "different-id".into(),
                kind: "sketch-rect".into(),
                position: Some(crate::motion::Point { x: 200.0, y: 200.0 }),
                size: Some(crate::motion::Size {
                    width: 800.0,
                    height: 400.0,
                }),
                animation: None,
                layer: 0,
                extra: std::collections::BTreeMap::new(),
            }],
        });
        let other = SvgRenderer.render_scene_svg(&project, &project.scenes[0]);
        assert_ne!(one, other, "different ids should wobble differently");
    }

    #[test]
    fn tutorial_frames_and_annotations_render() {
        // Browser frame with a URL and traffic lights.
        let browser = render_one(
            "browser-frame",
            serde_json::json!({ "url": "app.example.com" }),
        );
        assert!(browser.contains("app.example.com"));
        assert!(browser.contains("#ec6a5e")); // the red traffic light

        // Device frame clips its screenshot to a rounded screen.
        let device = render_one(
            "device-frame",
            serde_json::json!({ "asset": "assets/screenshots/x.png" }),
        );
        assert!(device.contains("clipPath"));

        // A cursor is a pointer path anchored at its position.
        let cursor = render_one("cursor", serde_json::json!({}));
        assert!(cursor.contains("<path") && cursor.contains("translate(200"));

        // A callout carries its text.
        let callout = render_one("callout", serde_json::json!({ "content": "Click here" }));
        assert!(callout.contains("Click here"));
    }
}
