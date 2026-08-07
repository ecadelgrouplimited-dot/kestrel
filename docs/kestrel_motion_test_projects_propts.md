# Kestrel Motion — test project prompts

Powerful, demo-ready prompts for exercising **Kestrel Motion**, the code-native
video system. Each is a real brief a user would type, paired with *what it tests*
and *measurable success criteria* so a run can be judged pass/fail rather than
"looks nice".

Motion is not an AI-video-clip generator. It builds a **structured, editable,
verifiable project of scenes** — script, storyboard, scenes, brand, captions,
audio, verification, and an exported MP4 — and the agent is the scriptwriter,
storyboard artist, animator, and QA. These prompts are designed to prove that.

---

## How to run these

**Desktop app:** open or create a project folder → click **▶ Motion** → describe
the video in the composer with **Agent · write files** on. The left panel shows
the project's scenes, verification badge, caption/voiced counts, and
**Preview / Verify / Export MP4** buttons; use **New video…** for a titled
project in its own folder.

**CLI:** run `kestrel`, then `/motion`, then type the brief.

The agent's own toolchain (what these prompts drive): `motion_new` · `write_scene`
(add/replace one scene by id) · `verify_motion` · `caption_motion` ·
`voice_motion` · `brand_motion` · `preview_motion` · `render_motion` (MP4).

**Recording tip:** the live HTML preview plays entry animations and captions on a
real timeline with a scrubber and safe-area/CC toggles — record *that* for motion.
The exported MP4 is a settled, branded, captioned cut with real voice-over —
record that as the "deliverable".

**Universal success criteria** (apply to every prompt below unless overridden):
- `verify_motion` passes (no errors) before the agent calls the work done.
- The preview is one self-contained `output/preview.html` — no external URLs.
- Re-rendering the same project is **deterministic** (byte-identical SVG/preview).
- The complete editable source project is preserved (script, scenes, theme,
  captions), not just an output file.

---

## 1. The MVP acceptance test — "The Missing Stock"

The canonical end-to-end brief (from the directive, §17). Everything at once.

> Create a 45-second vertical sketch-style explainer titled **"The Missing
> Stock."** Explain how businesses lose money when stock records aren't updated.
> Use five scenes, animated captions, hand-drawn sketch arrows, a shop-owner
> character and a final call to action. Apply a **Smart Business Book** brand kit
> (warm, trustworthy). Leave room for voice-over, verify every scene, and export
> the final video as a 1080×1920 MP4.

- **Tests:** the whole pipeline — project scaffold, five independently editable
  scenes, the hand-drawn sketch system, brand kit, generated captions,
  verification/repair, and MP4 export — in one run.
- **Success criteria:**
  - The project contains an editable script (`script/brief.md`), **five** scenes,
    a brand kit at `theme/brand-theme.json`, and captions (`captions/captions.json`
    + `.srt`).
  - Scenes use hand-drawn kinds (`sketch-arrow`, `sketch-rect`/`sketch-circle`,
    `checkmark`/`cross`), not clip-art geometry, and a `sketch-character`.
  - No overflowing text; every readable element inside the 5% safe area; vertical
    captions above the bottom 12%.
  - `output/final-video.mp4` exists and ffprobe reports **H.264 video + AAC audio
    at 1080×1920**, duration ≈ the sum of scene durations.
  - Ask the agent to **regenerate just scene 3** afterward — the git/file diff
    shows only that scene's JSON changed.

---

## 2. One prompt per project type

Kestrel Motion supports four kinds; each should pick suitable components.

### 2.1 Sketch explainer *(hand-drawn, the flagship)*
> Make a 30-second vertical sketch explainer: **"Three signs your stock is
> walking out the door."** One hook scene, one scene per sign with a sketched
> icon and a checkmark, and a closing CTA "Track every item." Whiteboard look,
> animated captions, room for voice-over.
- **Success:** every visual is a hand-drawn kind (rough, wobbly strokes — not
  smooth vectors); three sign-scenes plus hook and CTA; verifies clean; the
  preview reads as a whiteboard sketch.

