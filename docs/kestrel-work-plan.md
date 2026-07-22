# Kestrel Work — an AI colleague for your desktop

Kestrel becomes **two products on one engine**:

- **Kestrel Build** — the autonomous *coding* agent (what exists today).
- **Kestrel Work** — an autonomous *knowledge-work* agent: a dedicated space where
  it does real work in your real files and apps — reports, spreadsheets, data,
  research, email — on your Windows machine.

One line: **Kestrel Build writes your software. Kestrel Work does your work.**

## Decisions locked

- **First wedge: research → data → report.** Give it a goal and sources; it
  researches with citations, works the numbers in a real spreadsheet, and produces
  a finished, checked document.
- **Packaging: one application, a new top-level "Work" mode** alongside the
  existing views. Providers, keys, usage, budgets, and policy are shared; the
  Build (coding) interface is untouched. Separate branding/installers stay
  possible later from the same codebase.

## Why this is a natural second product, not a rewrite

The autonomy core we shipped is **not about code**. Look at what's already built
and task-agnostic:

| Already built (reused as-is) | What made it "coding" |
|---|---|
| Agent loop, streaming, sub-agents | — |
| 🗺 Task planner + live TODO ledger | — |
| Persistent project memory | — |
| Policy engine + permission prompts | — |
| Budgets, token economy, usage dashboard | — |
| Checkpoints & rollback, diff review | — |
| Verification / acceptance loop | — |
| Web search, http fetch, headless browser | — |
| Providers (Anthropic / OpenAI-compatible) | — |
| **Tools**: read_file, write_file, run_command, git, verify… | ← **only this** |
| **Prompt**: "you are a coding agent…" | ← **and this** |

So Kestrel Work = **the same engine + a different tool pack + a different UI**.
That's the whole architectural bet, and it's a good one.

## What Kestrel Work does (the capability map)

Grouped by what a knowledge worker actually does in a day:

**1. Documents & reports**
- Read, write, and *edit* `.docx`, `.pdf`, `.md`, `.txt`
- Draft, restructure, and revise long-form reports
- Export to Word/PDF; apply a house style

**2. Spreadsheets & data**
- Read/write ranges, formulas, formatting, charts in `.xlsx`
- Clean, filter, join, pivot, and summarize data (`.csv`, `.xlsx`)
- Sanity-check numbers and reconcile totals

**3. Research**
- Web search → read sources → synthesize with citations
- (Reuses `web_search`, `http_get`, and the headless-browser renderer verbatim)

**4. Email & comms**
- Triage an inbox, summarize threads, draft replies, send (heavily gated)

**5. Files & organization**
- Find, rename in bulk, reorganize, convert formats, deduplicate

**6. Desktop & applications** *(last, riskiest)*
- Launch/focus apps, drive Office directly, light UI automation

### The Windows-native moat

Kestrel Build's edge is that it's native to Windows. Kestrel Work's edge is the
*same trick, bigger payoff*: **COM automation through PowerShell**. Word, Excel,
and Outlook expose full automation objects on any machine with Office:

```powershell
$xl = New-Object -ComObject Excel.Application    # real Excel, real workbook
$ol = New-Object -ComObject Outlook.Application  # real inbox
```

That is exactly Kestrel's existing pattern — shell out, no heavy dependencies —
and it's something **no browser-based assistant can do**. ChatGPT can describe
your spreadsheet; Kestrel Work can *open it, fix it, and save it*. Fallback for
machines without Office: `.docx`/`.xlsx` are ZIP+XML, so read (and simple write)
support degrades gracefully.

## The wedge — where to start

The temptation is "everything a person does." That's unbounded and will sink the
product. The sharpest first wedge is:

> **Research → data → report.** Give it a goal and source material; it researches,
> works the numbers in a real spreadsheet, and produces a finished document.

Why this one:
- It's the **highest-frequency, highest-pain** knowledge work.
- It reuses the *most* of what already exists (research tools, planner, memory).
- It's **verifiable** — an acceptance loop maps directly ("every required section
  is present", "the totals reconcile"), which is Kestrel's trust story.
