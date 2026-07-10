# Kestrel

Kestrel is a Windows-native AI coding agent focused on fast local repository understanding, safe edits, verified diffs, and transparent model cost controls.

**Where this is going:** Kestrel is the local engineering runtime for the next decade of software. The near-term product delivers *verified diffs* — the human directs, the agent executes and proves its work. The destination is *continuous intent maintenance*: the human owns intent, invariants, and taste, while a fleet of local, verifying agents keeps the system perpetually correct against that intent. Code is a liability; intent is the asset. The full vision is in [docs/vision-horizon.md](docs/vision-horizon.md), and the credible bridge to it is in [docs/roadmap.md](docs/roadmap.md).

This repository currently starts with the Phase 0 foundation from the product roadmap: a Rust workspace with a CLI and local core capable of opening a repository, detecting its Git root, building a file inventory, detecting languages, and producing an onboarding summary. Every component here is designed as the literal substrate of the horizon — the Phase 0 context graph is the seed of the Living System Model, not throwaway scaffolding.

## Workspace

- `crates/kestrel-core`: local indexing and project analysis primitives, including the structural symbol-extraction layer (the seed of the Ghost Context Engine).
- `crates/kestrel-cli`: command-line entry point for all workflows.
- `crates/kestrel-ui`: a native desktop shell (egui/eframe) over `kestrel-core`.
- `docs`: product, architecture, requirements, roadmap, and planning documents.

## Native desktop app

Kestrel also ships a native, all-Rust desktop shell:

```powershell
cargo run -p kestrel-ui
```

- **Project management** — **Open…** picks an existing folder with a native dialog; **New project…** scaffolds a fresh, Kestrel-ready project (a `src/` folder, a starter `kestrel.toml`, `README.md`, `.gitignore`, and `git init` when git is present); **Recent ▾** reopens a recently used project. The recent list is remembered in your per-user settings. **Load** opens whatever path is typed in the Project box.
- **File explorer** — a real directory tree in the left panel. Expand folders, click a file to open it, and use **+ File** / **+ Folder** (or a right-click menu on any node) to **create**, **rename**, or **delete** files and folders. Build/VCS noise (`target`, `.git`, `node_modules`, …) is hidden.
- **Source editor** — the central **Editor** tab shows the selected file in a code editor with an **Outline** of its symbols; edit and **Save** (Ctrl+S), or **Format** a Rust file with `rustfmt`. An unsaved indicator shows pending changes. The **Output** tab holds the results of the action bar.
- **Action bar** — Inspect, Graph, a Context query box, plus **Verify** (runs the project's verification ladder) and **Env** (host shells/toolchains/WSL/Docker); results land on the Output tab.
- **Settings** (⚙, top-right) — your name/email and one or more **model providers**. Pick a preset (Anthropic, OpenAI, DeepSeek, or Kimi) to prefill its API kind, base URL, and a current default model, then paste your API key and choose which provider is active. Settings are written to your per-user config directory (`%APPDATA%\kestrel\settings.toml` on Windows), **never** into the project — because they hold secrets.
- **Chat** (💬, top-right) — a conversation with your **active provider** (configured in Settings). Press **Enter** to send (Shift+Enter for a new line), or the **Send** button; **Stop** cancels an in-flight reply and **New chat** clears the thread. Toggle **Include project context** to attach the files most relevant to your message — Kestrel seeds a context pack from your question, spreads relevance across the dependency graph, and sends the top files' source as grounding. The request runs on a background thread, so the window never freezes while the model replies.
- **Never freezes** — slow work (verification, indexing, model calls) runs on a background thread; the window stays responsive.

It calls `kestrel-core` directly (no subprocess, no web view). The CLI still hosts the scriptable model actions (`ask`/`edit`) with diff review and verification; the desktop Chat is the interactive counterpart.

#### Choosing a model

Each provider ships a short list of **suggested** models (latest/best first) that you can pick from a dropdown — but the model field is free text, so you can also type any model ID the provider currently supports. The suggestions are only a convenience; they intentionally don't lock you to one model. Provider APIs move fast (e.g. DeepSeek's v4 line, OpenAI's GPT-5 line), so if a newer model isn't in the list yet, just type its ID.

## Commands

Summarize a repository — Git root, languages, a structural symbol overview, project markers, and likely build/test commands:

```powershell
cargo run -p kestrel-cli -- inspect E:\Projects\some-repo
```

List the structural symbols (functions, methods, types, classes, constants…) in a file or across a directory, with visibility and containing scope:

```powershell
cargo run -p kestrel-cli -- symbols E:\Projects\some-repo\src\main.rs
```

Show the inferred file dependency graph — which files depend on which, and the symbols that connect them:

```powershell
cargo run -p kestrel-cli -- graph E:\Projects\some-repo
```

Show one file's place in that graph: what it depends on and what depends on it (the "relevant context" primitive — select a small related set instead of dumping the whole repo):

```powershell
cargo run -p kestrel-cli -- related E:\Projects\some-repo\src\service.ts
```

Build a **context pack**: a ranked, token-budget-bounded selection of the files most relevant to a seed, each with a reason for inclusion and an estimated token cost. This is what a model would actually receive as context:

```powershell
cargo run -p kestrel-cli -- context E:\Projects\some-repo\src\service.ts --budget 8000
```

Relevance spreads outward from the seed across dependency edges (both directions) with per-hop decay; the budget is filled greedily by relevance, and files that don't fit are listed as omitted so the selection stays transparent. Add `--format prompt` to emit the pack as assembled, ready-to-paste prompt text (each included file's full source in a fenced block):

