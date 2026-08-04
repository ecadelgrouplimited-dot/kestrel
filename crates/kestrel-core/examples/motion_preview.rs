//! Render a sample Motion project to `preview.html` for eyeballing the renderer.
//!
//! `cargo run -p kestrel-core --example motion_preview -- <out-dir>`
//! Writes <out-dir>/output/preview.html and prints its path.

use kestrel_core::motion::MotionProject;
use kestrel_core::motion_render::write_preview;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let json = r##"{
      "schemaVersion": "1.0",
      "project": { "title": "The Missing Stock", "type": "sketch-explainer",
        "format": "vertical", "width": 1080, "height": 1920, "fps": 30, "theme": "" },
      "scenes": [
        { "id": "scene-01", "name": "The Hook", "duration": 4,
          "narration": "Your business may be losing stock without you noticing.",
          "background": { "type": "solid", "color": "#F7F4EC" },
          "elements": [
            { "id": "title", "type": "title", "content": "Where did the stock go?",
              "position": { "x": 110, "y": 320 }, "size": { "width": 860, "height": 150 },
              "animation": { "type": "handwrite", "start": 0.3, "duration": 1.2 } },
            { "id": "owner", "type": "sketch-character", "character": "shop-owner",
              "pose": "confused", "position": { "x": 250, "y": 980 },
              "animation": { "type": "pop", "start": 1.2, "duration": 0.5 } },
            { "id": "shelf", "type": "image", "position": { "x": 620, "y": 1010 },
              "size": { "width": 280, "height": 220 },
              "animation": { "type": "fade", "start": 1.6, "duration": 0.6 } },
            { "id": "arrow", "type": "sketch-arrow", "from": "owner", "to": "shelf",
              "animation": { "type": "draw", "start": 2.2, "duration": 0.8 } }
          ] },
        { "id": "scene-02", "name": "The Cost", "duration": 4,
          "narration": "Unrecorded stock is money leaving without a trace.",
          "background": { "type": "gradient", "from": "#1b2a4a", "to": "#0a0f1e", "angle": 90 },
          "elements": [
            { "id": "stat", "type": "title", "content": "30% of losses go unnoticed",
              "position": { "x": 120, "y": 760 }, "size": { "width": 840, "height": 200 },
              "animation": { "type": "slide", "start": 0.2, "duration": 0.8 }, "color": "#ffffff" },
            { "id": "cta", "type": "cta", "content": "Track every item →",
              "position": { "x": 240, "y": 1500 }, "size": { "width": 600, "height": 110 },
              "animation": { "type": "fade", "start": 1.4, "duration": 0.6 }, "color": "#DC8D1F" }
          ] }
      ] }"##;

    let project = MotionProject::from_json(json).expect("sample must parse");
    let path = write_preview(std::path::Path::new(&out), &project).expect("write preview");
    println!("{}", path.display());
}
