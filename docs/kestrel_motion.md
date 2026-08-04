# KESTREL MOTION — PRODUCT ENGINEERING IMPLEMENTATION DIRECTIVE

## Product Context

Kestrel is becoming a unified agentic productivity platform with three major capabilities:

1. **Kestrel Build** — autonomous coding agents that create, test, inspect, repair and deliver complete software projects.
2. **Kestrel Co-worker** — an agentic workspace for daily computer work, including creating and editing documents, managing files and completing multi-step tasks.
3. **Kestrel Motion** — a code-native visual-content system for creating explainer videos, product tutorials, social shorts, sketch-style videos, presentations and other motion content.

Kestrel Motion must be implemented as a native Kestrel capability using the same agent runtime, project system, file access, verification loop and local execution environment already available to Kestrel Build.

It must not feel like a separate AI video website placed inside Kestrel.

---

# 1. Product Mission

Build a code-native video-production environment where a user can describe a video in natural language and Kestrel can:

* Write or improve the script.
* Divide the script into scenes.
* Develop a storyboard.
* Generate visual assets using code.
* Animate text, diagrams, characters, screenshots and interface elements.
* Synchronize scenes with voice-over.
* Generate captions.
* Preview and inspect the video.
* Detect and repair visual problems.
* Export the completed project as a standard video file.

The primary product promise is:

> Describe the video you need. Kestrel plans it, creates its assets, animates every scene, verifies the result and delivers an editable video project.

Kestrel Motion must prioritize deterministic, editable and reusable video creation over depending on AI-generated video clips.

AI-generated images and short video clips may be supported as optional assets, but they must not be the foundation of the system.

---

# 2. Initial Use Cases

The first implementation must concentrate on four project types:

## A. Sketch Explainer

Animated hand-drawn shapes, arrows, diagrams, characters, text, charts and visual metaphors.

Example:

> Create a 60-second sketch explainer showing how poor stock management causes business losses.

## B. Product Tutorial

A structured walkthrough containing screenshots, screen recordings, cursor movement, zooms, callouts, highlights, captions and narration.

Example:

> Create a tutorial showing users how to migrate product data into Smart Business Book.

## C. Social Short

Fast-paced vertical videos with strong hooks, animated captions, branded scenes and calls to action.

Example:

> Create a 30-second vertical short explaining three signs that a business is losing stock.

## D. Presentation Video

Animated business presentations, product announcements, reports and educational content.

Example:

> Turn this business report into a three-minute narrated presentation video.

---

# 3. Core Product Principle

The system must not generate one large, fragile video file or one uncontrolled block of animation code.

Each video must be stored as a structured Kestrel Motion project made up of:

* Project configuration
* Script
* Storyboard
* Scene definitions
* Visual assets
* Brand theme
* Audio
* Captions
* Render configuration
* Verification results
* Exported files

Every scene must be independently editable, regeneratable, previewable and testable.

A user must be able to tell Kestrel:

* “Make scene three shorter.”
* “Replace the chart in scene five.”
* “Use a sketch character instead of an icon.”
* “Change this project to vertical format.”
* “Make the captions larger.”
* “Regenerate only the introduction.”
* “Add my voice-over and synchronize the scenes.”
* “Translate this video into Swahili.”

Kestrel must perform the requested change without unnecessarily rebuilding the entire project.

---

# 4. Structured Scene Language

Create a versioned Kestrel Motion Scene Schema.

The AI should generate this structured scene representation by default instead of producing unrestricted rendering code.

Example:

```json
{
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
      "background": {
        "type": "solid",
        "color": "#F7F4EC"
      },
      "elements": [
        {
          "id": "title-01",
          "type": "text",
          "content": "Where did the stock go?",
          "position": {
            "x": 540,
            "y": 280
          },
          "animation": {
            "type": "handwrite",
            "start": 0.5,
            "duration": 1.2
          }
        },
        {
          "id": "character-01",
          "type": "sketch-character",
          "character": "shop-owner",
          "pose": "confused",
          "position": {
            "x": 350,
            "y": 1050
          }
        },
        {
          "id": "arrow-01",
          "type": "sketch-arrow",
          "from": "character-01",
          "to": "empty-shelf-01",
          "animation": {
            "type": "draw",
            "start": 2,
            "duration": 0.8
          }
        }
      ]
    }
  ]
}
```

The schema must support validation, migrations and backwards compatibility.

