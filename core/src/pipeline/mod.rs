//! Cleaning pipelines: compiling a recipe, processing documents, and collecting statistics.
//!
//! Most steps work on one document at a time, so batches are processed in parallel with rayon.
//! A document stops at the first step that drops it; that step and rule are recorded.
//!
//! Dedup steps need to see the whole corpus. Before the final pass, a run makes one extra pass
//! per dedup step: it processes documents up to that step, fingerprints the ones that reach it,
//! and resolves which are duplicates ([`Decisions`]). The final pass then drops them at that step.

mod dedup;
mod kept;
mod preview;
mod removed;
mod run;
pub mod steps;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;

use aeon_shared::{PipelineSpec, RuleCount, StepConfig, StepStats};

use crate::error::{AppError, AppResult};
use dedup::{DedupMethod, Duplicates};
use steps::{Step, Verdict};

pub use kept::{browse_changed, sample_removed};
pub use preview::preview;
pub use removed::RemovedReader;
pub use run::{RunRequest, read_report, run};
pub use storage::{delete_pipeline, list_pipelines, read_pipeline, save_pipeline};

/// Set by a step that calls something outside the app (plugin, language model) when it fails
/// for good. Steps then stop calling out, and the run or preview stops with this error after
/// the current batch, instead of every worker thread failing on its own.
#[derive(Debug, Default)]
pub struct Abort(std::sync::Mutex<Option<String>>);

impl Abort {
    pub fn fail(&self, message: String) {
        if let Ok(mut m) = self.0.lock()
            && m.is_none()
        {
            log::error!("{message}");
            *m = Some(message);
        }
    }

    pub fn is_set(&self) -> bool {
        self.0.lock().map(|m| m.is_some()).unwrap_or(true)
    }

    pub fn error(&self) -> Option<String> {
        self.0.lock().ok().and_then(|m| m.clone())
    }
}

/// What steps need from outside the recipe.
#[derive(Debug, Clone, Default)]
pub struct StepEnv {
    /// The workspace's `plugins/` folder.
    pub plugins_dir: Option<PathBuf>,
    /// Where language-model answers are saved for reuse (`cache/llm.sqlite`).
    pub llm_cache: Option<PathBuf>,
}

/// Checks a recipe without running it, so mistakes show up before a job starts. Plugins are
/// started once to check they answer.
pub fn steps_check(spec: &PipelineSpec, env: &StepEnv) -> AppResult<()> {
    let p = Pipeline::compile_with(spec, env)?;
    for s in &p.steps {
        if let StepImpl::Plugin(runner) = &s.imp {
            runner.check()?;
        }
    }
    Ok(())
}

/// Prefix of run folders still being written.
pub const RUN_TEMP_PREFIX: &str = ".running-";

/// Rule recorded when a normalizer leaves a document with no visible text.
pub const EMPTY_AFTER_STEP: &str = "empty_after_step";

enum StepImpl {
    Doc(Box<dyn Step>),
    Plugin(crate::plugins::PluginRunner),
    Dedup(DedupMethod),
}

pub struct CompiledStep {
    /// Index in the recipe's step list (disabled steps included), as shown to the user.
    pub index: u64,
    pub kind: &'static str,
    pub label: String,
    imp: StepImpl,
}

pub struct Pipeline {
    pub name: String,
    pub steps: Vec<CompiledStep>,
    /// Some step sends documents to a language model.
    pub uses_llm: bool,
    pub abort: std::sync::Arc<Abort>,
}

/// Resolved duplicates for each dedup step, by step position.
#[derive(Default)]
pub struct Decisions {
    by_pos: HashMap<usize, Duplicates>,
}

impl Decisions {
    pub fn set(&mut self, pos: usize, dups: Duplicates) {
        self.by_pos.insert(pos, dups);
    }
}

pub struct Dropped {
    /// Position in `Pipeline::steps`.
    pub pos: usize,
    pub rule: &'static str,
    pub detail: String,
    /// For duplicates: the input row of the document kept instead.
    pub related_row: Option<u64>,
}

pub struct Outcome {
    /// Final text, or the text as it was when the document was dropped (or reached the stop step).
    pub text: String,
    pub dropped: Option<Dropped>,
    /// The document got to the requested stop step (fingerprint passes).
    pub reached_stop: bool,
    /// Bit `i` is set when step `i` changed the text.
    pub modified: u64,
    pub chars_in: u64,
    /// Characters after each step the document passed.
    pub chars_after: Vec<u64>,
    /// Metadata fields steps added (scores in "tag" mode, plugin tags), in step order.
    pub tags: steps::Tags,
}

impl Pipeline {
    /// Compiles a recipe that uses no plugins.
    pub fn compile(spec: &PipelineSpec) -> AppResult<Self> {
        Self::compile_with(spec, &StepEnv::default())
    }

