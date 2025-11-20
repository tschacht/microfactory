use std::fs;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, filter::Targets, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

use microfactory::paths;

#[derive(Clone, Copy, Debug)]
pub enum JsonLogFormat {
    Pretty,
    Compact,
}

/// Initializes the tracing subscriber with layered output:
/// 1. Stdout: Formatted based on `log_json` and `verbose` flags.
/// 2. File: Full JSON debug logs to `~/.microfactory/logs/session-<id>.log` (if session_id provided).
///
/// Returns a WorkerGuard that must be held by main() to ensure file logs are flushed.
pub fn init(
    verbose: bool,
    log_json: bool,
    json_format: JsonLogFormat,
    session_id: Option<&str>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let registry = tracing_subscriber::registry();
    let stdout_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| default_env_filter(verbose));

    // --- 1. File Layer (Always JSON, Debug, Non-blocking) ---
    let (file_layer, guard) = if let Some(id) = session_id {
        let log_dir = paths::data_dir().join("logs");
        if let Err(e) = fs::create_dir_all(&log_dir) {
            eprintln!("Warning: Failed to create log dir {:?}: {}", log_dir, e);
            (None, None)
        } else {
            let file_name = format!("session-{}.log", id);
            // Use rolling::never because we want one file per session ID
            let file_appender = tracing_appender::rolling::never(&log_dir, &file_name);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            let layer = fmt::layer()
                .json()
                .with_writer(non_blocking)
                // Capture everything (DEBUG) in the file
                .with_filter(Targets::new().with_default(tracing::Level::DEBUG));

            (Some(layer), Some(guard))
        }
    } else {
        (None, None)
    };

    // --- 2. Stdout Layer ---
    // We use Box<dyn Layer<Registry> + Send + Sync> to erase the type differences
    let stdout_layer: Box<dyn Layer<Registry> + Send + Sync> = if log_json {
        match json_format {
            JsonLogFormat::Pretty => Box::new(
                fmt::layer()
                    .json()
                    .with_writer(|| PrettyJsonWriter::new(std::io::stdout()))
                    .with_filter(stdout_filter.clone()),
            ),
            JsonLogFormat::Compact => Box::new(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stdout)
                    .with_filter(stdout_filter.clone()),
            ),
        }
    } else if verbose {
        Box::new(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_filter(stdout_filter.clone()),
        )
    } else {
        Box::new(
            fmt::layer()
                .with_writer(std::io::stdout)
                .without_time()
                .with_target(false)
                .with_level(true)
                .with_filter(stdout_filter.clone()),
        )
    };

    registry.with(stdout_layer).with(file_layer).init();

    guard
}

struct PrettyJsonWriter<W: std::io::Write> {
    inner: W,
}

impl<W: std::io::Write> PrettyJsonWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: std::io::Write> std::io::Write for PrettyJsonWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Try to parse the buffer as JSON to pretty-print it.
        // Tracing JSON formatter usually emits one line per event.
        if let Ok(s) = std::str::from_utf8(buf) {
            let trimmed = s.trim();
            if trimmed.starts_with('{')
                && trimmed.ends_with('}')
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            {
                // We found valid JSON. Pretty print it.
                if serde_json::to_writer_pretty(&mut self.inner, &val).is_ok() {
                    // Add a newline since to_writer_pretty doesn't add one at the end usually,
                    // but we want distinct records.
                    let _ = self.inner.write(b"\n");
                    return Ok(buf.len());
                }
            }
        }
        // Fallback: write raw
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn default_env_filter(verbose: bool) -> EnvFilter {
    let spec = if verbose {
        "microfactory=debug,rig_core=warn,info"
    } else {
        "microfactory=info,rig_core=warn,warn"
    };
    EnvFilter::new(spec)
}