The renderer must consume this schema and convert it into frames without requiring the AI to reinvent rendering logic for every project.

Advanced users may be allowed to add custom TypeScript components, but unrestricted code must not be the default output.

---

# 5. Required System Architecture

Separate Kestrel Motion into clear modules.

## A. Motion Project Manager

Responsible for:

* Creating projects
* Opening projects
* Saving project state
* Versioning changes
* Duplicating projects
* Managing autosave
* Restoring previous versions
* Managing project assets

## B. Motion Schema

Responsible for:

* Project definitions
* Scene definitions
* Timeline objects
* Animation definitions
* Audio tracks
* Captions
* Brand themes
* Export settings
* Schema validation
* Schema migrations

## C. Motion Agent

Responsible for:

* Understanding the user’s objective
* Writing scripts
* Creating storyboards
* Selecting scene types
* Selecting visual metaphors
* Generating structured scenes
* Editing existing scenes
* Responding to natural-language revisions
* Repairing failed scenes
* Preserving unaffected scenes during revisions

## D. Motion Component Library

Create reusable components for:

* Text
* Titles
* Captions
* Images
* Video
* Audio
* Shapes
* Icons
* Arrows
* Connectors
* Charts
* Tables
* Diagrams
* Code blocks
* Screenshots
* Browser frames
* Device frames
* Cursor animations
* Callouts
* Highlights
* Sketch characters
* Speech bubbles
* Progress indicators
* Logos
* Intros
* Outros
* Calls to action

Every component must have documented properties, supported animations and editable parameters.

## E. Motion Renderer

Responsible for:

* Live preview
* Frame calculation
* Scene composition
* Animation interpolation
* Asset loading
* Audio playback
* Caption rendering
* Final frame generation
* MP4/WebM export

The rendering layer must be implemented behind an adapter interface so that the underlying rendering technology can be changed later.

The engineering team should evaluate:

* Remotion
* Motion Canvas
* Browser-based SVG/Canvas rendering
* FFmpeg
* Rough.js

A practical first implementation may use a TypeScript-based video framework, Rough.js for sketch visuals and FFmpeg for final encoding.

Do not bind the Kestrel Motion project format directly to any third-party renderer.

Complete a licensing and technical suitability review before making a third-party renderer part of the commercial product.

## F. Motion Verification Engine

Responsible for automated video QA.

It must inspect:

* Text overflow
* Text outside safe areas
* Insufficient contrast
* Missing assets
* Broken scene references
* Empty or frozen scenes
* Incorrect durations
* Caption overflow
* Caption timing
* Abrupt cuts
* Audio clipping
* Missing audio
* Unsupported fonts
* Incorrect resolution
* Export failures
* Excessive file size
* Invalid final output

The verification engine must generate actionable issues that the Kestrel agent can repair.

---

# 6. Sketch Animation System

The sketch system is a first-class Kestrel Motion capability.

Implement reusable sketch primitives for:

* Hand-drawn lines
* Rectangles
* Circles
* Ellipses
* Arrows
* Connectors
* Underlines
* Highlights
* Checkmarks
* Crosses
* Charts
* Flow diagrams
* Speech bubbles
* Thought bubbles
* Paper textures
* Handwriting effects
* Object-drawing animations

Use SVG and Canvas-based techniques so these assets remain resolution-independent and editable.

The sketch system must support deterministic randomization. A sketch element must look identical every time the same project is rendered unless the user explicitly regenerates it.

Do not use uncontrolled randomness during frame rendering.

---

# 7. Character System

Implement a reusable vector-character system rather than generating a completely new character image for every scene.

Characters should support:

* Saved identity
* Brand colours
* Head and body structure
* Facial expressions
* Arm positions
* Hand positions
* Basic poses
* Pointing
* Walking
* Thinking
* Speaking
* Celebrating
* Confusion
* Holding objects
* Looking in different directions

The initial version does not require advanced skeletal animation.

Start with configurable character parts and a controlled pose library. More sophisticated rigging can be introduced later.

---

# 8. Voice-over and Audio

Voice-over timing must be part of the project architecture from the beginning.

Support:

* Voice-over upload
* Voice recording
* Optional text-to-speech
* Background music
* Sound effects
* Multiple audio tracks
* Volume adjustment
* Fade-in and fade-out
* Muting
* Trimming
* Audio waveform display

Required workflow:

