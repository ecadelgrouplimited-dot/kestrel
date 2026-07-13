# Kestrel Test Projects

A battery of real prompts to put Kestrel through its paces — chosen to exercise
every capability, across many languages and frameworks, and to prove the one
claim that matters: **you can ask Kestrel to build an app or project in any
language or framework, and it does it.**

## How to run these

1. Open (or create) a project folder in Kestrel.
2. Open **Chat**, turn on **Agent mode**, paste a prompt, and **Build**.
3. Watch the live status (each tool call is shown), the **token meter**, and the
   **Files created** panel. When it finishes, review the **Diff** tab (now with
   red/green **+/−** line counts), run **⚠ Check** (diagnostics), and use the
   **Run** tab to launch and preview.
4. For the recurring ones (release readiness, security, docs), save them as
   **Workflows** so they're one click next time.

Each entry lists **the prompt**, **what it tests**, and **success criteria** so
a run can be scored objectively, not just "looks done."

---

## 1. Language & framework breadth — the "any stack" proof

The point of this tier: hand Kestrel a stack and watch it scaffold, build, and
verify — no matter the ecosystem.

### 1.1 Rust CLI with tests
> Build a Rust command-line tool called `jsonpeek` that reads a JSON file and
> prints a colored, collapsible tree of its structure, plus `--depth N` and
> `--type` filters. Include unit tests and a README. Make `cargo test` and
> `cargo build --release` pass.

- **Tests:** native Rust toolchain, `cargo` scaffolding, verify loop, diagnostics.
- **Success:** `cargo test` green; `--help` documented; README run steps accurate.

### 1.2 Go HTTP service
> Create a Go HTTP API (`chi` or stdlib) exposing `/healthz` and a `/shorten`
> URL-shortener with in-memory storage and table-driven tests. Run `go vet` and
> `go test ./...` and make them pass.

- **Tests:** Go toolchain, `go test`, gofmt-clean output, health-check via `http_check`.
- **Success:** `go test ./...` green; server starts via **Run** tab; `/healthz` 200.

### 1.3 TypeScript + React (Vite)
> Scaffold a Vite + React + TypeScript app: a Pomodoro timer with start/pause/
> reset, session history persisted to localStorage, and a settings panel.
> `npm run build` and `npx tsc --noEmit` must pass.

- **Tests:** `npm create vite`, dependency install, TS diagnostics, browser preview.
- **Success:** type-check clean; production build succeeds; preview renders and works.

### 1.4 Python FastAPI + SQLite
> Build a FastAPI service for a personal bookshelf: CRUD for books, SQLite via
> SQLAlchemy, Pydantic models, and pytest tests. Provide a `requirements.txt` and
> make `pytest` pass.

- **Tests:** Python venv/deps, `pytest`, `start_app` (uvicorn) without hanging.
- **Success:** `pytest` green; `/docs` loads; a create→list round trip works.

### 1.5 PHP / Laravel (toolchain install)
> Create a small Laravel app: a task board with tasks that have a title, status,
> and due date, with Eloquent models, migrations, and feature tests. Install
> composer/PHP if missing.

- **Tests:** `install_tool` (composer/php via winget), `composer create-project`, artisan.
- **Success:** migrations run; `php artisan test` green; served via **Run** tab.

### 1.6 C# / .NET minimal API
> Build a .NET minimal API for a currency converter with an in-memory rates
> table and xUnit tests. `dotnet build` and `dotnet test` must pass.

- **Tests:** `dotnet new`, non-Rust build/test detection, verify loop.
- **Success:** `dotnet test` green; endpoint returns a converted amount.

### 1.7 Flutter (mobile/desktop)
> Create a Flutter app: a habit tracker with a weekly grid, add/remove habits,
> and local persistence with `shared_preferences`. Run `flutter analyze` and make
> it clean.

- **Tests:** unfamiliar-SDK research, `flutter create`, analyzer diagnostics.
- **Success:** `flutter analyze` clean; widget tree builds; state persists.

### 1.8 Systems / low-level
> Write a small C program: a ring-buffer library with a header and a test
> harness, plus a Makefile. Build with the available compiler and run the tests.

- **Tests:** C toolchain discovery, Makefile build, non-managed verify.
- **Success:** compiles warning-clean; test harness passes.

> **Scoring the tier:** a pass is *scaffold → build → test → run*, all green,
> with the README's commands matching what actually works.

---

## 2. Research-required — building beyond what the model already knows

These deliberately require **current** API knowledge or an unfamiliar library, so
Kestrel must `http_get` the docs/registry before writing code rather than guessing.

