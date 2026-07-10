# Kestrel Product Requirements

## Release Goal

Ship an MVP that proves Kestrel can outperform generic AI coding tools on Windows by opening a real repository, understanding it structurally, making a safe code change, verifying the change, and showing transparent cost and context information.

## MVP User Stories

## 1. Open and Understand a Repository

As a developer, I can open a local repository and receive a useful summary of the project structure, languages, frameworks, commands, and likely test/build workflow.

Acceptance criteria:

- User can select a project folder.
- Kestrel detects Git root.
- Kestrel detects primary languages.
- Kestrel builds a file inventory.
- Kestrel extracts symbols for supported languages.
- Kestrel produces a project onboarding summary.
- Kestrel identifies likely install, run, test, lint, and build commands.

## 2. Ask Questions with Real Context

As a developer, I can ask questions about my codebase and receive answers grounded in relevant files, symbols, and Git history.

Acceptance criteria:

- Kestrel selects relevant files without full-repo dumping.
- Kestrel can cite file paths and symbols.
- Kestrel can explain why a file was included.
- Kestrel shows approximate token footprint.
- Kestrel avoids sensitive files according to policy.

## 3. Make a Reviewed Code Change

As a developer, I can ask Kestrel to change code and review the proposed diff before it is applied.

Acceptance criteria:

- Kestrel creates a plan for multi-file changes.
- Kestrel protects unrelated dirty files.
- Kestrel shows file-level and hunk-level diffs.
- User can approve, reject, or request revision.
- Kestrel records a checkpoint before applying approved changes.

## 4. Verify the Change

As a developer, I can have Kestrel run the right checks after a change.

Acceptance criteria:

- Kestrel detects relevant verification commands.
- User can approve command execution.
- Kestrel captures exit code and output.
- Kestrel parses common diagnostics.
- Kestrel attempts repair when checks fail.
- Final response includes verification status and residual risk.

## 5. Work Across WSL

As a Windows developer using WSL, I can run project commands inside the correct WSL distro without manually translating paths.

Acceptance criteria:

- Kestrel detects WSL distros.
- User can select the execution target.
- Kestrel can run commands inside WSL.
- Kestrel maps Windows and Linux paths.
- Kestrel streams output back to the UI.

## 6. Control Cost

As a developer, I can see and control token usage before expensive actions.

Acceptance criteria:

- Kestrel estimates token footprint for major tasks.
- Kestrel shows selected model mode.
- User can choose Fast, Balanced, Deep, or Local-only mode.
- User can set per-task budget.
- Kestrel warns before exceeding budget.

## Functional Requirements

## Repository Indexing

- Must support project open by folder.
- Must detect Git root.
- Must respect `.gitignore`.
- Must allow additional ignore rules.
- Must maintain incremental index updates.
- Must expose index freshness status.

## AST and Symbol Graph

- Must parse TypeScript, JavaScript, Python, Rust, and C# for MVP.
- Must extract top-level symbols.
- Must extract imports and exports where applicable.
- Must map symbol definitions to file ranges.
- Should map references when parser support is sufficient.

## Agent Workflow

- Must support chat.
- Must support task plans.
- Must support file reads.
- Must support patch proposals.
- Must support diff review.
- Must support command execution with approval.
- Must summarize completed work.

## Verification

- Must detect common package managers and build tools.
- Must run approved commands in the selected environment.
- Must capture structured output.
- Must feed diagnostics back into the repair loop.
- Must stop repair after configurable attempt limit.

## Security

- Must redact detected secrets from model-bound context.
- Must exclude sensitive paths by default.
- Must require approval for destructive commands.
- Must require approval for dependency installation.
- Must log tool calls locally.

## Non-Functional Requirements

- Core service idle memory target: under 100MB.
- UI cold start target: under 2 seconds.
- Project open should show progress within 500ms.
- Indexing must be cancellable.
- All model calls must be traceable to task ID.
- The app must remain usable while indexing.
- Crashes must not corrupt the local index.

## Release Gates

MVP is not ready until:

- It can handle at least 10 real repositories across supported stacks.
- It can produce verified diffs on common tasks.
- It protects dirty user changes.
- It can run checks in Windows and WSL.
- It shows cost estimates.
- It has a clear failure mode when context is insufficient.
- It has local logs useful enough for debugging support issues.

