# Kestrel Progress

A living record of what's actually shipped, mapped to the [roadmap](./roadmap.md).
Legend: ✅ done · 🟡 partial · ⬜ not started.

_Last updated: 2026-07-11._

## Snapshot

Kestrel is a Windows-native Rust workspace (`kestrel-core`, `kestrel-cli`,
`kestrel-ui`) with a working local context engine, a multi-provider model layer,
and a native desktop app whose agent can build, verify, and self-correct real
projects. Model calls go over the system `curl` (no bundled TLS) — a constraint
that turned into a strength (zero heavy deps). All model providers are
configurable and the agent has a full engineering toolset.

**Tests:** 128 core + 8 CLI, all green. Clippy/fmt clean on every commit.

## Phase 0 — Foundation ✅

- ✅ Rust workspace scaffold (core/cli/ui)
- ✅ Project open flow, file inventory, Git root detection, `.gitignore` awareness
- ✅ Local incremental index cache (`.kestrel/index.json`, keyed by size+mtime)
- ✅ CLI inspection commands: `inspect`, `symbols`, `graph`, `related`, `context`
- ✅ Parsing: **tree-sitter** backend (Rust/TS/TSX/JS/Python) behind the
  `SymbolExtractor` trait — precise symbols (nested methods, exported arrow fns,
  decorated Python defs); heuristic scanners kept as the fallback and for the
  import/reference edges the graph is built from
- ⬜ Native Windows service prototype (running as an app, not a service)

## Phase 1 — MVP Agent ✅

- ✅ Native UI (chose native over CLI-first for the product surface)
- ✅ Chat with repository context (streaming, token-by-token)
- ✅ Context pack builder (relevance spread over the dependency graph, budgeted)
- ✅ Model provider configuration: Anthropic, OpenAI, DeepSeek, Kimi, z.ai/GLM;
  per-provider model selection; keys in per-user config
- ✅ Patch proposal & diff review (CLI `edit`; UI **Diff** tab with Keep/Revert)
- ✅ Command execution (agent `run_command`; CLI `run` with shell selection)
- ✅ Verification loop (agent `verify` + CLI `verify`/`--repair`)
- ✅ Cost/estimate display (live token + cost meter in the chat compose bar)

## Phase 2 — Windows Superpower 🟡

- ✅ Environment discovery (`env`: shells, toolchains, WSL, Docker, versions)
- ✅ Windows shell integration (`run_command` via cmd; CLI `run --shell`)
- ✅ Terminal output streaming (build/verify output streams into the transcript)
- ⬜ USN Journal incremental tracking
- ⬜ WSL bridge daemon · Docker execution adapter · path mapping

## Phase 3 — Professional Reliability 🟡 (active focus)

- ✅ Shadow Build repair loop (agent self-critique + verify-and-fix; CLI `--repair`)
- ✅ Diff review / better code-review surface (colored git diff, Keep/Revert,
  and **red/green +/− line-change counts** — total and per file)
- ✅ **Checkpoints & rollback** (auto-checkpoint before each agent run; restore
  any recent checkpoint from the Diff tab)
- ✅ **Secret scanner** (dependency-free; flags likely keys/tokens/private keys
  in changed files, surfaced in the Diff tab before you commit)
- ✅ **Local audit log** (`.kestrel/audit.log`: every agent run and tool action,
  timestamped) + an in-app **Audit** viewer
- ✅ **Test selection** (pick the tests affected by the changes via the
  dependency graph; "Test changes" in the Diff tab runs only those)
- ✅ **Dirty-worktree protection** (uncommitted work is checkpointed before a
  run and the user is told, so nothing is lost)
- ✅ **Resilient networking** — model calls over `curl` now retry transient
  connection failures (TLS/reset/timeout, e.g. `curl (35)`/`(56)`) with backoff
  instead of aborting the run; a mid-stream drop still surfaces cleanly
- ✅ **Live code streaming** — the agent turn now streams; files are shown being
  written **token-by-token in real time** (like an IDE) via partial tool-argument
  parsing, instead of appearing all at once after the model finishes the file.
  Falls back to the buffered turn for any provider that can't stream tool calls
- ✅ **No more silent death / full run control** — hitting the step budget (now
  250, up from 100) is a graceful **pause** with a **▶ Continue** button that
  resumes from exactly where it left off, not a failure. **⏹ Stop** now truly
  halts the worker via a cancellation token (it used to keep running and
  spending); **New chat** stops any running agent and resets. Budget hard-stops
  also cancel the worker so it can't keep spending
- ✅ **Permission prompts** — an optional "ask before running commands / installs
  / git" mode (Settings) pops an **Allow / Allow-all-this-run / Deny** prompt
  before each system-touching action; **Deny** feeds back so the agent adapts
  instead of the run dying
- ✅ **Autonomy Core** (Phase A of [autonomy-plan.md](autonomy-plan.md)) — the
  agent now **plans**: an `update_plan` tool decomposes a goal into a checklist it
  works step by step, persisted to `.kestrel/plan.json` and shown as a live
  **🗺 Plan** ledger (progress, active/done). **Stall detection** nudges it off
  repeated no-progress actions; a **plan-aware reflect** step refuses to declare
  "done" while steps remain. Turns "wandered off at step 100" into "worked the
  checklist to completion."
