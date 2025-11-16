# Microfactory User Guide

Welcome to Microfactory, a MAKER-inspired workflow runner that orchestrates large language model (LLM) agents to tackle long-horizon software and analysis tasks. This guide introduces the ideas behind the project, how the system is structured, and how to operate it effectively as a technical user.

## 1. Conceptual Overview

Microfactory treats complex work (e.g., "fix the test suite" or "summarize quarterly adoption data") as a graph of tightly scoped steps. Each step is executed by ensembles of microagents—independent LLM samples—that decompose the task, solve minimal subtasks, and vote on the safest result. The orchestration follows the MAKER MDAP (Massively Decomposed Agentic Process) approach from the paper *Solving a Million-Step LLM Task with Zero Errors*.

### 1.1 Key Capabilities

- **MAKER fidelity:** Implements decomposition, solving, discriminators, first-to-ahead-by-*k* voting, and per-step verification.
- **Provider-neutral LLM backend:** Uses `rig-core` to talk to OpenAI, Anthropic (Claude), Google Gemini, and xAI Grok models with swappable prompts and models per agent role.
- **Safety features:** Configurable red-flag pipeline (length, syntax, etc.), adaptive voting margins, and human-in-loop pauses when disagreements stack up.
- **Persistence:** Automatically checkpoints state to SQLite so runs can be resumed or inspected later.
- **CLI + JSON observability:** `microfactory status --json` surfaces machine-readable session summaries for dashboards or supervising agents.

## 2. Research Roots and Design Principles

Microfactory extends the MAKER MDAP blueprint with general-purpose tooling:

1. **Massive decomposition:** A decomposition agent proposes subtasks; a discriminator picks the best breakdown. Recursion limits and heuristics keep depth under control.
2. **Microagent ensembles:** Solver agents sample multiple candidate fixes, while solution discriminators vote with first-to-ahead-by-*k* logic. Adaptive *k* tightens or relaxes consensus based on observed margins.
3. **Localized verification:** Steps update `Context` state with solver outputs, red-flag events, vote margins, and verification verdicts so errors stay local.
4. **Human-in-loop hooks:** When resample counts, red-flag streaks, or low vote margins breach thresholds, the runner emits `WaitForInput` so a human can review before continuing.

## 3. Project Layout

```
microfactory/
├── Cargo.toml           # Rust crate manifest
├── config.yaml          # Domain presets (code, analysis, …)
├── docs/
│   ├── architecture.md  # System design + phased plan
│   └── user_guide.md    # This guide
├── src/
│   ├── cli.rs           # clap definitions
│   ├── context.rs       # Runtime state & metrics
│   ├── llm.rs           # Rig-backed LLM client
│   ├── runner.rs        # Flow orchestration
│   ├── persistence.rs   # SessionStore (SQLite)
│   ├── red_flaggers.rs  # Built-in checker implementations
│   └── tasks/mod.rs     # Decomposition, solving, voting tasks
└── templates/           # Handlebars prompts referenced by config
```

## 4. Prerequisites & Installation

- **Rust toolchain:** Rust 1.76+ (edition 2024). Install via `rustup`.
- **LLM credentials:** API keys for your chosen providers (OpenAI, Anthropic, Gemini, Grok/xAI). No live calls occur during tests, but real runs require network access.
- **Optional:** `sqlite3` CLI for inspecting saved sessions.

Clone the repo and build:

```bash
git clone https://github.com/<your-org>/microfactory.git
cd microfactory
cargo build
```

## 5. Configuring API Keys

Microfactory autoloads `~/.env` once per process. Populate it with provider-specific keys (they will only be set if not already in the environment):

```bash
cat >> ~/.env <<'ENV'
OPENAI_API_KEY="sk-openai-..."
ANTHROPIC_API_KEY="sk-ant-..."
GEMINI_API_KEY="ai-gemini-..."
XAI_API_KEY="xai-grok-..."
ENV
```

You can also pass `--api-key` explicitly or set env vars before launching the CLI. Keys are resolved in this order: CLI flag → current env → `~/.env`.

## 6. Domain Configuration

Domains describe how Microfactory should behave for a class of tasks. The default `config.yaml` ships with two domains:

- **`code`:** Designed for repo maintenance. Decomposition prompts live under `templates/code_*.hbs`, solver agents default to `gpt-5.1-codex`, and red-flaggers include both length and Python syntax checks.
- **`analysis`:** Targets research/reporting workflows with Anthropic + Gemini models and Markdown-oriented templates.

Each domain defines:

```yaml
agents:
  decomposition:
    prompt_template: "templates/..."
    model: "..."
    samples: 3
  decomposition_discriminator:
    prompt_template: "..."
    model: "..."
    k: 2
  # solver / solution_discriminator similar ...
step_granularity:
  max_files: 1
  max_lines_changed: 20
red_flaggers:
  - type: "length"
    max_tokens: 2048
  - type: "syntax"
    language: "python"
```

`src/config.rs` validates each domain (non-empty templates/models, positive `samples`/`k`, mandatory params for red-flaggers) and hydrates template files relative to the config’s directory.

## 7. CLI Reference

### 7.1 `microfactory run`

```
microfactory run \
  --prompt "Fix flaky tests" \
  --domain code \
  --config config.yaml \
  --llm-provider openai \
  --llm-model gpt-5.1-codex \
  --samples 8 --k 3 --adaptive-k \
  --max-concurrent-llm 4
```