```powershell
cargo run -p kestrel-cli -- context E:\Projects\some-repo\src\service.ts --format prompt
```

You can also seed a pack from a **natural-language query** instead of a file — the files whose symbols or paths best match the query become the seeds, then relevance spreads across the graph from all of them:

```powershell
cargo run -p kestrel-cli -- context E:\Projects\some-repo --query "user authentication" --budget 8000
```

Extraction runs behind a swappable `SymbolExtractor` trait, with dependency-free heuristic extractors for Rust, TypeScript/JavaScript, and Python that resolve symbols, imports, and cross-file references. The scanners are string- and comment-aware (block comments, raw strings, multi-line strings, BOMs, Rust lifetimes vs char literals). Dependency edges fuse two kinds of evidence: shared symbol references, and import specifiers resolved to concrete files (Rust `crate::`/`self::`/`super::` module-tree resolution; TS/JS relative imports with extension and `index.*` conventions; Python relative and absolute-from-root module resolution). The trait boundary is deliberate: a full tree-sitter backend can replace any extractor later without changing a single caller. Together the symbol index and the `ProjectGraph`/`DependencyEdge` structures are the Phase 0 substrate of the Living System Model described in [docs/vision-horizon.md](docs/vision-horizon.md).

Ask a natural-language question about the codebase. Kestrel seeds a context pack from the question, assembles an Anthropic Messages request, and answers using a model:

```powershell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
cargo run -p kestrel-cli -- ask "how are dependency graph edges built and ranked?" E:\Projects\some-repo
```

The model call is made through the system `curl` (no bundled TLS stack), so it works anywhere `curl` is on `PATH` (Windows 10+, macOS, Linux all ship it). Flags: `--model NAME` (default `claude-opus-4-8`), `--budget N` (context tokens), `--max-tokens N` (answer cap), and `--dry-run` to print the exact request JSON without sending it. Without `ANTHROPIC_API_KEY` set, `ask` prints the assembled prompt instead of calling the API, so it stays useful offline.

Propose a **reviewed diff** for a file. Kestrel gathers the file plus its graph-related context, asks the model for the complete updated file, and prints a unified diff. Nothing is written unless you pass `--apply`:

```powershell
cargo run -p kestrel-cli -- edit E:\Projects\some-repo\src\service.ts "add input validation to the load() method"
# review the diff, then:
cargo run -p kestrel-cli -- edit E:\Projects\some-repo\src\service.ts "add input validation to the load() method" --apply
```

This is the *verified diff* wedge from [docs/vision-horizon.md](docs/vision-horizon.md): the human directs, the agent produces a concrete change, and the human reviews before it lands. Same transport and flags as `ask`, plus `--apply` to write.

Run the project's **verification ladder** — the detected format/test/build commands, executed in order and short-circuiting on the first failure:

```powershell
cargo run -p kestrel-cli -- verify E:\Projects\some-repo
```

Each step reports PASS/FAIL with duration and, on failure, the tail of its output; the process exits non-zero if any step fails. The ladder is derived from the project's markers (e.g. a Cargo workspace runs `cargo fmt --all -- --check` then `cargo test`; a Node project runs its package-manager `test` script; Python runs `pytest`; Go/`.NET` run build+test).

Combine them for a **safe, verified apply** — Kestrel applies the edit, runs verification, and (with `--revert-on-fail`) rolls the file back if verification fails:

```powershell
cargo run -p kestrel-cli -- edit src\service.ts "add a null check" --apply --verify --revert-on-fail
```

That is the full Era-2 loop: the human directs, the agent produces a concrete change, verification proves it, and a failing change is automatically undone.

Add **`--repair[=N]`** to make the change *self-heal*: if verification fails, Kestrel feeds the failing step's output back to the model to fix the file, re-applies, and re-verifies — up to `N` attempts (default 2). `--repair` implies `--apply --verify`.

```powershell
cargo run -p kestrel-cli -- edit src\parser.rs "handle the empty-input case" --repair=3 --revert-on-fail
```

