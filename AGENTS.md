# Repository Guidelines

## Project Structure & Module Organization
- `src/` holds the CLI, runner, persistence, and domain logic; start with `src/cli.rs` for clap wiring and `src/runner.rs` for execution flow.
- `templates/` contains Handlebars prompts referenced by `config.yaml`; keep template/model names aligned with their domain blocks.
- `docs/` hosts architecture notes, the user guide, and research context; sync this folder when behavior changes.
- `integration-tests/` is a standalone crate that exercises CLI flows end-to-end; fixture data lives under `integration-tests/tests`.
- `scripts/` bundles helper bash wrappers (tests, cleanup) that the Makefile invokes.
- `TODO.md` tracks technical debt and planned features (e.g., per-agent red-flaggers). Refer to this file before starting new features or refactors.

## Build, Test, and Development Commands
- `make build` (or `cargo build --workspace`) compiles every crate.
- `make fmt`, `make fmt-check`, and `make clippy` manage formatting and linting; the clippy target runs with `-D warnings` and `--fix`.
- `make unit-test`, `make integration-test`, and `make test` delegate to the scripts under `scripts/` for quiet, deterministic runs.
- `make serve` launches `microfactory serve` for inspecting sessions via HTTP.
- `scripts/run_all_tests.sh` is the CI-friendly entry point; pair it with `make ensure-pristine` before pushing.

## CLI Help & Discoverability
- The CLI uses `clap`, so `microfactory --help` (or `microfactory -h`) prints every top-level flag plus the available subcommands.
- Each subcommand exposes its own help: e.g., `microfactory run --help` covers domain selection and solver tuning, while `microfactory status --help` explains `--json`, `--limit`, and session filters.
- Operational tools like `microfactory serve --help` and `microfactory subprocess --help` document bind addresses, polling intervals, and JSON output formats; check them before scripting against the CLI.
- Pass `-V` to display the binary version embedded in `Cargo.toml`, useful when filing issues or comparing with deployment targets.

## Coding Style & Naming Conventions
- Rust 2024 edition with the standard `cargo fmt` profile (4-space indents, trailing commas, module ordering via rustfmt).
- Treat `clippy` diagnostics as blockers; prefer explicit types when they clarify async boundaries or trait objects.
- Keep module and file names snake_case (e.g., `red_flaggers.rs`), and favor descriptive struct names (`SessionStore`, `DomainConfig`).
- When editing Handlebars templates, keep placeholders `{{like_this}}` and document any custom helpers in `docs/`.

## Testing Guidelines
- Unit tests live alongside code and run via `scripts/run_unit_tests.sh` (skips the integration crate for faster feedback).
- Cross-crate and CLI flows belong in `integration-tests/src` with assertions under `integration-tests/tests`; run them via `scripts/run_integration_tests.sh`.
- Prefer deterministic seeds and avoid live network calls; mock LLM providers through the rig client stubs.
- Gate new features with at least one regression test plus coverage for failure paths (e.g., invalid config, SQLite errors).

## Commit & Pull Request Guidelines
- NEVER commit, stage changes or do any destructive operations with git (e.g. no touching the staging area, no reset index etc.). this is only manged by the human.
- also do not ask to do any state-changing or destructive operations via git.
- non-destructive and non-state-changing reading opertions via git are allowed.
- create new branches or PRs only when explicitly prompted.
- suggest imperative, ≤72-character commit subjects ("Add adaptive k flag"), followed by focused bodies that explain intent + verification.
- Squash fixups locally; open PRs only after `make ensure-pristine` passes and docs/config diffs are included when behavior changes.
- Reference relevant issues in the PR description, enumerate verification commands, and call out any follow-up work or manual steps.
- Include screenshots or terminal transcripts when UI/CLI output changes, and note any migrations affecting `~/.microfactory` data.

## Security & Configuration Tips
- Never commit API keys; store them in `~/.env` as described in `docs/user_guide.md#5` and rely on env resolution order (flag → env → file).
- SQLite session data lives under `~/.microfactory`; redact UUIDs or prompts when sharing logs.
- Config changes should preserve safe defaults (workspace-write sandbox, conservative red-flag thresholds) unless reviewers agree otherwise.
