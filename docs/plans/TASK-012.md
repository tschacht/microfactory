# TASK-012: LLM Inspection View (Layered Logs) - Implementation Plan

This document details the implementation strategy for the "Inspection View," a layered logging approach designed to peel back the complexity of LLM interactions in the Microfactory CLI.

## Motivation from the Greeter Shakedown (Nov 21, 2025)
- Running the canonical Rust greeter example with `MICROFACTORY_HOME=/tmp/...` produced only high-level stdout (“session started / paused”) even though the workflow sampled three decomposition candidates and hit a low-margin pause.
- The persisted JSON log captured the rich `rig::providers::openai` spans, but every `gen_ai.input.messages` / `gen_ai.output.messages` field was double-encoded, forcing manual parsing before we could read the actual prompts and completions.
- Without a structured “ops” view we could not see token usage, latency, or which model/provider pair was active until spelunking through raw JSON.
- A layered inspection surface must therefore (1) expose per-span summaries, (2) expose payloads/messages in decoded form, and (3) keep the existing `~/.microfactory/logs` JSON for audit parity.

## The 4(+1) Layers of Inspection

We define four primary layers of data visible in the logs, plus a fifth, file-centric view. The new `--inspect <MODE>` flag will allow users to select which layer they want to see, bypassing the default application logs.

### 1. `envelope` (Layer 1)
*   **What:** The raw system event and metadata.
*   **Content:** JSON formatted log lines including timestamp, level (`INFO`/`DEBUG`), and target module.
*   **Existing Equivalent:** Controlled by `--log-json` and `--verbose`.
*   **Behavior:** If `--inspect` is NOT used, this is the default (or verbose) output.

### 2. `ops` (Layer 2)
*   **What:** High-level summary of LLM interactions (Operations).
*   **Content:** One line per request/response cycle showing the model used, provider, token usage, and latency.
*   **Example Output:**
    ```text
    [LLM] chat gpt-5-nano (OpenAI) | In: 502 tok | Out: 150 tok | 1200ms
    ```
*   **Implementation:** Extract `gen_ai.request.model`, `gen_ai.provider.name`, `gen_ai.operation.name`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, and duration (`event.timestamp - span.start`).
*   **Output contract:** One line per finished span, sorted by completion time. Include an incrementing span counter plus session id fragment to help correlating with file logs.

### 3. `payloads` (Layer 3)
*   **What:** The detailed protocol data structure.
*   **Content:** Pretty-printed JSON of the *actual* request and response bodies sent to the provider API, after undoing any string escaping.
*   **Crucial Step (Double-Decoding):** The `rig-core` library serializes the message history into a string (e.g., `"[{{\"role\":...}}]"`) before logging it. This mode must detect these strings, parse them into real JSON objects, and then print the result. When payloads contain HTML entities (e.g., `&lt;file path=\"…\"&gt;`), they must be HTML-unescaped before further parsing.
*   **Additional requirements:** Redact `api_key`, `bearer_token`, or other credential-looking keys before printing; collapse long tool blobs behind `"... truncated (N chars) ..."`.
*   **Example Output:**
    ```json
    {
      "messages": [
        {
          "role": "user",
          "content": "..."
        }
      ],
      "tools": [...] 
    }
    ```

### 4. `messages` (Layer 4)
*   **What:** The human-readable content.
*   **Content:** Clean text blocks showing only the conversation flow.
*   **Behavior:** Like `payloads`, it double-decodes the message history. Then, it iterates through the message list and renders them as text, stripping away JSON syntax.
*   **Example Output:**
    ```text
    ─── [User] ─────────────────────────────────────────────────────
    Write a hello world program in Rust...

    ─── [Assistant] ────────────────────────────────────────────────
    Sure! Here is the code:
    ...
    ```

### 5. `files` (Layer 5)
*   **What:** The code files being proposed in solver outputs.
*   **Content:** Parsed `<file path=\"...\">…</file>` XML blocks extracted from assistant messages, summarized as file paths plus short previews.
*   **Behavior:** Reuses the same `extract_xml_files` heuristic as the applier (`src/tasks/mod.rs:extract_xml_files`) to turn XML blocks into `(path, content)` pairs; groups them per span and per step so humans can see “what will be written where” without scanning raw XML.
*   **Example Output:**
    ```text
    [FILES span=resp_abc123 step=2]
    - src/main.rs (37 lines)
      fn main() { … }
    - src/lib.rs (12 lines)
      pub fn greet(..) { … }
    ```

---

## Implementation Steps

### 1. CLI Updates (`src/cli.rs`)
*   Add a new global enum argument to `Cli` struct:
    ```rust
    #[arg(long, global = true, value_enum, help = "Inspect internal LLM events (ops, payloads, messages)")]
    pub inspect: Option<InspectMode>
    ```
*   Define the enum:
    ```rust
    #[derive(Debug, Clone, Copy, ValueEnum)]
    pub enum InspectMode {
        Ops,
        Payloads,
        Messages,
        Files,
    }
    ```
*   `--inspect` is mutually exclusive with `--log-json`, `--pretty`, and `--compact`; the inspection layer replaces stdout formatting but still honors `--verbose` to widen span filters.
*   Surface the flag in `microfactory --help`, `run --help`, and `resume --help`, describing that it streams layer-2/3/4 details to stdout while file logs stay untouched.

