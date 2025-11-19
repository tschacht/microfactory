# TASK-012: LLM Inspection View (Layered Logs) - Implementation Plan

This document details the implementation strategy for the "Inspection View," a layered logging approach designed to peel back the complexity of LLM interactions in the Microfactory CLI.

## The 4 Layers of Inspection

We define four distinct layers of data visible in the logs. The new `--inspect <MODE>` flag will allow users to select which layer they want to see, bypassing the default application logs.

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
*   **Implementation:** Extract `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens` from the `rig-core` span attributes.

### 3. `payloads` (Layer 3)
*   **What:** The detailed protocol data structure.
*   **Content:** Pretty-printed JSON of the *actual* request and response bodies sent to the provider API.
*   **Crucial Step (Double-Decoding):** The `rig-core` library serializes the message history into a string (e.g., `"[{{\"role\":...}}]"`) before logging it. This mode must detect these strings, parse them into real JSON objects, and then print the result.
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
    }
    ```

### 2. New Inspection Module (`src/tracing_inspect.rs`)
*   Create a new module to keep `tracing_setup.rs` clean.
*   Implement a `struct InspectionLayer` that implements `tracing_subscriber::Layer`.
*   **Logic:**
    *   **Filter:** In `on_new_span` or `on_event`, ignore spans/events that do not match the `rig::providers` target or lack `gen_ai` fields.
    *   **Extraction:** In `on_close` (when the span finishes and has all data), extract the fields `gen_ai.input.messages` and `gen_ai.output.messages`.
    *   **Parsing:** Use `serde_json` to parse the double-encoded strings.
    *   **Rendering:** Switch on the `InspectMode` (passed to the layer's constructor) to decide how to print to stdout.

### 3. Integration (`src/tracing_setup.rs`)
*   Modify the `init` function to accept the new `inspect` option.
*   **Switching Logic:**
    *   **IF** `inspect` is set: Do **not** install the default `fmt::layer()` (Layer 1 stdout logger). Instead, install the `InspectionLayer`.
    *   **ELSE**: Install the default human-friendly or JSON logger as before.
    *   **ALWAYS**: Keep the file logger (Layer 1 JSON to `~/.microfactory/logs`) active. This ensures that even if the user is just "inspecting" messages, the full debug trace is still saved to disk for later analysis.

---

## ⚠️ Status Note
**TODO: Do NOT implement this plan yet.** 
This plan requires refinement. The architect has additional ideas to incorporate (e.g., regarding CLI flag structure or output formatting) but deferred them due to time constraints. Wait for final approval before coding.