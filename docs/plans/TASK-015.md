# TASK-015 Plan: Align CLI/Server/Main with Ring Architecture

## Goal
Replace the temporary re-export aliases in `src/adapters/inbound/mod.rs` with real inbound adapters. The CLI and HTTP server must consume application service traits (use-case ports) defined in the core, translating external stimuli into application commands without depending on outbound infrastructure.

## Background
- The layered ring/clean architecture (see `TASK-014`) expects each ring to depend inward only. `src/adapters/inbound/mod.rs` currently re-exports legacy `cli` and `server` modules from the crate root, so the inbound ring is nominal.
- In hexagonal/ports-and-adapters architecture, **driving adapters** (CLI, HTTP) call into **use-case ports** (application service traits). This is distinct from **driven adapters** (persistence, LLM clients) which *implement* outbound port traits.
- Today `src/cli.rs`, `src/server.rs`, and `src/main.rs` contain transport/parsing logic, outbound wiring (LLM clients, persistence, tracing), and direct orchestration of domain structs. We must split them so clap/HTTP parsing occurs in inbound modules while the composition root focuses on wiring and the application runner remains isolated.

## Clarification: Inbound Ports as Application Service Traits

The original plan framed inbound ports as traits that CLI/server adapters *implement*. However, the more idiomatic hexagonal architecture approach is:

1. **Application Service Traits (Use-Case Ports):** Define traits like `WorkflowService` in `core::ports::inbound` that represent what the application layer offers to driving adapters. Methods include `run_session`, `resume_session`, `get_session_status`, etc.

2. **Inbound Adapters Consume Ports:** The CLI and HTTP server are driving adapters that *call* the application service traits. They don't implement port traits themselves—they translate transport-specific inputs (clap args, HTTP requests) into calls on the service interface.

3. **Composition Root Wires It Together:** `main.rs` constructs a concrete implementation of `WorkflowService` (backed by `FlowRunner` + outbound adapters) and injects it into the inbound adapters.

This aligns with:
- The existing `FlowRunner` pattern (application service that orchestrates domain logic)
- Standard hexagonal architecture where driving adapters call use-case ports
- Rust idioms where traits define capabilities that implementations provide

## Layer Placement Guide
| Concern / Characteristic | Examples in Current Code | Target Ring |
| --- | --- | --- |
| **Core domain** | `Context`, `WorkItem`, `RunnerOptions`, domain errors | `src/core` (pure logic, traits only) |
| **Application service traits** | `WorkflowService` trait with `run_session`, `resume_session`, `get_status` | `src/core/ports/inbound` (use-case port definitions) |
| **Application services** | `FlowRunner`, `AppService` impl of `WorkflowService` | `src/application` (implements use-case ports) |
| **Inbound adapters** | Clap parsing (`Cli`), HTTP routing/handlers, help rendering | `src/adapters/inbound/{cli,server}` consuming `WorkflowService` |
| **Outbound adapters** | `SessionStore`, `RigLlmClient`, `HandlebarsRenderer`, filesystem/clock/telemetry | `src/adapters/outbound/*` implementing `core::ports::outbound::*` |
| **Composition root** | Dependency wiring, tracing setup, env/config resolution | `src/main.rs` — constructs services, injects into adapters |

## Current `src/main.rs` Responsibilities
`src/main.rs` currently mixes three distinct concerns:

1. **Inbound Adapter Responsibilities**
   - Defines the CLI entrypoint by importing clap structs (`microfactory::cli::*`), parsing args, and dispatching each subcommand via `run_command`, `status_command`, etc.
   - Implements HTTP server startup through the `serve_command` helper, effectively acting as the transport adapter.

2. **Outbound/Infrastructure Wiring**
   - Configures tracing/logging (`tracing_setup::init`), resolves API keys, loads configs, opens `SessionStore`, and instantiates concrete outbound adapters (`RigLlmClient`, `HandlebarsRenderer`, `StdFileSystem`, clock/telemetry).

3. **Application Invocation / Domain Glue**
   - Constructs `Context`, `RunnerOptions`, and `FlowRunner`, invoking domain logic directly from each command helper.

