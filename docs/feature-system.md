# Kestrel Feature System

This document defines the complete feature universe for Kestrel. It is intentionally ambitious, but organized into layers so the team can ship a focused MVP and then compound toward category leadership.

## Feature Tiers

### Tier 0: Non-Negotiable Foundation

- Native Windows shell with low idle memory.
- Rust background service.
- Project open, scan, and incremental file tracking.
- Git awareness.
- AST parsing for priority languages.
- Chat with repository context.
- File editing with diff review.
- Terminal command execution with approval gates.
- Basic test/lint/build verification.
- Model provider configuration.
- Token and cost estimate display.

### Tier 1: MVP Differentiators

- Ghost Context Engine.
- Shadow Build self-healing loop.
- WSL bridge.
- Docker bridge.
- Code Architecture Map.
- Verified diff workflow.
- Repo onboarding summary.
- Task planning mode.
- Context budget preview.
- Local secrets detection and redaction.
- Per-project settings.

### Tier 2: Professional Daily Driver

- Multi-step agent workflows.
- Persistent task memory.
- Language-server diagnostic integration.
- Branch-aware change planning.
- Test selection and impact analysis.
- Safe dependency install workflow.
- Rollback and checkpoint system.
- Code review assistant.
- PR description generation.
- Commit message generation.
- Local documentation generation.
- Architecture diagram generation.
- Workspace command palette.

### Tier 3: Team and Enterprise

- Team policy console.
- Central model routing policy.
- Usage and cost dashboards.
- Audit logs.
- Sensitive file policies.
- Approval workflows.
- Shared repository knowledge packs.
- Private model gateway support.
- SSO and role-based access control.
- Compliance export.
- Admin-managed extension and command allowlists.

### Tier 4: Category Leadership

- Autonomous verified task execution.
- Multi-repository reasoning.
- Production incident assistant.
- Migration agent.
- Security remediation agent.
- Performance profiling agent.
- Release readiness agent.
- Design-to-code workflow.
- Natural language observability queries.
- Organization-wide engineering memory.
- Marketplace for tools, skills, workflows, and connectors.

### Tier 5: The Horizon (Era 3 — Continuous Intent Maintenance)

This tier is the ten-year summit described in [Kestrel Horizon](./vision-horizon.md). It is intentionally beyond the buildable roadmap, but it is not vague: each item is a direct evolution of a Tier 0–4 component, which is how we keep it honest. We do not build toward these directly; we build the earlier tiers so well that these become reachable.

- **Living System Model** — a persistent, semantic, causal digital twin of the whole system (code, runtime behavior, schema, infra, history), never re-derived per prompt. *(Evolution of the Ghost Context Engine.)*
- **Provable Diffs** — changes ship with machine-checked evidence: auto-synthesized property tests, invariants, and formal proofs on tractable critical paths. *(Evolution of the Shadow Build loop.)*
- **Agent Fleets** — orchestrated swarms of specialized local agents (implement, test, review, secure, optimize) running in parallel across a repo or org under one scheduler. *(Evolution of the agentic editing workflow.)*
- **Intent as Source of Truth** — specs, invariants, and acceptance criteria as first-class version-controlled artifacts kept bidirectionally synchronized with code. *(Evolution of task planning + the knowledge layer.)*
- **Simulation and Time-Travel** — simulate a change's blast radius before applying; replay history; branch reality; answer "what would happen if" and "when did this break" as queries. *(Evolution of change-impact analysis.)*
- **On-Device Frontier** — the majority of work runs on local models at build-time latency; hosted frontier models reserved for the hardest 5%. *(Evolution of the model router.)*
- **Ambient Engineering** — always-on background maintenance of dependency drift, security advisories on real call paths, performance regressions, flaky tests, and doc rot, surfaced through an approval queue. *(Evolution of the verification loop + policy engine.)*
- **Multimodal Intent Capture** — voice, sketches, diagrams, screenshots, incidents, and complaints grounded into structural changes. *(Evolution of natural-language task intake.)*
- **Organizational Engineering Memory** — a shared, permissioned world model of an entire org's engineering reality, conventions, and scars. *(Evolution of persistent task memory + knowledge packs.)*
- **Governed Autonomy** — a trust ladder from "approve every diff" to "autonomous within explicit, auditable guardrails," scaling from individual to enterprise. *(Evolution of the policy and audit layer.)*

## Core Feature Areas

## 1. Native Performance Layer

Purpose: make Kestrel feel invisible until needed.

Features:

- Rust indexing daemon.
- WinUI 3 desktop shell.
- Optional CLI-only mode.
- Low-memory idle mode.
- Fast cold start.
- Background priority management.
- Battery-aware scanning.
- Large repository throttling.
- Native file dialogs and shell integration.
- Windows notifications.
- System tray status.

