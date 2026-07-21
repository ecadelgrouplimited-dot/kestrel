# Kestrel Autonomy Plan — the skill set for "best of the best"

The goal: move Kestrel from a strong **reactive tool loop** to a **self-directing
agent** that reliably finishes large, open-ended tasks — plans, acts, verifies,
recovers when stuck, and remembers — with the human in control, not in the loop.

This is the design; sequencing is at the end. Everything is grounded in modules
Kestrel already has, so each piece is an addition, not a rewrite.

## Where Kestrel is today (the honest baseline)

Already strong: a tool-using loop (read/write/edit/search/run/git/verify/
install/serve/screenshot/multi-repo), **live streaming**, the **token economy**
(caching, budgets, usage), **trust rails** (policy, permission prompts,
checkpoints, secret scan, audit, diagnostics), **run control** (pause/continue/
stop), **workflows + marketplace**, and any-language/any-framework building with
up-front research.

The gap: Kestrel is **reactive**. It responds turn-to-turn but has no explicit
**plan**, no **loop/stuck detection**, no **learned memory**, no **parallelism**,
and no **acceptance test** of "is this actually done?" On big tasks it wanders and
burns the step budget. Closing that gap is what "autonomous and complete" means.

## The autonomy loop

Every autonomous run should cycle through five phases, not one:

```
        ┌──────────── Plan ───────────┐
        │  decompose goal → task graph │
        ▼                              │
      Act  ──►  Verify  ──►  Reflect ──┘
   (surgical    (build/test   (critique vs. the request;
    tool use)    /run/accept)  re-plan or mark done)
        ▲                              │
        └──────── Remember ◄───────────┘
        (persist what was learned about this repo)
```

Today Kestrel does **Act** well and **Verify** partially. The skill set below
builds the other three and deepens Verify.

---

## The skill set

Each skill lists **what**, **why it matters**, and **how** (the Kestrel modules /
new agent tools it uses).

### 1. Task Planner + live TODO ledger  ★ backbone
- **What:** the agent decomposes a goal into an ordered, checkable task list, works
  it item by item, and marks progress. A `plan` tool (create/update/complete
  items) backed by `.kestrel/plan.json`; a **Plan panel** in the UI the user
  watches and can edit.
- **Why:** this single feature is the biggest lever on finishing large tasks. It
  gives the model a spine, makes progress legible, and lets a paused run resume
  against a concrete checklist instead of vibes.
- **How:** new `crate::plan` module + `plan` agent tool; UI panel beside the build
  preview; the loop injects the current checklist into each turn.

### 2. Stuck / loop detection + recovery  ★
- **What:** detect no-progress — the same tool+args repeated, N steps with no file
  change, or repeated failing verifies — and force a **re-plan**, switch strategy,
  or ask the user, instead of silently grinding to the step limit.
- **Why:** protects the token budget and turns "wandered off" into "noticed and
  corrected." The #1 cause of a bad autonomous run.
- **How:** a progress monitor in `run_agent` (hashes of recent calls + a
  file-change watermark); on stall, inject a "you appear stuck — re-plan"
  directive or emit an `AgentEvent::NeedsHelp` the UI surfaces.

### 3. Semantic navigation tools (definition / references / rename)  ★
- **What:** give the *agent* precise code intelligence: `definition(symbol)`,
  `references(symbol)`, `outline(file)`, `rename_symbol(old,new)` across the repo.
- **Why:** edits become surgical instead of grep-and-hope; refactors land safely;
  far less flailing (and fewer tokens) on unfamiliar code.
- **How:** Kestrel already has the **tree-sitter** backend + the **project graph**
  — this exposes them as agent tools. Rename builds on the reference index.

### 4. Deep acceptance verification  ★
- **What:** "done" must be *demonstrated*, not asserted. After changes: run the
  build/tests, **run the app and drive it** (headless browser for web: load pages,
  click, assert, screenshot), check the planner's **acceptance criteria**, and —
  when sensible — **write new tests** for the change and run them.
