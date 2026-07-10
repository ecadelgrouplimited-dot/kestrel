# Kestrel Technical Architecture

## Architectural Goal

Kestrel should be a local-first AI engineering runtime with a native Windows user experience, a fast Rust core, structured code intelligence, safe execution tools, and flexible model orchestration.

## High-Level System

```text
Native UI / CLI / IDE Extensions
        |
        v
Kestrel Control Plane
        |
        +-- Conversation and task planner
        +-- Diff and approval manager
        +-- Model router
        +-- Policy engine
        |
        v
Rust Local Core
        |
        +-- File watcher and USN Journal indexer
        +-- AST parser and symbol graph
        +-- Git telemetry engine
        +-- Local cache and context pack builder
        +-- Verification runner
        +-- Secret scanner
        |
        v
Execution Adapters
        |
        +-- Native Windows shell
        +-- PowerShell
        +-- WSL bridge daemon
        +-- Docker bridge daemon
        +-- IDE and LSP adapters
        |
        v
Model Providers
        |
        +-- Hosted frontier models
        +-- Private model gateways
        +-- Local models
```

## Core Components

## 1. Native Shell

Recommended options:

- WinUI 3 for the primary Windows desktop experience.
- Rust CLI for terminal-first workflows.
- Optional IDE extensions that communicate with the local core.

Responsibilities:

- Project selection.
- Conversation UI.
- Plan and diff review.
- Command approval.
- Settings and model configuration.
- Cost and verification visibility.

## 2. Rust Local Core

Responsibilities:

- Repository indexing.
- File system monitoring.
- Context graph creation.
- Local cache management.
- Tool execution coordination.
- Policy enforcement.
- IPC server for UI, CLI, and extensions.

Recommended crates and technologies:

- `notify` or native Windows APIs for file watching.
- Windows USN Journal integration for high-scale incremental tracking.
- `tree-sitter` for AST parsing.
- `git2` or shell-backed Git integration.
- `tokio` for async runtime.
- `serde` for structured data.
- SQLite or RocksDB for local index storage.
- Tantivy or similar for local search.

## 3. Ghost Context Engine

Pipeline:

1. Discover project root and language stack.
2. Build file inventory.
3. Parse supported files with tree-sitter.
4. Extract symbols, definitions, references, imports, exports, routes, schemas, and tests.
5. Overlay Git telemetry: blame, recency, churn, authorship, branches.
6. Build dependency graph.
7. Summarize modules locally.
8. Store context packs in local cache.
9. Assemble prompt context based on task, budget, and model.

Key data structures:

- `ProjectGraph`
- `FileNode`
- `SymbolNode`
- `ReferenceEdge`
- `DependencyEdge`
- `GitTelemetry`
- `DiagnosticRecord`
- `ContextPack`
- `TaskTrace`

## 4. Verification Runner

The verification runner should execute checks in a controlled ladder:

- Syntax parse.
- Formatter.
- Linter.
- Type checker.
- Targeted tests.
- Full test suite.
- Build.
- Custom user commands.

Every verification result should be stored as structured data:

- Command.
- Working directory.
- Environment.
- Exit code.
- stdout and stderr excerpts.
- Parsed diagnostics.
- Duration.
- Files implicated.
- Repair attempt count.

## 5. WSL Bridge

The WSL bridge should be a small daemon installed inside the target WSL distro.

Responsibilities:

- Receive authenticated local IPC requests.
- Execute shell commands in the correct Linux environment.
- Read file metadata.
- Stream logs.
- Return structured command results.
- Avoid slow and fragile ad hoc path translation.

Security:

- Pairing token per Windows user.
- Project-scoped access.
- No remote network listener by default.
- Clear install and uninstall flow.

## 6. Docker Bridge

The Docker bridge should support:

- Running commands in active containers.
- Devcontainer detection.
- Container file path mapping.
- Log streaming.
- Test/build execution.
- Environment discovery.

## 7. Model Router

Responsibilities:

- Select model by task type.
- Estimate token usage and cost.
- Apply user budget.
- Prefer cached context when available.
- Escalate only when the task requires it.
- Support hosted, private, and local providers.