    pub fn compile_with(spec: &PipelineSpec, env: &StepEnv) -> AppResult<Self> {
        if spec.name.trim().is_empty() {
            return Err(AppError::InvalidInput("Give the pipeline a name".into()));
        }
        // Dedup passes re-run the steps before each dedup step, so a model step placed there
        // would see (and bill for) every document several times.
        let enabled: Vec<_> = spec.steps.iter().enumerate().filter(|(_, s)| s.enabled).collect();
        if let Some((i, llm)) = enabled.iter().find(|(_, s)| s.config.is_llm())
            && let Some((j, dedup)) = enabled.iter().find(|(j, s)| j > i && s.config.info().category == aeon_shared::StepCategory::Dedup)
        {
            return Err(AppError::InvalidInput(format!(
                "Move step {} ({}) after step {} ({}): language model steps must come after deduplication, so each document is sent once and duplicates are not paid for.",
                i + 1,
                llm.config.info().label,
                j + 1,
                dedup.config.info().label
            )));
        }
        let abort = std::sync::Arc::new(Abort::default());
        // One answer cache for all model steps of this pipeline, opened only when one uses it.
        let wants_cache = enabled.iter().any(|(_, s)| match &s.config {
            StepConfig::LlmScore(c) => c.use_cache,
            StepConfig::LlmRewrite(c) => c.use_cache,
            _ => false,
        });
        let cache = match (&env.llm_cache, wants_cache) {
            (Some(path), true) => Some(std::sync::Arc::new(crate::llm::LlmCache::open(path)?)),
            _ => None,
        };
        let mut steps = Vec::new();
        for (index, s) in enabled {
            let info = s.config.info();
            let context = |e: AppError| AppError::InvalidInput(format!("Step {} ({}): {e}", index + 1, info.label));
            let imp = match &s.config {
                StepConfig::ExactDedup(c) => StepImpl::Dedup(DedupMethod::exact(c)),
                StepConfig::NearDedup(c) => StepImpl::Dedup(DedupMethod::near(c).map_err(context)?),
                StepConfig::Plugin(c) => {
                    StepImpl::Plugin(
                        crate::plugins::PluginRunner::new(env.plugins_dir.as_deref(), c, abort.clone()).map_err(context)?,
                    )
                }
                StepConfig::LlmScore(c) => StepImpl::Doc(Box::new(crate::llm::ScoreStep::new(c, abort.clone()).map_err(context)?.with_cache(cache.clone()))),
                StepConfig::LlmRewrite(c) => StepImpl::Doc(Box::new(crate::llm::RewriteStep::new(c, abort.clone()).map_err(context)?.with_cache(cache.clone()))),
                other => StepImpl::Doc(steps::build(other).map_err(context)?),
            };
            steps.push(CompiledStep { index: index as u64, kind: info.kind, label: info.label.to_string(), imp });
        }
        if steps.is_empty() {
            return Err(AppError::InvalidInput("The pipeline has no enabled steps".into()));
        }
        if steps.len() > 64 {
            return Err(AppError::InvalidInput("A pipeline can have at most 64 enabled steps".into()));
        }
        let uses_llm = spec.steps.iter().any(|s| s.enabled && s.config.is_llm());
        Ok(Self { name: spec.name.trim().to_string(), steps, uses_llm, abort })
    }

    /// The error of a plugin or model step that failed for good, if any.
    pub fn check_abort(&self) -> AppResult<()> {
        match self.abort.error() {
            Some(e) => Err(AppError::InvalidInput(e)),
            None => Ok(()),
        }
    }

    /// Positions of dedup steps, in order.
    pub fn dedup_positions(&self) -> Vec<usize> {
        (0..self.steps.len()).filter(|&p| matches!(self.steps[p].imp, StepImpl::Dedup(_))).collect()
    }

    fn dedup(&self, pos: usize) -> &DedupMethod {
        match &self.steps[pos].imp {
            StepImpl::Dedup(d) => d,
            _ => panic!("step {pos} is not a dedup step"),
        }
    }

    pub fn bands(&self, pos: usize) -> usize {
        self.dedup(pos).bands()
    }

    pub fn fingerprint(&self, pos: usize, text: &str) -> Option<Vec<u64>> {
        self.dedup(pos).fingerprint(text)
    }

    /// Applies the per-document step at `pos` (dedup steps do nothing); used to rebuild
    /// before/after examples.
    pub fn apply_doc_step(&self, pos: usize, text: &mut String) {
        match &self.steps[pos].imp {
            StepImpl::Doc(s) => {
                let _ = s.apply(text);
            }
            StepImpl::Plugin(p) => {
                let _ = p.apply(text);
            }
            StepImpl::Dedup(_) => {}
        }
    }

