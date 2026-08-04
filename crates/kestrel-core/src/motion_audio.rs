//! Kestrel Motion — voice-over and audio (§8).
//!
//! The directive puts voice-over timing in the architecture from the start: a
//! scene carries a narration line and, once recorded, a voice-over clip. This
//! module is the mechanical half of the §8 workflow — determine a clip's real
//! duration (ffprobe), align the scene to it (scene-level sync, the MVP bar),
//! and assemble one narration track the export mixes into the MP4.
//!
//! It shells out to FFmpeg/ffprobe, the same "no heavy dependency" pattern as
//! the rest of Kestrel. Everything here degrades gracefully when those tools
//! aren't installed: probing returns `None`, and export falls back to a silent
//! track rather than failing.

use crate::motion::MotionProject;
use std::path::Path;

/// Locate ffprobe: on PATH, or the WinGet shim directory it installs to.
pub fn find_ffprobe() -> Option<String> {
    tool_on_path("ffprobe").or_else(|| winget_link("ffprobe.exe"))
}

/// Locate ffmpeg the same way (mirrors the renderer's finder, kept here so the
/// audio pipeline doesn't depend on a private item).
pub fn find_ffmpeg() -> Option<String> {
    tool_on_path("ffmpeg").or_else(|| winget_link("ffmpeg.exe"))
}

fn tool_on_path(name: &str) -> Option<String> {
    std::process::Command::new(name)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| name.to_string())
}

fn winget_link(exe: &str) -> Option<String> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    let shim = format!(r"{local}\Microsoft\WinGet\Links\{exe}");
    Path::new(&shim).exists().then_some(shim)
}

/// The duration of an audio (or video) file in seconds, via ffprobe.
pub fn probe_duration(path: &Path) -> Option<f32> {
    let ffprobe = find_ffprobe()?;
    let out = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f32>()
        .ok()
}

/// A per-scene view of the audio state, for status and alignment reporting.
pub struct SceneAudio {
    pub scene_id: String,
    pub clip: Option<String>,
    /// The clip's real duration, if a clip is set and probeable.
    pub clip_duration: Option<f32>,
    pub scene_duration: f32,
}

/// Report each scene's audio alignment against its clip.
pub fn narration_status(project: &MotionProject, root: &Path) -> Vec<SceneAudio> {
    project
        .scenes
        .iter()
        .map(|s| {
            let clip_duration = s
                .audio
                .as_ref()
                .and_then(|rel| probe_duration(&root.join(rel)));
            SceneAudio {
                scene_id: s.id.clone(),
                clip: s.audio.clone(),
                clip_duration,
                scene_duration: s.duration,
            }
        })
        .collect()
}

/// Whether any scene carries a voice-over clip.
pub fn has_voiceover(project: &MotionProject) -> bool {
    project.scenes.iter().any(|s| s.audio.is_some())
}

