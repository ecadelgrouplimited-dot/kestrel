# Kestrel Master Product Blueprint

## Executive Summary

Kestrel is a high-performance, Windows-native AI coding agent built for developers who want the intelligence of a senior engineer without the latency, memory cost, and context fragility of browser-based tools. It combines a Rust core, native Windows integration, structural code intelligence, local environment execution, and model orchestration into one agentic development system.

The first wedge is clear: become the best AI coding agent for Windows developers working across local repositories, WSL, Docker, Git, and modern language servers. From that wedge, Kestrel can expand into a full engineering operating layer for individuals, teams, and enterprises.

That expansion has a destination, and it is not "more features." It is a change in what engineering *is*: the human keeps intent, invariants, and taste, while a fleet of local, verifying agents keeps the system correct against that intent — continuously and provably. The full articulation of that destination lives in [Kestrel Horizon](./vision-horizon.md). This blueprint covers the wedge and the near-to-mid strategy; the Horizon covers the summit. Both were designed together, so that every step toward the wedge is also a step toward the summit.

## Vision

Kestrel should make software development feel like working with a calm, fast, precise engineering partner that already understands the codebase, knows the local environment, can safely make changes, and verifies its work before interrupting the developer.

## Mission

Help developers understand, modify, test, and ship software faster by giving AI exact local context, reliable execution tools, and strong safety controls.

## Strategic Positioning

Kestrel is not "ChatGPT in an editor." It is a local intelligence runtime for software engineering.

Positioning statement:

> Kestrel is the native Windows AI coding agent that understands your code structurally, executes safely in your real environment, and delivers verified diffs without heavy editor overhead.

Horizon positioning (where this leads):

> Kestrel is the local engineering runtime that holds a living model of your system, acts on it with a fleet of verifying agents, and keeps your code perpetually correct against the intent you define.

## The Three Eras (Strategic Context)

Kestrel's strategy is a deliberate march through three eras of engineering. Understanding where a competitor sits explains why the fundamentals matter.

- **Era 1 — Assisted Editing.** The human writes code; AI suggests. Unit of work: the keystroke. Most tools are stuck here. Nothing about this era compounds.
- **Era 2 — Delegated Verified Change.** The human directs; the agent executes and proves. Unit of work: the verified diff. This is the MVP wedge — already a category jump over Era 1.
- **Era 3 — Continuous Intent Maintenance.** The human curates intent and taste; a fleet of local agents keeps the system correct as a living artifact. Unit of work: the maintained invariant. This is the summit.

The full narrative, including the ten horizon capabilities and the enduring principles, is in [Kestrel Horizon](./vision-horizon.md). Everything in this blueprint is chosen because it moves Kestrel through these eras without ever building throwaway scaffolding.

## Target Users

### Primary Users

- Windows-first professional developers.
- Full-stack engineers using WSL and Docker.
- Indie hackers and startup builders who need speed and low cost.
- Enterprise developers in restricted Windows environments.
- Engineers working in large monorepos where context quality matters.

### Secondary Users

- Engineering managers who want reliable AI adoption without uncontrolled spend.
- DevOps and platform engineers who maintain complex local environments.
- Security-conscious teams that need local-first context handling.
- Students and self-taught developers who need guided codebase understanding.

## Product Principles

1. Performance is a feature.
   The agent must feel light enough to run all day. Idle memory, indexing cost, and command latency are first-class product metrics.

2. Context must be structural.
   Kestrel should prefer ASTs, symbol graphs, dependency maps, Git history, compiler traces, and LSP diagnostics over loose text dumps.

3. Verification beats confidence.
   The agent should run formatters, tests, type checks, linters, and build commands whenever possible before presenting work as complete.

4. Local-first by default.
   Code indexing, file watching, environment detection, command execution, and sensitive metadata should remain local unless the user explicitly chooses otherwise.

5. Costs must be visible and controllable.
   Developers should know when an action is cheap, expensive, cached, or likely to require a larger model.

6. The user stays in command.
   Kestrel can be proactive, but destructive actions, dependency installs, file deletion, secrets access, and external publishing require clear approval.

## Why Kestrel Can Win

The market is crowded, but most tools share weak points:

- Heavy Electron shells that consume large memory before doing useful work.
- Raw text context that misses architecture and wastes tokens.
- Poor Windows-native support, especially across WSL and Docker boundaries.
- Weak execution loops that suggest code without proving it works.
- Unclear token spend and model routing.
- Limited trust controls for enterprises.

Kestrel can win by being meaningfully better on the fundamentals: speed, context, verification, and trust.

## Strategic Moat

Kestrel's defensibility should come from compounding local intelligence:

- Repository symbol graph and dependency intelligence.
- Incremental file tracking through the Windows USN Journal.
- Cross-boundary execution bridge for WSL and containers.
- Task memory that learns repository conventions without leaking private code.
- Verification traces from builds, tests, language servers, and Git.
- Model routing data that improves cost and quality over time.
- Enterprise policy and audit layer for safe adoption.

The decisive property of this moat is that it **compounds across eras.** The symbol graph becomes the Living System Model. Verification traces become Provable Diffs. Task memory becomes Organizational Engineering Memory. Routing data becomes the on-device/cloud spectrum. An Era 1 competitor has nothing that accrues; Kestrel's every task makes the next one cheaper, safer, and better. A rival cannot buy the years of local intelligence a Kestrel install has quietly accumulated on a real codebase.

## Product Surface

Kestrel can exist in multiple surfaces while sharing the same local core:

- Native desktop app for project navigation, chat, diffs, plans, and settings.
- CLI for terminal-first workflows.
- IDE extensions for VS Code, Visual Studio, JetBrains, and Neovim.
- Background service for indexing, file watching, and environment state.
- WSL and Docker agent daemons for fast cross-environment execution.
- Team console for policy, usage, audit, and shared knowledge.

## North Star Metric

Verified engineering work completed per active developer per week.

The horizon successor to this metric — the one that measures Era 3 — is **correct system-behavior maintained per engineer, over time, at trusted autonomy.** Not lines written, not diffs merged, but how much true, verified behavior one human can be responsible for while safely delegating the labor of keeping it true. The near-term metric is a faithful leading indicator of the horizon metric, which is why we can optimize the former today without drifting from the summit.

Supporting metrics:

- Time to first useful answer.
- Time from request to verified diff.
- Percentage of agent changes that pass first verification.
- Manual edits required after agent output.
- Token cost per verified task.
- Idle memory footprint.
- Index freshness.
- User trust actions: accepted diffs, repeated workflows, automation approvals.

## Category Leadership Bar

Kestrel becomes number one only if it is not merely useful, but obviously better in daily engineering work. The leadership bar is:

- Opens fast.
- Indexes large repos without drama.
- Understands architecture better than chat-only tools.
- Produces smaller, cleaner diffs.
- Verifies changes before presenting them.
- Works beautifully with Windows, WSL, Docker, Git, and local terminals.
- Gives teams control over cost, security, and policy.
- Extends to the developer's actual stack instead of forcing a narrow workflow.

