# Microfactory: A Rust-Based Tool for MAKER-Inspired Workflows

## Overview

Microfactory is a command-line tool written in Rust that abstracts the principles from the MAKER paper ("Solving a Million-Step LLM Task with Zero Errors"). It enables reliable, scalable execution of long-horizon tasks using large language models (LLMs). The tool decomposes a high-level task (e.g., "fix tests in the repo") into atomic steps, executes each step with ensembles of "microagents" (LLM samples), applies voting for consensus, and performs local verification to achieve near-zero errors.

Key features:
- **Generic Abstraction**: Supports any domain (e.g., code fixing, data processing) via configurable presets.
- **MAKER Fidelity**: Maximal Agentic Decomposition (MAD), microagent sampling, "first-to-ahead-by-k" voting, and red-flagging.
- **Integration**: Uses the `rig` Rust library as the LLM backend; underlying models and API keys are configured via provider-specific environment variables and configuration.
- **Performance**: Asynchronous, concurrent execution with state persistence for large-scale tasks.

This document outlines the architecture, components, and implementation details.

## Design Principles

- **Genericity**: Not tied to specific tasks; configurable via domains (e.g., "code" for test-fixing).
- **Modularity**: Workflow as a directed graph of tasks (nodes) with conditional edges.
- **Scalability**: Handles 1M+ steps by keeping contexts short and errors local.
- **Safety**: Human-in-loop options, retries, and dry-run mode.
- **Rust Advantages**: Type safety, concurrency (Tokio), error handling (anyhow).

## MAKER Alignment and Extensions

Microfactory aims to follow the MAKER massively decomposed agentic process (MDAP) framework while remaining generic:

- **Explicit agent roles**: Separate agents for decomposition, decomposition discrimination, solution discrimination, and minimal problem solving, each with their own prompts and models.
- **Step granularity control**: Domains define how aggressively tasks are decomposed into minimal subtasks, so MAD-style behavior can be tuned per workflow.
- **Adaptive error correction**: Support both fixed and adaptive `k` in first-to-ahead-by-k voting, informed by per-step verification statistics.
- **Pluggable red-flagging**: Red-flag checks (length, syntax, LLM-based critique, etc.) are modeled as a configurable pipeline per domain.
- **Metrics and observability**: The context records per-step metrics (samples, resamples, votes, verification outcomes) to enable analysis and tuning.
- **Resource and rate control**: Concurrency limits are applied to LLM API calls to avoid overload and rate-limit issues.
- **Human-in-loop hooks**: The runner can pause and ask for human input when repeated failures, high disagreement, or red flags occur.

## CLI Interface

Using the `clap` crate for parsing.

Example usage:
```
microfactory run --prompt "fix tests in the repo" --api-key $OPENAI_API_KEY --llm-model gpt-5.1-codex-mini --samples 10 --k 3 --domain code --repo-path ./ --dry-run
```

- `--prompt`: Global task description (required).
- `--api-key`: API key for the configured LLM provider (optional if configured via environment variables). By default, Microfactory attempts to load provider-specific variables (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`) from `~/.env` before falling back to the current process environment.
- `--llm-provider`: Selects the backend provider (`openai`, `anthropic`, `gemini`, or `grok`, which maps to xAI's Grok models). Defaults to `openai`.
- `--llm-model`: Model identifier to pass to the `rig` provider (default: gpt-5.1-codex-mini).
- `--samples`: Number of microagents per step (default: 10).
- `--k`: Voting margin (default: 3).
- `--adaptive-k`: Enable adaptive adjustment of `k` based on observed verification statistics (optional).
- `--max-concurrent-llm`: Maximum number of concurrent LLM API calls (default: derived from CPU cores).
- `--domain`: Preset domain (e.g., "code").
- `--repo-path`: Domain-specific path (e.g., repo root).
- `--dry-run`: Simulate without applying changes.

Subcommands:
- `microfactory status`: Check session progress.
- `microfactory resume --session-id <UUID>`: Resume interrupted sessions.

## Core Components

### 1. LLM Integration
- Primary integration is via the `rig` crate, which provides typed clients for LLM providers (e.g., OpenAI-compatible APIs).
- Microfactory uses `rig` to obtain a single response per microagent; models and providers are selected via the `--llm-provider`/`--llm-model` CLI flags or per-agent `model` settings.
- Provider-neutral backend: the CLI currently supports OpenAI, Anthropic (Claude), Google Gemini, and xAI Grok via rig's dynamic client builder, so the orchestration stack behaves identically regardless of which foundation model is chosen.
- API keys are resolved lazily per provider by reading `~/.env` (if present) before falling back to the active process environment; vars are scoped (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`) and never overwrite already-set values.
- Prompt templating: Use `handlebars` for dynamic prompts (e.g., insert state/context).
- Wrap calls behind an `LlmClient` trait so that the rest of the system is independent of the concrete backend and provider.
- Use a concurrency limiter (e.g., semaphore) around LLM calls, configured via `--max-concurrent-llm` and/or domain configuration.

