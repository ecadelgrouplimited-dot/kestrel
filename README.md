# Kestrel

Kestrel is a Windows-native AI coding agent focused on fast local repository understanding, safe edits, verified diffs, and transparent model cost controls.

**Where this is going:** Kestrel is the local engineering runtime for the next decade of software. The near-term product delivers *verified diffs* — the human directs, the agent executes and proves its work. The destination is *continuous intent maintenance*: the human owns intent, invariants, and taste, while a fleet of local, verifying agents keeps the system perpetually correct against that intent. Code is a liability; intent is the asset. The full vision is in [docs/vision-horizon.md](docs/vision-horizon.md), and the credible bridge to it is in [docs/roadmap.md](docs/roadmap.md).

This repository currently starts with the Phase 0 foundation from the product roadmap: a Rust workspace with a CLI and local core capable of opening a repository, detecting its Git root, building a file inventory, detecting languages, and producing an onboarding summary. Every component here is designed as the literal substrate of the horizon — the Phase 0 context graph is the seed of the Living System Model, not throwaway scaffolding.

## Workspace

- `crates/kestrel-core`: local indexing and project analysis primitives, including the structural symbol-extraction layer (the seed of the Ghost Context Engine).
- `crates/kestrel-cli`: command-line entry point for Phase 0 inspection workflows.
- `docs`: product, architecture, requirements, roadmap, and planning documents.

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

Extraction runs behind a swappable `SymbolExtractor` trait, with dependency-free heuristic extractors for Rust, TypeScript/JavaScript, and Python that resolve symbols, imports, and cross-file references. The scanners are string- and comment-aware (block comments, raw strings, multi-line strings, BOMs, Rust lifetimes vs char literals). Dependency edges fuse two kinds of evidence: shared symbol references, and import specifiers resolved to concrete files (Rust `crate::`/`self::`/`super::` module-tree resolution; TS/JS relative imports with extension and `index.*` conventions; Python relative and absolute-from-root module resolution). The trait boundary is deliberate: a full tree-sitter backend can replace any extractor later without changing a single caller. Together the symbol index and the `ProjectGraph`/`DependencyEdge` structures are the Phase 0 substrate of the Living System Model described in [docs/vision-horizon.md](docs/vision-horizon.md).

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

This machine has no MSVC linker or Windows SDK, so the crates are built with the self-contained `x86_64-pc-windows-gnu` Rust toolchain (a directory-local `rustup override` is set for this repo). Installing the MSVC Build Tools + Windows SDK would let the default `msvc` toolchain link as well.