- It demos in 60 seconds and the value is undeniable.

Email and desktop control are bigger *wow*, but far higher risk — do them once
the trust rails are proven on documents.

## Architecture

**Shared core, two profiles.** `kestrel-core` stays the single engine. The only
real refactor: today `builtin_tools()` returns one fixed list. That becomes
**tool packs**:

```rust
enum Profile { Build, Work }
fn tools_for(profile: Profile) -> Vec<ToolSpec>
fn system_prompt_for(profile: Profile, root: &Path) -> String
```

New `work` tool pack (each shelling out via PowerShell/COM, in Kestrel's style):

| Tool | Purpose |
|---|---|
| `read_doc(path)` / `write_doc(path, content)` / `edit_doc(path, old, new)` | Word/PDF/markdown |
| `read_sheet(path, range)` / `write_sheet(path, range, values)` | Excel ranges |
| `sheet_formula(...)` / `sheet_chart(...)` | formulas, charts |
| `export(path, format)` | → PDF/DOCX |
| `find_files(query)` / `organize_files(...)` | file operations |
| `list_mail` / `read_mail` / `draft_mail` / `send_mail` | Outlook (gated) |
| `run_app` / `focus_window` | desktop (gated, late) |
| *reused:* `web_search`, `http_get`, `check_page`, `update_plan`, `remember`, `spawn_subagent` | — |

**Packaging recommendation:** one application, a new top-level **Work** mode
alongside the existing views — *not* a second binary, at least at first. Settings,
providers, usage, and budgets are shared; a mode switch costs a fraction of a
second app and keeps the coding interface untouched, exactly as you want. If the
two need separate branding/installers later, they can be split from the same
codebase cheaply.

**The Work UI shape** — chat on one side, the **artifact** on the other:
- Live document/sheet preview that updates as it works (the streaming file
  preview, retargeted)
- The 🗺 **Plan ledger** carries over unchanged — you watch it work a checklist
- **Change review**: "here's what I changed in your report" with accept/reject,
  built on the existing checkpoint + diff machinery

## Phasing

- **W0 — Foundation.** ✅ **shipped.** Tool-pack split in core
  (`Profile::{Build,Work}`, `tools_for`, `system_prompt_for` + a work-shaped
  prompt); `run_agent` takes a profile. New **💼 Work** mode: its own scoped
  folder (defaults to Documents, persisted in settings), a workspace panel that
  lists the folder and opens files in their default app, the chat + compose bar
  reused, and the artifact pane (🗺 Plan ledger + documents produced) on the
  right. Build and Work keep **separate conversations**, each saved to its own
  root. Work already researches the web and writes real documents into the
  workspace. *Nothing user-visible changed in Build.*
- **W1 — Documents.** ✅ **shipped.** `read_doc` reads what a Work folder actually
  holds: `.docx`/`.odt` (unzip + de-XML), `.xlsx` (shared strings → TSV per
  sheet), `.pdf` (pdftotext), `.rtf`. Reads a file that is **open in Word** right
  now (`FileShare::ReadWrite`).
- **W2 — Documents out & spreadsheets.** ✅ **shipped.** `write_doc` produces a
  **real `.docx`** from Markdown (headings, bold/italic/code, bullet + numbered
  lists, quotes, bordered tables) and `write_sheet` a **real `.xlsx`** from
  CSV/TSV (numbers stored as numbers). Both verified opening natively in Word and
  Excel, no Compatibility Mode. Work folders are now **any folder**, with a
  recents list for quick switching.

  > **Design note.** Both reading and writing deliberately avoid **Office COM**.
  > Testing showed COM attaches to the user's *running* Word — hiding their
  > window and mutating their session — and hangs forever on modal dialogs. Open
  > XML files are ZIP+XML, so Kestrel builds and parses the parts directly and
  > zips them via PowerShell's built-in compression: no Office required, open
  > documents untouched, and nothing can hang. (Entry names must use forward
  > slashes — .NET's `CreateFromDirectory` emits backslashes and Word rejects the
  > result as corrupt.)

- **W2b — Spreadsheet intelligence & provable documents.** ✅ **shipped.**
  Spreadsheets now carry **live formulas** (`=SUM(B2:B9)` becomes a real formula
  Excel evaluates, not text), a **bold frozen header row**, and **columns sized
  to their content**. A new **`check_doc(path, expect)`** closes the acceptance
  loop for Work — it re-opens what was produced and verifies the required
  sections and figures are actually there, exactly as `check_page` does for web
  apps. The Work prompt now treats a `check_doc` FAIL like a broken build and
  tells the agent to use formulas rather than hard-coded totals.

  *Verified visually in Excel:* `=SUM(B2:B4)` computed to **3600**, header bold,
  columns auto-sized, sheet named, no repair prompt.

- **W2c — Multi-sheet workbooks & real charts.** ✅ **shipped.** `write_sheet` now
  takes a `sheets` array (tabs in order, names sanitised to Excel's rules) and an
  optional `chart` — a genuine **bar / line / pie chart** embedded on the first
  sheet, its series pointing at real ranges so it redraws when the numbers
  change. **Cross-sheet formulas** work (`=SUM('Data'!B2:B9)`), so a Summary tab
  can total a Data tab.

  *Verified visually in Excel:* a two-tab workbook with a titled bar chart over
  four regions, correct axis scaling, and a cross-sheet total — no repair prompt.

- **W2d — Further data work.** Cleaning, joins, pivots, conditional formatting.
- **W3 — Research → report.** The wedge, end to end: research with citations →
  data → finished document → export to PDF/DOCX, with acceptance checks.
- **W4 — Email.** Outlook triage, summarize, draft; **send is always
  permission-gated**.
- **W5 — Desktop control.** App launch/focus, light UI automation. Heavily gated,
  last because it's the most fragile and highest-blast-radius.

## Trust & safety — mandatory here, and already built

Touching someone's real documents and inbox raises the stakes far above code:

- **Permission prompts** (already shipped) become *default-on* for Work — and
  mandatory for `send_mail`, deletes, and anything outside a chosen folder.
- **Checkpoints/undo** for documents: never modify an original without a restorable
  copy. The existing checkpoint machinery maps directly.
- **Scoped workspace**: Work operates in folders the user explicitly grants, the
  same way Build is sandboxed to a project root.
- **Local & private**: files never leave the machine — only model calls go out.
  That's a genuine enterprise selling point over cloud assistants.

## Risks & open questions

1. **Scope explosion.** The biggest threat. Hold the wedge; resist "it should also…".
2. **Office dependency.** COM needs Office installed. Need honest capability
   detection and graceful degradation (and a clear story for LibreOffice/none).
3. **Email is high-stakes.** Sending on someone's behalf is a trust cliff — gate
   hard, always show the draft, never auto-send.
4. **UI automation is fragile.** Screen-driving breaks constantly; treat as a
   late, optional power feature, not a pillar.
5. **Verification is fuzzier than code.** There's no `cargo test` for a report.
   Acceptance criteria must come from the *plan* ("sections present, totals
   reconcile, every claim cited") — a genuinely interesting problem.

## Positioning

| | Kestrel Build | Kestrel Work |
|---|---|---|
| For | Developers | Everyone else (and developers off-hours) |
| Does | Writes, verifies, and ships software | Researches, writes, computes, and files real work |
| Proof | The build passes; the app runs | The report is complete; the numbers reconcile |
| Shared | Windows-native · bring-your-own-key · radically cheap · verified · private · autonomous |

The shared story is the strongest part: **one autonomous engine, native to your
machine, that you point at whatever work you have.** Build doubles as the proof
that the engine is real; Work multiplies the addressable market from
"developers" to "anyone with a deadline."
