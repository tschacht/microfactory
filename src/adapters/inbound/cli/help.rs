//! Help rendering for CLI topics.

use serde::Serialize;

use super::HelpTopic;

#[derive(Debug, Clone, Serialize)]
pub struct HelpSection {
    pub topic: &'static str,
    pub summary: &'static str,
    pub usage_examples: Vec<&'static str>,
    pub key_flags: Vec<FlagHelp>,
    pub notes: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlagHelp {
    pub flag: &'static str,
    pub description: &'static str,
}

pub fn render_help_text(section: &HelpSection) {
    println!("Topic: {}", section.topic);
    println!("Summary: {}", section.summary);
    if !section.usage_examples.is_empty() {
        println!();
        println!("Usage examples:");
        for example in &section.usage_examples {
            println!("  {example}");
        }
    }
    if !section.key_flags.is_empty() {
        println!();
        println!("Key flags:");
        for flag in &section.key_flags {
            println!("  {:<24}{}", flag.flag, flag.description);
        }
    }
    if !section.notes.is_empty() {
        println!();
        println!("Notes:");
        for note in &section.notes {
            println!("  - {note}");
        }
    }
    println!();
    println!("Tip: every subcommand also supports the standard `--help` output.");
}

pub fn build_help_section(topic: HelpTopic) -> HelpSection {
    match topic {
        HelpTopic::Overview => HelpSection {
            topic: "overview",
            summary: "Microfactory runs MAKER-inspired workflows (decompose → solve → verify) with persistence, resume, and HTTP monitoring endpoints.",
            usage_examples: vec![
                r#"microfactory run --prompt "refactor api" --domain code"#,
                "microfactory status --json --limit 5",
                "microfactory serve --bind 0.0.0.0 --port 8080",
            ],
            key_flags: vec![
                FlagHelp {
                    flag: "run",
                    description: "Start a new workflow session backed by the domain config.",
                },
                FlagHelp {
                    flag: "status",
                    description: "Query stored sessions (human output by default, JSON via --json).",
                },
                FlagHelp {
                    flag: "resume",
                    description: "Continue a paused session after addressing the wait reason.",
                },
                FlagHelp {
                    flag: "subprocess",
                    description: "Execute a single MAKER step in isolation and emit JSON.",
                },
                FlagHelp {
                    flag: "serve",
                    description: "Expose sessions over HTTP (REST + SSE) for higher-level tooling.",
                },
                FlagHelp {
                    flag: "--inspect <mode>",
                    description: "Stream detailed LLM ops/messages (ops, payloads, messages, files) to stdout.",
                },
            ],
            notes: vec![
                "Use `microfactory help --topic <command>` for focused instructions or `--format json` for machine parsing.",
                "API keys load from ~/.env first, then fall back to real env vars.",
                "Session data lives under ~/.microfactory (override via MICROFACTORY_HOME).",
            ],
        },
        HelpTopic::Run => HelpSection {
            topic: "run",
            summary: "Execute the full MAKER workflow for a given prompt within a configured domain.",
            usage_examples: vec![
                r#"microfactory run --prompt "stabilize auth" --domain code --config config.yaml"#,
                r#"microfactory run --prompt "audit notebooks" --domain analysis --dry-run --samples 6 --k 4"#,
                r#"microfactory run --prompt "fix bug" --inspect messages"#,
            ],
            key_flags: vec![
                FlagHelp {
                    flag: "--prompt <text>",
                    description: "Required task description fed into the decomposition agent.",
                },
                FlagHelp {
                    flag: "--domain <name>",
                    description: "Selects which domain section of the YAML config to load (e.g. code).",
                },
                FlagHelp {
                    flag: "--config <path>",
                    description: "Defaults to ./config.yaml; point to custom configs per domain.",
                },
                FlagHelp {
                    flag: "--api-key <key>",
                    description: "Override provider API key; otherwise resolves from env/~/\\.env.",
                },
                FlagHelp {
                    flag: "--llm-provider <id>",
                    description: "openai | anthropic | gemini | grok; determines API key lookup.",
                },
                FlagHelp {
                    flag: "--llm-model <name>",
                    description: "Model identifier passed directly to the provider (e.g. gpt-4.1).",
                },
                FlagHelp {
                    flag: "--samples <n>",
                    description: "Samples per microagent step (default 10).",
                },
                FlagHelp {
                    flag: "--k <n>",
                    description: "First-to-ahead-by-k voting margin (default 3).",
                },
                FlagHelp {
                    flag: "--adaptive-k",
                    description: "Enable adaptive voting margins driven by live metrics.",
                },
                FlagHelp {
                    flag: "--max-concurrent-llm <n>",
                    description: "Cap simultaneous LLM calls (default 4) for rate limits.",
                },
                FlagHelp {
                    flag: "--repo-path <path>",
                    description: "Run steps relative to a specific repository or workspace.",
                },
                FlagHelp {
                    flag: "--dry-run",
                    description: "Skips persistence and issues a single LLM probe for validation.",
                },
                FlagHelp {
                    flag: "--step-by-step",
                    description: "Pause after decomposition and step completion for manual review.",
                },
                FlagHelp {
                    flag: "--human-low-margin-threshold <n>",
                    description: "Human pause trigger for thin vote margins (set 0 to keep running despite ties).",
                },
                FlagHelp {
                    flag: "-o, --output-dir <path>",
                    description: "Directory for output files (default: current working directory).",
                },
                FlagHelp {
                    flag: "-v, --verbose",
                    description: "Global logging toggle for timestamps + debug-level stdout.",
                },
                FlagHelp {
                    flag: "--log-json [--pretty|--compact]",
                    description: "Emit structured logs instead of human text (indent vs single-line).",
                },
                FlagHelp {
                    flag: "--inspect <mode>",
                    description: "Bypass default logs to show internal LLM events (ops, payloads, messages, files).",
                },
            ],
            notes: vec![
                "Successful runs persist context + metadata; inspect progress via `status` or the HTTP service.",
                "Set MICROFACTORY_HOME to isolate state per project or CI worker.",
            ],
        },
        HelpTopic::Status => HelpSection {
            topic: "status",
            summary: "Inspect stored sessions (human-readable or JSON).",
            usage_examples: vec![
                "microfactory status --limit 5",
                "microfactory status --session-id a1b2 --json",
            ],
            key_flags: vec![
                FlagHelp {
                    flag: "--session-id <id>",
                    description: "Show detailed information for a single session.",
                },
                FlagHelp {
                    flag: "--limit <n>",
                    description: "Restrict the number of listed sessions (default 10).",
                },
                FlagHelp {
                    flag: "--json",
                    description: "Emit structured summaries matching the HTTP API schema.",
                },
                FlagHelp {
                    flag: "-v, --verbose",
                    description: "Include timestamps/debug output in the human-readable listing.",
                },
                FlagHelp {
                    flag: "--log-json [--pretty|--compact]",
                    description: "Use JSON logging for status output (indented or single-line).",
                },
            ],
            notes: vec![
                "Use JSON output for LLM or dashboard ingestion without scraping stdout.",
                "Combine with `jq`/`gron` to filter for paused or failed sessions quickly.",
            ],
        },
        HelpTopic::Resume => HelpSection {
            topic: "resume",
            summary: "Continue a paused/failed session using stored metadata (override options available).",
            usage_examples: vec![
                "microfactory resume --session-id a1b2",
                "microfactory resume --session-id a1b2 --llm-provider anthropic --llm-model claude-3.5",
            ],
            key_flags: vec![
                FlagHelp {
                    flag: "--session-id <id>",
                    description: "Target session UUID (required).",
                },
                FlagHelp {
                    flag: "--config <path>",
                    description: "Override the saved config path if files moved.",
                },
                FlagHelp {
                    flag: "--api-key <key>",
                    description: "Swap credentials when resuming (falls back to stored/env otherwise).",
                },
                FlagHelp {
                    flag: "--llm-provider|--llm-model",
                    description: "Swap providers/models without editing persisted metadata.",
                },
                FlagHelp {
                    flag: "--samples|--k|--max-concurrent-llm",
                    description: "Tweak runtime parameters prior to resuming.",
                },
                FlagHelp {
                    flag: "--human-low-margin-threshold <n>",
                    description: "Override the low-margin pause guard (0 disables).",
                },
                FlagHelp {
                    flag: "-v, --verbose / --log-json",
                    description: "Global logging controls apply just like on `run`.",
                },
                FlagHelp {
                    flag: "--inspect <mode>",
                    description: "Stream decoded LLM interactions during the resumed session.",
                },
            ],
            notes: vec![
                "Wait-state triggers are cleared automatically so execution can continue.",
                "Failures update their status immediately; inspect via `status --session-id <id>`.",
            ],
        },
        HelpTopic::Subprocess => HelpSection {
            topic: "subprocess",
            summary: "Run a single microtask (e.g., solver) with JSON I/O for tooling hooks.",
            usage_examples: vec![
                r#"microfactory subprocess --domain code --step solver --context-json '{"files":["lib.rs"]}' --samples 4"#,
            ],
            key_flags: vec![
                FlagHelp {
                    flag: "--domain <name>",
                    description: "Required domain key matching your config file.",
                },
                FlagHelp {
                    flag: "--config <path>",
                    description: "Config to load agent definitions from (defaults to ./config.yaml).",
                },
                FlagHelp {
                    flag: "--step <name>",
                    description: "Select the microtask (solver, verifier, etc.).",
                },
                FlagHelp {
                    flag: "--context-json <blob>",
                    description: "Inline JSON merged into the domain-specific context.",
                },
                FlagHelp {
                    flag: "--samples / --k",
                    description: "Sampling + vote settings for this isolated run.",
                },
                FlagHelp {
                    flag: "--llm-provider / --llm-model",
                    description: "Choose the backend + model for the subprocess call.",
                },
                FlagHelp {
                    flag: "--api-key <key>",
                    description: "Provide credentials explicitly if env resolution is insufficient.",
                },
                FlagHelp {
                    flag: "--max-concurrent-llm <n>",
                    description: "Limit simultaneous LLM calls (default 2).",
                },
                FlagHelp {
                    flag: "-v, --verbose",
                    description: "Show human-friendly logs during the subprocess run.",
                },
                FlagHelp {
                    flag: "--log-json [--pretty|--compact]",
                    description: "Emit the subprocess logs as JSON instead of text.",
                },
                FlagHelp {
                    flag: "--inspect <mode>",
                    description: "See the exact prompt/response for this isolated step.",
                },
            ],
            notes: vec![
                "Outputs SubprocessOutput JSON: session, step, candidates, winner, metrics.",
                "Great for editor commands or CI bots needing a single reasoning step.",
            ],
        },
        HelpTopic::Serve => HelpSection {
            topic: "serve",
            summary: "Expose sessions via REST + SSE so dashboards or agents can monitor progress.",
            usage_examples: vec!["microfactory serve --bind 0.0.0.0 --port 8080"],
            key_flags: vec![
                FlagHelp {
                    flag: "--bind <ip>",
                    description: "Interface for the Axum HTTP server (default 127.0.0.1).",
                },
                FlagHelp {
                    flag: "--port <n>",
                    description: "Port number (default 8080).",
                },
                FlagHelp {
                    flag: "--limit <n>",
                    description: "Default page size for GET /sessions when clients omit limit.",
                },
                FlagHelp {
                    flag: "--poll-interval-ms <n>",
                    description: "SSE polling cadence for /sessions/stream (min 250ms).",
                },
                FlagHelp {
                    flag: "-v, --verbose",
                    description: "Emit INFO/DEBUG logs for HTTP access + background tasks.",
                },
                FlagHelp {
                    flag: "--log-json [--pretty|--compact]",
                    description: "Switch server logs to structured JSON output.",
                },
                FlagHelp {
                    flag: "--inspect <mode>",
                    description: "Trace background LLM calls if the server performs any (rare).",
                },
            ],
            notes: vec![
                "Endpoints: GET /sessions, GET /sessions/{id}, GET /sessions/stream (SSE).",
                "Combine with `curl` or dashboards to watch sessions without invoking the CLI.",
                "Serve shares the same serialization structs as status --json for parity.",
            ],
        },
    }
}
