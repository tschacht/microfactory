# Microfactory Todo List

This document tracks planned improvements and technical debt for the Microfactory project. It is structured to be readable by both human developers and LLM agents.

## Task Structure

Each task should include:
- **ID**: A unique identifier (e.g., `TASK-001`).
- **Title**: A concise summary of the task.
- **Status**: `[ ]` (Pending), `[-]` (In Progress), `[x]` (Completed).
- **Priority**: High, Medium, Low.
- **Dependencies**: Optional list of blocking task IDs.
- **Context**: Why this is needed.
- **Implementation Details**: Specific instructions, files to touch, and expected behavior.

---

## Pending Tasks

### TASK-008: Human-Friendly Logging Strategy
- **Status**: `[x]`
- **Priority**: High
- **Context**: The current CLI output is flooded with raw JSON from `rig-core`, making it hard for humans to follow the MAKER process. We need a clean default view while preserving debug data.
- **Implementation Details**:
    - Refactor `src/tracing_setup.rs` to support layered logging.
    - **Default Mode**: Print clean, formatted `INFO` messages from `microfactory` crate only to stdout. Hide `rig-core` logs.
    - **File Logging**: Always write full debug logs (including raw JSON) to `.microfactory/logs/session-<id>.log`.
    - **JSON Output**: Add `--log-json` flag to `run` command. Support `--pretty` (default for humans) and `--compact` (for LLM/machine ingestion).
    - **Verbose Mode**: `-v` flag enables more detailed console logs (e.g. subtask lists, red-flag details).

### TASK-009: Progress UI (Spinners & Bars)
- **Status**: `[ ]`
- **Priority**: Medium
- **Context**: Long-running phases (Decomposition, Solving 10 samples) look like the CLI is hanging. Visual feedback is needed.
- **Implementation Details**:
    - Add `indicatif` dependency.
    - Instrument `FlowRunner` and `SampleCollector` to show spinners during LLM calls and progress bars for sample collection/voting.
    - Ensure progress bars play nicely with the logging system (suspend bar for log lines).

### TASK-010: Context-Aware Syntax Red-Flagger
- **Status**: `[x]`
- **Priority**: High
- **Context**: The `syntax` red-flagger currently fails on XML-wrapped Solver output because it treats the entire response as code. We need it to validate the *content* inside the XML blocks.
- **Implementation Details**:
    - Move `extract_xml_files` to a shared module (e.g., `src/utils.rs` or `src/lib.rs`).
    - Update `SyntaxRedFlagger` in `src/red_flaggers.rs`:
        - Add `extract_xml` (bool) configuration param.
        - If true, parse the response into file blocks.
        - Infer language from file extension (e.g., `.rs` -> Rust, `.py` -> Python) or fall back to the configured language.
        - Validate each file's content separately.
    - Update `config.yaml` to re-enable syntax checking with `extract_xml: true` for the code solver.

### TASK-011: Update README with Real Trace
- **Status**: `[ ]`
- **Priority**: Medium
- **Dependencies**: TASK-008, TASK-010
- **Context**: The README currently lacks a usage example. We want a real capture that is naturally clean and informative, showcasing the new "Human-Friendly Logging".
- **Implementation Details**:
    - Once Logging (TASK-008) and Syntax Checks (TASK-010) are done, run the `greeter` demo again.
    - Capture the output using the new default (human) mode.
    - Add the "Execution Trace" section in README.md with this authentic artifact.

### TASK-012: LLM Inspection View (Layered Logs)
- **Status**: `[x]`
- **Priority**: High
- **Dependencies**: TASK-008
- **Context**: While `TASK-008` improved general logging, the `rig-core` library "double-encodes" LLM message history (serializing JSON strings inside JSON logs). This makes debugging prompts and model responses painful. We need a specialized view to "peel the onion" and show exactly what is being sent to the model.
- **Implementation Details**:
    - See detailed plan in [docs/plans/TASK-012.md](docs/plans/TASK-012.md).
    - **Summary**:
        - Add `--inspect <MODE>` global flag (`ops`, `payloads`, `messages`).
        - Implement `InspectionLayer` in `src/tracing_inspect.rs` to intercept and decode `rig-core` spans.
        - Replace default stdout logging when inspection mode is active, but keep file logging intact.

### TASK-013: Inspection View Streaming Support
- **Status**: `[ ]`
- **Priority**: Low
- **Dependencies**: TASK-012
- **Context**: The `ops` inspection layer (TASK-012) currently assumes request/response cycles. If/when we move to streaming LLM responses, we need to update the inspection logic to handle chunked spans correctly.
- **Implementation Details**:
    - Update `InspectionLayer` to aggregate streaming chunks or display incremental progress.
    - Ensure token usage metrics are captured correctly for streams.

### TASK-014: Layered Ring/Clean Architecture
- **Status**: `[ ]`
- **Priority**: Medium
- **Dependencies**: _None_
- **Context**: Flow orchestration, domain state, and adapters are currently interleaved, which makes it harder to test in isolation or swap implementations (LLM provider, persistence, templating). Adopting a hexagonal/ring structure will clarify responsibilities and let inner logic evolve independently.
- **Implementation Details**:
    - See detailed plan in [docs/plans/TASK-014-clean-architecture.md](docs/plans/TASK-014-clean-architecture.md).
    - Introduce explicit ports for persistence, templating, telemetry, and red-flagging so `FlowRunner`/tasks depend solely on abstractions.
    - Restructure modules (or crates) into `core`, `application`, and `adapters`, ensuring the core layer owns workflow state (`Context`, steps) and task coordination logic while adapters host CLI, persistence, LLM, and templating code.
    - Add lightweight contract tests/fakes for the new ports to keep future refactors safe.

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
    - Implement a real `patch_file` function logic.
    - **Clarification**: This must support applying partial changes (diffs) to existing files, preserving context. It should NOT just overwrite files.
    - **Requirement**: Define a standard diff format for the LLM (e.g., Unified Diff with `---`/`+++` headers) and use a library or the `patch` CLI to apply it.

### TASK-005: Robust Overwrite Applier (XML Support)
- **Status**: `[x]`
- **Priority**: Medium
- **Context**: Enhance the simple overwrite applier to support multiple files and explicit paths from the LLM solution.
- **Implementation Details**:
    - Add support for parsing `<file path="...">...</file>` XML blocks in `ApplyVerifyTask`.
    - Write full content to the specified paths.
    - Maintain fallback heuristic for backward compatibility.