### 2.1 Current-version framework
> Build a Next.js 15 app using the **App Router** and Server Actions: a guestbook
> with a form that writes to a JSON file and revalidates the page. Confirm the
> current API from the official docs before writing — don't assume older APIs.

- **Tests:** research step (fetch Next.js docs), correct current-API usage, build.
- **Success:** uses App Router/Server Actions correctly; `next build` passes.

### 2.2 Unfamiliar third-party API
> Build a small CLI that fetches the current ISS position from the open Where the
> ISS at API and prints it, with a `--watch` mode. Look up the exact endpoint and
> response shape first, then implement and test against it.

- **Tests:** `http_get` to confirm a live API contract, error handling, tests with mocks.
- **Success:** correct endpoint/fields; handles network errors; tests pass offline.

### 2.3 Pin correct dependency versions
> Create a Python data pipeline using Polars (not pandas). Check the current
> Polars API for lazy frames and the right version to pin, then build a script
> that reads a CSV, groups, and writes Parquet, with tests.

- **Tests:** registry lookup (PyPI), correct version pin, API-accurate code.
- **Success:** installs cleanly; script runs on sample data; tests pass.

---

## 3. Multi-repository reasoning

Link a second repo (Explorer → **🔗 Repo**) before running these.

### 3.1 Cross-repo trace
> This workspace has a frontend repo and a linked backend repo. A button in the
> frontend calls an endpoint that returns 500. Use `list_repos` and search across
> both to trace the request from the UI click to the backend handler, find the
> bug, and fix it in the repo that owns it.

- **Tests:** `list_repos`, `search(repo=…)`, cross-repo reading, scoped writes.
- **Success:** identifies the true owner of the bug; fix lands in the right repo.

### 3.2 Shared-contract change
> The linked `shared-types` repo defines an API response type. I want to add a
> `createdAt` field. Show everywhere in **both** repos that consumes this type and
> outline the change; then apply it in the primary repo and list the follow-ups
> needed in the linked one.

- **Tests:** multi-repo impact analysis, honest scoping (writes stay in primary).
- **Success:** complete consumer list across repos; correct primary-repo edit; clear hand-off list.

---

## 4. Workflows & marketplace

### 4.1 Run built-in specialized agents
> On a moderately messy existing project, run **Release readiness**, then
> **Security remediation**, then **Raise test coverage** from the Workflows view.

- **Tests:** workflow runner inherits checkpoints/verify/policy/budget; param-less recipes.
- **Success:** each produces a real report and verified fixes, not just prose.

### 4.2 Author + share a workflow
> Create a custom workflow "Accessibility sweep" that audits a web UI and fixes
> obvious a11y issues. Export it, then re-import it into a fresh project.

- **Tests:** editor (`{param}` validation, slugify), install, export/import round trip.
- **Success:** workflow saved, runs, and survives an export→import cycle.

---

## 5. Reliability, safety, and the token economy

### 5.1 Self-correcting build
> Intentionally: build a TypeScript project but introduce a type error partway,
> then keep going. I want to see you run the type-checker, read the error, and fix
> it before claiming success.

- **Tests:** verify loop, diagnostics parsing, "don't claim success without verifying."
- **Success:** the failure is detected and fixed; final state type-checks.

### 5.2 Policy guardrails
> With `run_command` disabled in the Policy settings, build a static site. Adapt to
> the restriction instead of trying to run a dev server.

- **Tests:** policy engine (blocked tool → agent adapts), audit log entries.
- **Success:** no blocked-tool loop; task completes within the guardrails.

### 5.3 Budget cap
> Set a low per-conversation budget, then ask for a large multi-file app. Confirm
> the run hard-stops at the cap with a clear message rather than blowing past it.

- **Tests:** live budget line, hard-stop mid-run, usage logging.
- **Success:** run halts at the cap; usage dashboard reflects the spend.

### 5.4 Prompt-caching savings
> Have a long back-and-forth refactor on one project and watch the **cache-saved**
> readout and the Usage dashboard.

- **Tests:** Anthropic prompt caching, real usage accounting, savings math.
- **Success:** cache-read tokens grow across turns; savings are non-trivial.

---

## 6. Windows-native superpowers

### 6.1 Detect + install a missing toolchain
> Build a project that needs a toolchain that isn't installed yet (e.g. a PHP or
> Python project on a machine without it). Detect what's missing and install it
> via winget, then continue.

- **Tests:** environment discovery, `install_tool` (winget), resume after install.
- **Success:** missing tool installed; build proceeds to green.

### 6.2 Run + preview + screenshot
> Build a small web app, start it, confirm it's up, open a browser preview, and
> take a screenshot for visual review.

