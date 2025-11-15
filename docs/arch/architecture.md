# Microfactory: A Rust-Based Tool for MAKER-Inspired Workflows

## Overview

Microfactory is a command-line tool written in Rust that abstracts the principles from the MAKER paper ("Solving a Million-Step LLM Task with Zero Errors"). It enables reliable, scalable execution of long-horizon tasks using large language models (LLMs). The tool decomposes a high-level task (e.g., "fix tests in the repo") into atomic steps, executes each step with ensembles of "microagents" (LLM samples), applies voting for consensus, and performs local verification to achieve near-zero errors.

Key features:
- **Generic Abstraction**: Supports any domain (e.g., code fixing, data processing) via configurable presets.
- **MAKER Fidelity**: Maximal Agentic Decomposition (MAD), microagent sampling, "first-to-ahead-by-k" voting, and red-flagging.
- **Integration**: Uses OpenAI API (via Rust crates) for LLM calls; optional fallback to Codex CLI.
- **Performance**: Asynchronous, concurrent execution with state persistence for large-scale tasks.

This document outlines the architecture, components, and implementation details.

## Design Principles

- **Genericity**: Not tied to specific tasks; configurable via domains (e.g., "code" for test-fixing).
- **Modularity**: Workflow as a directed graph of tasks (nodes) with conditional edges.
- **Scalability**: Handles 1M+ steps by keeping contexts short and errors local.
- **Safety**: Human-in-loop options, retries, and dry-run mode.
- **Rust Advantages**: Type safety, concurrency (Tokio), error handling (anyhow).

## CLI Interface

Using the `clap` crate for parsing.

Example usage:
```
microfactory run --prompt "fix tests in the repo" --api-key $OPENAI_API_KEY --model gpt-4o-mini --samples 10 --k 3 --domain code --repo-path ./ --dry-run
```

- `--prompt`: Global task description (required).
- `--api-key`: OpenAI API key (required).
- `--model`: LLM model (default: gpt-4o-mini).
- `--samples`: Number of microagents per step (default: 10).
- `--k`: Voting margin (default: 3).
- `--domain`: Preset domain (e.g., "code").
- `--repo-path`: Domain-specific path (e.g., repo root).
- `--dry-run`: Simulate without applying changes.

Subcommands:
- `microfactory status`: Check session progress.
- `microfactory resume --session-id <UUID>`: Resume interrupted sessions.

## Core Components

### 1. LLM Integration
- Primary: `rig` crate for direct OpenAI API calls (faster, structured outputs).
- Fallback: Spawn `codex` CLI processes via `tokio::process::Command` if needed.
- Prompt templating: Use `handlebars` for dynamic prompts (e.g., insert state/context).

Example microagent creation (API):
```rust
use rig::completion::PromptCompletion;
use rig::providers::openai::Client;

async fn create_microagent(prompt: &str, model: &str, api_key: &str) -> anyhow::Result<String> {
    let provider = Client::new(api_key);
    let agent = rig::agent::AgentBuilder::new(provider.model(model)).build();
    let response = agent.chat(prompt).await?;
    Ok(response.content)
}
```

CLI fallback:
```rust
use tokio::process::Command;

async fn call_codex_cli(prompt: &str) -> anyhow::Result<String> {
    let output = Command::new("codex")
        .arg("--model").arg("gpt-4o-mini")
        .arg("--prompt").arg(prompt)
        .output().await?;
    Ok(String::from_utf8(output.stdout)?)
}
```

### 2. State Management
- `Context` struct: Thread-safe (`Arc<RwLock<Context>>`).
  - Fields: Global prompt, decomposed steps (Vec<Step>), current index, domain data (e.g., HashMap<PathBuf, String> for files), LLM history.
- Persistence: Serialize to JSON or SQLite (`serde`, `rusqlite`) with session UUIDs.

### 3. Workflow Graph
- Use `petgraph` or build on `graph-flow` crate for orchestration.
- Nodes: Tasks implementing a `MicroTask` trait.
- Edges: Sequential or conditional (e.g., loop on verify fail).

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

Key tasks:
- **DecompositionTask**: LLM prompt to generate atomic steps (output: JSON Vec<String>).
- **SamplingTask**: Run `samples` concurrent microagents; collect outputs.
- **VotingTask**: Fuzzy grouping (`strsim` crate), "first-to-ahead-by-k" logic; resample if needed.
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
    verifier: "pytest -v"
    applier: "patch_file"  # Built-in function
```
- Traits for custom domains (e.g., `Verifier` impl).

## Microagent Execution Flow (Per Step)

1. **Prompt Generation**: Template with local context (e.g., code snippet + error).
2. **Sampling**: Parallel LLM calls (Tokio futures).
3. **Voting**: Count similar outputs; select leader if ahead by `k`.
4. **Red-Flagging**: Check length, syntax, etc.
5. **Apply/Verify**: Update state; run external tools (e.g., `pytest`); rollback on fail.

## Error Handling & Logging
- `anyhow` for unified errors.
- `tracing` for logs/progress bars.
- Retries: Max resamples per step.
- Human-in-Loop: Stdin prompts for approval.

## Project Structure
```
microfactory/
├── src/
│   ├── main.rs          # CLI entrypoint
│   ├── context.rs       # State management
│   ├── tasks/           # Task implementations (DecompositionTask.rs, etc.)
│   ├── orchestrator.rs  # Graph builder and runner
│   └── domains/         # Domain-specific logic (CodeDomain.rs)
├── Cargo.toml           # Dependencies: clap, rig, tokio, serde, petgraph, etc.
└── config.yaml          # Domain presets
```

## Dependencies (Cargo.toml Excerpt)
```toml
[dependencies]
clap = { version = "4.0", features = ["derive"] }
rig = "0.1"  # LLM API
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
petgraph = "0.6"  # Or graph-flow
strsim = "0.10"   # Fuzzy matching
rusqlite = "0.29" # Optional persistence
handlebars = "4.3" # Templating
tracing = "0.1"
```

## Alternative: As an Agent Tool
- Subcommand: `microfactory subprocess --step "propose fix" --context-json '{"code": "..."}' --samples 10`
- Returns JSON output for integration with higher agents (e.g., Codex CLI orchestrator).

## Implementation Notes
- Start prototyping with a single-task graph.
- Test with small tasks (e.g., fix one test) before scaling.
- Inspired by rs-graph-llm (GitHub) for orchestration.
