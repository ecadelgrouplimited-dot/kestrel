//! Kestrel Motion — captions, as editable data.
//!
//! The directive is firm on this (§9): captions must stay editable data and
//! must never be baked into the render. So a caption track lives beside the
//! project as its own file, the renderer overlays it live in the preview rather
//! than drawing it into the scene SVG, and it round-trips through SRT so it can
//! be edited in any subtitle tool and re-imported.
//!
//! Generation is scene-level, which §16 says is the MVP bar (word-level sync
//! comes later): each scene's `narration` becomes one or more cues spanning that
//! scene's slot on the timeline, split at word boundaries into readable lines and
//! distributed across the scene's duration in proportion to their length. It is
//! deterministic — the same project yields the same cues every time.

use crate::motion::MotionProject;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One caption cue: the text shown between `start` and `end` (seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// A whole caption track for a project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptionTrack {
    #[serde(default)]
    pub cues: Vec<Caption>,
}

/// Roughly the longest caption line that reads comfortably on one line.
const MAX_LINE_CHARS: usize = 42;

impl CaptionTrack {
    /// Generate a track from a project's scene narration.
    ///
    /// Each scene occupies its slot on the timeline; its narration is split into
    /// readable lines and those lines share the scene's duration in proportion
    /// to their length. Scenes without narration contribute no cues.
    pub fn from_project(project: &MotionProject) -> Self {
        let mut cues = Vec::new();
        let mut clock = 0.0f32;
        for scene in &project.scenes {
            let slot_start = clock;
            clock += scene.duration;
            let Some(narration) = scene.narration.as_ref() else {
                continue;
            };
            let narration = narration.trim();
            if narration.is_empty() {
                continue;
            }
            let lines = split_lines(narration, MAX_LINE_CHARS);
            let total_chars: usize = lines.iter().map(|l| l.chars().count().max(1)).sum();
            let mut cue_start = slot_start;
            for (i, line) in lines.iter().enumerate() {
                // Share the slot by line length, so a long line lingers longer.
                let share = line.chars().count().max(1) as f32 / total_chars as f32;
                let mut cue_end = cue_start + scene.duration * share;
                if i == lines.len() - 1 {
                    // Absorb any rounding drift into the final cue's end.
                    cue_end = slot_start + scene.duration;
                }
                cues.push(Caption {
                    start: round_ms(cue_start),
                    end: round_ms(cue_end),
                    text: line.clone(),
                });
                cue_start = cue_end;
            }
        }
        CaptionTrack { cues }
    }

    /// The cue active at time `t`, if any (half-open: start ≤ t < end).
    pub fn active_at(&self, t: f32) -> Option<&Caption> {
        self.cues.iter().find(|c| t >= c.start && t < c.end)
    }

    /// Render the track as an SRT file.
    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, cue) in self.cues.iter().enumerate() {
            out.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                srt_timestamp(cue.start),
                srt_timestamp(cue.end),
                cue.text
            ));
        }
        out
    }

    /// Parse an SRT file into a track. Tolerant of CRLF, blank runs, and cues
    /// whose text spans several lines.
    pub fn from_srt(text: &str) -> Self {
        let mut cues = Vec::new();
        let normalized = text.replace("\r\n", "\n");
        for block in normalized.split("\n\n") {
            let mut lines = block.lines().filter(|l| !l.trim().is_empty());
            // An optional numeric index, then the time line.
            let first = match lines.next() {
                Some(l) => l.trim(),
                None => continue,
            };
            let time_line = if first.contains("-->") {
                first
            } else {
                match lines.next() {
                    Some(l) => l.trim(),
                    None => continue,
                }
            };
            let Some((start, end)) = parse_time_range(time_line) else {
                continue;
            };
            let text: String = lines.collect::<Vec<_>>().join("\n");
            if !text.trim().is_empty() {
                cues.push(Caption {
                    start,
                    end,
                    text: text.trim().to_string(),
                });
            }
        }
        CaptionTrack { cues }
    }
}

/// Where a project's caption track lives (§12).
pub fn captions_path(root: &Path) -> PathBuf {
    root.join("captions").join("captions.json")
}

/// The SRT sidecar, for editing in a subtitle tool or muxing on export (§14).
pub fn srt_path(root: &Path) -> PathBuf {
    root.join("captions").join("captions.srt")
}