/// Build one narration track spanning the whole timeline, written to `out` as a
/// WAV. Each scene contributes exactly its `duration`: its voice-over clip
/// (trimmed or silence-padded to fit), or silence when it has none — so the
/// track lines up with the video frame-for-frame. Returns an error string if
/// FFmpeg is missing or a step fails.
///
/// `out`'s parent is used for the intermediate per-scene segments, which are
/// cleaned up before returning.
pub fn build_narration_track(
    project: &MotionProject,
    root: &Path,
    out: &Path,
) -> Result<(), String> {
    let ffmpeg = find_ffmpeg().ok_or("FFmpeg is required to build the narration track")?;
    let work = out.parent().unwrap_or(root).to_path_buf();
    std::fs::create_dir_all(&work).map_err(|e| format!("could not create audio work dir: {e}"))?;

    const RATE: &str = "44100";
    let mut segs = Vec::new();
    let mut concat = String::from("ffconcat version 1.0\n");
    for (i, scene) in project.scenes.iter().enumerate() {
        let dur = scene.duration.max(0.05);
        let seg = work.join(format!(".aseg-{i:03}.wav"));
        let seg_name = seg.file_name().unwrap().to_string_lossy().to_string();
        let result = match &scene.audio {
            Some(rel) if root.join(rel).exists() => {
                // Pad the clip with trailing silence, then hard-cut to the scene
                // duration — so a short clip is padded and a long one is trimmed
                // to keep audio and video aligned.
                std::process::Command::new(&ffmpeg)
                    .args(["-y", "-i"])
                    .arg(root.join(rel))
                    .args([
                        "-af",
                        "apad",
                        "-t",
                        &format!("{dur:.3}"),
                        "-ar",
                        RATE,
                        "-ac",
                        "2",
                    ])
                    .arg(&seg)
                    .output()
            }
            _ => std::process::Command::new(&ffmpeg)
                .args([
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("anullsrc=r={RATE}:cl=stereo"),
                    "-t",
                    &format!("{dur:.3}"),
                ])
                .arg(&seg)
                .output(),
        };
        match result {
            Ok(o) if o.status.success() && seg.exists() => {}
            Ok(o) => {
                cleanup(&segs);
                return Err(format!(
                    "FFmpeg failed on scene '{}' audio: {}",
                    scene.id,
                    String::from_utf8_lossy(&o.stderr)
                        .lines()
                        .last()
                        .unwrap_or("")
                ));
            }
            Err(e) => {
                cleanup(&segs);
                return Err(format!("could not run FFmpeg for audio: {e}"));
            }
        }
        concat.push_str(&format!("file '{seg_name}'\n"));
        segs.push(seg);
    }

    let list = work.join(".asegs.txt");
    std::fs::write(&list, concat).map_err(|e| format!("audio list write failed: {e}"))?;
    let status = std::process::Command::new(&ffmpeg)
        .current_dir(&work)
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            ".asegs.txt",
            "-c",
            "pcm_s16le",
        ])
        .arg(out)
        .output();
    cleanup(&segs);
    let _ = std::fs::remove_file(&list);

    match status {
        Ok(o) if o.status.success() && out.exists() => Ok(()),
        Ok(o) => Err(format!(
            "FFmpeg failed to assemble the narration track: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("")
        )),
        Err(e) => Err(format!("could not run FFmpeg to assemble audio: {e}")),
    }
}

fn cleanup(files: &[std::path::PathBuf]) {
    for f in files {
        let _ = std::fs::remove_file(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Format, MotionProject, ProjectType, Scene};

    fn two_scene_project() -> MotionProject {
        let mut p = MotionProject::new("Voiced", ProjectType::SketchExplainer, Format::Vertical);
        p.scenes.push(Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 2.0,
            narration: Some("hello".into()),
            audio: None,
            background: Default::default(),
            elements: vec![],
        });
        p.scenes.push(Scene {
            id: "s2".into(),
            name: String::new(),
            duration: 3.0,
            narration: None,
            audio: None,
            background: Default::default(),
            elements: vec![],
        });
        p
    }

    #[test]
    fn has_voiceover_reflects_scene_clips() {
        let mut p = two_scene_project();
        assert!(!has_voiceover(&p));
        p.scenes[0].audio = Some("assets/audio/s1.wav".into());
        assert!(has_voiceover(&p));
    }

    #[test]
    fn status_lines_up_scenes() {
        let p = two_scene_project();
        let status = narration_status(&p, Path::new("."));
        assert_eq!(status.len(), 2);
        assert_eq!(status[0].scene_id, "s1");
        assert!(status[0].clip.is_none());
        assert_eq!(status[1].scene_duration, 3.0);
    }

    #[test]
    fn builds_a_silent_track_matching_total_duration() {
        // Needs FFmpeg + ffprobe; skip cleanly where they aren't installed so
        // the suite still passes on a bare machine.
        let (Some(_), Some(_)) = (find_ffmpeg(), find_ffprobe()) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("kestrel-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project = two_scene_project(); // no clips -> all silence, 5.0s total
        let out = dir.join("narration.wav");
        build_narration_track(&project, &dir, &out).expect("build track");
        assert!(out.exists());
        let dur = probe_duration(&out).expect("probe track");
        assert!((dur - 5.0).abs() < 0.15, "track was {dur}s, expected ~5.0s");
        // Intermediate segments were cleaned up.
        assert!(!dir.join(".aseg-000.wav").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
