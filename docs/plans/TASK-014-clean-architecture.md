# TASK-014 Plan: Layered Ring/Clean Architecture

## Goal
Adopt a layered ring/clean architecture so Microfactory's core workflow logic (context + runner) is isolated from infrastructure concerns and can be tested or extended without touching adapters.

## Current Gaps
- **Tight Coupling:** `FlowRunner` and tasks construct concrete adapters (Handlebars, `RedFlagPipeline`) directly.
- **Circular/Mixed Dependencies:** Persistence, templating, and LLM access live in the same crate level with no clear direction.
- **Error Leaking:** Infrastructure errors (like `rusqlite` or `reqwest` errors) propagate directly into domain logic, making the core dependent on specific libraries.
- **Configuration Coupling:** The internal domain logic relies on `config.yaml` structures that are heavily decorated with `serde` and tied to file parsing logic.
- **Missing Ports:** Filesystem mutations, wall-clock/time, telemetry, and environment/config sourcing are implicit globals rather than explicit abstractions, making deterministic tests difficult.

## Target Architecture

The system will be composed of three distinct concentric layers. Dependencies point **inwards**.

1.  **Core (The Center)**
    *   **Responsibility:** Pure domain logic, runtime state (`Context`), config views, and abstractions (`Ports`).
    *   **Dependencies:** Standard Library + foundational crates needed for serialization/async (`serde`, `async-trait`).
    *   **Contents:**
        *   `Domain Models`: `Context`, `WorkItem`, `Step`.
        *   `Ports (Traits)`: `SessionRepository`, `LlmClient`, `PromptRenderer`, `RedFlagger`, `Clock`, `FileSystem`, `TelemetrySink`.
        *   `Errors`: A unified `core::error::Error` enum that encompasses all logical failure modes.

2.  **Application (The Orchestrator)**
    *   **Responsibility:** Coordinating the flow of data between the Core and the Adapters.
    *   **Dependencies:** `Core`.
    *   **Contents:** `FlowRunner`, `MicroTask` implementations (`DecompositionTask`, `SolveTask`), `ApplyVerifyTask`.

3.  **Adapters (The Edge)**
    *   **Responsibility:** Implementing the `Ports` defined in Core using concrete libraries.
    *   **Dependencies:** `Core`, `Application`, and External Crates (`rusqlite`, `rig-core`, `handlebars`).
    *   **Contents:** `SqliteSessionStore`, `RigLlmClient`, `HandlebarsRenderer`, `Cli` (Input Adapter), `HttpServer` (Input Adapter).

## Key Architectural Decisions

### 1. Directional Ports in the Type System
Explicit namespaces encode direction:
- **Inbound** (things that push data *into* the core, e.g., CLI/server handlers) live under `core::ports::inbound` and `adapters::inbound` (currently re-exporting the legacy CLI/server modules until they migrate fully).
- **Outbound** (services the core *calls out* to, e.g., persistence, LLM, filesystem) live under `core::ports::outbound` and `adapters::outbound::<capability>`.
Every trait the core/application depends on sits in `core::ports::outbound`, and adapters implement them in `adapters::outbound::<capability>`. Input adapters expose structs under `adapters::inbound` that translate external stimuli into core commands/events.

### 2. Error Boundaries
The Core must not know about `rusqlite::Error` or `reqwest::Error`.
*   We will define a central `core::error::Error` enum (e.g., `Error::PersistenceFailed`, `Error::LlmProviderError`).
*   **Adapters are responsible for mapping errors.** When `SqliteSessionStore` catches a SQL error, it must convert it into `core::error::Error::PersistenceFailed` before returning it to the Application layer.

### 3. Configuration Decoupling
*   `src/config.rs` (the file loader) currently acts as both the file parser and the internal config object.
*   We will split this:
    *   **Core:** Defines what it *needs* (e.g., `core::config::DecompositionSettings` struct).
    *   **Adapter:** Loads the YAML/JSON, handles `serde` parsing, and *converts* it into the Core structs.

### 4. Dependency Guardrails
Use `cargo deny` (with the `bans` and `sources` checks) plus lint-time checks to ensure `core` never depends on adapter crates and `application` never imports concrete adapter types. Add a doc test or CI script (implemented as `cargo xtask check-architecture`) that fails if forbidden modules are referenced.