/// Load a project's caption track (empty if none/invalid).
pub fn load_captions(root: &Path) -> CaptionTrack {
    std::fs::read_to_string(captions_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist a caption track as both JSON (editable data) and an SRT sidecar.
pub fn save_captions(root: &Path, track: &CaptionTrack) -> std::io::Result<()> {
    let json_path = captions_path(root);
    if let Some(parent) = json_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(track)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(json_path, json)?;
    std::fs::write(srt_path(root), track.to_srt())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Split text into lines no longer than `max` characters, breaking at word
/// boundaries. A single word longer than `max` is kept whole rather than cut.
fn split_lines(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= max {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(text.trim().to_string());
    }
    lines
}

/// Round seconds to whole milliseconds, so serialized times are stable.
fn round_ms(seconds: f32) -> f32 {
    (seconds * 1000.0).round() / 1000.0
}

/// Format seconds as an SRT timestamp `HH:MM:SS,mmm`.
fn srt_timestamp(seconds: f32) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// Parse `HH:MM:SS,mmm --> HH:MM:SS,mmm` into (start, end) seconds.
fn parse_time_range(line: &str) -> Option<(f32, f32)> {
    let (a, b) = line.split_once("-->")?;
    Some((parse_timestamp(a.trim())?, parse_timestamp(b.trim())?))
}

/// Parse a single SRT timestamp. Accepts `,` or `.` as the millisecond mark.
fn parse_timestamp(s: &str) -> Option<f32> {
    let s = s.replace(',', ".");
    let (hms, frac) = match s.split_once('.') {
        Some((hms, frac)) => (hms, frac),
        None => (s.as_str(), "0"),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<f32>().ok()?,
            m.parse::<f32>().ok()?,
            s.parse::<f32>().ok()?,
        ),
        [m, s] => (0.0, m.parse::<f32>().ok()?, s.parse::<f32>().ok()?),
        _ => return None,
    };
    let ms: f32 = format!("0.{frac}").parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec + ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Format, MotionProject, ProjectType, Scene};

    fn project_with_narration() -> MotionProject {
        let mut p = MotionProject::new("Demo", ProjectType::SketchExplainer, Format::Vertical);
        p.scenes.push(Scene {
            id: "s1".into(),
            name: String::new(),
            duration: 6.0,
            narration: Some("Your business may be losing stock without you noticing it.".into()),
            background: Default::default(),
            elements: vec![],
        });
        p.scenes.push(Scene {
            id: "s2".into(),
            name: String::new(),
            duration: 4.0,
            narration: None, // no narration -> no cues
            background: Default::default(),
            elements: vec![],
        });
        p.scenes.push(Scene {
            id: "s3".into(),
            name: String::new(),
            duration: 5.0,
            narration: Some("Track every item.".into()),
            background: Default::default(),
            elements: vec![],
        });
        p
    }

    #[test]
    fn generates_scene_aligned_cues() {
        let track = CaptionTrack::from_project(&project_with_narration());
        // Scene 2 has no narration, so no cue falls in [6, 10).
        assert!(track.active_at(7.0).is_none());
        // Scene 1's narration is captioned from t=0.
        let first = track.active_at(0.5).expect("a cue at 0.5s");
        assert!(first.text.to_lowercase().contains("business"));
        // Scene 3 starts at 6+4 = 10s.
        let s3 = track.active_at(11.0).expect("a cue in scene 3");
        assert_eq!(s3.text, "Track every item.");
        assert!(s3.start >= 10.0 && s3.end <= 15.0 + f32::EPSILON);
    }

    #[test]
    fn long_narration_splits_into_readable_lines() {
        let mut p = MotionProject::new("D", ProjectType::SocialShort, Format::Vertical);
        p.scenes.push(Scene {
            id: "s".into(),
            name: String::new(),
            duration: 8.0,
            narration: Some(
                "This is a deliberately long narration line that should wrap across \
                 several caption cues because no single caption should be too wide to read."
                    .into(),
            ),
            background: Default::default(),
            elements: vec![],
        });
        let track = CaptionTrack::from_project(&p);
        assert!(track.cues.len() >= 2, "expected multiple cues");
        for cue in &track.cues {
            assert!(
                cue.text.chars().count() <= MAX_LINE_CHARS + 1,
                "line too long: {}",
                cue.text
            );
            assert!(cue.end > cue.start);
        }
        // Cues tile the scene without gaps or overlap, ending exactly at 8s.
        assert!((track.cues.last().unwrap().end - 8.0).abs() < 0.01);
        for pair in track.cues.windows(2) {
            assert!(
                (pair[0].end - pair[1].start).abs() < 0.001,
                "cues must abut"
            );
        }
    }

    #[test]
    fn srt_round_trips() {
        let track = CaptionTrack::from_project(&project_with_narration());
        let srt = track.to_srt();
        assert!(srt.contains("-->"));
        assert!(srt.contains("00:00:0"));
        let reparsed = CaptionTrack::from_srt(&srt);
        assert_eq!(reparsed.cues.len(), track.cues.len());
        // Times survive the round-trip to millisecond precision.
        for (a, b) in track.cues.iter().zip(&reparsed.cues) {
            assert!(
                (a.start - b.start).abs() < 0.001,
                "{} vs {}",
                a.start,
                b.start
            );
            assert!((a.end - b.end).abs() < 0.001);
            assert_eq!(a.text, b.text);
        }
    }

    #[test]
    fn parses_srt_with_or_without_indices_and_crlf() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\n\r\n\
                   00:00:03,000 --> 00:00:04,000\r\nNo index here\r\n";
        let track = CaptionTrack::from_srt(srt);
        assert_eq!(track.cues.len(), 2);
        assert_eq!(track.cues[0].text, "Hello");
        assert!((track.cues[0].start - 1.0).abs() < 0.001);
        assert!((track.cues[0].end - 2.5).abs() < 0.001);
        assert_eq!(track.cues[1].text, "No index here");
    }

    #[test]
    fn timestamp_formatting_is_srt_shaped() {
        assert_eq!(srt_timestamp(0.0), "00:00:00,000");
        assert_eq!(srt_timestamp(1.5), "00:00:01,500");
        assert_eq!(srt_timestamp(3661.25), "01:01:01,250");
    }

    #[test]
    fn saves_json_and_srt_sidecars() {
        let dir = std::env::temp_dir().join(format!("kestrel-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let track = CaptionTrack::from_project(&project_with_narration());
        save_captions(&dir, &track).unwrap();
        assert!(captions_path(&dir).exists());
        assert!(srt_path(&dir).exists());
        let loaded = load_captions(&dir);
        assert_eq!(loaded, track);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
