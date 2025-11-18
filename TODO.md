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
1.  **Architecture**: Decide on a strategy (e.g., spawning a new thread, using a dedicated worker pool, or shelling out to `microfactory resume`).
2.  **Worker**: Implement a background task that listens for resume signals (or polls for "Pending Resume" state if we add that status).
3.  **Integration**: Connect the `resume_session_handler` in `src/server.rs` to trigger this worker.
4.  **Concurrency**: Ensure the worker can safely access the SQLite database and `FlowRunner` logic without blocking the HTTP server.