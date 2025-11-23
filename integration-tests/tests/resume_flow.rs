use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;
use microfactory::{
    adapters::{
        outbound::{
            clock::SystemClock, filesystem::StdFileSystem, telemetry::TracingTelemetrySink,
        },
        templating::HandlebarsRenderer,
    },
    config::MicrofactoryConfig,
    context::Context,
    core::ports::{Clock, FileSystem, LlmClient, LlmOptions, TelemetrySink},
    runner::{FlowRunner, RunnerOptions, RunnerOutcome},
};

struct ScriptedLlm {
    batches: Mutex<VecDeque<Vec<String>>>,
}

impl ScriptedLlm {
    fn new(script: Vec<Vec<String>>) -> Self {
        Self {
            batches: Mutex::new(script.into_iter().collect()),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat_completion(
        &self,
        _model: &str,
        _prompt: &str,
        _options: &LlmOptions,
    ) -> microfactory::core::Result<String> {
        let mut guard = self.batches.lock().unwrap();
        let current_batch = guard.front_mut().ok_or_else(|| {
            microfactory::core::error::Error::System("No scripted responses left".into())
        })?;

        if current_batch.is_empty() {
            guard.pop_front();
            if let Some(next) = guard.front_mut() {
                return Ok(next.remove(0));
            } else {
                return Err(microfactory::core::error::Error::System(
                    "No scripted responses left".into(),
                ));
            }
        }

        Ok(current_batch.remove(0))
    }
}

#[tokio::test]
async fn runner_pauses_and_resumes_after_low_margin_vote() -> Result<()> {
    let yaml = r#"
domains:
  mini:
    agents:
      decomposition:
        prompt_template: "Decompose: {{task}}"
        model: "mock-decompose"
        samples: 1
      decomposition_discriminator:
        prompt_template: "Vote: {{task}}"
        model: "mock-decompose-vote"
        samples: 2
        k: 1
      solver:
        prompt_template: "Solve: {{task}}"
        model: "mock-solve"
        samples: 2
      solution_discriminator:
        prompt_template: "Decide: {{task}}"
        model: "mock-solution-vote"
        samples: 2
        k: 2
"#;

    let config = Arc::new(MicrofactoryConfig::from_yaml_str(yaml)?);
    let llm: Arc<dyn LlmClient> = Arc::new(ScriptedLlm::new(vec![
        vec!["- Draft patch".into()],                   // decomposition
        vec!["1".into(), "1".into()],                   // decomposition vote
        vec!["Solution A".into(), "Solution B".into()], // solver first pass
        vec!["1".into(), "2".into()],                   // low-margin solution vote (pause)
        vec!["Solution A refined".into(), "Solution A++".into()], // solver retry
        vec!["1".into(), "1".into()],                   // decisive vote
    ]));

    let options = RunnerOptions {
        default_samples: 2,
        default_k: 2,
        adaptive_k: false,
        max_decomposition_depth: 1,
        min_words_for_decomposition: usize::MAX,
        human_red_flag_threshold: usize::MAX,
        human_resample_threshold: usize::MAX,
        human_low_margin_threshold: 1,
        step_by_step: false,
    };

    let renderer = Arc::new(HandlebarsRenderer::new());
    let file_system: Arc<dyn FileSystem> = Arc::new(StdFileSystem::new());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let telemetry: Arc<dyn TelemetrySink> = Arc::new(TracingTelemetrySink::new());
    let runner = FlowRunner::new(
        config,
        Some(llm),
        renderer,
        options,
        file_system,
        clock,
        telemetry,
    );
    let mut ctx = Context::new("Patch flaky test", "mini");

    let outcome = runner.execute(&mut ctx).await?;
    match outcome {
        RunnerOutcome::Paused(wait) => {
            assert!(wait.trigger.contains("low_margin"));
        }
        other => panic!("expected pause, got {other:?}"),
    }

    assert!(ctx.wait_state.is_some(), "wait state stored for resume");

    ctx.clear_wait_state();
    let resumed = runner.execute(&mut ctx).await?;
    assert!(matches!(resumed, RunnerOutcome::Completed));

    let root = ctx.root_step_id().expect("root step exists");
    let children = ctx.step(root).unwrap().children.clone();
    assert_eq!(children.len(), 1, "one child step tracked");
    let child = ctx.step(children[0]).unwrap();
    assert_eq!(
        child.winning_solution.as_deref(),
        Some("Solution A refined")
    );

    Ok(())
}
