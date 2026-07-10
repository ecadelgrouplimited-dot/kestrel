# Kestrel Document Backlog

These are the additional documents Kestrel should have as the project moves from concept to build to launch.

## Build Documents

- Engineering Design Document: detailed Rust crate boundaries, IPC protocol, storage schema, and execution adapters.
- UI/UX Product Spec: screens, navigation, command palette, diff review, settings, and onboarding.
- CLI Spec: commands, flags, output formats, exit codes, and scripting guarantees.
- IPC Protocol Spec: message types, authentication, versioning, streaming, and error handling.
- Index Schema Spec: file, symbol, edge, diagnostic, Git, cache, and task tables.
- Model Routing Spec: provider abstraction, model selection, fallback, caching, and budget policy.
- Verification Runner Spec: command detection, execution sandboxes, parsing, retries, and repair loop.
- WSL Bridge Spec: daemon installation, pairing, command protocol, path mapping, and uninstall.
- Docker Bridge Spec: container discovery, devcontainer support, command execution, and logs.

## Product Documents

- Positioning and Messaging Guide.
- Pricing and Packaging Strategy.
- Competitive Analysis.
- Launch Plan.
- Beta Program Plan.
- Customer Interview Script.
- Persona Research.
- Activation and Retention Metrics Plan.
- Support and Feedback Workflow.

## Trust and Enterprise Documents

- Security Architecture.
- Threat Model.
- Data Handling and Privacy Policy.
- Secrets Handling Policy.
- Enterprise Admin Guide.
- Audit Logging Specification.
- Compliance Readiness Checklist.
- Incident Response Plan.

## Quality Documents

- Test Strategy.
- Performance Benchmark Plan.
- Repository Fixture Matrix.
- Golden Task Evaluation Set.
- Model Quality Evaluation Rubric.
- Accessibility Checklist.
- Release Readiness Checklist.
- Bug Triage Policy.

## Horizon Research Documents

These documents de-risk the Era 3 summit described in [Kestrel Horizon](./vision-horizon.md). They are research and design spikes, not build specs — their purpose is to prove (or disprove) that each horizon capability is reachable from the shipped architecture before we commit to it.

- Living System Model Design: persistence model, runtime-behavior overlay, temporal indexing, and stable symbol identity across refactors.
- Provable Diffs Research: evidence-synthesis pipeline, property-test generation, and the tractable boundary for auto-synthesized formal proofs.
- Agent Fleet Orchestration Spec: scheduler, agent specialization, parallel task partitioning, diff reconciliation, and unified approval surface.
- Intent-as-Source-of-Truth Spec: intent artifact format, bidirectional code/intent synchronization, and drift detection.
- On-Device Model Strategy: local-inference roadmap, capability thresholds for local-vs-cloud routing, and hardware assumptions.
- Ambient Engineering Design: background maintenance triggers, the approval queue, and noise control.
- Governed Autonomy Framework: the trust ladder, per-agent capability grants, and the autonomous-action audit model.
- Multimodal Intent Capture Study: grounding voice, sketches, diagrams, and incidents into structural changes.

## Open Strategic Questions

- Should the first public product be UI-first, CLI-first, or IDE-extension-first?
- Which model providers should be supported at launch?
- Should local model support be MVP or post-MVP?
- Which language stack should receive the deepest first-class support?
- Should the product initially target individual developers or teams?
- How much autonomy should be available before enterprise controls are mature?
- What pricing model best aligns with cost transparency: seat-based, usage-based, or hybrid?
- At what point does the pricing unit shift from seats/usage to maintained-behavior (value delivered by ambient and autonomous work)?
- How do we prove Provable Diffs to users without demanding they understand formal methods — what is the trust UX for evidence?
- What is the minimum viable Living System Model that a single developer feels immediately, versus the version that only pays off at org scale?
- How much autonomy do we expose, and how fast, given that trust is earned mechanically through verification rather than asserted?