Example microagent creation (rig-based):
```rust
use rig::providers::openai::Client;
use rig::agent::AgentBuilder;

async fn create_microagent(prompt: &str, model: &str, api_key: &str) -> anyhow::Result<String> {
    let provider = Client::new(api_key);
    let agent = AgentBuilder::new(provider.model(model)).build();
    let response = agent.chat(prompt).await?;
    Ok(response.content)
}
```

### 2. State Management
- `Context` struct: Thread-safe (`Arc<RwLock<Context>>`).
  - Fields: Global prompt, decomposed steps (Vec<Step>), current index, domain data (e.g., HashMap<PathBuf, String> for files), LLM history.
  - Metrics: Per-step statistics such as sample counts, vote counts, red-flag hits, verification outcomes, and timings, used for analysis and adaptive algorithms (e.g., adaptive `k`).
- Persistence: Serialize to JSON or SQLite (`serde`, `rusqlite`) with session UUIDs.

### 3. Workflow Graph
- Use `petgraph` or build on `graph-flow` crate for orchestration.
- Nodes: Tasks implementing a `MicroTask` trait.
- Edges: Sequential or conditional (e.g., loop on verify fail).

MicroTasks can be parameterized to implement MAKER's four agent roles: decomposition agents, decomposition discriminator agents, solution discriminator agents, and solver agents for minimal subtasks. The workflow graph wires these tasks into a decomposition-plus-solve pipeline with localized verification and error correction.

`MicroTask` trait:
```rust
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use anyhow::Result;

#[async_trait]
trait MicroTask {
    async fn run(&self, ctx: Arc<RwLock<Context>>) -> Result<NextAction>;
}

enum NextAction {
    Continue,
    WaitForInput,  // Human approval
    GoTo(String),  // Jump to node
    End,
    Error(String),
}
```

Key tasks (MAKER-style roles):
- **DecompositionTask**: Decomposition agents that break a task into subtasks and composition metadata.
- **DecompositionVoteTask**: Decomposition discriminator agents that vote among alternative decompositions using first-to-ahead-by-k.
- **SolveTask**: Problem-solver agents that attempt minimal subtasks without further decomposition.
- **SolutionVoteTask**: Solution discriminator agents that vote among candidate minimal-step solutions.
- **RedFlagTask**: Rule-based or LLM check for risky outputs.
- **ApplyVerifyTask**: Domain-specific (e.g., patch files, run `pytest`).

Orchestrator (`FlowRunner`):
- Builds graph from decomposed steps.
- Executes asynchronously until completion.

### 4. Domain Configuration
- YAML config for extensibility:
```yaml
domains:
  code:
    agents:
      decomposition:
        prompt_template: "code_decompose.hbs"
        model: "gpt-5.1-codex"
        samples: 3
      decomposition_discriminator:
        prompt_template: "code_decompose_vote.hbs"
        model: "gpt-5.1-codex-mini"
        k: 3
      solver:
        prompt_template: "code_solve_step.hbs"
        model: "gpt-5.1-codex"
        samples: 10
      solution_discriminator:
        prompt_template: "code_solution_vote.hbs"
        model: "gpt-5.1-codex-mini"
        k: 3
    step_granularity:
      max_files: 1
      max_lines_changed: 20
    verifier: "pytest -v"
    applier: "patch_file"  # Built-in function
    red_flaggers:
      - type: "length"
        max_tokens: 2048
      - type: "syntax"
        language: "python"
```
- Traits for custom domains (e.g., `Verifier`, `Applier`, `RedFlagger`) and utilities for mapping agent configs to `MicroTask` implementations.

### 5. MAKER Agent Types & Configuration