This is the "Shadow Build" self-healing loop from the docs: propose → verify → on failure, repair against the real error output → re-verify, bounded by an attempt limit, reverting if it never passes.

Show the **host environment** Kestrel can build and run against — OS, shells, WSL/Docker availability, and installed language toolchains with versions (each probed by actually invoking the tool):

```powershell
cargo run -p kestrel-cli -- env
```

Run a command in a chosen shell from the project root, with its output streamed live and its exit code propagated:

```powershell
cargo run -p kestrel-cli -- run "cargo test" --shell powershell
```

`--shell` accepts `default` (the platform shell), `powershell`, `pwsh`, `cmd`, `bash`, or `sh`. This is Kestrel's shell-integration layer — the same execution path `verify` uses, exposed directly.

### Configuration (`kestrel.toml`)

Drop an optional `kestrel.toml` at the project root to set defaults and pin the verification ladder. Everything is optional; CLI flags always override config, and config overrides the built-in defaults.

```toml
[defaults]
model = "claude-sonnet-5"   # default model for ask/edit
budget = 12000              # default context token budget
max_tokens = 8192           # default answer/edit token cap

[verify]
# Override the auto-detected ladder with exactly the checks a change must pass.
steps = [
  "cargo fmt --all -- --check",
  "cargo clippy --all-targets -- -D warnings",
  "cargo test",
]
```

`kestrel verify` reports whether it used the detected ladder or your `kestrel.toml`, and `ask`/`edit` fall back to the config's defaults when you omit `--model`/`--budget`/`--max-tokens`.

### Incremental index cache

The `graph`, `related`, and `context` commands persist their parse results to `<project-root>/.kestrel/index.json`, keyed by each file's size and modification time. On the next run only changed files are re-parsed — the first real step from a re-derived context engine toward a *living*, incrementally updated one. The cache directory is git-ignored; delete `.kestrel/` to force a full rebuild.

## Building, testing, and running it yourself

### Prerequisites

- **Rust** (stable, 1.85+). Install from <https://rustup.rs>.
- On this machine there is no MSVC linker or Windows SDK, so builds use the self-contained **`x86_64-pc-windows-gnu`** toolchain. If you hit a `link.exe not found` error, install and select it:

  ```powershell
  rustup toolchain install stable-x86_64-pc-windows-gnu
  rustup override set stable-x86_64-pc-windows-gnu   # run once, inside the repo
  rustup component add rustfmt clippy --toolchain stable-x86_64-pc-windows-gnu
  ```

  If you *do* have the Visual Studio Build Tools + Windows SDK installed, you can skip all of that and use the default `msvc` toolchain instead.

### Build

```powershell
cargo build            # debug build of the whole workspace
cargo build --release  # optimized build
```

### Run

Run the CLI straight from source with `cargo run -p kestrel-cli -- <command> [args]`. Point it at *any* repository on your machine (it does not have to be Kestrel). A quick tour:

```powershell
# Summarize a project (languages, symbols, markers, likely commands)
cargo run -p kestrel-cli -- inspect E:\Projects\some-repo

# List structural symbols for one file or a whole folder
cargo run -p kestrel-cli -- symbols E:\Projects\some-repo\src

# Show the dependency graph and how files connect
cargo run -p kestrel-cli -- graph E:\Projects\some-repo

# See what one file depends on and what depends on it
cargo run -p kestrel-cli -- related E:\Projects\some-repo\src\service.ts

# Build a ranked, budget-bounded context pack for a file
cargo run -p kestrel-cli -- context E:\Projects\some-repo\src\service.ts --budget 8000
```

You can also dogfood it on Kestrel itself — run any command with `.` as the path from the repo root:

```powershell
cargo run -p kestrel-cli -- inspect .
cargo run -p kestrel-cli -- graph .
```

For a faster binary, build once and call it directly:

```powershell
cargo build --release
.\target\release\kestrel-cli.exe inspect .
```

### Test and lint

```powershell
cargo test                              # run the unit test suite
cargo fmt --all -- --check              # verify formatting
cargo clippy --all-targets -- -D warnings   # lint with warnings treated as errors
```

All three should pass cleanly before you commit. `cargo test` alone is enough for a quick check while iterating.

## Toolchain note

Kestrel builds with the standard **`x86_64-pc-windows-msvc`** toolchain (Visual Studio Build Tools 2022 + Windows SDK). No special setup is required beyond a normal Rust install and the C++ build tools.

If you are on a machine without the MSVC linker/SDK, the self-contained **`x86_64-pc-windows-gnu`** toolchain also works as a fallback (`rustup toolchain install stable-x86_64-pc-windows-gnu` then `rustup override set …` in the repo), with the caveat that C-dependent and `windows-sys`-dependent crates won't build there.
