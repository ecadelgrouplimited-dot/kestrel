# Kestrel Roadmap

## Phase 0: Foundation

Goal: prove the local core can index and reason over a repository.

Deliverables:

- Rust workspace scaffold.
- Native Windows service prototype.
- Project open flow.
- File inventory.
- Git root detection.
- `.gitignore` support.
- Tree-sitter parsing prototype.
- Local index storage.
- Basic CLI inspection commands.

Exit criteria:

- Open a repo and list files, languages, symbols, and Git metadata.
- Incremental updates work after file changes.

## Phase 1: MVP Agent

Goal: ship the first usable Kestrel loop.

Deliverables:

- Native UI or CLI-first MVP.
- Chat with repository context.
- Context pack builder.
- Model provider configuration.
- Task plan generation.
- Patch proposal and diff review.
- Command execution with approval.
- Basic verification loop.
- Cost estimate display.

Exit criteria:

- User can ask for a small code change.
- Kestrel selects relevant context.
- Kestrel proposes a diff.
- User approves the diff.
- Kestrel runs checks.
- Kestrel reports result clearly.

## Phase 2: Windows Superpower

Goal: become obviously better on Windows than generic agents.

Deliverables:

- USN Journal incremental tracking.
- PowerShell and Windows shell integration.
- WSL bridge daemon.
- Docker execution adapter.
- Path mapping.
- Terminal output streaming.
- Environment discovery.

Exit criteria:

- Kestrel can work smoothly in Windows, WSL, and Docker projects.
- Cross-boundary command execution is reliable.

## Phase 3: Professional Reliability

Goal: make Kestrel trustworthy for daily engineering.

Deliverables:

- Shadow Build repair loop.
- LSP diagnostics integration.
- Test selection.
- Checkpoints and rollback.
- Dirty worktree protection.
- Secret scanner.
- Policy engine.
- Local audit log.
- Better code review mode.

Exit criteria:

- Most successful agent edits are verified before final delivery.
- Failed checks produce useful repair attempts or clear residual risk.

## Phase 4: Team Product

Goal: support teams and paid professional usage.

Deliverables:

- Team settings.
- Usage dashboard.
- Budget controls.
- Shared policy packs.
- Shared repository knowledge.
- Admin-managed provider settings.
- SSO.
- Audit export.

Exit criteria:

- A team can adopt Kestrel with central controls and predictable cost.

## Phase 5: Category Leadership

Goal: become the default AI engineering layer for Windows-first teams.

Deliverables:

- Multi-repository reasoning.
- Autonomous verified workflows.
- Migration agent.
- Security remediation agent.
- Release readiness agent.
- Incident assistant.
- Marketplace for workflows and connectors.
- Private deployment option.

Exit criteria:

- Kestrel handles complex, multi-step engineering tasks with strong verification and team governance.

## Phase 6: The Living Runtime (Era 3, Part 1)

Goal: cross from "delegated verified change" to "continuously maintained systems." This is where the [Horizon](./vision-horizon.md) begins to become buildable — not as a rewrite, but as the natural next layer on the architecture shipped in Phases 0–5.

Deliverables:

- Living System Model: give the context graph persistence and a runtime-behavior overlay (hot paths, held invariants, historical break points).
- Provable Diffs, phase one: auto-synthesized property tests and invariant checks attached to every change.
- Agent fleet orchestrator: parallel specialized agents (implement, test, review, secure) under one scheduler with a unified plan and diff.
- Intent artifacts: version-controlled specs and invariants kept synchronized with code, with drift detection both directions.
- Simulation preview: report a change's blast radius against the living model before applying.
- On-device model tier as the default execution path where local capability is sufficient.

Exit criteria:

- Kestrel maintains a persistent, queryable model of a real system across sessions rather than re-deriving context per task.
- A meaningful share of changes ship with generated evidence beyond passing tests.
- A fleet can complete a multi-part task in parallel and present it as one coherent, reviewable change.

## Phase 7: Continuous Intent Maintenance (Era 3, Full)

Goal: reach the summit — the human owns intent, invariants, and taste; the fleet owns the perpetual labor of keeping the system correct against that intent.

Deliverables:

- Ambient engineering: always-on background maintenance of dependency drift, security advisories on real call paths, performance regressions, flaky tests, and doc rot, surfaced through an approval queue.
- Provable Diffs, phase two: formal proofs on tractable critical paths (auth, money, memory safety, concurrency).
- Multimodal intent capture: voice, sketches, diagrams, incidents, and complaints grounded into structural changes.
- Organizational engineering memory: a shared, permissioned world model across repos, services, and teams.
- Governed autonomy at scale: a trust ladder from full human approval to bounded autonomous operation with complete audit.

Exit criteria:

- One engineer can be responsible for correct, verified system-behavior across a large surface while safely delegating the maintenance labor.
- Autonomy operates only within explicit, auditable, human-owned policy, and every autonomous action is fully traceable.
- The horizon metric — correct system-behavior maintained per engineer, over time, at trusted autonomy — becomes measurable and improving.

## A Note on Sequencing

The horizon phases are shown to prove the architecture has a summit, not to invite building them early. The discipline of this roadmap is strict: **do not build Phase 6 scaffolding during Phase 1.** Each phase earns the next by being genuinely excellent. The reason every step compounds toward the summit is that the components were designed for it from the first commit — the Ghost Context Engine was always going to become the Living System Model — not because we skip ahead. Ship Phase 0. Ship the verified diff. The summit takes care of itself when the foundation is honest.

