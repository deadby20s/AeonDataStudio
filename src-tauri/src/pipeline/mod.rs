//! Cleaning pipelines: compiling a recipe, processing documents, and collecting statistics.
//!
//! Steps run per document, so a batch of documents is processed in parallel with rayon.
//! A document stops at the first step that drops it; that step and rule are recorded.

mod preview;
mod removed;
mod run;
pub mod steps;
mod storage;

use std::collections::HashMap;

use aeon_shared::{PipelineSpec, RuleCount, StepStats};

use crate::error::{AppError, AppResult};
use steps::{Step, Verdict};

pub use preview::preview;
pub use removed::RemovedReader;
pub use run::{RunRequest, read_report, run};
pub use storage::{delete_pipeline, list_pipelines, read_pipeline, save_pipeline};

/// Checks a recipe without running it, so mistakes show up before a job starts.
pub fn steps_check(spec: &PipelineSpec) -> AppResult<()> {
    Pipeline::compile(spec).map(|_| ())
}

/// Prefix of run folders still being written.
pub const RUN_TEMP_PREFIX: &str = ".running-";

/// Rule recorded when a normalizer leaves a document with no visible text.
pub const EMPTY_AFTER_STEP: &str = "empty_after_step";

pub struct CompiledStep {
    /// Index in the recipe's step list (disabled steps included), as shown to the user.
    pub index: u64,
    pub kind: &'static str,
    pub label: String,
    step: Box<dyn Step>,
}

pub struct Pipeline {
    pub name: String,
    pub steps: Vec<CompiledStep>,
}

pub struct Dropped {
    /// Position in `Pipeline::steps`.
    pub pos: usize,
    pub rule: &'static str,
    pub detail: String,
}

pub struct Outcome {
    /// Final text, or the text as it was when the document was dropped.
    pub text: String,
    pub dropped: Option<Dropped>,
    /// Bit `i` is set when step `i` changed the text.
    pub modified: u64,
    pub chars_in: u64,
    /// Characters after each step the document passed.
    pub chars_after: Vec<u64>,
}

impl Pipeline {
    pub fn compile(spec: &PipelineSpec) -> AppResult<Self> {
        if spec.name.trim().is_empty() {
            return Err(AppError::InvalidInput("Give the pipeline a name".into()));
        }
        let mut steps = Vec::new();
        for (index, s) in spec.steps.iter().enumerate().filter(|(_, s)| s.enabled) {
            let info = s.config.info();
            let step = steps::build(&s.config)
                .map_err(|e| AppError::InvalidInput(format!("Step {} ({}): {e}", index + 1, info.label)))?;
            steps.push(CompiledStep {
                index: index as u64,
                kind: info.kind,
                label: info.label.to_string(),
                step,
            });
        }
        if steps.is_empty() {
            return Err(AppError::InvalidInput("The pipeline has no enabled steps".into()));
        }
        if steps.len() > 64 {
            return Err(AppError::InvalidInput("A pipeline can have at most 64 enabled steps".into()));
        }
        Ok(Self { name: spec.name.trim().to_string(), steps })
    }

    pub fn process(&self, mut text: String) -> Outcome {
        let chars_in = text.chars().count() as u64;
        let mut chars = chars_in;
        let mut modified = 0u64;
        let mut chars_after = Vec::with_capacity(self.steps.len());
        for (pos, s) in self.steps.iter().enumerate() {
            match s.step.apply(&mut text) {
                Verdict::Unchanged => {}
                Verdict::Changed => {
                    modified |= 1 << pos;
                    chars = text.chars().count() as u64;
                    if text.trim().is_empty() {
                        let dropped = Dropped { pos, rule: EMPTY_AFTER_STEP, detail: "no text left".into() };
                        return Outcome { text, dropped: Some(dropped), modified, chars_in, chars_after };
                    }
                }
                Verdict::Drop { rule, detail } => {
                    return Outcome { text, dropped: Some(Dropped { pos, rule, detail }), modified, chars_in, chars_after };
                }
            }
            chars_after.push(chars);
        }
        Outcome { text, dropped: None, modified, chars_in, chars_after }
    }
}

#[derive(Default)]
struct StepAcc {
    docs_in: u64,
    dropped: u64,
    modified: u64,
    chars_in: u64,
    chars_out: u64,
    rules: HashMap<&'static str, u64>,
}

/// Aggregates outcomes into per-step statistics.
pub struct StatsAccumulator {
    steps: Vec<StepAcc>,
    pub docs_in: u64,
    pub docs_out: u64,
    pub chars_in: u64,
    pub chars_out: u64,
}

impl StatsAccumulator {
    pub fn new(pipeline: &Pipeline) -> Self {
        Self {
            steps: pipeline.steps.iter().map(|_| StepAcc::default()).collect(),
            docs_in: 0,
            docs_out: 0,
            chars_in: 0,
            chars_out: 0,
        }
    }

    pub fn add(&mut self, o: &Outcome) {
        self.docs_in += 1;
        self.chars_in += o.chars_in;
        let reached = o.dropped.as_ref().map_or(self.steps.len(), |d| d.pos + 1);
        let mut prev = o.chars_in;
        for (pos, acc) in self.steps.iter_mut().enumerate().take(reached) {
            acc.docs_in += 1;
            acc.chars_in += prev;
            if o.modified & (1 << pos) != 0 {
                acc.modified += 1;
            }
            match &o.dropped {
                Some(d) if d.pos == pos => {
                    acc.dropped += 1;
                    *acc.rules.entry(d.rule).or_default() += 1;
                }
                _ => {
                    prev = o.chars_after[pos];
                    acc.chars_out += prev;
                }
            }
        }
        if o.dropped.is_none() {
            self.docs_out += 1;
            self.chars_out += prev;
        }
    }