- **Tests:** `start_app` (no hang), `http_check`, `open_url`, `screenshot`, Run tab.
- **Success:** server runs; preview opens; screenshot captured in the gallery.

---

## 7. Flagship — build Kestrel's own product website

This is the capstone. **Not a generic template site** — the whole point is that
the site *tells Kestrel's story through its features*, where each feature is a
reason a developer should switch. If the copy could describe any IDE, it fails.

> Build the official product website for **Kestrel** — a Windows-native AI coding
> agent — as a fast, self-contained static site (plain HTML/CSS/JS, or Astro if
> you prefer; no heavy framework, must build and preview locally). It is a
> **story told through features**: every section names a real Kestrel capability
> and, more importantly, **why it matters to a developer**. Use this narrative:
>
> **Hero.** One line that lands the thesis: Kestrel is an autonomous coding agent
> that runs *natively on Windows* and is *radically cheaper to run* than the
> alternatives — because token economy is a first-class feature, not an
> afterthought. A primary "Download for Windows" call to action.
>
> **The story, section by section — each is a feature *and* the reason it matters:**
> - **Native to Windows.** Real toolchain discovery, winget installs, cmd/PowerShell
>   execution, a real app runner. *Why it matters:* the millions of developers on
>   Windows finally get an agent built for their machine, not ported to it.
> - **Token economy that respects your bill.** Prompt caching, a live token +
>   cost meter, token-aware auto-compaction, per-conversation and per-day budget
>   caps, and a usage dashboard with CSV export. *Why:* bring your own API key and
>   spend a fraction of what the same work costs in other tools — with the numbers
>   on screen, in real time, so there are no surprises.
> - **Any language, any framework.** It researches unfamiliar stacks from the docs
>   before writing, scaffolds with each ecosystem's own tools, and verifies. *Why:*
>   one agent for your whole stack — Rust to Flutter to Laravel — that doesn't
>   guess.
> - **Verified, not vibes.** A build/test verify loop, LSP-style diagnostics, a
>   diff review with red/green line counts, checkpoints & rollback, a secret
>   scanner, and an audit log. *Why:* autonomy you can actually trust to touch a
>   real codebase.
> - **Reason across repositories.** Link sibling repos into a workspace and trace a
>   call from your app into a shared library. *Why:* real systems are many repos,
>   not one.
> - **Workflows & a marketplace.** One-click specialized agents (release readiness,
>   security remediation, migration, incident triage) plus a catalog you install
>   from and share `.toml` recipes with your team. *Why:* your team's best
>   practices become one button.
> - **Guardrails for autonomy.** A policy engine that blocks tools/commands and a
>   shared team policy committed to the repo. *Why:* safe to let it run.
>
> **Design:** developer-grade and confident — dark theme by default with a warm
> amber accent (Kestrel's color, ~#E88A2E), clean typography, a light/dark toggle,
> responsive, and genuinely fast (no external CDNs; inline or local assets only).
> Include a short "How it works" three-step (Open a project → Ask in plain English
> → Review the diff and ship), a comparison line vs. using native Codex/Claude
> directly (cheaper per token, Windows-native, verified), and a footer.
>
> Then: build it, run it locally, open a browser preview, take a screenshot of the
> hero, and give me the verify results. The copy must be specific to Kestrel — a
> reader should finish the page understanding *what Kestrel is and why each
> feature is a reason to choose it.*

- **Tests:** everything at once — any-framework build, research (if Astro), the
  Run tab + preview + screenshot, diagnostics, and diff review. Above all it
  tests whether Kestrel can produce a **product narrative**, not boilerplate.
- **Success criteria:**
  - The site builds and previews locally with no external dependencies.
  - Every section maps to a **real** Kestrel feature (cross-check against the
    README) — no invented capabilities.
  - Each feature is paired with a concrete **"why it matters"** — the value, not
    just the mechanism.
  - The token-economy and Windows-native angles are front-and-center (they're the
    differentiators).
  - Visual identity matches (dark, amber accent, fast, responsive), and the hero
    screenshot looks shippable.
  - A skeptical developer reading it can answer: *"What is Kestrel, and why would
    I switch?"*

---

## Regression checklist (quick pass after any change)

- [ ] Scaffold + build + test green in at least one Rust, one JS/TS, and one
      Python project.
- [ ] Diff tab shows correct red/green **+/−** counts (total and per file).
- [ ] **Format** works on `.rs` (rustfmt) and degrades gracefully when a
      formatter (prettier/black) isn't installed.
- [ ] A workflow installs from the Catalog, runs, and exports/imports cleanly.
- [ ] Policy block, budget hard-stop, and checkpoint restore all behave.
- [ ] `start_app` never hangs the agent; Run tab preview + screenshot work.