MAKER’s generalized framework uses four agent types, which Microfactory exposes through the `agents` block in the domain configuration:

- **Decomposition agents**: Take a task description and propose a decomposition into smaller subtasks plus a composition function.
- **Decomposition discriminator agents**: Vote (with first-to-ahead-by-k) among candidate decompositions.
- **Problem solver agents**: Solve minimal subtasks without further decomposition.
- **Solution discriminator agents**: Vote among candidate minimal-step solutions proposed by solver agents.

At the Rust level these map to an `AgentKind` and corresponding configuration:

```rust
enum AgentKind {
    Decomposition,
    DecompositionDiscriminator,
    Solver,
    SolutionDiscriminator,
}

struct AgentConfig {
    prompt_template: String,
    model: String,
    samples: usize,
    k: Option<usize>, // for voting agents
}
```

`FlowRunner` reads the per-domain `agents` configuration, instantiates `AgentConfig` values for each `AgentKind`, and wires them into the workflow graph using the tasks described above. Domains can override global settings such as `k`, `samples`, and step granularity, or rely on CLI defaults (e.g., `--k`, `--samples`, `--adaptive-k`).

## Microagent Execution Flow (Per Step)

1. **Prompt Generation**: Template with local context (e.g., code snippet + error).
2. **Sampling**: Parallel LLM calls (Tokio futures), subject to concurrency limits.
3. **Voting**: Count similar outputs; select leader if ahead by `k` (fixed or adaptive).
4. **Red-Flagging**: Check length, syntax, etc.
5. **Apply/Verify**: Update state; run external tools (e.g., `pytest`); rollback on fail.

## Error Handling & Logging
- `anyhow` for unified errors.
- `tracing` for logs/progress bars.
- Retries: Max resamples per step.
- Adaptive `k`: Optionally adjust `k` based on aggregated per-step metrics (success/failure rates) when `--adaptive-k` is enabled.
- Human-in-Loop: Stdin prompts for approval when configured triggers fire (e.g., repeated verification failures, persistent red flags, or high disagreement between agents).

## Project Structure
```
microfactory/
├── src/
│   ├── main.rs          # CLI entrypoint
│   ├── context.rs       # State management
│   ├── tasks/           # Task implementations (DecompositionTask.rs, etc.)
│   ├── orchestrator.rs  # Graph builder and runner
│   └── domains/         # Domain-specific logic (CodeDomain.rs)
├── Cargo.toml           # Dependencies: clap, tokio, serde, petgraph, etc.
└── config.yaml          # Domain presets
```

## Dependencies (Cargo.toml Excerpt)
```toml
[dependencies]
clap = { version = "4.0", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
petgraph = "0.6"  # Or graph-flow
strsim = "0.10"   # Fuzzy matching
rusqlite = "0.29" # Optional persistence
handlebars = "4.3" # Templating
tracing = "0.1"
rig = "0.1"       # LLM client
```

## Alternative: As an Agent Tool
- Subcommand: `microfactory subprocess --step "propose fix" --context-json '{"code": "..."}' --samples 10`
- Returns JSON output for integration with higher agents (e.g., Codex CLI orchestrator).

## Phased Implementation Plan (for LLM Implementor)

This section describes a suggested implementation sequence assuming the work is carried out by an LLM-based coding agent. Each phase should be completed and minimally validated before proceeding to the next. Do not change this architecture document from within the implementation phases.

### Phase 0: Scaffold and Wiring

- Create a Rust binary crate named `microfactory` with the dependencies listed in this document.
- Implement the `clap`-based CLI with `run`, `status`, and `resume` subcommands and the options described under "CLI Interface".
- Add a configuration loader that reads `config.yaml` into strongly-typed structs (including the `agents` and `step_granularity` blocks) with clear error messages on invalid config.
- Define the `Context` type, `AgentKind`, `AgentConfig`, `LlmClient` trait, and `FlowRunner` skeleton (interfaces only, no real graph logic yet).
- Add basic tests for CLI parsing and configuration loading; ensure the crate builds and these tests pass.

**Implementation Status (Nov 15, 2025):** Completed. The repository now contains a Rust binary crate with the specified dependencies plus `async-trait`/`serde_yaml` to support the planned abstractions. The CLI exposes `run`, `status`, and `resume` subcommands with all documented flags, and unit tests confirm CLI parsing behavior. `config.rs` loads `config.yaml` into typed structs for domains, agents, step granularity, and red-flaggers, with helpful error context. Core runtime scaffolding is in place: `Context`, `AgentKind`, and `AgentConfig` structures capture runtime state, `LlmClient` defines the async sampling interface, and `FlowRunner` validates domains and logs placeholder execution. `cargo test` passes, covering the CLI and configuration loader.

