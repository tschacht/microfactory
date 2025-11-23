# TASK-014-FIXUP: Complete Ring/Clean Architecture Refactor

## Goal

Close the remaining gaps from `TASK-014` so that Microfactory’s layered ring/clean architecture is fully realized in code, tooling, and docs, without regressing test coverage or CLI behavior.

This plan builds on `docs/plans/TASK-014-clean-architecture.md` and the implementation in commit `b592f19562fd41aae231addc734c4f9f9c8dcbde`.

---

## Gaps Identified After TASK-014

1. **Inbound/Outbound Namespaces Missing**
   - Plan calls for `core::ports::{inbound,outbound}` and `adapters::{inbound,outbound}`.
   - Current code uses a flat `core::ports` and `adapters::{llm,persistence,templating}`, with `cli.rs` and `server.rs` at the crate root.

2. **Domain Models Still Outside `core`**
   - `Context`, `WorkflowStep`, `WorkItem`, `AgentKind`, and `AgentConfig` remain in `src/context.rs`.
   - These types reference `crate::config::RedFlaggerConfig`, keeping config/serde concerns mixed into domain state.

3. **Outbound Ports Partially Unused**
   - `FileSystem`, `Clock`, and `TelemetrySink` are defined in `core::ports` but not implemented or injected.
   - Application and adapters call `std::fs`, `SystemTime`/`Instant`, and `tracing` directly.

4. **Red-Flagging Only Partially Ported**
   - `RedFlagger` lives in `core::ports`, but `RedFlagPipeline` is an application-level construct that still orchestrates concrete flaggers.
   - `FlowRunner` wires `RedFlagPipeline` directly instead of consuming a higher-level port.

5. **Inbound Adapter Separation Not Reflected in Modules**
   - CLI and HTTP server act as inbound adapters but live as `src/cli.rs` and `src/server.rs` rather than under `adapters::inbound`.

6. **Configuration Decoupling Incomplete**
   - No `core::config` module; `FlowRunner` depends on `config::MicrofactoryConfig` and `config::RedFlaggerConfig` directly.
   - Domain state (`AgentConfig` in `Context`) embeds config-layer types.

7. **No Automated Architecture Guardrails**
   - No `deny.toml`, `cargo deny` integration, or `xtask` enforcing “core depends only inward; adapters depend outward”.

8. **Architecture Docs Out of Sync**
   - `docs/arch/architecture.md` still references old module layout (e.g., `src/tasks`, `orchestrator.rs`, `domains/`).
   - The actual structure (`src/core`, `src/application`, `src/adapters`) is only documented implicitly.

9. **Core Dependency Story vs. Reality**
   - Plan states “Core depends on Standard Library only”, but `core::ports` uses `async-trait` and `serde_json`.
   - This is probably acceptable, but the specification and implementation now disagree.

Additionally, test runs in restricted environments revealed that logging currently panics when the log directory is not writable (from `tracing_appender::rolling::never`), which can break integration tests even though the production design is sound.

---

## High-Level Fixup Strategy

1. **Align modules and namespaces with the ring architecture** so that inbound/outbound direction is obvious from type paths.
2. **Move workflow domain models into `core`** and introduce a small `core::config` surface that the application layer depends on, with adapters responsible for mapping from YAML/CLI config.
3. **Wire all outbound ports end-to-end** (LLM, persistence, filesystem, clock, telemetry, red-flagging) so the application layer has no direct dependency on infrastructure APIs.
4. **Add automated guardrails** (`cargo deny` + a small `xtask`) to keep dependencies pointing inward.
5. **Refresh documentation and tests** to match the new structure while preserving existing coverage and observable behavior.

---

## Phase A – Namespace & Module Alignment

**A.1 Split core ports into inbound/outbound modules**
- Create:
  - `src/core/ports/mod.rs`
  - `src/core/ports/outbound.rs`
  - `src/core/ports/inbound.rs` (initially very small or even empty).
- Move existing outbound traits and DTOs into `outbound.rs`:
  - `SessionRepository`, `SessionSaveRequest`, `SessionLoadResponse`
  - `LlmClient`, `LlmOptions`
  - `PromptRenderer`
  - `RedFlagger`
  - `FileSystem`
  - `Clock`
  - `TelemetrySink`
- Re-export outbound ports from `core::ports` for ergonomics:
  - e.g., `pub use outbound::*;`