### 2. New Inspection Module (`src/tracing_inspect.rs`)
*   Create a new module to keep `tracing_setup.rs` clean.
*   Implement a `struct InspectionLayer` that implements `tracing_subscriber::Layer`.
*   **Logic:**
    *   **Filter:** Capture spans whose target starts with `rig::providers` or that emit `gen_ai.*` fields; bubble up `reqwest::connect` timing data so OPS output can include connect latency where available.
    *   **Extraction:** In `on_close` (when the span finishes and has all data), pull `gen_ai.input.messages`, `gen_ai.output.messages`, token counts, latency, request ids, and any error metadata.
    *   **Parsing:** Use `serde_json::from_str` on the stringified arrays; tolerate failures by falling back to the original string with a warning prefix. After decoding JSON, apply HTML-entity unescaping on message text fields so constructs like `&lt;file path=\"src/main.rs\"&gt;` become real `<file>` tags before any XML/file parsing.
    *   **Rendering:** Switch on `InspectMode` (passed to the layer's constructor) to decide how to print to stdout. Payloads render pretty JSON; messages render text blocks grouped by role; ops render one-liners; files render per-span file summaries.
    *   **Redaction hook:** Before printing payloads/messages, walk each JSON value and redact known secret keys or values longer than 200 characters containing `"sk-"`, `"xai-"`, etc.

### 3. Integration (`src/tracing_setup.rs`)
*   Modify the `init` function to accept the new `inspect` option.
*   **Switching Logic:**
    *   **IF** `inspect` is set: Do **not** install the default `fmt::layer()` (Layer 1 stdout logger). Instead, install the `InspectionLayer`.
    *   **ELSE**: Install the default human-friendly or JSON logger as before.
    *   **ALWAYS**: Keep the file logger (Layer 1 JSON to `~/.microfactory/logs`) active. This ensures that even if the user is just "inspecting" messages, the full debug trace is still saved to disk for later analysis.
*   Add a `TracingInspectConfig` struct that bundles the chosen mode and writer so unit tests can inject a buffer.

### 4. Output Examples & Help Snippets (`docs/user_guide.md`, `docs/architecture.md`)
*   Document how to use `--inspect` with `run`, `resume`, and `serve`.
*   Include screenshots or transcripts showing:
    *   `--inspect ops` summarizing the decomposition low-margin fight from the greeter run.
    *   `--inspect payloads` showing decoded JSON for a solver span.
    *   `--inspect messages` showing the clean conversation transcript.

### 5. Validation
*   Unit-test the double-decoding helper against fixtures that cover:
    *   Proper JSON arrays inside strings.
    *   Already-real JSON (no decoding needed).
    *   Malformed payloads (ensure we log a warning but still print raw data).
*   Base tests on concrete fixtures under `docs/plans/TASK-012/example-fixtures/`:
    *   `rig-openai-decomposition-input.jsonl` (double-encoded `gen_ai.input.messages`).
    *   `rig-openai-solution-vote-candidates-with-xml.jsonl` (HTML-escaped `<file path=\"…\">` blocks inside candidate solutions).
    *   `microfactory-apply-overwrite-file.log` (apply/verify lifecycle for XML-based overwrites).
*   Add an integration test under `integration-tests/` that runs a `microfactory run --inspect messages --dry-run` against a stub provider to guarantee the CLI emits the new view without touching disk logs.

## Layer Formatting Reference

| Mode | Example (abridged) | Notes |
| --- | --- | --- |
| `ops` | `[LLM #12] chat gpt-5-nano (OpenAI) | In: 690 tok | Out: 2,119 tok | 1230ms | span=resp_0f355b...` | Include span ordinal, provider, model, op kind, tokens, latency, span id fragment. |
| `payloads` | ```{ "request": { "messages": [...] }, "response": { ... } }``` | Pretty JSON, redact credential-like fields, truncate tool blobs >2KB. |
| `messages` | `─── [User] ───\nPrompt text\n\n─── [Assistant] ───\nReply…` | Render role headers with ASCII rulers; show content parts (text, code, tool) sequentially. |
| `files` | `[FILES span=resp_123 step=2]\n- src/main.rs (37 lines)\n- src/lib.rs (12 lines)` | Summarize XML `<file>` blocks per span, including path, line counts, and optional first-line preview. |

## Safety and Redaction Rules
- Never print `OPENAI_API_KEY`, Anthropic keys, or bearer tokens even if they appear in payloads; replace with `"[redacted]"`.
- When payloads include user files (e.g., embeddings), cap output at 4KB per span and indicate `[payload truncated]`.
- Respect `MICROFACTORY_SILENT=1` (if set) to suppress inspection output entirely, ensuring CI can still run noiselessly.

## Rollout Checklist
1. Implement CLI flag and tracing layer.
2. Update docs and help text.
3. Record a sample session transcript demonstrating each inspect mode.
4. Verify `microfactory run --inspect ops` still writes the full JSON log under `~/.microfactory/logs`.
5. Ship behind the `--inspect` flag (no behavior change unless the user opts in).

## Observations & Minor Adjustments
1. **HTML Unescaping:** Must occur *after* JSON string decoding but *before* parsing inner XML/files.
2. **File Extraction:** Use `src/utils.rs:extract_xml_files` (refactored in TASK-010) instead of `src/tasks/mod.rs`.
3. **Streaming:** The `ops` layer assumes request/response spans. Future support for streaming spans will be tracked separately.
4. **CLI Conflicts:** Use `clap`'s `conflicts_with` to enforce mutual exclusivity between `--inspect` and `--log-json`.