### Phase 1: LLM Client and Sampling

- Implement a concrete `LlmClient` using the `rig` crate, targeting an OpenAI-compatible provider and using the `--api-key` and `--llm-model` options.
- Implement concurrency limiting for LLM calls using a semaphore; make the limit configurable via `--max-concurrent-llm`.
- Expose a simple internal API `sample_n` that requests `n` responses from the configured model for a given prompt.
- Extend the `run` subcommand so that, in a "debug" or "dry-run only" mode, it can issue a single LLM call via `LlmClient` on the global prompt and print the response, without yet running the full workflow graph.
- Add tests or lightweight checks around error handling for LLM invocation (invalid API key, network errors, provider errors).

**Implementation Status (Nov 15, 2025):** Completed. `RigLlmClient` now wraps `rig-core`'s dynamic client builder so the CLI can target OpenAI, Anthropic, Gemini, or xAI's Grok models with a shared semaphore-based concurrency cap (`--max-concurrent-llm`). API keys come from `--api-key` or provider-specific environment variables that are first hydrated from `~/.env`, and `--llm-provider` selects which backend + env var pairing to use. The `run` command instantiates the provider-specific client (also stored inside `FlowRunner`) and, when invoked with `--dry-run`, performs a single model probe on the global prompt and streams the response without entering the workflow. New tests cover API-key resolution precedence, dry-run error surfacing, `.env` parsing, and validation of blank credentials so LLM failures are caught early; `cargo test` remains green.

### Phase 2: Context and Linear Workflow

- Implement the `Context` struct fully, including fields for steps, current index, domain data, and metrics (but metrics can initially be populated with placeholders).
- Implement a minimal linear `FlowRunner` that:
  - Treats the global prompt as a single step or uses a trivial decomposition provided by the domain.
  - Executes only `SolveTask` and `ApplyVerifyTask` with fixed `k` and simple majority voting across solver samples.
- Implement the `MicroTask` trait and a first set of concrete tasks for this linear flow.
- Wire the `run` subcommand to build an initial context, execute the linear workflow, and report status and verification results.
- Do not implement recursive decomposition or adaptive `k` yet; keep behavior simple and observable.

**Implementation Status (Nov 16, 2025):** Completed. `Context` now owns a hierarchical step tree with parent/child links, per-step depth, solution buffers, and pending decomposition/solution queues so that microtasks can pass data without extra globals. `WorkflowMetrics` tracks sample/vote counters, and helper APIs expose `ensure_root`, `add_child_step`, and status mutation utilities. The `MicroTask` trait plus concrete solver tasks drive a minimal linear pipeline inside `FlowRunner`, which is now wired to the CLI `run` command so real executions mutate the context rather than logging placeholders.

### Phase 3: MAKER Agent Roles and Graph Expansion

- Implement concrete tasks for the four MAKER roles: `DecompositionTask`, `DecompositionVoteTask`, `SolveTask`, and `SolutionVoteTask`, using the per-domain `agents` configuration (prompt templates, models, samples, and `k`).
- Extend the `FlowRunner` to:
  - Read `AgentConfig` entries from the loaded domain config.
  - Construct a workflow graph that supports recursive decomposition into subtasks, followed by solving minimal subtasks and discriminating among candidate solutions.
- Ensure the "code" domain example in this file is supported end-to-end: its YAML should parse and drive which prompts/models are used for each role.
- Add tests or small integration runs that exercise decomposition and solution voting on synthetic tasks, even if not yet applied to a real repository.

**Implementation Status (Nov 16, 2025):** Completed. The `tasks` module now implements MAKER's four agent roles (`DecompositionTask`, `DecompositionVoteTask`, `SolveTask`, `SolutionVoteTask`) with shared `NextAction`/`TaskEffect` plumbing plus first-to-ahead-by-`k` voting helpers. `FlowRunner` builds a `petgraph`-backed workflow DAG, instantiates agents from the per-domain `agents` config (including per-role prompts/models/samples/`k`), and recursively expands subtasks until they reach solver depth, then routes results through solution discriminators. A scripted `cargo test` covers the end-to-end graph on synthetic prompts to ensure decomposition, solving, and voting cooperate without hitting the real providers.

