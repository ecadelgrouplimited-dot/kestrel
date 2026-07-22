# Kestrel Work — demo & test prompts

Twelve prompts that exercise Kestrel Work end to end, ordered so a recording
builds from "clearly useful" to "I didn't know software could do that."

**Before recording**
- Pick a **Work folder** (📂 Folder…) — a clean demo folder reads better than a
  cluttered Documents. Drop in any source files a prompt needs.
- Keep the **🗺 Plan** panel visible: watching steps tick over is the story.
- Turn **ask permission** on in Settings if you want the approval popup on camera.
- Each prompt is written to be pasted verbatim.

**What to watch in every run:** the plan appearing and self-updating, the live
document streaming into the right pane, the self-check near the end, and the
finished file opening by itself.

---

## 1. Research → Word report *(the flagship)*

> Research **[company or topic]** thoroughly — their website, what they do, their
> products, leadership, and market. Then write me a comprehensive, well-structured
> company profile as a Word document. Include an executive summary, what they do,
> their products, leadership, market position, and a facts table. Cite your
> sources. Save it in this folder and open it when you're done.

- **Shows:** planning, multi-source research, `.docx` with headings/tables, the
  self-check, `open_file` at the end.
- **Success:** a formatted Word document opens by itself, sections all present,
  no invented facts.

## 2. Read what's already on the disk

> Read **[filename.pdf / .docx]** in this folder and give me a one-page executive
> summary as a Word document: what it's about, the key points, the numbers that
> matter, and anything that looks like a risk or an open question.

- **Shows:** `read_doc` on real formats (this is the bit most assistants can't do),
  synthesis, a second document produced from the first.
- **Success:** the summary reflects the *actual* file contents.

## 3. Data → charted workbook

> Here is our regional revenue: Kampala 1500, Nairobi 900, Lagos 1200, Accra 700,
> Kigali 450. Build me an Excel workbook with a **Data** tab holding this table
> and a total row using a real formula, a **Summary** tab that pulls the total and
> the top region, and a bar chart of revenue by region. Format the money column
> properly and open it when done.

- **Shows:** multi-sheet workbook, live `=SUM()`, cross-sheet formula, embedded
  chart, number formatting, frozen bold header, auto-filter.
- **Success:** Excel opens with a real chart and a total that *recalculates* if
  you edit a number on camera. **This is the best single visual moment.**

## 4. Competitive analysis with sources

> Research the top 5 competitors in **[market]**. For each, capture what they do,
> their positioning, pricing if public, and strengths/weaknesses. Produce a Word
> comparison report with a table, plus a spreadsheet with one row per competitor
> so I can sort it. Every claim must have a source link.

- **Shows:** sustained research, sub-agents on a wide task, two artifacts from one
  request, citation discipline.
- **Success:** sources are real and check out; the spreadsheet sorts/filters.

## 5. Inbox triage → drafted replies *(email)*

> Look at my last 15 emails. Summarise what I've missed, grouped by how urgent
> they are and who they're from. For the two that most need a reply, draft
> sensible responses and save them to my Drafts — do not send anything.

- **Shows:** `list_mail` / `read_mail` / `draft_mail` through the user's own
  Outlook; drafts saved for review, never sent.
- **Success:** drafts appear in Outlook Drafts. **Nothing is sent.**
- ⚠️ *Recording note:* this shows real mail on screen — use a test account or
  crop the frame.

## 6. Rewrite an existing document

> Take **[filename.docx]** and rewrite it for a non-technical audience: plain
> English, shorter sentences, keep every section and all the facts. Save it as a
> new document called `[name]-plain-english.docx` — don't touch the original —
> then tell me what you changed.

- **Shows:** read → transform → write, and that the original is preserved.
- **Success:** both files exist; the rewrite keeps the structure and the facts.

## 7. Synthesise a whole folder

> Read every document in this folder and give me a single consolidated briefing:
> what these documents collectively say, where they agree, where they contradict
> each other, and what's missing. Word document, with a table listing each source
> file and its one-line summary.

- **Shows:** `list_dir` + `read_doc` across mixed formats, genuine synthesis,
  contradiction-spotting.
- **Success:** every file in the folder is accounted for in the table.

## 8. The board pack *(the "everything" demo)*

> Prepare a Q3 board pack for **[company]**. Research the market context, then
> produce: (1) a Word report with an executive summary, market context,
> performance commentary, risks, and recommendations; and (2) an Excel workbook
> with a data tab, a summary tab with formulas, and a chart. Make the numbers in
> the report agree with the spreadsheet. Check your work before you finish, then
> open both.

- **Shows:** the full wedge — research → data → two polished artifacts →
  cross-artifact consistency → self-verification.
- **Success:** the figures quoted in the Word report match the workbook exactly.

## 9. Reconciliation — make the numbers add up

> Here's our expense data *(paste a messy table with a deliberate error, e.g. a
> total that doesn't match its parts)*. Clean it up, find and explain any errors
> or inconsistencies, and produce a corrected spreadsheet with live formulas so
> the totals can't drift again. Tell me exactly what was wrong.

- **Shows:** data cleaning, arithmetic checking, formulas as the fix for a
  human-error class.
- **Success:** it **finds the planted error** and the corrected file recomputes.

## 10. Organise a messy folder

> Look at everything in this folder and give me an inventory: what each file is,
> its type, roughly what it contains, and when it was last changed. Produce a
> spreadsheet I can sort, and suggest a sensible folder structure — describe it,
> don't move anything yet.

- **Shows:** working across many file types, restraint (proposes before acting).
- **Success:** the inventory is accurate and nothing is moved.

## 11. Long-horizon work *(shows the planner and Continue)*

> Produce a complete market-entry study for **[product]** in **[country]**:
> market size, competitors, regulation, route to market, pricing, risks, and a
> 12-month plan. Research each properly. Deliver a Word report with a section per
> topic plus a spreadsheet of the competitor and pricing data.

- **Shows:** a long plan, sub-agents, auto-compaction, and — if it hits the step
  budget — a graceful **⏸ pause** and **▶ Continue** resuming the same checklist.
- **Success:** it either finishes or pauses cleanly and continues. It never dies.

## 12. Prove the self-check works *(the trust demo)*

> Write me a Word project proposal for **[project]** with exactly these sections:
> Background, Objectives, Scope, Timeline, Budget, Risks, and Success Criteria.
> Before you tell me it's done, verify the document actually contains all seven
> sections and tell me the result of that check.

- **Shows:** `check_doc` as an explicit acceptance gate.
- **Success:** you see the **✅ PASS** line naming the sections. Even better on
  camera: if it ever FAILs, it goes back and fixes the document — that's the
  moment that separates Kestrel from a chat box.

---

## Bonus: stress and safety checks

These aren't pretty, but run them before a public demo.

- **Stop mid-run.** Start #11, hit **⏹ Stop**. It should halt promptly, keep what
  it wrote, and offer **Continue**.
- **Deny a permission.** With prompts on, deny a command. The agent should adapt
  and carry on, not crash.
- **Send confirmation.** Ask it to *send* an email. You must get a confirmation
  popup **even with permission prompts turned off** — sending is always confirmed.
- **Budget cap.** Set a low per-conversation cap and start a big task; it should
  stop cleanly at the cap.
- **Missing tool.** Ask it to read a `.pdf` on a machine without `pdftotext` — it
  should explain exactly how to install it rather than failing silently.
- **Folder switch.** Switch Work folders mid-session; conversation, plan, and
  files should swap with it, and switch back intact.
