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

***

### `TASK-006` [ ] Implement Background Worker for Session Resumption
**Priority**: High
**Context**:
The `POST /sessions/:id/resume` endpoint currently only validates the request but does not actually restart the workflow. To make the API fully functional, we need a mechanism to execute the `FlowRunner` in the background when a resume request is received.

**Implementation Details**:
1.  **Strategy**: Use the "shelling out" approach for V1. The server will spawn a new `microfactory resume` process. This avoids the complexity of a full library refactor (see **Phase 8** in `architecture.md` for the long-term plan).
2.  **Worker**: Implement a simple logic in `resume_session_handler` (or a helper) to:
    -   Construct the `std::process::Command`.
    -   Point it to the current executable (`std::env::current_exe()`).
    -   Pass `resume --session-id <ID>`.
    -   Spawn it detached (`.spawn()`).
3.  **Integration**: Connect this logic to the existing `resume_session_handler` in `src/server.rs`.
4.  **Concurrency**: Since it's a separate process, concurrency is managed by the OS. We might want to add a simple counter or semaphore in `ServeState` to limit the number of concurrent child processes if needed, but for V1, unbounded spawning is acceptable.