### Phase 4: Red-Flagging, Metrics, and Adaptive k

- Implement a `RedFlagger` trait with built-in implementations matching the `red_flaggers` examples (e.g., length-based, syntax-based).
- Integrate red-flagging into the workflow so that flagged responses are discarded and resampled where appropriate.
- Populate the metrics fields in `Context` for each step: sample counts, resample counts, red-flag hits, vote margins, verification success/failure, and timings.
- Implement an optional adaptive `k` strategy (enabled via `--adaptive-k`) that uses metrics to adjust `k` per step or step type, following the spirit of MAKER’s scaling laws (keep it simple and well-bounded).
- Add trace logging using `tracing` for key events (decomposition decisions, vote outcomes, red flags, verification results).

**Implementation Status (Nov 16, 2025):** Completed. Domains now build a `RedFlagPipeline` from the YAML `red_flaggers` block (with built-in `length` and `syntax` checkers), and every sampling task routes responses through that pipeline. Flagged samples are logged via `tracing`, recorded in per-step metrics (including sample/resample counts, red-flag incidents, vote margins, timing, and verification outcomes), and resampled until the quorum of clean candidates is met. The CLI’s `--adaptive-k` switch now activates a heuristic that tightens or relaxes the voting margin per agent type based on the rolling average of recent margins captured in `WorkflowMetrics`. Decomposition, solving, and voting tasks emit structured trace events so Phase 4 observability requirements are satisfied, and new unit tests cover the red-flag resampling path.

### Phase 5: Human-in-Loop, Persistence, and Subprocess Mode

- Implement `NextAction::WaitForInput` handling so that steps can pause for human approval or intervention based on configured triggers (e.g., repeated failures, high disagreement, frequent red flags).
- Implement persistence of `Context` and workflow state to disk (JSON or SQLite) with session IDs, and wire up the `status` and `resume` subcommands to inspect and resume sessions.
- Implement the `microfactory subprocess` subcommand described in the "Alternative: As an Agent Tool" section, using the same internal tasks and `FlowRunner` but constrained to a single step or small subgraph.
- Add smoke tests for resuming a session, and for using `microfactory subprocess` in a simple pipeline.

**Implementation Status (Nov 16, 2025):** Completed. `FlowRunner` now surfaces `NextAction::WaitForInput` as a first-class pause, with heuristics for red-flag streaks, excessive resamples, and tight vote margins automatically parking the session while recording a `WaitState`. The runtime context, pending work queue, and metrics are serialized (via `serde_json`) into a bundled `rusqlite` database under `~/.microfactory/sessions.sqlite3`, and the CLI’s `status`/`resume` subcommands inspect or restart those sessions (including API-key/provider overrides). New session metadata captures provider/model/option knobs so resumes reuse the same LLM backdrop by default. An additional `microfactory subprocess` command reuses the solver + discriminator stack for single-step workflows and emits JSON suitable for higher-level orchestrators. Unit tests cover the SQLite session store, CLI env-layer, and the existing scripted runner regression, and `cargo test` stays green.

### Phase 6: Hardening and Domain Expansion

- Improve error messages, configuration validation, and logging for real-world use (e.g., missing templates, invalid YAML structures, unsupported domains).
- Add additional example domains (beyond `code`) to validate genericity, reusing the same core architecture and MAKER-style agent roles.
- Avoid premature optimization: do not introduce new dependencies or significant refactors unless required to support new domains or performance bottlenecks observed in practice.
- Keep all phases backward-compatible with the CLI and config structures described in this document; if changes are necessary, update this document first (not as part of automated LLM implementation steps).
- Surface session summaries (e.g., structured JSON export from `microfactory status`) so external orchestrators can monitor workflows without scraping stdout.
- Provide guidance for a follow-on HTTP status surface: prioritize an embedded `microfactory serve` command that exposes the existing JSON session summaries over HTTP (REST + SSE/WebSocket) so dashboards or supervising agents can subscribe once and avoid repeated CLI polling; daemon-style sockets are optional icing for tightly coupled integrations but carry higher tooling friction.