### 5. Testing & Coverage Discipline
Identify critical scenarios (happy-path decomposition/solve flow, low-margin pause, adapter failures, file-write success/failure) and cover them with in-memory port implementations. Maintain current integration-test coverage by running the existing suites unchanged.

## Work Breakdown & Implementation Steps

### Phase 1: Core Foundation
1.  Create `src/core/mod.rs`, `src/core/error.rs`, and `src/core/ports.rs`.
2.  Define the `core::error::Error` enum and a `core::error::Result<T>` type alias.
3.  Move `Context` and `WorkItem` to `src/core/domain.rs` (or similar).
4.  Define traits in `src/core/ports.rs`: `SessionRepository`, `LlmClient`, `PromptRenderer`, `RedFlagger`, `Clock`, `FileSystem`, `TelemetrySink`.

### Phase 2: Adapter Alignment
1.  **Persistence:** Modify `src/persistence.rs` (move to `src/adapters/outbound/persistence.rs`) to implement `core::ports::outbound::SessionRepository`. Ensure it maps `rusqlite` errors to `core::Error`.
2.  **LLM:** Modify `src/llm.rs` (move to `src/adapters/outbound/llm.rs`) to implement `core::ports::outbound::LlmClient`.
3.  **Templating:** Create `src/adapters/outbound/templating.rs` to implement `PromptRenderer` using Handlebars.
4.  **Filesystem/Clock/Telemetry:** Introduce adapters under `src/adapters/outbound/` wrapping std/fs, chrono/time, and tracing layers so application code receives traits instead of globals, and group CLI/server/http entrypoints under `src/adapters/inbound/`.

### Phase 3: Application Layer Extraction
1.  Create `src/application/mod.rs`.
2.  Move `src/runner.rs` to `src/application/runner.rs`.
3.  Refactor `FlowRunner` to accept `Arc<dyn Port>` traits instead of concrete structs (SessionRepository, LlmClient, PromptRenderer, RedFlagger, FileSystem, Clock, TelemetrySink).
4.  Move `src/tasks/` to `src/application/tasks/` and update imports.
5.  Add constructors/builders that accept explicit port structs so tests can inject fakes.

### Phase 4: Wiring & Entry Points
1.  Refactor `src/main.rs` (The Composition Root).
2.  Instantiate all outbound adapters (`adapters::outbound::persistence::SqliteSessionStore`, `adapters::outbound::llm::RigLlmClient`, `adapters::outbound::templating::HandlebarsRenderer`, filesystem/time/telemetry wrappers) and wire inbound adapters (`adapters::inbound::cli`, `adapters::inbound::server`).
3.  Inject them into the `FlowRunner` via a builder or constructor.
4.  Ensure `src/cli.rs` and `src/server.rs` only interact with the `Application` layer (Runner) or `Core` types.
5.  Add a `cargo xtask check-architecture` command that shells out to `cargo deny --workspace` with a curated config to enforce dependency direction and scans `src/core` for forbidden adapter imports.

### Phase 5: Tests & Documentation
1.  Cover the critical scenarios with in-memory adapters:
    - Happy-path decomposition→solve→apply flow.
    - Low-margin vote pause path.
    - Adapter failure propagation (persistence error, LLM exhaustion).
    - File-write success/failure paths for Apply/Verify.
2.  Keep existing integration suites intact (`make test`, `scripts/run_all_tests.sh`, `cargo xtask check-architecture`) to prove no regression in coverage.
3.  Update `docs/architecture.md` (and README summary) with the new layer diagram and port table.

## Acceptance Criteria
- `core` module has **zero** dependencies on `rusqlite`, `rig-core`, `handlebars`, or other adapter crates (enforced by `cargo deny`).
- `FlowRunner` and tasks consume only trait objects (`dyn SessionRepository`, `dyn LlmClient`, etc.).
- Unit tests cover the listed critical scenarios using in-memory adapters, and overall test coverage does not regress (existing suites remain green).
- `cargo check`, `cargo test`, and the architecture guard (`cargo xtask check-architecture`) all pass.
- Existing integration tests pass without modification (proving behavior is preserved).
- Documentation describes the layer boundaries, available ports, and how to add new adapters.