- Reserve `core::ports::inbound` for future explicit command/query interfaces if/when the core needs stable inbound contracts (for now it can stay minimal or just contain documentation).

**A.2 Restructure adapters into inbound/outbound**
- Replace `src/adapters/mod.rs` with:
  - `src/adapters/mod.rs`
  - `src/adapters/outbound/mod.rs`
  - `src/adapters/inbound/mod.rs`
- Move existing outbound implementations:
  - `src/adapters/llm.rs` → `src/adapters/outbound/llm.rs`
  - `src/adapters/persistence.rs` → `src/adapters/outbound/persistence.rs`
  - `src/adapters/templating.rs` → `src/adapters/outbound/templating.rs`
  - Update `lib.rs`, `main.rs`, `server.rs`, and tests to import from `microfactory::adapters::outbound::*`.
- Introduce inbound modules:
  - `src/adapters/inbound/cli.rs` that re-exports or wraps the existing `Cli` and related types.
  - `src/adapters/inbound/server.rs` that re-exports or wraps the HTTP server entrypoints.
  - Over time, move the bodies of `src/cli.rs` and `src/server.rs` into these modules; for an initial step, keep the code where it is and add `pub use crate::cli::*;` / `pub use crate::server::*;` from the inbound modules so call sites can start using the new paths.

**A.3 Update re-exports and call sites**
- Update `src/lib.rs` to re-export:
  - `pub mod core;`
  - `pub mod application;`
  - `pub mod adapters;`
  - Keep CLI/server convenience exports if desired, but prefer `adapters::inbound` paths in new code.
- Grep for imports and migrate them to the new namespaces (e.g., `core::ports::outbound::LlmClient` where explicitness helps).

**Acceptance for Phase A**
- All uses of adapter implementations come from `adapters::outbound::*`.
- Inbound interfaces (CLI/server) are reachable via `adapters::inbound::*`.
- No call site is broken; `cargo check` and tests remain green.

---

## Phase B – Move Domain Models into `core`

**B.1 Introduce `core::domain`**
- Create `src/core/domain.rs` for workflow domain types:
  - `Context`
  - `WorkflowStep`
  - `StepStatus`
  - `DecompositionProposal`
  - `WorkflowMetrics`, `StepMetrics`, `VoteStats`
  - `WorkItem`
  - `WaitState`
  - `AgentKind`
- Initially copy implementations from `src/context.rs`, then adjust imports to use `crate::core` rather than other top-level modules where appropriate.

**B.2 Leave a thin compatibility layer in `src/context.rs`**
- Turn `src/context.rs` into either:
  - A re-export module: `pub use crate::core::domain::*;`, or
  - A very thin wrapper that forwards to `core::domain` while maintaining the same public API for downstream crates (like integration tests) during the migration period.

**B.3 Remove config coupling from domain types**
- Replace `AgentConfig.red_flaggers: Option<Vec<crate::config::RedFlaggerConfig>>` with:
  - A core-owned shape that expresses only what the runner needs (e.g., `CoreRedFlaggerConfig` with `kind: String` and a `serde_json::Value`/`BTreeMap<String, String>` for parameters), or
  - A generic `Vec<RedFlaggerDescriptor>` that keeps the core independent of `serde_yaml` and file formats.
- Add conversion functions at the boundary (in `config.rs` or a separate `config_mapper.rs`) that map from `config::RedFlaggerConfig` → the new core shape.

**B.4 Update application code to use `core::domain`**
- Adjust `FlowRunner`, tasks, and status export code to import domain models from `core::domain` (or via the `context` re-export if you choose that style).
- Ensure serialization is still correct by running tests that serialize/deserialize `Context` to/from JSON.

**Acceptance for Phase B**
- All workflow state and behavior types live under `core::domain`.
- No domain type imports `crate::config` directly.
- `Context` remains serializable and backwards compatible (existing sessions can still be loaded).

---

## Phase C – Implement and Wire All Outbound Ports

**C.1 File system adapter**
- Create `src/adapters/outbound/filesystem.rs` implementing `core::ports::outbound::FileSystem`:
  - `StdFileSystem` using `std::fs` and `std::path`.
  - Map IO errors to `core::Error::FileSystem`.
- Use `FileSystem` where appropriate:
  - In `ApplyVerifyTask` for reading/writing target files instead of calling `std::fs` directly.
  - Optionally in other write-heavy paths (but persistence can stay on `rusqlite` since it’s already an adapter).

