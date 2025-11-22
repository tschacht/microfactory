use std::io::Write;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::cli::InspectMode;
use crate::utils::extract_xml_files;

/// A tracing layer that inspects LLM spans and prints structured data to stdout.
pub struct InspectionLayer<W: Write> {
    mode: InspectMode,
    writer: std::sync::Mutex<W>,
}

impl<W: Write> InspectionLayer<W> {
    pub fn new(mode: InspectMode, writer: W) -> Self {
        Self {
            mode,
            writer: std::sync::Mutex::new(writer),
        }
    }

    fn write(&self, msg: String) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{msg}");
        }
    }
}

impl<S, W> Layer<S> for InspectionLayer<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: Write + 'static,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // We only care about span completion data, not standalone events.
        // However, we could capture `reqwest::connect` events here if needed for ops,
        // but the plan focuses on `on_close` for spans.
        let _ = event;
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = match ctx.span(&id) {
            Some(s) => s,
            None => return,
        };

        // We are interested in `rig` provider spans which contain `gen_ai.*` fields.
        // These are usually populated by `tracing` instrumentation in libraries.
        // Since we can't easily access the span's fields directly in `on_close` without a visitor extension,
        // we rely on the fact that `tracing-subscriber`'s `Registry` usually stores extensions.
        //
        // However, `rig-core` (and `tracing`) doesn't automatically expose field values to `on_close`
        // unless we recorded them. The standard way to get field values in a Layer is via `on_new_span`
        // or `on_record`. But `gen_ai.usage.*` and `gen_ai.output.*` are often added at the *end*
        // of the span via `record`.
        //
        // To handle this properly, we need a Visitor that accumulates fields into the Span's extensions.
        // For simplicity in this implementation, we will assume a separate mechanism (like a `JsonStorageLayer` equivalent)
        // or a custom Visitor has populated a `HashMap` in the extensions, OR we accept that we might need
        // to implement `on_record` to catch these values.
        //
        // WAIT: `tracing-subscriber` doesn't persist fields by default. We need to implement `on_new_span`
        // and `on_record` to store relevant fields in a struct within the Span's extensions.

        let ext = span.extensions();
        let data = match ext.get::<SpanData>() {
            Some(d) => d,
            None => return, // Not a span we tracked
        };

        // Filter: only process spans that look like LLM calls (have provider/model info)
        if data.provider.is_none() && data.model.is_none() {
            return;
        }

        match self.mode {
            InspectMode::Ops => self.inspect_ops(data, &span),
            InspectMode::Payloads => self.inspect_payloads(data),
            InspectMode::Messages => self.inspect_messages(data),
            InspectMode::Files => self.inspect_files(data, &span),
        }
    }

    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found");
        let mut data = SpanData::default();

        // Use a visitor to extract initial fields
        let mut visitor = SpanVisitor(&mut data);
        attrs.record(&mut visitor);

        // Store start time for duration calculation
        data.start_time = Some(std::time::Instant::now());

        span.extensions_mut().insert(data);
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found");
        let mut ext = span.extensions_mut();
        if let Some(data) = ext.get_mut::<SpanData>() {
            let mut visitor = SpanVisitor(data);
            values.record(&mut visitor);
        }
    }
}

// -- Data Structures for Span Storage --

#[derive(Debug, Default)]
struct SpanData {
    start_time: Option<std::time::Instant>,
    provider: Option<String>,
    model: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    input_messages: Option<String>,  // Serialized JSON string
    output_messages: Option<String>, // Serialized JSON string
}

struct SpanVisitor<'a>(&'a mut SpanData);

impl<'a> tracing::field::Visit for SpanVisitor<'a> {
    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "gen_ai.provider.name" => self.0.provider = Some(value.to_string()),
            "gen_ai.request.model" => self.0.model = Some(value.to_string()),
            "gen_ai.input.messages" => self.0.input_messages = Some(value.to_string()),
            "gen_ai.output.messages" => self.0.output_messages = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "gen_ai.usage.input_tokens" => self.0.input_tokens = Some(value),
            "gen_ai.usage.output_tokens" => self.0.output_tokens = Some(value),
            _ => {}
        }
    }
}