1. Generate the script.
2. Estimate scene durations.
3. Allow narration to be uploaded or recorded.
4. Determine the actual narration duration.
5. Align narration with scenes.
6. Adjust scene timings.
7. Generate captions.
8. Let the user manually refine timing.
9. Mix and export the final audio.

The first release may use scene-level synchronization. Word-level synchronization can follow after the initial system is stable.

---

# 9. Captions

Implement a dedicated caption system supporting:

* Automatic caption generation
* Manual editing
* SRT import and export
* Word or phrase highlighting
* Multiple caption styles
* Position controls
* Safe-area validation
* Brand colours and fonts
* Vertical-video caption layouts
* Caption timing adjustments
* Multilingual captions

Captions must remain editable data and must not be permanently embedded into source screenshots or generated images.

---

# 10. Product Tutorial Creation

Kestrel Motion must reuse Kestrel’s existing browser and computer-control capabilities.

The tutorial workflow should eventually allow Kestrel to:

* Open a website or application.
* Follow a defined task.
* Capture relevant screens.
* Record a walkthrough.
* Hide or blur sensitive information.
* Add cursor movement.
* Highlight clicked controls.
* Zoom into important areas.
* Add arrows and callouts.
* Generate narration.
* Add chapters.
* Export the final tutorial.

Kestrel must not expose passwords, tokens, customer records or other sensitive information in captured tutorials.

The first implementation can use imported screenshots and recordings before fully automated capture is introduced.

---

# 11. Kestrel Motion Interface

Add a dedicated Motion workspace to Kestrel Build.

The initial interface should contain:

## Left Panel

* Project files
* Scenes
* Assets
* Brand kit
* Templates

## Centre

* Video preview
* Canvas controls
* Play and pause
* Current time
* Resolution preview
* Safe-area overlay

## Bottom Panel

* Scene timeline
* Audio tracks
* Caption track
* Scene durations
* Reordering controls

## Right Panel

* Selected-element properties
* Position
* Size
* Colour
* Typography
* Animation
* Start time
* Duration
* Layer controls

## Agent Panel

The existing Kestrel conversational interface should remain available for natural-language instructions.

Users should be able to edit through either:

* Natural-language commands
* Visual controls
* Structured scene data
* Code, where appropriate

All editing methods must update the same underlying project state.

---

# 12. Project Structure

Use a project structure similar to:

```text
project-name/
├── motion.project.json
├── script/
│   ├── brief.md
│   └── narration.md
├── storyboard/
│   └── storyboard.json
├── scenes/
│   ├── scene-01.json
│   ├── scene-02.json
│   └── scene-03.json
├── components/
│   └── custom-components.tsx
├── assets/
│   ├── images/
│   ├── icons/
│   ├── characters/
│   ├── screenshots/
│   ├── video/
│   └── audio/
├── theme/
│   └── brand-theme.json
├── captions/
│   └── captions.json
├── verification/
│   └── latest-report.json
└── output/
    └── final-video.mp4
```

The exact structure may be adjusted during implementation, but scripts, scenes, assets, themes, captions, verification and outputs must remain logically separated.

---

# 13. Brand Kits

Users must be able to create reusable brand kits containing:

* Brand name
* Logo
* Colours
* Typography
* Watermark
* Character assets
* Intro
* Outro
* CTA
* Caption style
* Transition style
* Music preferences

A user should be able to say:

> Apply the Smart Business Book brand kit.

Kestrel should then apply the saved brand consistently without modifying the underlying message.

---

# 14. Export Requirements

The initial release must export:

* MP4 using H.264 video
* AAC audio
* 1080 × 1920 vertical
* 1920 × 1080 horizontal
* 1080 × 1080 square
* User-selectable frame rate
* User-selectable quality
* Silent video where required
* Video with mixed narration and music
* SRT caption file
* Individual scene previews

Rendering should run locally for the initial Kestrel Build implementation.

Render progress, errors and cancellation must be visible to the user.

A failed render must produce a useful diagnostic report and allow Kestrel to attempt repair.

---

# 15. Security and Reliability

AI-generated code must not be executed without controls.

Implement:

* Schema validation
* Asset-path validation
* Restricted rendering environment
* Network restrictions during rendering
* Execution timeouts
* Memory limits
* Font validation
* Media-format validation
* Safe file access
* Dependency controls
* Render cancellation

Normal users should primarily use approved scene components.

Custom code should run in an isolated environment and require stricter validation.