Acceptance bar:

- Idle memory target under 100MB for the core service and lean shell.
- Cold start under 2 seconds on common developer machines.
- Index updates should not visibly slow the editor or terminal.

## 2. Ghost Context Engine

Purpose: give models exact, compressed, high-signal context.

Features:

- Tree-sitter AST parsing.
- Symbol extraction.
- Call graph generation.
- Import and dependency graph.
- Class and interface map.
- Function signature inventory.
- Routes, controllers, jobs, commands, and schema detection.
- Git blame and commit recency overlay.
- Ownership and churn scoring.
- Test coverage proximity map.
- Semantic search over local summaries.
- Context pack builder.
- Token-cache aware prompt assembly.

Outputs:

- Code Architecture Map.
- Relevant files list.
- Risk map.
- Change impact map.
- Test recommendation set.
- Context budget preview.

## 3. Agentic Editing Workflow

Purpose: let the agent make real changes safely.

Features:

- Natural language task intake.
- Clarifying question detection.
- Plan generation.
- File read/write toolchain.
- Multi-file patch generation.
- Diff review.
- Change grouping.
- Checkpoints before edits.
- Undo/rollback.
- Conflict detection.
- User-owned dirty change protection.
- Patch explanation.
- Follow-up task suggestions.

Safety controls:

- Never overwrite unrelated user changes.
- Require approval for destructive actions.
- Require approval for dependency installs.
- Require approval for network calls when policy demands it.
- Show exact file list before large edits.

## 4. Shadow Build and Verification Loop

Purpose: make verified diffs the default output.

Features:

- Language and framework detection.
- LSP diagnostics ingestion.
- Formatter execution.
- Linter execution.
- Type checker execution.
- Unit test execution.
- Targeted test selection.
- Build command execution.
- Compiler trace parsing.
- Automatic repair loop.
- Verification summary.
- Confidence and residual risk report.

Verification ladder:

1. Parse-level validation.
2. Format validation.
3. Static diagnostics.
4. Targeted tests.
5. Full test suite.
6. Build/package validation.
7. User-defined release checks.

## 5. WSL and Docker Bridge

Purpose: remove Windows/Linux boundary friction.

Features:

- WSL distro discovery.
- Docker container discovery.
- Bridge daemon installation.
- Path translation.
- Fast file metadata access.
- Remote command execution.
- Environment variable capture.
- Process status.
- Port forwarding awareness.
- Container log streaming.
- Devcontainer support.
- Permission and shell profile handling.

User value:

- The agent runs commands where the project actually lives.
- The user does not debug path translation problems.
- Build/test output is captured and fed back into the repair loop.

## 6. Cost and Model Intelligence

Purpose: make AI usage transparent and efficient.

Features:

- Token estimation before execution.
- Provider price configuration.
- Model routing by task type.
- Prompt caching support.
- Local summary cache.
- Reuse of repository context packs.
- Cheap-model preflight.
- Expensive-model escalation.
- Spending limits.
- Per-project budget.
- Per-task budget.
- Team usage dashboard.

User-facing modes:

- Fast.
- Balanced.
- Deep.
- Local-only.
- Budget-protected.

## 7. Developer Knowledge Layer

Purpose: make the agent understand the project like a teammate.

Features:

- Repo onboarding report.
- Architecture summary.
- Build and test command discovery.
- Conventions detection.
- Coding style memory.
- Framework inventory.
- Service map.
- API map.
- Database schema map.
- Environment setup guide.
- Runbook generation.
- ADR generation.
- README improvement.
- Internal docs search.

## 8. Collaboration and Review

Purpose: help teams ship better code.

Features:

- Code review mode.
- PR risk analysis.
- Test gap detection.
- Security smell detection.
- Performance risk detection.
- Backward compatibility check.
- Migration guide generation.
- Release note generation.
- Changelog generation.
- Reviewer assignment suggestions.
- Team comment drafting.

## 9. Security and Trust

Purpose: make Kestrel safe enough for serious teams.

Features:

- Local-first code indexing.
- Sensitive file detection.
- Secret redaction.
- Policy-based file allow/deny lists.
- Command allowlists and blocklists.
- Network access policy.
- Audit log.
- Tool call transcript.
- Model data retention settings.
- Private provider gateway.
- Enterprise key management.
- Offline/local model support.

## MVP Feature Set

The MVP should be narrow but unmistakably strong:

- Native Windows app and Rust service.
- Open local repo.
- Incremental file tracking.
- AST map for TypeScript, JavaScript, Python, Rust, and C#.
- Git-aware context selection.
- Chat with repo context.
- Edit files through reviewed diffs.
- Run project commands with approval.
- Verification loop using detected lint/test/build commands.
- WSL execution bridge.
- Cost preview and model mode selection.
- Project onboarding summary.

