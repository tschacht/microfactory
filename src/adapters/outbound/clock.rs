use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::ports::Clock;

#[derive(Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }
}
