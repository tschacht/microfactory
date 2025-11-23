use handlebars::Handlebars;
use serde_json::Value;
use std::sync::Arc;

use crate::core::error::Error as CoreError;
use crate::core::ports::PromptRenderer;

#[derive(Clone)]
pub struct HandlebarsRenderer {
    engine: Arc<Handlebars<'static>>,
}

impl Default for HandlebarsRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl HandlebarsRenderer {
    pub fn new() -> Self {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(false);
        Self {
            engine: Arc::new(handlebars),
        }
    }
}

impl PromptRenderer for HandlebarsRenderer {
    fn render(&self, template: &str, data: &Value) -> crate::core::Result<String> {
        self.engine
            .render_template(template, data)
            .map_err(|e| CoreError::TemplateRendering(e.to_string()))
    }
}
