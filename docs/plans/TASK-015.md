# TASK-015 Plan: Align CLI/Server/Main with Ring Architecture

## Goal
Replace the temporary re-export aliases in `src/adapters/inbound/mod.rs` with real inbound adapters. The CLI and HTTP server must implement inbound port traits defined in the core so they can translate external stimuli into application commands without depending on outbound infrastructure.

## Background
- The layered ring/clean architecture (see `TASK-014`) expects each ring to depend inward only. `src/adapters/inbound/mod.rs` currently re-exports legacy `cli` and `server` modules from the crate root, so the inbound ring is nominal.
- Clean architecture (ports-and-adapters) models inbound adapters as implementations of port-in traits defined in the core. Idiomatic Rust uses `trait`s for these ports (e.g., `core::ports::inbound::CliPort`, `ServerPort`), and adapters provide concrete types that satisfy those traits while calling into application/core logic.
- Today `src/cli.rs`, `src/server.rs`, and `src/main.rs` contain transport/parsing logic, outbound wiring (LLM clients, persistence, tracing), and direct orchestration of domain structs. We must split them so clap/HTTP parsing occurs in inbound modules while the composition root focuses on wiring and the application runner remains isolated.

## Layer Placement Guide
| Concern / Characteristic | Examples in Current Code | Target Ring |
| --- | --- | --- |
| **Core domain** | `Context`, `WorkItem`, `RunnerOptions`, domain errors | `src/core` (pure logic, traits only) |
| **Application services** | `FlowRunner`, task orchestration, session commands | `src/application` (depends on core traits) |
| **Inbound adapters** | Clap parsing (`Cli`), HTTP routing/handlers, help rendering | `src/adapters/inbound/{cli,server}` implementing `core::ports::inbound::*` |
| **Outbound adapters** | `SessionStore`, `RigLlmClient`, `HandlebarsRenderer`, filesystem/clock/telemetry | `src/adapters/outbound/*` implementing `core::ports::outbound::*` |
| **Composition root / bootstrap** | Dependency wiring, tracing setup, env/config resolution, handing runner handles to adapters | `src/main.rs` (or `src/bootstrap.rs`) — may reference outbound adapters but never contain transport-specific logic |
| **Executable façade** | Minimal `main` that parses CLI, instantiates adapters, and dispatches | `src/main.rs` calling into bootstrap + inbound adapters |

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
- Inbound parsing/HTTP wiring lives under `adapters::inbound::{cli,server}` and implements the new inbound port traits.
- Composition-root responsibilities (building adapters, wiring dependencies) live in either a slim `main.rs` or a new `bootstrap` module that `main.rs` calls.
- Domain/application logic remains untouched inside the `application` layer; inbound adapters merely call into it via the port traits.

## Design Principles
1. **Inbound Ports as Traits:** Define traits under `src/core/ports/inbound.rs` (e.g., `trait CliPort { fn run(&self, opts: CliCommand) -> Result<()>; }`). These traits represent the domain-facing API that inbound adapters invoke.
2. **Adapters Implement Ports:** `adapters::inbound::cli::CliAdapter` and `adapters::inbound::server::ServerAdapter` implement the inbound traits and translate parsed requests into application commands (`RunnerHandle`, `SessionCommands`, etc.).
3. **CLI/Server Split:** Keep parsing/transport logic (clap argument structs, HTTP routing) inside the inbound modules. Push domain execution into application/core via port methods.
4. **Composition Root:** `src/main.rs` (or a dedicated builder) constructs outbound adapters, the application runner, and the inbound adapters. Only the composition root wires everything together.
5. **Documentation + Tests:** Update docs and tests to describe/cover the new adapter boundary, ensuring integration tests call the inbound modules rather than legacy paths.

## Work Breakdown

1. **Audit Current Entrypoints**
   - Inventory how `src/cli.rs`, `src/server.rs`, and `src/main.rs` instantiate the runner, load config, and touch outbound adapters.
   - Capture integration tests/scripts (`integration-tests`, `scripts/*.sh`) referencing the old modules.

2. **Introduce Inbound Port Traits**
   - Add `src/core/ports/inbound.rs` with traits for CLI/server interactions (e.g., `CliInvoker`, `ServerInvoker`). These traits should accept domain/application-level request structs (`RunCommand`, `ServeCommand`).
   - Document the new traits and ensure `core::ports::mod.rs` re-exports them.

3. **Refactor CLI Entrypoint**
   - Move clap definitions and parsing logic into `src/adapters/inbound/cli.rs`.
   - Create a `CliAdapter` struct that holds the application runner handle and implements the inbound trait.
   - Keep `src/cli.rs` as a thin wrapper (or remove it) that only delegates to `CliAdapter`.

4. **Refactor HTTP Server Entrypoint**
   - Move server framework setup/handlers into `src/adapters/inbound/server.rs`.
   - Implement the inbound server trait, converting HTTP requests into application commands.
   - Ensure each handler interacts solely with the application/core API surface.

5. **Restructure `src/main.rs` as Composition Root**
   - Extract a `bootstrap` module (or keep `main.rs` minimal) that instantiates outbound adapters, builds the application runner, and hands trait objects to inbound adapters.
   - Move command-specific logic (`run_command`, `status_command`, `resume_command`, `subprocess_command`, `serve_command`, `help_command`) into the appropriate inbound modules so `main.rs` only orchestrates startup (parse CLI, call adapter entrypoint, handle errors).
   - Ensure logging setup and shared utilities (e.g., `default_runner_deps`) reside either in outbound adapters or bootstrap code, not in inbound modules that should avoid infrastructure details.

6. **Verification & Documentation**
   - Update `docs/plans/TASK-014-clean-architecture.md` or other architecture docs to mention the inbound port traits.
   - Adjust integration tests and scripts to import the new modules.
   - Run `make fmt`, `make clippy`, `scripts/run_all_tests.sh`, and any architecture checks.
   - Update `README`/user guide if command usage output or modules change.

## Acceptance Criteria
- `src/adapters/inbound/mod.rs` exposes real adapter modules rather than re-exporting legacy ones.
- CLI and server code are split so parsing/transport logic lives in `adapters::inbound::*`, while application/core logic is invoked through inbound port traits.
- `core::ports::inbound` defines the domain-facing traits used by inbound adapters, expressed as idiomatic Rust `trait`s.
- Composition root wires inbound adapters without leaking outbound dependencies into them.
- Tests and docs cover the new structure; all existing suites remain green.