In a strict layered setup, only the composition root should assemble outbound adapters and inject them into inbound adapters. Therefore, this file must be split so:
- Inbound parsing/HTTP wiring lives under `adapters::inbound::{cli,server}`.
- Inbound adapters receive an injected `WorkflowService` trait object and call its methods.
- Composition-root responsibilities (building adapters, wiring dependencies) live in a slim `main.rs`.
- Domain/application logic remains untouched inside the `application` layer.

## Design Principles
1. **Application Service Trait:** Define `WorkflowService` in `src/core/ports/inbound.rs` with async methods for session operations. This is the use-case port that driving adapters consume.
2. **Adapters Consume Services:** `adapters::inbound::cli` and `adapters::inbound::server` receive a `Arc<dyn WorkflowService>` and translate their transport inputs into service calls.
3. **CLI/Server Split:** Keep parsing/transport logic (clap argument structs, HTTP routing) inside the inbound modules. They call the injected service for business logic.
4. **Composition Root:** `src/main.rs` constructs outbound adapters, builds the application service implementation, and passes it to inbound adapters.
5. **Documentation + Tests:** Update docs and tests to describe/cover the new adapter boundary, ensuring integration tests exercise the service trait.

## Work Breakdown

### 1. Define Application Service Trait
- Add `WorkflowService` trait to `src/core/ports/inbound.rs` with methods:
  - `run_session(request: RunSessionRequest) -> Result<RunSessionResponse>`
  - `resume_session(request: ResumeSessionRequest) -> Result<RunSessionResponse>`
  - `get_session(session_id: &str) -> Result<Option<SessionDetail>>`
  - `list_sessions(limit: usize) -> Result<Vec<SessionSummary>>`
  - `run_subprocess(request: SubprocessRequest) -> Result<SubprocessResponse>`
- Define DTOs for requests/responses in the same module.
- Re-export from `core::ports::mod.rs`.

### 2. Implement Application Service
- Create `src/application/service.rs` with `AppService` struct implementing `WorkflowService`.
- `AppService` holds: config, LLM client factory, renderer, persistence, and runtime deps.
- Move business logic from `main.rs` command handlers into `AppService` methods.
- Keep `FlowRunner` as the internal orchestrator; `AppService` wraps it with session management.

### 3. Refactor CLI Adapter
- Create `src/adapters/inbound/cli/mod.rs` with:
  - Re-export clap definitions from a `definitions.rs` submodule (move from `src/cli.rs`)
  - `CliAdapter` struct holding `Arc<dyn WorkflowService>`
  - `execute(&self, command: Commands) -> Result<()>` method that dispatches to service
- Move help rendering logic into the CLI adapter.
- Remove or minimize `src/cli.rs` (keep only if needed for lib re-exports).

### 4. Refactor Server Adapter
- Create `src/adapters/inbound/server/mod.rs` with:
  - `ServerAdapter` struct holding `Arc<dyn WorkflowService>` and `ServeOptions`
  - Axum router setup and handlers that call the service
  - Move from `src/server.rs`
- Server handlers translate HTTP requests to service calls.

### 5. Restructure `src/main.rs` as Composition Root
- Keep only:
  - CLI parsing (`Cli::parse()`)
  - Tracing initialization
  - Outbound adapter construction (LLM client, persistence, renderer, etc.)
  - `AppService` construction with injected dependencies
  - Inbound adapter construction with injected service
  - Command dispatch to inbound adapter
- Move all command-specific logic out of `main.rs`.

### 6. Update Module Structure
- Update `src/adapters/inbound/mod.rs` to expose real modules instead of re-exports.
- Update `src/lib.rs` to reflect new module organization.
- Ensure backward compatibility for any external imports.

### 7. Verification & Documentation
- Run `make fmt`, `make clippy`, `make test`.
- Update `docs/plans/TASK-014-clean-architecture.md` to reference the service trait pattern.
- Verify integration tests still pass.

## Acceptance Criteria
- `src/adapters/inbound/mod.rs` exposes real adapter modules (not re-exports).
- `core::ports::inbound` defines `WorkflowService` trait as the use-case port.
- `application::service::AppService` implements `WorkflowService`.
- CLI and server adapters receive injected `WorkflowService` and call its methods.
- `src/main.rs` is a clean composition root (~100-150 lines) with no business logic.
- All existing tests pass; new tests cover the service trait boundary.
