# Microfactory Todo List

This document tracks planned improvements and technical debt for the Microfactory project. It is structured to be readable by both human developers and LLM agents.

## Task Structure

Each task should include:
- **ID**: A unique identifier (e.g., `TASK-001`).
- **Title**: A concise summary of the task.
- **Status**: `[ ]` (Pending), `[-]` (In Progress), `[x]` (Completed).
- **Priority**: High, Medium, Low.
- **Context**: Why this is needed.
- **Implementation Details**: Specific instructions, files to touch, and expected behavior.

---

## Pending Tasks

### TASK-007: Parallelize Red-Flagger Evaluation
- **Status**: `[x]`
- **Priority**: High
- **Context**: Currently, the `SampleCollector` iterates sequentially through LLM samples when running red-flag checks. If using `llm_critique` (which makes network calls), this adds significant latency (e.g., 5 samples * 2s = 10s delay). Parallelizing this aligns with the MAKER paper's scalability goals.
- **Implementation Details**:
    - Refactor `SampleCollector::collect_inner` in `src/tasks/mod.rs`.
    - Use `tokio::task::JoinSet` (or `futures::stream`) to spawn evaluation tasks for the entire batch concurrently.
    - Ensure metrics (red flag counts, accepted samples) are aggregated correctly after the parallel wait.

### TASK-006: Step-by-Step Execution Mode (Debugger)
- **Status**: `[x]`
- **Priority**: High
- **Context**: Users need a way to verify intermediate steps during complex tasks without racing against the agent (e.g., for shakedown tests or debugging).
- **Implementation Details**:
    - Add `--step-by-step` flag to `microfactory run`.
    - Update `RunnerOptions` to carry this boolean.
    - In `FlowRunner::execute`, trigger a `WaitState` at two checkpoints if enabled:
        1. **Post-Decomposition**: After subtasks are generated but before they are executed.
        2. **Post-Execution**: After `ApplyVerifyTask` completes (step done) but before the next step starts.
    - Ensure `WaitState` details clearly indicate this is a "step-by-step breakpoint".

### TASK-001: Per-Agent Red-Flagger Configuration
- **Status**: `[x]`
- **Priority**: High
- **Context**: Currently, `red_flaggers` are defined globally for a domain. This causes issues where "syntax" checks (meant for code generation) run against Decomposition agents (which output text plans), leading to false positives and wasted tokens.
- **Implementation Details**:
    - Update `src/config.rs` to allow `red_flaggers` to be defined inside the `agents` block (e.g., specifically for `solver`).
    - Update `src/runner.rs` or `src/tasks/mod.rs` to load the correct pipeline for the specific task type.

### TASK-002: Persist Candidate Proposals in Context
- **Status**: `[x]`
- **Priority**: Medium
- **Context**: When a session pauses due to low voting margins, the user needs to see the conflicting proposals to make a decision. Currently, `DecompositionVoteTask` consumes (removes) proposals from the context, leaving the `status --json` output empty of this critical data.
- **Implementation Details**:
    - Update `Context` struct to retain `decomposition_candidates` and `solution_candidates` in the `WorkflowStep` struct (or a separate history map) instead of removing them.
    - Update `status_export.rs` to include these candidates in the session details JSON.
    - Ensure `persistence.rs` serializes/deserializes this history correctly.

### TASK-003: Investigate rig-core GPT-5 Support
- **Status**: `[ ]`
- **Priority**: High
- **Context**: During the "Phase 8 Shakedown", `gpt-5-mini` caused a panic in `rig-core` (`ToolDefinitions([])`) and `gpt-5.1` caused `JsonError: unknown variant none` for reasoning effort. We reverted to `gpt-4o`/`gpt-4o-mini` to proceed.
- **Implementation Details**:
    - Investigate `rig-core` v0.24.0 compatibility with GPT-5 models.
    - Determine if the `none` reasoning variant is supported or requires a library update/fork.
    - Debug the `ToolDefinitions` panic—is it caused by Microfactory failing to initialize tools, or a library bug when tools are unused?
    - Goal: Enable `gpt-5-mini` usage safely.

### TASK-004: Implement Real Applier Logic
- **Status**: `[ ]`
- **Priority**: High
- **Context**: The "Shakedown" revealed that `ApplyVerifyTask` uses a mock log-only applier for `patch_file`. No code is actually written to disk.
- **Implementation Details**:
    - Implement a real `patch_file` function (or similar) that takes the LLM's output (diff or full file) and applies it to the filesystem.
    - Handle parsing of code blocks from the LLM response.

### TASK-005: Simple Overwrite File Applier
- **Status**: `[x]`
- **Priority**: Medium
- **Context**: Simplified alternative to TASK-004. Instead of complex patching, implement a "dumb" applier that overwrites files with the full content provided by the agent. Useful for initial file creation tasks.
- **Implementation Details**:
    - Add a new applier type `overwrite_file` in `config.yaml` and `runner.rs`.
    - Logic: Extract the first code block from the solution and write it to the target path (if the step context or prompt implies a filename).