### 2.2 Product tutorial *(screens, cursor, callouts)*
> Create a 60-second horizontal product tutorial: **"Import your products into
> Smart Business Book."** Walk through four steps inside a browser window with a
> cursor moving to each control and a callout naming the action. End on a success
> checkmark.
- **Success:** uses `browser-frame` (with `url: "smartbusinessbook.io"`),
  `cursor`, and `callout` components; four step-scenes; 1920×1080; the cursor and
  callouts land inside the frame; ends on the `smartbusinessbook.io` CTA; verifies
  clean.

### 2.3 Social short *(fast, punchy, vertical)*
> A 20-second vertical social short with a strong hook, three fast stat cards,
> and a bold CTA. Big animated captions, high energy, on-brand colours.
- **Success:** 1080×1920; 4–6 short scenes (~3s each); a `chart` or stat `title`
  cards; captions present; total ≈ 20s; verifies clean.

### 2.4 Presentation video *(data, charts, calm)*
> Turn this into a 3-minute narrated presentation video: quarterly revenue up 18%,
> churn down to 4%, two new markets opened. Use bar and line charts, section
> titles, and a calm, professional theme.
- **Success:** uses `chart` (`chartKind` bar **and** line) with real `data`
  arrays; section scenes; 1920×1080; charts render with value + category labels;
  no `empty-chart` warnings; verifies clean.

---

## 3. Capability spotlights

Each isolates one subsystem so a demo can show it cleanly.

### 3.1 Brand kit — apply, then re-brand
> Build a 6-scene vertical explainer about cutting waste. First apply a brand kit
> called **"sbb"**: gold `#DC8D1F`, dark ground, a watermark "Smart Business
> Book". Then, without changing the message, re-brand it to **"midnight"**: deep
> blue ground `#0e1b2a`, cyan accent `#22d3ee`, no watermark.
- **Tests:** §13 — a brand recolours without touching the message; `theme`
  background inheritance; partial-update merge.