Options include `--repo-path`, `--dry-run` (single model probe), and `--max-concurrent-llm` for rate limiting. Runs create a UUID session, enqueue decomposition work, and persist progress to `~/.microfactory/sessions.sqlite3`.

### 7.2 `microfactory status`

Inspect sessions:

- `microfactory status` → recent sessions (text)
- `microfactory status --session-id <UUID>` → detailed view
- `microfactory status --json --limit 50` → machine-readable summaries

### 7.3 `microfactory resume`

```
microfactory resume --session-id <UUID> [--llm-provider ... overrides]
```

Loads the stored context + metadata, clears wait states, and continues execution with either the original provider/model settings or overrides you supply.

### 7.4 `microfactory subprocess`

Executes a single step using the solver + solution discriminator stack and prints structured JSON. Useful when embedding Microfactory as a helper tool inside larger agent systems.

## 8. Execution Flow

For each step:

1. **Decomposition (`AgentKind::Decomposition`):** Renders the template with the step description, samples multiple decompositions, and stores proposals.
2. **Decomposition vote:** The discriminator compares proposals via first-to-ahead-by-*k*; ties fall back to majority.
3. **Solve:** Solver agents generate concrete patches/plans. Responses pass through the `RedFlagPipeline`; flagged samples trigger resampling (budgeted per runner options).
4. **Solution vote:** Discriminator picks the winning candidate; metrics record vote margin, duration, sample counts.
5. **Apply / verify (domain-specific):** Currently tracked in metrics and step status; integration with repo mutations or command runners is planned for future phases.
6. **Human pause (optional):** If resample counts, red-flag incidents, or vote margins cross thresholds, the runner records a `WaitState` and returns `RunnerOutcome::Paused` so you can inspect before resuming.

## 9. Persistence & Observability

- **SessionStore:** Each `run`/`resume` interaction saves the serialized `Context` plus CLI metadata to SQLite. Files live under `~/.microfactory/sessions.sqlite3` by default (see `src/paths.rs`).
- **Metrics:** `Context.metrics` stores per-step sample counts, resamples, red-flag incidents, vote margins, duration (ms), and verification flags. These metrics surface in `status --json` output via `SessionDetailExport`.
- **Tracing:** Tasks emit `tracing` events (e.g., decomposition proposals ready, solution votes, red-flag hits). Configure `RUST_LOG=info` to view them.

## 10. Working With Human-in-Loop Pauses

Triggers (configurable in `RunnerOptions`) include:

- `human_red_flag_threshold` (default 4 incidents per step)
- `human_resample_threshold` (default 4 resamples)
- `human_low_margin_threshold` (default vote margin ≤ 1)

When triggered, Microfactory:

1. Records `WaitState { step_id, trigger, details }` in context.
2. Saves session with status `paused` and prints guidance.
3. Requires a `resume` command after you resolve the issue (e.g., adjusting prompts, editing config, or approving the candidate output).

## 11. Advanced Features

- **Adaptive `k`:** `--adaptive-k` enables per-agent tuning based on recent vote margins (rolling window). Helpful when solver outputs are highly divergent.
- **Multiple domains:** Add more entries to `config.yaml` with domain-specific prompts/models. Ensure associated templates exist; `config.rs` will error if missing.
- **Subprocess integration:** Pair `microfactory subprocess` with a supervising agent to run targeted steps and ingest JSON results directly.
- **Structured status exports:** The JSON format emitted by `status` is identical to what a future `microfactory serve` HTTP surface will expose (see Phase 7 roadmap).

## 12. Roadmap & Extensions

- **Phase 7 (planned):** `microfactory serve` HTTP endpoint exposing `/sessions` and SSE/WebSocket updates so dashboards can subscribe to changes without polling the CLI.
- **Tool integrations:** Hook `Context` metrics into dashboards (Grafana, OpenTelemetry) or send `WaitState` notifications to Slack/Teams.
- **Domain expansion:** Add templates + configs for security reviews, data labeling, or creative writing workflows.

## 13. Troubleshooting

| Symptom | Likely Cause | Fix |
| --- | --- | --- |
| `Missing API key` error | No `--api-key`, env var, or `~/.env` entry | Set provider-specific env var or pass `--api-key` |
| `Domain 'foo' not defined` | Typo or missing config entry | Check `config.yaml`; CLI now lists available domains |
| `Prompt template ... not found` | Template path in YAML does not exist | Place `.hbs` file under `templates/` or use inline prompt text |
| Session stuck paused | Human-in-loop trigger fired repeatedly | Run `microfactory status --session-id ...` to inspect trigger, adjust prompts/thresholds, then `resume` |
| Tests fail on first run | Templates absent or config invalid | Ensure repository templates exist (default ones are checked in) and rerun `cargo test` |

## 14. Further Reading

- `docs/architecture.md` – Deep dive into the phased implementation plan, data structures, and future phases.
- MAKER paper: *Solving a Million-Step LLM Task with Zero Errors* (arXiv 2511.09030). The design mirrors its decomposition + voting pipeline.
- `src/runner.rs` tests – Examples of scripted LLM flows for both `code` and `analysis` domains.

With this guide you should be able to configure domains, supply API keys, and run Microfactory workflows end-to-end. If you plan to integrate Microfactory into a larger orchestration stack, pay special attention to the JSON status exports and upcoming HTTP service phase.