- **Why:** this is the difference between "the model says it's done" and "it is
  done." It's also Kestrel's trust story made real.
- **How:** extends `verify` + `start_app`/`http_check`/`screenshot`; adds a
  browser-drive tool and an `acceptance` step tied to plan items.

### 5. Persistent project memory
- **What:** a learned model of *this* repo — conventions, architecture notes,
  build/test/run commands, gotchas, past decisions — in `.kestrel/memory/`, read at
  the start of every run and updated as the agent learns.
- **Why:** stops re-deriving the project every session; each run makes the next one
  smarter and cheaper. (Kestrel's own `MEMORY.md` system is the proven model.)
- **How:** new `crate::memory` module; the loop loads a compact memory pack into
  the system prompt and offers a `remember` tool.

### 6. Subagents (parallel, isolated context)
- **What:** spawn focused workers — `explore`, `implement <module>`, `write_tests`
  — each with its own small context, running in parallel, reporting back a result.
- **Why:** speed on big tasks and a lean main context (lower cost, less drift). The
  main agent orchestrates; subagents do bounded jobs.
- **How:** reuse `run_agent` recursively with a restricted toolset + its own
  cancel token; a `spawn_subagent(task)` tool; results fold into the main plan.

### 7. First-class web research (search + fetch)
- **What:** a `web_search` tool feeding the existing `http_get`, so research isn't
  limited to URLs the model already knows.
- **Why:** current APIs/framework versions change; real research beats guessing.
  Already half-built (http_get) — search closes it.
- **How:** a search provider behind a trait (pluggable/keyed), results summarized
  into the context.

### 8. Skills (capability packs beyond prompt recipes)
- **What:** evolve **workflows** into **skills** — named capabilities that can
  carry scripts, templates, and reference files (e.g. "set up CI", "add auth",
  "dockerize + deploy"), loaded on demand.
- **Why:** encodes repeatable expertise the marketplace can share; the natural next
  step for the workflow/marketplace foundation already shipped.
- **How:** extend the workflow format with attached resources + an optional
  verification script; loader injects the skill's procedure and files.

---

## Sequencing

Ordered by leverage-per-effort. Each phase is independently shippable and makes
the test-project battery (see `test-projects.md`) measurably more likely to pass.

- **Phase A — Autonomy Core** (skills 1, 2, and a deeper Reflect step) — ✅ **shipped**.
  The spine: plan → act → verify → reflect, with stuck-detection. Delivered:
  `crate::plan` (`.kestrel/plan.json`), the `update_plan` agent tool, a live
  **🗺 Plan** panel (checklist + progress) in the UI, loop **stall detection**
  (repeated-action nudge, bounded), and a **plan-aware reflect** that refuses to
  finish while steps remain. *Success:* a large task works a visible checklist to
  completion instead of wandering into the step limit.
- **Phase B — Precision & Reach** (skills 3, 7) — ✅ **shipped**. Semantic nav +
  web search. Delivered: `crate::codenav` (whole-word-precise
  `definition`/`references`/`outline`/`rename_symbol` agent tools on the
  tree-sitter backend) and `crate::websearch` (keyless `web_search` over
  DuckDuckGo HTML via curl). The loop prompt now tells the agent to web-search
  the current docs before writing and to navigate/refactor with these tools.
  *Success:* a cross-file rename lands correctly; an unfamiliar-framework build
  researches the real API first.
- **Phase C — Scale & Memory** (skills 5, 6). Memory + subagents. *Success:* a
  second run on a repo is visibly faster/cheaper; a multi-module build parallelizes.
- **Phase D — Proof & Packaging** (skills 4, 8). Browser-driven acceptance + skills
  packs. *Success:* Kestrel drives a web app it built and proves the feature works;
  a shared skill sets up CI end-to-end.

## Scope guard (what we won't do now)

No cloud/hosted agent, no bespoke ML — Kestrel stays a dependency-light, native,
bring-your-own-key tool. Autonomy comes from **orchestration and verification**,
not a bigger model. Everything above is buildable on the current stack.