- ✅ **Diagnostics** (LSP-style, dependency-free) — run the project's checker
  (`cargo check` / `tsc` / `ruff`), parse errors/warnings into a **⚠ Problems**
  tab (click to open the file) and an inline strip in the editor
- ✅ **Policy engine** — disable tools or block command patterns; a denied tool
  call is refused (the agent adapts), enforced on every call, editable in
  Settings, with destructive defaults blocked out of the box

## Beyond the roadmap line (shipped early because cheap and high-value)

- ✅ Native file explorer: create/rename/delete files & folders
- ✅ Source editor: syntax highlighting (dependency-free), symbol outline,
  save, and **multi-language Format** — dispatches to the right tool by file
  type (rustfmt, gofmt, black, prettier), gracefully reporting when one isn't
  installed
- ✅ **Any language, any framework** — the agent is instructed to build in any
  stack, to **research unfamiliar frameworks/APIs from the docs before writing**
  (via `http_get`), and to scaffold with each ecosystem's own tools
- ✅ Light/dark theme
- ✅ Agent toolset: `read_file`, `list_dir`, `http_get`, `search`, `write_file`,
  `edit_file` (diff-style), `run_command`, `git`, `verify`
- ✅ System agent: `open_url` (preview in browser), `start_app`/`app_logs`/
  `list_apps`/`stop_app` (background dev servers with captured logs + health
  check), `http_check` (poll until a server responds), `screenshot`,
  `install_tool` (detect + winget install missing toolchains, e.g.
  composer/php/node). `run_command` refuses to hang on servers.
- ✅ **Run tab** in the UI: start/stop the app, watch its logs live, and open a
  browser preview — a real runner beside the agent.
- ✅ Intelligent live status (the stream shows the real action with icons) and a
  visual polish pass (amber accent, rounded widgets, iconified controls)
- ✅ Token economy (a core differentiator):
  - **Prompt caching** (Anthropic) — the system+tools prefix and a rolling
    history breakpoint are cache-controlled, so repeat agent turns re-read the
    conversation at ~10% cost instead of re-billing it
  - **`edit_file`** snippet diffs instead of full-file rewrites
  - **Token-aware auto-compaction** tied to each model's real context window
  - **Real usage accounting** — actual input/output/cache tokens parsed from
    responses and streams, shown as a live session meter with cost
  - **Live context gauge** (used / window), **cache-savings** readout, and
    **quick model switch** in the chat bar (route a cheaper model mid-conversation)
  - **Usage dashboard** (📊) — per-request usage logged to `.kestrel/usage.jsonl`;
    a view with this-conversation + all-time totals, cost, per-model breakdown,
    how much prompt caching saved, and **CSV export** (Phase 4 wedge)
  - **Budget controls** — per-conversation and per-day USD caps (Settings); a
    live budget status line, blocked send, and a hard-stop that halts the agent
    mid-run when a cap is reached
  - **Shared policy** — a `[policy]` section in `kestrel.toml` a team commits to
    the repo, merged (union) with each user's policy (more restricted, never less)
- ✅ Conversation memory + per-project session persistence (`.kestrel/agent-session.json`)
- ✅ Live build preview: created-files history with click-to-preview

## Phase 5 — Category Leadership 🟡 (started)

- ✅ **Autonomous verified workflows** (⚡) — named, reusable agent recipes that
  run the agent (with checkpoints, verification, policy, budgets) on a filled
  prompt. Built-ins deliver the roadmap's specialized agents: **release
  readiness**, **security remediation**, **migration** (from→to), **incident
  assistant** (root-cause a log), raise test coverage, update dependencies. User
  workflows persist to `<config>/kestrel/workflows.toml` — a shareable file that
  seeds a **workflow marketplace**.
- ✅ **Multi-repository reasoning** (🔗) — link sibling repos into a workspace
  (stored in `.kestrel/workspace.json`); the agent gets `list_repos()` and a
  `repo`-scoped `search(query, repo="name")`, and reads any linked repo by
  absolute path, so it can trace and reason across repo boundaries (writes stay
  in the primary project). Link/open/unlink repos from the Explorer.
- ✅ **Workflow marketplace** (🛍) — a curated **Catalog** of ready-made recipes
  (document, performance pass, accessibility audit, dockerize, build-API-from-
  spec, remove dead code) you **Install** with one click; **author your own**
  (name/description/params/prompt, with `{param}` validation); **edit** or
  **remove** any workflow (a customized built-in reverts to default); and
  **Import/Export** `.toml` files to share recipes with a team or community.
- ⬜ Private deployment

## Next candidates

1. **Team settings / shared config** — the rest of Phase 4.
2. **Audit export** — export the audit + usage logs (Phase 4).
3. **Multi-repo reasoning** — reach across repositories (Phase 5).
4. **tree-sitter parsing** — semantic-perfect symbols (deferred Phase 0 upgrade).