Routing examples:

- Cheap model: classification, file ranking, summary refresh.
- Balanced model: normal edits, explanations, docs.
- Deep model: architecture changes, multi-file refactors, hard debugging.
- Local model: private code summarization, offline mode, cheap autocomplete.

## 8. Policy Engine

Policy decisions should be centralized and auditable.

Policy domains:

- File read access.
- File write access.
- Command execution.
- Network access.
- Dependency installation.
- Secret handling.
- Model provider routing.
- Token and spending limits.
- WSL/Docker bridge permissions.

## 9. Storage

Local storage should be explicit and inspectable.

Suggested stores:

- SQLite for metadata, tasks, settings, and audit logs.
- RocksDB or equivalent for high-volume index data if needed.
- File-backed cache for context packs.
- OS credential vault for API keys and secrets.

## 10. Security Model

Default stance:

- Local code remains local unless included in an approved model request.
- Sensitive files are excluded by default.
- Secrets are detected and redacted.
- Tool calls are logged.
- Destructive operations require approval.
- Enterprise policy can enforce stricter defaults.

## Horizon Architecture (Era 3 Extensions)

The components above are the buildable system. This section describes how each one extends toward the [Horizon](./vision-horizon.md) without a rewrite. The design rule is strict: **the MVP components must be the literal substrate of the horizon components.** If a horizon capability would require throwing away an MVP component, the MVP component is designed wrong.

### From Ghost Context Engine to Living System Model

The context graph gains persistence and a runtime overlay. `ProjectGraph` stops being rebuilt per task and becomes a long-lived, incrementally maintained store with three new layers:

- `BehaviorOverlay` — runtime facts joined onto the static graph: hot paths, observed invariants, exception sites, historical break points per symbol.
- `IntentLink` — edges from code symbols to the version-controlled intent artifacts they implement.
- `TemporalIndex` — a queryable history of the graph itself, enabling "when did this invariant first break" as a lookup rather than an investigation.

Design requirement: every field the MVP writes into `ProjectGraph` must be addressable by a stable symbol identity that survives refactors, because the temporal and behavior overlays key on it.

### From Verification Runner to Provable Diffs

The verification runner's structured results become the raw material for evidence synthesis. A new `EvidenceSynthesizer` stage runs after the verification ladder and, scaled to change risk, emits:

- Generated property tests and invariants for ordinary changes.
- Auto-synthesized formal proofs for tractable critical paths.
- A signed `EvidenceBundle` attached to the diff, so a change carries its own proof of correctness.

Design requirement: verification results must already be fully structured and deterministic (they are, per the MVP spec), because synthesized evidence must be reproducible and auditable.

### From Agent Workflow to Agent Fleets

The single-agent task loop is wrapped by a `FleetScheduler` in the control plane that runs specialized agents (implementer, tester, reviewer, security, performance) in parallel, each against a partition of the plan, reconciled into one unified diff and one coherent approval surface. The IPC server must support concurrent task traces and fine-grained cancellation from day one so that the fleet is an orchestration change, not a core change.

### From Model Router to the On-Device Frontier

The router's cost/quality selection generalizes into a local-first spectrum where on-device models are the default execution path and hosted frontier models are the escalation. The provider abstraction the MVP ships must already be indifferent to where a model runs, so that shifting the default from cloud to local over the coming years is a policy change, not an architecture change.

### From Policy Engine to Governed Autonomy

The policy engine gains a trust-ladder model: per-agent, per-action-class capability grants, an approval queue for ambient background work, and a complete, tamper-evident audit trail for every autonomous action. Autonomy is expressed as loosened human-owned policy, never as an agent operating outside the policy engine.

## Initial Language Support

Priority order:

1. TypeScript and JavaScript.
2. Python.
3. Rust.
4. C#.
5. Go.
6. Java.
7. PHP.
8. Ruby.

Selection rationale:

- Strong Windows developer relevance.
- Available tree-sitter grammars.
- Common LSP and test tooling.
- High usage in modern full-stack work.

