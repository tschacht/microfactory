use std::collections::HashMap;

use tracing::event;

use crate::core::ports::TelemetrySink;

#[derive(Debug, Default)]
pub struct TracingTelemetrySink;

impl TracingTelemetrySink {
    pub fn new() -> Self {
        Self
    }
}

impl TelemetrySink for TracingTelemetrySink {
    fn record_event(&self, event_name: &str, properties: HashMap<String, String>) {
        event!(target: "microfactory::telemetry", tracing::Level::INFO, %event_name, props = ?properties);
    }
}
