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

## Pending Tasks

### `TASK-005` [ ] Add Resume Endpoint to HTTP Server
**Priority**: Medium
**Context**:
The current `microfactory serve` implementation is read-only. To allow external tools (IDEs, dashboards) to control the workflow, we need an endpoint to resume paused sessions.

**Implementation Details**:
1.  **Route**: Add `POST /sessions/:id/resume` to `src/server.rs`.
2.  **Handler**: Implement a handler that:
    -   Loads the session from `SessionStore`.
    -   Validates it is in a paused state.
    -   (Optional) Accepts a JSON body to override parameters (like `api_key` or `samples`).
    -   Spawns a background task (or uses a shared runner handle) to resume execution. *Note: This is complex because `FlowRunner` currently runs in the foreground of the CLI process. The server might need to spawn a new process or communicate with a worker.*
    -   For V1, it might be sufficient to just update the state in SQLite to "Pending" so a separate worker can pick it up, or return a 501 Not Implemented if the architecture doesn't support background resumption yet.
3.  **Testing**: Add an integration test in `src/server.rs`.