**C.2 Clock adapter**
- Create `src/adapters/outbound/clock.rs` implementing `Clock`:
  - `SystemClock` using `SystemTime::now()` or `Instant`.
  - Return `now_ms()` as a monotonic or wall-clock timestamp as appropriate.
- Inject `Clock` into components that currently call `Instant::now()` or `SystemTime` directly when the timestamp is part of domain behavior or metrics (e.g., `WorkflowMetrics.record_duration_ms` can stay as-is, but anything that observes real-world time in decisions should use `Clock`).

**C.3 Telemetry adapter**
- Create `src/adapters/outbound/telemetry.rs` implementing `TelemetrySink`:
  - `TracingTelemetrySink` that records structured events via `tracing::event!` (or similar).
- Update application code to send a small set of well-defined events through `TelemetrySink` instead of ad-hoc logs where that improves observability (e.g., session lifecycle, step completion, red-flag incidents).

**C.4 Red-flagging boundary**
- Decide on the boundary:
  - Option A: Keep `RedFlagPipeline` as an application helper and treat `RedFlagger` (plus `RedFlaggerConfig` mapping) as the outbound port; this is essentially already the case.
  - Option B (stricter): Introduce a `GuardrailService` port in `core::ports::outbound` that encapsulates collecting samples, running red-flaggers, and returning accepted outputs, and move `RedFlagPipeline` under `adapters::outbound::guardrails`.
- For minimal change, keep `RedFlagPipeline` in the application layer but:
  - Ensure all concrete flaggers implement `core::ports::RedFlagger` only.
  - Make `RedFlagPipeline` itself not leak infrastructure types.

**C.5 Injection**
- Extend `FlowRunner` and/or a new `ApplicationContext` struct to carry:
  - `Arc<dyn FileSystem>`
  - `Arc<dyn Clock>`
  - `Arc<dyn TelemetrySink>`
  - (and, if desired, any higher-level guardrail services).
- Thread these through tasks that need them, starting with:
  - `ApplyVerifyTask` (filesystem)
  - Any future time-based logic (clock)
  - Centralized telemetry points (telemetry sink).

**Acceptance for Phase C**
- Application and adapters no longer call `std::fs` or raw time APIs in places where ports exist.
- Telemetry flows through `TelemetrySink` in at least a few critical paths.
- Red-flaggers remain pure implementations of `core::ports::RedFlagger`.

---

## Phase D – Configuration Boundary & Core Config Types

**D.1 Define minimal core config structs**
- Add `src/core/config.rs` with types such as:
  - `AgentRuntimeConfig` (prompt template, model, samples, k, red-flag descriptors).
  - `DomainRuntimeConfig` (map `AgentKind` → `AgentRuntimeConfig`, applier/verifier identifiers, domain-level flags needed by the runner).
- Ensure these structs have no `serde` attributes and no file-path knowledge.

**D.2 Map from `MicrofactoryConfig` to core config**
- In `config.rs` or a new `config/runtime_mapper.rs`, implement:
  - `impl DomainConfig { fn to_runtime(&self, cli_defaults: &CliDefaults) -> DomainRuntimeConfig { ... } }`
  - Where `CliDefaults` bundles CLI-level defaults like samples/k/adaptive_k.
- Update `FlowRunner` to accept `DomainRuntimeConfig` instead of the full `DomainConfig`:
  - The composition root (`main.rs`) becomes responsible for calling `config.domain(&context.domain)?.to_runtime(&defaults)`.

**D.3 Remove direct `config` references from core/application**
- Replace usages of `config::RedFlaggerConfig` in `Context` / `AgentConfig` with the new core config shapes.
- Confine all `serde_yaml`/file IO and domain loading logic to `config.rs` and adapters.

**Acceptance for Phase D**
- `FlowRunner` and tasks depend only on `core::{domain, ports, config}` and not on `config::MicrofactoryConfig` directly.
- Config parsing and file-path manipulation remain isolated to `config.rs` and the composition root.

---

## Phase E – Architecture Guardrails (`cargo deny` + xtask)

**E.1 Introduce `deny.toml`**
- Add a top-level `deny.toml` with:
  - `[bans]` to detect duplicate/old dependencies if desired.
  - `[sources]` to restrict disallowed registries if needed.
  - `[advisories]` as appropriate for security checks.