    pub fn step_stats(&self, pipeline: &Pipeline) -> Vec<StepStats> {
        self.steps
            .iter()
            .zip(&pipeline.steps)
            .map(|(acc, s)| {
                let mut drops_by_rule: Vec<RuleCount> =
                    acc.rules.iter().map(|(rule, count)| RuleCount { rule: rule.to_string(), count: *count }).collect();
                drops_by_rule.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.rule.cmp(&b.rule)));
                StepStats {
                    index: s.index,
                    kind: s.kind.to_string(),
                    label: s.label.clone(),
                    docs_in: acc.docs_in,
                    dropped: acc.dropped,
                    modified: acc.modified,
                    chars_in: acc.chars_in,
                    chars_out: acc.chars_out,
                    drops_by_rule,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aeon_shared::{LengthFilter, NormalizeWhitespace, PipelineStep, StepConfig};

    fn spec() -> PipelineSpec {
        PipelineSpec {
            name: "t".into(),
            description: None,
            steps: vec![
                PipelineStep { enabled: true, config: StepConfig::NormalizeWhitespace(NormalizeWhitespace::default()) },
                PipelineStep {
                    enabled: false,
                    config: StepConfig::LengthFilter(LengthFilter { min_chars: Some(1000), ..Default::default() }),
                },
                PipelineStep {
                    enabled: true,
                    config: StepConfig::LengthFilter(LengthFilter { min_words: Some(3), ..Default::default() }),
                },
            ],
        }
    }

    #[test]
    fn disabled_steps_are_skipped_but_keep_their_index() {
        let p = Pipeline::compile(&spec()).unwrap();
        assert_eq!(p.steps.iter().map(|s| s.index).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn stats_follow_documents_through_steps() {
        let p = Pipeline::compile(&spec()).unwrap();
        let mut acc = StatsAccumulator::new(&p);
        for text in ["  three  words here  ", "two words", "   \n  ", "already fine text"] {
            acc.add(&p.process(text.to_string()));
        }
        let s = acc.step_stats(&p);
        assert_eq!((acc.docs_in, acc.docs_out), (4, 2));
        // Whitespace step: all 4 enter, 2 are changed, the blank one is emptied and dropped.
        assert_eq!((s[0].docs_in, s[0].modified, s[0].dropped), (4, 2, 1));
        assert_eq!(s[0].drops_by_rule[0].rule, EMPTY_AFTER_STEP);
        // Length step: 3 enter, "two words" is dropped.
        assert_eq!((s[1].docs_in, s[1].dropped), (3, 1));
        assert_eq!(s[1].drops_by_rule[0].rule, "min_words");
        assert_eq!(acc.chars_out, "three words here".len() as u64 + "already fine text".len() as u64);
    }

    /// Throughput of each template step. Run with:
    /// `cargo test --release -p aeondatastudio bench_steps -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_steps() {
        use rayon::prelude::*;
        let words = "the of and to in is that for it with as on was by be are this from data model language \
                     training quality corpus filter document science history news page"
            .split(' ')
            .collect::<Vec<_>>();
        let docs: Vec<String> = (0..50_000u64)
            .map(|i| {
                let n = 40 + (i * 7919 % 200) as usize;
                let mut s = String::new();
                for j in 0..n {
                    s.push_str(words[(i as usize * 31 + j * 17 + j * j) % words.len()]);
                    s.push(if j % 15 == 14 { '.' } else { ' ' });
                    if j % 60 == 59 {
                        s.push_str("\n\n");
                    }
                }
                s
            })
            .collect();
        let chars: usize = docs.iter().map(|d| d.len()).sum();
        let spec = PipelineSpec::template("bench");
        for (i, step) in spec.steps.iter().enumerate() {
            let single = PipelineSpec { name: "b".into(), description: None, steps: vec![step.clone()] };
            let p = Pipeline::compile(&single).unwrap();
            let t = std::time::Instant::now();
            let _: Vec<_> = docs.par_iter().map(|d| p.process(d.clone())).collect();
            let secs = t.elapsed().as_secs_f64();
            println!("{:>2}. {:<24} {:>8.0} docs/s {:>7.1} MB/s", i + 1, p.steps[0].label, docs.len() as f64 / secs, chars as f64 / secs / 1e6);
        }
        let p = Pipeline::compile(&spec).unwrap();
        let t = std::time::Instant::now();
        let _: Vec<_> = docs.par_iter().map(|d| p.process(d.clone())).collect();
        let secs = t.elapsed().as_secs_f64();
        println!("    whole template           {:>8.0} docs/s {:>7.1} MB/s", docs.len() as f64 / secs, chars as f64 / secs / 1e6);
    }

    #[test]
    fn empty_or_unnamed_pipelines_are_rejected() {
        let mut s = spec();
        s.steps.iter_mut().for_each(|st| st.enabled = false);
        assert!(Pipeline::compile(&s).is_err());
        let mut s = spec();
        s.name = " ".into();
        assert!(Pipeline::compile(&s).is_err());
    }
}