Every video project must render deterministically. The same project, assets and render configuration should produce the same output.

---

# 16. First MVP Scope

The first MVP must create:

* Sketch explainers
* Social shorts
* 30–90 second videos
* Vertical and horizontal formats
* Four to ten scenes
* Animated text
* Sketch shapes and arrows
* Icons and screenshots
* Basic charts
* Uploaded voice-over
* Scene-level audio synchronization
* Automatic captions
* Brand kits
* Local preview
* Local MP4 export
* Automated visual verification
* Scene-level regeneration

Do not attempt to build a complete CapCut, Canva or Adobe Premiere replacement in the first release.

Do not prioritize:

* Advanced nonlinear editing
* Complex 3D animation
* Hollywood visual effects
* Real-time collaboration
* Cloud rendering
* A public template marketplace
* Advanced character rigging
* Mobile editing

Those capabilities may be considered after the core generation and verification workflow is proven.

---

# 17. MVP Acceptance Test

The MVP is complete when the following instruction can be executed successfully:

> Create a 45-second vertical sketch-style explainer titled “The Missing Stock.” Explain how businesses lose money when stock records are not updated. Use the Smart Business Book brand kit, five scenes, animated captions, sketch arrows, a shop-owner character and a final call to action. Leave space for voice-over, verify every scene and export the final video as a 1080 × 1920 MP4.

The resulting project must:

* Contain an editable script.
* Contain five separate scenes.
* Use reusable components.
* Apply the correct brand kit.
* Preview correctly.
* Pass safe-area validation.
* Contain no overflowing text.
* Accept an uploaded voice-over.
* Adjust scene timings after audio is added.
* Generate editable captions.
* Allow one scene to be regenerated independently.
* Export a valid MP4.
* Produce the same visual result when rendered again.
* Preserve the complete editable source project.

---

# 18. Implementation Phases

## Phase 0: Technical Validation

Deliver:

* Renderer comparison
* Licensing review
* Scene-schema proposal
* Basic rendering proof of concept
* Rough.js sketch test
* FFmpeg export test
* Audio synchronization test
* Architectural decision records

## Phase 1: Rendering Foundation

Deliver:

* Motion project format
* Scene schema
* Renderer adapter
* Text, image and shape components
* Basic animation system
* Local preview
* MP4 export

## Phase 2: Agentic Creation

Deliver:

* Script-generation workflow
* Storyboarding
* Prompt-to-scene generation
* Natural-language scene editing
* Scene regeneration
* Project-level planning

## Phase 3: Sketch and Branding

Deliver:

* Sketch primitives
* Drawing animations
* Basic character system
* Brand kits
* Reusable intros, outros and CTAs

## Phase 4: Audio and Captions

Deliver:

* Audio upload
* Waveform display
* Voice-over synchronization
* Caption generation
* Caption editor
* Audio mixing

## Phase 5: Verification

Deliver:

* Visual overflow checks
* Safe-area checks
* Missing-asset detection
* Timing validation
* Audio validation
* Automated repair loop
* Export verification

## Phase 6: Tutorial Production

Deliver:

* Screenshot import
* Screen-recording import
* Browser and device frames
* Cursor animations
* Zooms
* Highlights
* Callouts
* Automated walkthrough capture

---

# 19. Required Team Deliverables

Before beginning full implementation, the engineering team must produce:

1. Product Requirements Document.
2. Technical architecture document.
3. Renderer decision and licensing report.
4. Version-one Motion Scene Schema.
5. Component-library specification.
6. Interface wireframes.
7. Security and sandboxing plan.
8. Render-performance plan.
9. Phased implementation backlog.
10. Testing and acceptance plan.

Each deliverable must identify assumptions, dependencies, risks and unresolved decisions.

---

# 20. Final Engineering Direction

Kestrel Motion is not simply a video editor.

It is an autonomous visual-production agent built on the strengths Kestrel already possesses:

* Planning
* Coding
* File generation
* Browser and computer control
* Visual inspection
* Testing
* Error recovery
* Iterative improvement
* Project delivery

Treat every video as a structured software project.

Treat every scene as a testable unit.

Treat assets as reusable project components.

Treat the Kestrel agent as the scriptwriter, storyboard artist, animator, editor and quality-control system.

The objective is not merely to generate a video.

The objective is for Kestrel to understand the communication goal, construct the complete visual project, verify that it works and deliver an editable, reusable and exportable result.