- Use the `[[targets]]` / `skip` mechanisms only sparingly; the focus here is to plug into an `xtask`, not to over-constrain dependencies.

**E.2 Add an `xtask` crate (or script)**
- Create a small `xtask` (either as a binary crate in `xtask/` or as a Rust binary under `src/bin/xtask.rs`) with a subcommand like:
  - `cargo xtask check-architecture`
  - Implementation:
    - Runs `cargo deny --workspace`.
    - Optionally runs a lightweight static check:
      - For example, grep the workspace for `use microfactory::adapters::` inside `src/core` and `src/application`, and fail if found.

**E.3 Wire into CI / scripts**
- Update `scripts/run_all_tests.sh` or CI configuration to optionally include:
  - `cargo xtask check-architecture` or `cargo deny --workspace` as a separate stage.

**Acceptance for Phase E**
- A single command (`cargo xtask check-architecture` or similar) enforces both dependency health and simple layering rules.
- The command passes on a clean tree and fails when someone introduces a forbidden dependency edge.

---

## Phase F – Docs, Logging Robustness, and Tests

**F.1 Update architecture docs**
- Refresh `docs/arch/architecture.md` to:
  - Reflect the actual modules: `core`, `application`, `adapters::{inbound,outbound}`, and their responsibilities.
  - Reference both `TASK-014-clean-architecture.md` and this `TASK-014-FIXUP.md` as implementation notes.
  - Replace outdated path examples (`src/tasks`, `orchestrator.rs`, `domains/`) with the new structure.

**F.2 Clarify “core dependencies” language**
- Decide on the intended constraint:
  - Option A: Allow small foundational crates (`async-trait`, `serde_json`) in core and update docs to say “Core depends only on std + foundational crates, not on infrastructure crates like `rusqlite`, `rig-core`, `handlebars`, `axum`.”
  - Option B: Move async boundaries and JSON shapes out of core so it truly depends on std only.
- Update `TASK-014-clean-architecture.md` to reflect whichever choice is adopted.

**F.3 Harden logging when log directories are not writable**
- Modify `tracing_setup::init` so that:
  - Failure to create the log directory or session log file is treated as a warning and results in “no file layer” rather than a panic.
  - This can be done by attempting a simple `File::create` or by catching errors from `rolling::never` and falling back to `None` for the file layer.
- Adjust or extend integration tests to:
  - Prefer setting `MICROFACTORY_HOME` to a temp directory when asserting on logging behavior.
  - Verify that the CLI still emits JSON logs to stdout (`--log-json`, `--pretty`) even when file logging is silently disabled due to permissions.

**F.4 Strengthen tests around new ports**
- Add unit tests around:
  - `StdFileSystem` (read/write, path validation integration with `validate_target_path`).
  - `SystemClock` (basic monotonicity or format).
  - `TracingTelemetrySink` (ensuring it does not panic on arbitrary event payloads).
- Add focused tests for core/domain behavior using in-memory implementations of:
  - `SessionRepository`
  - `LlmClient`
  - `FileSystem`
  - `Clock`
  - `TelemetrySink`
  - to exercise the critical scenarios already covered by `FlowRunner` integration tests, but using pure, deterministic adapters.

**Acceptance for Phase F**
- Docs accurately describe the architecture and dependency rules.
- Logging integration tests are robust to restricted filesystems while preserving production behavior.
- New port implementations and their use in the application layer are covered by unit tests, with no reduction in existing coverage.

---

## Overall Acceptance Criteria for TASK-014-FIXUP

- **Namespaces:** Inbound/outbound directions are visible in module paths (`core::ports::{inbound,outbound}`, `adapters::{inbound,outbound}`).
- **Domain in Core:** Workflow state (`Context`, steps, metrics, work items) is owned by `core::domain` and does not depend on `config` or adapter crates.
- **Ports Wired:** All defined outbound ports (LLM, persistence, filesystem, clock, telemetry, red-flagging) have concrete adapter implementations where appropriate, and the application layer depends only on these abstractions.
- **Guardrails:** A documented command (`cargo xtask check-architecture` or similar) runs `cargo deny` and simple static checks to keep dependencies pointing inward.
- **Docs Updated:** `docs/arch/architecture.md` and `TASK-014-clean-architecture.md` are consistent with the codebase and this fixup plan.
- **Tests & Coverage:** `cargo test` (including integration tests) remains green under normal permissions; integration tests are stable, and overall coverage is at least as strong as before TASK-014.

