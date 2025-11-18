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

### `TASK-001` [ ] Implement Tree-sitter Syntax Red-Flagger
**Priority**: Medium
**Context**:
Currently, `SyntaxRedFlagger` only checks for unbalanced delimiters (`()`, `[]`, `{}`). This is insufficient for catching actual syntax errors (e.g., invalid keywords, bad indentation in Python) before the expensive voting/verification phase. Using `tree-sitter` would allow for robust, language-aware parsing.

**Implementation Details**:
1.  **Dependencies**: Add `tree-sitter` and language-specific crates (e.g., `tree-sitter-python`, `tree-sitter-rust`) to `Cargo.toml`.
2.  **Configuration**: Update `RedFlaggerConfig` in `src/config.rs` to accept a `language` parameter that maps to specific tree-sitter parsers.
3.  **Logic**:
    -   Modify `SyntaxRedFlagger` in `src/red_flaggers.rs`.
    -   Initialize the correct parser based on the config.
    -   Parse the candidate code.
    -   Traverse the resulting tree to check for `ERROR` or `MISSING` nodes.
    -   Return a flag message if errors are found.
4.  **Testing**: Add unit tests with valid and invalid code snippets for supported languages.

---

### `TASK-002` [ ] Robust Handlebars Templating
**Priority**: Low
**Context**:
The current templating logic in `src/tasks/mod.rs` uses simple string replacement (`.replace("{{prompt}}", ...)`). This prevents the use of advanced prompt features like loops (for history), conditionals, or partials. The project already depends on the `handlebars` crate but doesn't fully utilize it.

**Implementation Details**:
1.  **Refactor**: Update `render_prompt` in `src/tasks/mod.rs` to use the `handlebars` registry.
2.  **State**: Ensure the `Handlebars` instance is initialized once (likely in `FlowRunner` or `Context`) and passed down, rather than re-created per task.
3.  **Data**: Pass a structured context (e.g., `serde_json::json!({ "prompt": ..., "history": ..., "step": ... })`) to the render function instead of raw strings.
4.  **Templates**: Update existing `.hbs` files in `templates/` to use standard Handlebars syntax if they don't already.

---

### `TASK-003` [ ] Add Resume Endpoint to HTTP Server
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