- **Success:** `theme/brand-theme.json` reflects the final kit exactly (check the
  hex values, don't eyeball); scenes using `background:{type:"theme"}` change
  ground colour between brands; the scene **text/narration is identical** across
  both brands (git diff on scene files shows only the theme reference / colours,
  not the words); watermark present in v1, absent in v2.

### 3.2 Captions — generate, edit, export SRT, translate
> Caption the video from the narration. Then export the captions as SRT, make the
> caption lines shorter for readability, and produce a **Swahili** caption track
> alongside the English one.
- **Tests:** §9 — captions as editable data, SRT round-trip, multilingual.
- **Success:** `captions/captions.srt` is valid SRT (numbered cues,
  `HH:MM:SS,mmm --> …`); cues are scene-aligned; captions overlay live in the
  preview (toggle **CC**) and are **not** baked into the scene SVGs; a second
  language track exists; times survive an SRT export→import round-trip.

### 3.3 Voice-over — attach and re-time
> Here are five narration clips (`assets/audio/scene-01.wav` … `scene-05.wav`).
> Attach each to its scene, set each scene's duration to match its clip, then
> re-caption so the captions realign, and export the MP4 with the voice-over.
- **Tests:** §8 — determine clip duration (ffprobe), align scene timing, mix
  audio into the export.
- **Success:** each scene's `duration` equals its clip length (± a few ms); the
  panel shows **N/N voiced**; the exported MP4's audio stream is **not silent**
  (`volumedetect` mean well above −80 dB) and its duration equals the summed clip
  lengths; captions still cover the timeline after re-timing.

### 3.4 Charts — real data, bar and line
> Make a horizontal data explainer from this table: Jan 32, Feb 48, Mar 41, Apr
> 67, May 59. Show it first as a bar chart, then as a line chart on the next
> scene, with the peak month called out.
- **Success:** two `chart` elements with the same `data`, `chartKind` bar then
  line; value labels (32…67) and month labels render; a `callout` points at the
  peak; verifies clean (no `empty-chart`).

### 3.5 Sketch system — deterministic hand-drawn art
> Build a one-scene whiteboard diagram: two labelled boxes joined by a hand-drawn
> arrow, a circled key term, a highlighter swipe under the takeaway, a green tick
> and a red cross. Then render it twice and confirm it's identical both times.
- **Tests:** §6 — the sketch primitives and deterministic randomness.
- **Success:** boxes/arrow/circle are visibly *rough* (double-stroke, wobbly);
  the two renders are **byte-identical**; changing one element's id changes only
  that element's wobble.

---

## 4. Editing & revision — the structured-format payoff

These prove Kestrel changes what you ask without rebuilding the project.

### 4.1 Regenerate one scene
> Regenerate only scene 4 — make it a customer testimonial with a speech bubble —
> and leave every other scene exactly as it is.
- **Success:** only scene 4's JSON changes on disk; the rest are byte-identical;
  verifies clean.

### 4.2 Retime and re-flow
> Scene 2 feels rushed. Make it 2 seconds longer and shorten the closing scene by
> the same amount so the total runtime is unchanged. Re-caption to match.
- **Success:** the two scene durations change, total is preserved, captions
  realign to the new timings.

### 4.3 Reformat
> Turn this vertical short into a 1920×1080 horizontal version without losing any
> content — reposition elements so nothing falls outside the safe area.
- **Success:** project dimensions become 1920×1080; every element passes safe-area
  and off-canvas verification in the new frame; captions reposition for landscape.

### 4.4 Swap a component
> Replace the icon in scene 3 with a hand-drawn sketch character pointing at the
> chart, and make the chart a line chart instead of bars.
- **Success:** scene 3 now has a `sketch-character` and a line `chart`; only scene
  3 changed; verifies clean.

---

## 5. Verification & repair — trust the QA

Deliberately hand Kestrel broken input and require a clean repair.

### 5.1 Fix the broken video
> This project has problems — text spilling off the edge, an arrow pointing at a
> scene that no longer exists, and a scene with no elements. Find every issue and
> fix it, then confirm it's clean.
- **Tests:** the verification engine (§5-F) and the repair loop.
- **Success:** the agent lists the concrete issues (off-canvas / outside-safe-area,
  `broken-reference`, `empty-scene`), fixes each with targeted `write_scene`
  edits, and `verify_motion` ends with **no errors**.

### 5.2 Don't ship it broken
> Build a quick 4-scene explainer and export it.
- **Success (behavioural):** the agent does **not** call the video done or export
  while `verify_motion` reports errors — it repairs first. A clean pass is the bar
  for "done".

---

## 6. Export & delivery

### 6.1 Every format
> Export this project three ways: 1080×1920 vertical, 1920×1080 horizontal, and
> 1080×1080 square — each a valid MP4.
- **Success:** three MP4s; ffprobe confirms each resolution, H.264 + AAC; content
  reflows to fit each frame (no letterboxed clipping of readable text).

### 6.2 Deliverable pass
> Finish the video: verify, caption, apply the brand, preview it, and export the
> final MP4. Tell me exactly what you produced and where.
- **Success:** the run ends with a clean verify, a preview, a branded captioned
  MP4, and a summary listing the scene count, runtime, verification result, and
  file paths — plus what the user should do next (record/upload voice-over).

---

## 7. Flagship — "Why Kestrel Motion" explainer

The capstone: Kestrel Motion explaining *itself*, the way the flagship website
prompt has Kestrel Build sell itself. It must communicate the product's story
through its own features.

> Create a 60-second horizontal explainer titled **"Describe it. Kestrel builds
> the video."** that pitches Kestrel Motion to a founder who's tired of fighting
> CapCut. Tell the story through the product's real strengths, one scene each:
>
> - **It's a project, not a blob.** Every scene is structured data you can edit —
>   "make scene 3 shorter", "swap the chart" — not one fragile timeline.
> - **It verifies itself.** Off-safe-area text, broken references, overflowing
>   timing — caught and repaired before you ever hit export.
> - **On brand, automatically.** One brand kit recolours the whole video without
>   touching the message.
> - **Real voice-over and captions.** Attach narration, the scenes time to it,
>   captions generate and export as SRT.
> - **A real MP4, locally.** H.264 + AAC, no cloud round-trip.
>
> Use charts to show the "edit one scene" idea, a browser-frame for the app, a
> hand-drawn accent, animated captions, and a confident CTA: "Open Kestrel →
> Describe your video." Apply the Kestrel brand (gold `#DC8D1F` on near-black
> `#0A0A0B`). Verify, preview, and export a 1920×1080 MP4.
- **Tests:** everything — narrative authoring, charts, tutorial frames, the sketch
  accent, brand, captions, verification, and export, in a piece that has to be
  *persuasive*, not just correct.