// -- Inspection Logic --

impl<W: Write> InspectionLayer<W> {
    fn inspect_ops<'a, S>(
        &self,
        data: &SpanData,
        span: &tracing_subscriber::registry::SpanRef<'a, S>,
    ) where
        S: LookupSpan<'a>,
    {
        let duration = data
            .start_time
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);

        let provider = data.provider.as_deref().unwrap_or("?");
        let model = data.model.as_deref().unwrap_or("?");
        let in_tok = data.input_tokens.unwrap_or(0);
        let out_tok = data.output_tokens.unwrap_or(0);

        // Unique span ID fragment
        let span_id = format!("{:x}", span.id().into_u64());
        let short_id = &span_id[0..std::cmp::min(span_id.len(), 6)];

        let msg = format!(
            "[LLM] {model} ({provider}) | In: {in_tok} tok | Out: {out_tok} tok | {duration}ms | span={short_id}"
        );
        self.write(msg);
    }

    fn inspect_payloads(&self, data: &SpanData) {
        // Input
        if let Some(raw_input) = &data.input_messages {
            match decode_and_redact(raw_input) {
                Ok(json) => self.write(format!("Request:\n{json:#}")),
                Err(_) => self.write(format!("Request (raw):\n{raw_input}")),
            }
        }

        // Output
        if let Some(raw_output) = &data.output_messages {
            match decode_and_redact(raw_output) {
                Ok(json) => self.write(format!("Response:\n{json:#}")),
                Err(_) => self.write(format!("Response (raw):\n{raw_output}")),
            }
        }
    }

    fn inspect_messages(&self, data: &SpanData) {
        if let Some(raw_input) = &data.input_messages {
            self.write_messages(raw_input, "Request");
        }
        if let Some(raw_output) = &data.output_messages {
            self.write_messages(raw_output, "Response");
        }
    }

    fn write_messages(&self, raw: &str, label: &str) {
        let json_val = match parse_double_encoded(raw) {
            Ok(v) => v,
            Err(_) => return, // Skip if not parsable
        };

        // Assume it's a list of messages: [{"role": "...", "content": "..."}]
        if let Some(arr) = json_val.as_array() {
            for msg in arr {
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("?");
                let content = msg.get("content");

                self.write(format!(
                    "─── [{role}] ({label}) ─────────────────────────────────────────────────────"
                ));

                if let Some(c_str) = content.and_then(|c| c.as_str()) {
                    self.write(c_str.to_string());
                } else if let Some(c_arr) = content.and_then(|c| c.as_array()) {
                    // Handle multi-part content (text + image/tool)
                    for part in c_arr {
                        self.write(format!("{part:?}"));
                    }
                } else {
                    self.write(format!("{content:?}"));
                }
                self.write("\n".to_string());
            }
        }
    }

    fn inspect_files<'a, S>(
        &self,
        data: &SpanData,
        span: &tracing_subscriber::registry::SpanRef<'a, S>,
    ) where
        S: LookupSpan<'a>,
    {
        // Only interested in assistant output
        let raw_output = match &data.output_messages {
            Some(s) => s,
            None => return,
        };

        let json_val = match parse_double_encoded(raw_output) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Find assistant content
        let mut content_buffer = String::new();
        if let Some(arr) = json_val.as_array() {
            for msg in arr {
                if msg.get("role").and_then(|r| r.as_str()) == Some("assistant")
                    && let Some(c) = msg.get("content").and_then(|c| c.as_str())
                {
                    content_buffer.push_str(c);
                }
            }
        }

        let files = extract_xml_files(&content_buffer);
        if files.is_empty() {
            return;
        }

        let span_id = format!("{:x}", span.id().into_u64());
        let short_id = &span_id[0..std::cmp::min(span_id.len(), 6)];

        self.write(format!("[FILES span={short_id}]"));
        for (path, content) in files {
            let line_count = content.lines().count();
            let preview = content.lines().next().unwrap_or("").trim();
            self.write(format!("- {path} ({line_count} lines)"));
            if !preview.is_empty() {
                self.write(format!("  Preview: {preview}"));
            }
        }
    }
}