    /// Runs `text` (input row `row`) through the steps. With `stop_at`, stops when the document
    /// reaches that (dedup) step. A dedup step without decisions keeps every document.
    pub fn process(&self, mut text: String, row: u64, decisions: &Decisions, stop_at: Option<usize>) -> Outcome {
        let chars_in = text.chars().count() as u64;
        let mut chars = chars_in;
        let mut modified = 0u64;
        let mut chars_after = Vec::with_capacity(self.steps.len());
        let mut tags = steps::Tags::new();
        let outcome = |text, dropped, reached_stop, modified, chars_after, tags| Outcome {
            text,
            dropped,
            reached_stop,
            modified,
            chars_in,
            chars_after,
            tags,
        };
        for (pos, s) in self.steps.iter().enumerate() {
            if stop_at == Some(pos) {
                return outcome(text, None, true, modified, chars_after, tags);
            }
            let verdict = match &s.imp {
                StepImpl::Doc(step) => step.apply_tagged(&mut text, &mut tags),
                StepImpl::Plugin(p) => p.apply_tagged(&mut text, &mut tags),
                StepImpl::Dedup(method) => match decisions.by_pos.get(&pos).and_then(|d| d.get(&row)) {
                    Some(&kept) => {
                        let dropped = Dropped {
                            pos,
                            rule: method.rule(),
                            detail: format!("duplicate of input row {}", kept + 1),
                            related_row: Some(kept),
                        };
                        return outcome(text, Some(dropped), false, modified, chars_after, tags);
                    }
                    None => Verdict::Unchanged,
                },
            };
            match verdict {
                Verdict::Unchanged => {}
                Verdict::Changed => {
                    modified |= 1 << pos;
                    chars = text.chars().count() as u64;
                    if text.trim().is_empty() {
                        let dropped =
                            Dropped { pos, rule: EMPTY_AFTER_STEP, detail: "no text left".into(), related_row: None };
                        return outcome(text, Some(dropped), false, modified, chars_after, tags);
                    }
                }
                Verdict::Drop { rule, detail } => {
                    let dropped = Dropped { pos, rule, detail, related_row: None };
                    return outcome(text, Some(dropped), false, modified, chars_after, tags);
                }
            }
            chars_after.push(chars);
        }
        outcome(text, None, false, modified, chars_after, tags)
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
    use aeon_shared::{ExactDedup, LengthFilter, NormalizeWhitespace, PipelineStep};

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

    fn process(p: &Pipeline, text: &str) -> Outcome {
        p.process(text.to_string(), 0, &Decisions::default(), None)
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
            acc.add(&process(&p, text));
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

    #[test]
    fn dedup_step_drops_rows_from_decisions_and_stop_at_halts_before_it() {
        let mut s = spec();
        s.steps.push(PipelineStep { enabled: true, config: StepConfig::ExactDedup(ExactDedup::default()) });
        let p = Pipeline::compile(&s).unwrap();
        assert_eq!(p.dedup_positions(), vec![2]);

        // Fingerprint pass: normalization runs, then the document stops at the dedup step.
        let o = p.process("  three   words here ".into(), 0, &Decisions::default(), Some(2));
        assert!(o.reached_stop && o.dropped.is_none());
        assert_eq!(o.text, "three words here");
        // Documents filtered before the dedup step never reach it.
        assert!(!p.process("two words".into(), 1, &Decisions::default(), Some(2)).reached_stop);

        let mut decisions = Decisions::default();
        decisions.set(2, Duplicates::from([(7, 3)]));
        let dup = p.process("some three words".into(), 7, &decisions, None);
        let d = dup.dropped.unwrap();
        assert_eq!((d.pos, d.rule, d.related_row), (2, dedup::EXACT_RULE, Some(3)));
        assert!(p.process("some three words".into(), 3, &decisions, None).dropped.is_none());
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
        let report = |label: &str, secs: f64| {
            println!("{label:<28} {:>8.0} docs/s {:>7.1} MB/s", docs.len() as f64 / secs, chars as f64 / secs / 1e6);
        };
        for (i, step) in spec.steps.iter().enumerate() {
            let single = PipelineSpec { name: "b".into(), description: None, steps: vec![step.clone()] };
            let p = Pipeline::compile(&single).unwrap();
            let t = std::time::Instant::now();
            if p.dedup_positions().is_empty() {
                let _: Vec<_> = docs.par_iter().map(|d| process(&p, d)).collect();
            } else {
                // Dedup cost is the fingerprinting; resolution is a sort.
                let _: Vec<_> = docs.par_iter().map(|d| p.fingerprint(0, d)).collect();
            }
            report(&format!("{:>2}. {}", i + 1, p.steps[0].label), t.elapsed().as_secs_f64());
        }
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