- **Success criteria:**
  - Every scene maps to a **real** Kestrel Motion feature — no invented claims
    (cross-check against the tools/components that exist).
  - Uses at least a `chart`, a `browser-frame`, a hand-drawn element, and animated
    captions; on the Kestrel brand (hex exact).
  - Verifies clean; preview plays end-to-end; a valid 1920×1080 H.264/AAC MP4 is
    exported.
  - A founder watching it can answer: *"What is Kestrel Motion, and why is it
    better than a timeline editor?"*

---

## 8. Smart Business Book — one pain, one fix, one CTA

A campaign of short **vertical sketch explainers**, each aimed at a single
business problem a small shop, vendor, or service owner feels — and how **Smart
Business Book (SBB)** fixes it, ending on one clear call to action. Whiteboard
look throughout: hand-drawn boxes, arrows, checkmarks/crosses, a shop-owner
`sketch-character`, animated captions, room for voice-over.

All of these follow the **§2.1 sketch-explainer success criteria** (every visual
is a hand-drawn kind, verifies clean, whiteboard feel) **plus**: the video names
a *real, relatable* business pain, shows the SBB fix as the turn, and ends on
**one** unambiguous CTA. Aim for **25–35s, 5–6 scenes, 1080×1920**. Use a
consistent SBB brand kit across the set (warm gold on near-black, a "Smart
Business Book" watermark) so they read as one campaign.

**CTA convention (all sections):** every CTA ends on the site —
**`smartbusinessbook.io`** — as the final on-screen line (a `cta` component),
spoken in the last narration line, and set as the `browser-frame` `url` wherever
the app appears. Keep it short: an action + the domain, e.g. *"Know your stock —
smartbusinessbook.io."*

### 8.1 Stock — never run out, never overstock
> 30-second vertical sketch explainer: **"Stop guessing your stock."** Hook: a
> shop owner staring at half-empty shelves, a customer walking out because the
> item's gone. Problem: ordering by memory means dead stock on some shelves and
> empty gaps on others — money frozen or money lost. Turn: SBB tracks every item
> in and out and flags what to reorder and when. Close on a shelf that's just
> right, with a checkmark. **CTA: "Know your stock — smartbusinessbook.io."**

### 8.2 Debtors — who owes you money?
> 30-second vertical sketch explainer: **"Your money is in other people's
> pockets."** Hook: a friendly "pay you next week" that never comes; a notebook
> full of scribbled IOUs. Problem: credit sales you can't track become sales you
> never collect. Turn: SBB records every credit sale and shows exactly who owes
> what, so you can follow up. Close on a paid-up ledger with a green checkmark.
> **CTA: "Get paid what you're owed — smartbusinessbook.io."**

### 8.3 Cash flow — are you actually making a profit?
> 30-second vertical sketch explainer: **"Busy isn't the same as profitable."**
> Hook: a packed shop, a smiling owner — then an empty cash box. Problem: strong
> sales can still hide a loss when you don't know your real costs. Turn: SBB puts
> sales, costs, and profit on one screen so you see what you actually keep. Close
> on a clear "profit up" line chart. **CTA: "See your real profit — smartbusinessbook.io."**

### 8.4 Expenses — where did the money go?
> 25-second vertical sketch explainer: **"Small leaks sink the boat."** Hook:
> coins slipping through a hand — a little here, a little there. Problem:
> untracked small expenses quietly eat the month's profit. Turn: SBB logs every
> expense and shows where the money really goes, in a quick bar chart. Close on a
> plugged leak with a checkmark. **CTA: "Plug the leaks — smartbusinessbook.io."**

### 8.5 Getting paid — send invoices, get paid faster
> 30-second vertical sketch explainer: **"Chasing payments all day?"** Hook: an
> owner on the phone, again, asking for money owed. Problem: handwritten bills and
> "I'll pay later" stretch a quick sale into a month-long chase. Turn: SBB creates
> a clean invoice in seconds and tracks who's paid. Close on a "PAID" stamp with a
> checkmark. **CTA: "Invoice in seconds — smartbusinessbook.io."**

### 8.6 Records — ready for tax season in minutes
> 30-second vertical sketch explainer: **"Tax season shouldn't be a nightmare."**
> Hook: a shoebox exploding with crumpled receipts. Problem: a year of loose paper
> means days of stress and guesswork. Turn: SBB keeps every sale and expense in
> one place, so your records are ready when you need them. Close on a tidy report
> with a checkmark. **CTA: "Stay ready — smartbusinessbook.io."**

### 8.7 Knowing your numbers — your whole business at a glance
> 30-second vertical sketch explainer: **"Do you know your numbers?"** Hook: an
> owner asked "how's business?" — and shrugging. Problem: running on gut feel
> means missing what's working and what's bleeding. Turn: SBB shows today's sales,
> top products, and profit on one dashboard. Close on a confident owner reading a
> clear chart. **CTA: "Know your numbers — smartbusinessbook.io."**

### 8.8 From paper to app — ditch the exercise book
> 25-second vertical sketch explainer: **"Still running your business in an
> exercise book?"** Hook: a worn paper ledger, smudged and torn, a cross over it.
> Problem: paper gets lost, adds up wrong, and tells you nothing. Turn: SBB is the
> business book that adds up for you and remembers everything. Close on the paper
> book replaced by a phone, checkmark. **CTA: "Upgrade your book — smartbusinessbook.io."**

### 8.9 Multiple staff / branches — know what everyone's doing
> 30-second vertical sketch explainer: **"Who sold what, where?"** Hook: two shop
> branches, an owner who can't be in both. Problem: without a shared record, sales
> and stock across staff and branches are a black box. Turn: SBB gives every branch
> one book the owner can see from anywhere. Close on both branches ticking green.
> **CTA: "See every branch — smartbusinessbook.io."**

### 8.10 Reorder timing — restock at the right moment
> 25-second vertical sketch explainer: **"Order too late, lose the sale. Too
> early, freeze your cash."** Hook: a "SOLD OUT" sign next to a shelf overflowing
> with the wrong thing. Problem: reordering by guesswork costs you either way.
> Turn: SBB watches your stock levels and tells you what to reorder, and when.
> Close on a perfectly-timed delivery, checkmark. **CTA: "Reorder at the right time — smartbusinessbook.io."**

> **Campaign tip:** run 8.1–8.10 with the *same* SBB `brand_motion` kit and a
> shared CTA style, then `render_motion` each to a 1080×1920 MP4 — that's a
> ready-to-post set of shorts, one per objection, all on brand.

---

## 9. Business tips — make owners smarter (value-first)

Sell nothing; teach something. These vertical sketch explainers give small
business owners a genuinely useful lesson — pricing, cash flow, margins,
customers — **without pitching an SBB feature**. Value-first content builds
trust and an audience; the brand is a light sign-off, not the message.

Same **§2.1 sketch-explainer** style (hand-drawn kinds, verifies clean,
25–35s, 1080×1920) with these differences: the video's job is to make the
viewer *think differently* by the end; use a `chart` or a couple of big numbers
to make the lesson land; and close on a **soft** sign-off — a short line like
*"Run a smarter business — smartbusinessbook.io"* rather than a feature CTA.
Keep the SBB watermark on for brand recall.

### 9.1 A discount is bigger than it looks
> Sketch explainer: **"A 10% discount isn't 10% off."** Draw it: an item costs
> you 800, you sell at 1000 — profit is 200. Now knock 10% off the price (100
> off) — your profit drops from 200 to 100. A **10% discount just halved your
> profit.** Lesson: discount your *margin*, not your price, and know the number
> before you say yes. Soft close: *"Know your numbers — smartbusinessbook.io."*

### 9.2 Profit is not cash
> Sketch explainer: **"Profitable on paper, broke in the till."** A shop sells
> 500,000 this month — but half is on credit and rent is due today. Profit and
> cash are two different clocks. Lesson: watch the *timing* of money in and out,
> not just the totals; a profitable business can still run out of cash. Soft
> close: *"Run a smarter business — smartbusinessbook.io."*

### 9.3 The 80/20 rule
> Sketch explainer: **"Most of your profit hides in a few places."** A simple
> bar chart: roughly **80% of profit comes from 20% of your customers (and
> products).** Lesson: find your vital few — your best customers and best-margin
> lines — and protect and grow them, instead of spreading yourself thin. Soft
> close: *"Grow what works — smartbusinessbook.io."*

### 9.4 Two wallets, not one
> Sketch explainer: **"Your money and the business's money are not the same
> money."** One pocket for both = you never know if you're winning. Draw the
> split: a business account, a personal account, and you paying yourself a fixed
> wage from it. Lesson: separate the two and pay yourself deliberately. Soft
> close: *"See your business clearly — smartbusinessbook.io."*

### 9.5 Know your break-even
> Sketch explainer: **"How many sales before you earn a single shilling?"** Draw
> it: fixed costs 300,000 a month, 200 profit per sale → you must sell **1,500
> just to break even**; sale 1,501 is your first real profit. Lesson: know that
> number, and every day is a race to pass it — not a guess. Soft close: *"Know
> your break-even — smartbusinessbook.io."*

### 9.6 Your profit is made when you buy
> Sketch explainer: **"You don't make money when you sell — you make it when you
> buy."** Two owners buy the same goods; one negotiates 10% off and buys the
> right quantity. Lesson: a good purchase price and smart reordering set your
> profit before a customer ever walks in. Soft close: *"Buy smarter, keep more —
> smartbusinessbook.io."*

### 9.7 Keep a rainy-day buffer
> Sketch explainer: **"Every business has a slow season — plan for it."** Draw a
> line chart dipping in the lean months. Lesson: set aside a buffer (aim for ~3
> months of costs) while times are good, so a slow month is an inconvenience, not
> a crisis. Soft close: *"Build a safety net — smartbusinessbook.io."*

### 9.8 Keeping a customer beats finding one
> Sketch explainer: **"A repeat customer is cheaper than a new one."** It costs
> far more to win a stranger than to bring back someone who already trusts you.
> Draw a happy customer returning three times. Lesson: chase loyalty — service,
> follow-up, a reason to come back — not just the next new face. Soft close:
> *"Keep them coming back — smartbusinessbook.io."*

### 9.9 Don't compete on price alone
> Sketch explainer: **"There's always someone cheaper — don't race them to the
> bottom."** A price war ends with everyone broke. Draw the alternative: win on
> trust, speed, quality, and service. Lesson: give people a reason to choose you
> that isn't the lowest number. Soft close: *"Stand out, don't sell out —
> smartbusinessbook.io."*

### 9.10 Track one number every week
> Sketch explainer: **"You can't improve what you don't measure."** One owner
> checks a fancy report once a year; another tracks a single number every week —
> sales, or cash, or top product. Draw the second owner's line climbing. Lesson:
> a small weekly habit beats a big annual guess. Soft close: *"Track it weekly —
> smartbusinessbook.io."*

### 9.11 Reinvest before you upgrade your lifestyle
> Sketch explainer: **"Grow the tree before you eat the fruit."** First profit
> comes in — one owner buys a bigger car, another restocks and reinvests. Draw
> the second business branching and growing. Lesson: feed the business first;
> lifestyle can wait until the tree is strong. Soft close: *"Grow on purpose —
> smartbusinessbook.io."*

> **Series tip:** these make an evergreen "smart business tips" playlist. Run
> them with the SBB brand kit but a lighter, teacher-y tone; each is a standalone
> 1080×1920 short. Because they teach rather than pitch, they earn the follow —
> and the soft `smartbusinessbook.io` sign-off does the quiet selling.

---

## Regression checklist (quick pass after any change)

- [ ] `motion_new` scaffolds the folder tree; a titled project lands in its own
      folder (not the folder that happened to be open).
- [ ] A well-formed project **verifies clean**; a malformed one produces
      addressed, actionable issues.
- [ ] `write_scene` replaces exactly one scene by id; others are byte-identical.
- [ ] Captions generate from narration, export valid SRT, and round-trip.
- [ ] A voiced project exports an MP4 with a **non-silent** audio stream.
- [ ] A brand kit recolours the video without changing the words.
- [ ] Sketch primitives render rough and **deterministic**; same project → same
      output.
- [ ] `render_motion` produces a valid H.264/AAC MP4 at the project's resolution
      and the correct duration.
- [ ] The preview is self-contained (no external URLs) and plays every scene on
      its timeline.