// -- Helpers --

fn parse_double_encoded(raw: &str) -> Result<serde_json::Value, serde_json::Error> {
    // 1. Decode the outer JSON string (which rig-core emits)
    // Actually, rig-core might emit a JSON object that has been serialized to a string.
    // Let's try to parse the string as JSON.
    let val: serde_json::Value = serde_json::from_str(raw)?;

    // If it's a string, it might be double-encoded?
    // Usually `gen_ai.input.messages` is recorded as a String representation of the JSON array.
    // So `serde_json::from_str` should be enough to get the Value.

    // However, we also need to unescape HTML if present in text fields.
    // We'll do a recursive walk to unescape strings.
    Ok(unescape_html_recursive(val))
}

fn decode_and_redact(raw: &str) -> Result<serde_json::Value, serde_json::Error> {
    let mut val = parse_double_encoded(raw)?;
    redact_recursive(&mut val);
    Ok(val)
}

fn unescape_html_recursive(mut val: serde_json::Value) -> serde_json::Value {
    match &mut val {
        serde_json::Value::String(s) => {
            // Basic HTML unescape ( < > & " ' )
            let new_s = s
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&#x3D;", "=");
            *s = new_s;
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                *v = unescape_html_recursive(v.take());
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map {
                *v = unescape_html_recursive(v.take());
            }
        }
        _ => {}
    }
    val
}

fn redact_recursive(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::String(s) => {
            if s.len() > 200 && (s.contains("sk-") || s.contains("xai-")) {
                *s = "[redacted_key]".to_string();
            }
            // Simple truncation for huge blobs (optional, based on plan)
            if s.len() > 4000 {
                s.truncate(4000);
                s.push_str("... [truncated]");
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                redact_recursive(v);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k.contains("api_key") || k.contains("token") || k.contains("auth") {
                    *v = serde_json::Value::String("[redacted]".to_string());
                } else {
                    redact_recursive(v);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_double_encoded_with_html_entities() {
        // Simulates what rig-core might produce: a JSON string containing an array of messages,
        // where one message content has HTML entities (e.g. from a solution discriminator prompt context).
        let inner_text = "Option 2:\\n&lt;file path&#x3D;&quot;src/main.rs&quot;&gt;\\nfn main() {}&lt;/file&gt;";
        let messages = json!([
            {
                "role": "user",
                "content": inner_text
            }
        ]);
        let raw = messages.to_string(); // "double-encoded" as a string

        let parsed = parse_double_encoded(&raw).expect("should parse");

        // Navigate to the content
        let content = parsed[0]["content"].as_str().expect("content is string");

        // Verify unescaping
        assert!(content.contains("<file path=\"src/main.rs\">"));
        assert!(content.contains("fn main() {}</file>"));
        assert!(!content.contains("&lt;"));
        assert!(!content.contains("&#x3D;"));
    }

    #[test]
    fn test_redaction_removes_keys() {
        let mut data = json!({
            "api_key": "secret-123",
            "nested": {
                "bearer_token": "secret-abc",
                "safe": "value"
            }
        });

        redact_recursive(&mut data);

        assert_eq!(data["api_key"], "[redacted]");
        assert_eq!(data["nested"]["bearer_token"], "[redacted]");
        assert_eq!(data["nested"]["safe"], "value");
    }

    #[test]
    fn test_redaction_removes_sensitive_strings() {
        let secret = "sk-abcdef1234567890".repeat(20); // > 200 chars
        let mut data = json!({
            "content": secret
        });

        redact_recursive(&mut data);

        assert_eq!(data["content"], "[redacted_key]");
    }

    #[test]
    fn test_truncation_for_huge_blobs() {
        let huge = "a".repeat(5000);
        let mut data = json!({
            "blob": huge
        });

        redact_recursive(&mut data);

        let s = data["blob"].as_str().unwrap();
        assert!(s.len() < 5000);
        assert!(s.ends_with("... [truncated]"));
    }
}
