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

**Tests:** 94 core + 8 CLI, all green. Clippy/fmt clean on every commit.

## Phase 0 — Foundation ✅ (tree-sitter deferred)

- ✅ Rust workspace scaffold (core/cli/ui)
- ✅ Project open flow, file inventory, Git root detection, `.gitignore` awareness
- ✅ Local incremental index cache (`.kestrel/index.json`, keyed by size+mtime)
- ✅ CLI inspection commands: `inspect`, `symbols`, `graph`, `related`, `context`
- 🟡 Parsing: dependency-free heuristic extractors (Rust/TS/JS/Python) behind a
  swappable `SymbolExtractor` trait — tree-sitter backend deferred, not blocking
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
- ✅ Diff review / better code-review surface (colored git diff, Keep/Revert)
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
- ⬜ LSP diagnostics integration
- ⬜ Policy engine

## Beyond the roadmap line (shipped early because cheap and high-value)

- ✅ Native file explorer: create/rename/delete files & folders
- ✅ Source editor: syntax highlighting (dependency-free), symbol outline,
  save, `rustfmt`
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
- ✅ Token economy: `edit_file` over full rewrites, automatic history compaction
- ✅ Conversation memory + per-project session persistence (`.kestrel/agent-session.json`)
- ✅ Live build preview: created-files history with click-to-preview

## Next candidates

1. **LSP diagnostics** — surface language-server errors/warnings inline.
2. **Policy engine** — allow/deny rules for tools, paths, and commands.
3. **Usage dashboard** — session token/cost totals over time (Phase 4).
4. **Multi-repo reasoning** — reach across repositories (Phase 5).