**Implementation Status (Nov 16, 2025):** Completed. `config.rs` now validates each domain’s agents, granularity, and red-flaggers while hydrating prompt templates from disk, and the repository ships Handlebars templates for both the existing `code` preset and a new `analysis` domain to demonstrate cross-domain coverage. The CLI’s `status` command gained `--json`/`--limit` flags so higher-level tooling can pull machine-readable session summaries, while runtime errors now enumerate available domains when a mismatch occurs. `config.yaml` documents the expanded domains, and `templates/` holds the prompts referenced by the configuration, ensuring hardening + domain expansion goals are met.

### Phase 7: Session Service Surface (Optional Extension)

- Introduce a `microfactory serve` subcommand that runs an embedded Tokio HTTP server, reusing the SQLite session store to expose `GET /sessions` (list with pagination) and `GET /sessions/{id}` (detailed view) endpoints that return the same JSON payloads emitted by `microfactory status --json`.
- Add an SSE or WebSocket stream (e.g., `/sessions/stream`) that pushes session state transitions (running → paused/completed, wait-state updates) so operator dashboards and supervising agents receive near-real-time notifications without polling.
- Keep authentication simple (localhost-only token or OS permissions) and document how to deploy as a background service (systemd, tmux, etc.). Offer guidance on when to prefer this HTTP surface over a bespoke daemon: HTTP is universally consumable and easy to proxy, whereas a custom daemon/socket can be reserved for tightly coupled workflows that need bidirectional control.
- Optional future work: layer a Unix-domain-socket daemon or message-queue publisher atop the same event stream if a particular deployment requires it, but treat that as additive, not a replacement for the HTTP surface.

**Implementation Status (Nov 16, 2025):** Completed. The CLI now exposes `microfactory serve`, which binds an embedded Axum/Tokio HTTP server to configurable host/port, serving `GET /sessions` (with optional `?limit=`) and `GET /sessions/{id}` responses identical to `status --json`. A `/sessions/stream` SSE endpoint emits periodic JSON snapshots so dashboards and supervising agents can subscribe once rather than poll the CLI. The server reuses the existing SQLite `SessionStore`, enforces sensible defaults (localhost bind, rate-limited polling), and includes unit tests for the REST handlers. Future daemon/socket integrations remain optional add-ons atop this HTTP surface.

### Phase 8: Library-First Refactor & Background Workers

**Goal:** Decouple the core orchestration logic from the CLI binary to support a robust, long-running background server with a thread worker pool. This transforms Microfactory from a "script runner" into a "platform" capable of managing concurrent sessions programmatically.

**Reasoning:**
Currently, `FlowRunner` and session management logic are tightly coupled to the CLI entry points (`main.rs`, `cli.rs`). The HTTP server (`serve`) is read-only because it cannot easily spawn a `FlowRunner` without blocking its async runtime or duplicating complex setup logic. To support `POST /resume` and future features like job queues, the core logic must be extractable.

**Implementation Plan:**

1.  **Extract Core Logic (`lib.rs`):**
    *   Move "glue" logic (config loading, API key resolution, runner initialization) from `main.rs` into a new `SessionManager` struct in the library crate.
    *   `SessionManager` should handle the full lifecycle: creating sessions, loading from SQLite, initializing `FlowRunner`, and executing workflows.

2.  **Make `FlowRunner` Shareable:**
    *   Ensure `FlowRunner` and its dependencies (LLM client, Handlebars registry) are cheap to clone or share (via `Arc`) across threads.

3.  **Implement Job Queue & Worker Pool:**
    *   Introduce an in-memory job queue (e.g., `tokio::sync::mpsc` or `deadqueue`) within the `serve` process.
    *   Spawn a background worker task that consumes session IDs from the queue and uses `SessionManager` to execute them.
    *   This replaces the need to "shell out" to the CLI for background tasks, improving performance and observability.

4.  **Refactor CLI & Server:**
    *   Update `microfactory run` and `resume` to be thin wrappers around `SessionManager`.
    *   Update `POST /sessions/:id/resume` to push the session ID into the job queue instead of returning 501 or shelling out.

**Note:** For the immediate term (TASK-006), the `resume` endpoint may use a simpler "shell out" strategy (executing `microfactory resume` as a subprocess) to provide functionality before this major refactor is undertaken.

## Implementation Notes
- Start prototyping with a single-task graph.
- Test with small tasks (e.g., fix one test) before scaling.
- Inspired by rs-graph-llm (GitHub) for orchestration.
