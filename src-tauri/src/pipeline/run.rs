//! Running a pipeline over a dataset as a background job.
//!
//! ```text
//! runs/<run-id>/
//! ├── report.json        RunReport: frozen recipe, per-step stats, removed/kept parts
//! ├── pipeline.toml      the recipe exactly as run
//! ├── removed/           removed-*.parquet (input_row, id, source, step_index, rule, detail, related_row, text)
//! └── kept/              kept-*.parquet (input_row, modified) — one row per output document
//! ```
//!
//! A recipe with dedup steps takes one extra pass per dedup step (see [`super::dedup`]);
//! progress counts all passes. The run folder and the output dataset are written to temp
//! folders and only moved into place when the run succeeds.

use std::sync::Arc;

use aeon_shared::{DatasetManifest, PipelineSpec, ProducedBy, RunReport};
use rayon::prelude::*;

use super::dedup::BandFiles;
use super::kept::KeptWriter;
use super::removed::{RemovedRow, RemovedWriter};
use super::{Decisions, Pipeline, RUN_TEMP_PREFIX, StatsAccumulator};
use crate::dataset::{self, DatasetReader, DatasetWriter, NewDoc, TempDataset};
use crate::error::{AppError, AppResult, now_ms};
use crate::import::summary_of;
use crate::jobs::{JobCtx, JobOutcome};
use crate::workspace::Workspace;

/// Documents processed in parallel at a time.
const BATCH_DOCS: usize = 4096;

pub struct RunRequest {
    pub spec: PipelineSpec,
    pub input_dataset_id: String,
    pub output_name: String,
}

struct InputDoc {
    row: u64,
    id: String,
    text: String,
    source: String,
    meta: String,
}

pub fn run(ctx: &mut JobCtx, ws: Arc<Workspace>, req: RunRequest) -> AppResult<JobOutcome> {
    let pipeline = Pipeline::compile(&req.spec)?;
    let input_dir = ws.dataset_dir(&req.input_dataset_id)?;
    let reader = DatasetReader::open(&input_dir)?;
    let input_name = reader.manifest().name.clone();
    let output_name = match req.output_name.trim() {
        "" => format!("{input_name} · {}", pipeline.name),
        name => name.to_string(),
    };

    let runs_dir = ws.runs_dir();
    let run_id = dataset::new_prefixed_id(&runs_dir, "run", RUN_TEMP_PREFIX);
    let started_at_ms = now_ms();
    ws.catalog()?.insert_run(&run_id, &pipeline.name, &req.input_dataset_id, started_at_ms)?;

    let names = Names { run_id: &run_id, input_name: &input_name, output_name: &output_name };
    let result = execute(ctx, &ws, &req, &pipeline, &reader, &names, started_at_ms);

    // Record how the run ended, whatever happened.
    let finished = now_ms();
    let catalog = ws.catalog()?;
    match &result {
        Ok(Some((output_id, docs_in, docs_out))) => {
            catalog.finish_run(&run_id, "completed", Some(output_id), Some(*docs_in), Some(*docs_out), finished, None)?
        }
        Ok(None) => catalog.finish_run(&run_id, "cancelled", None, None, None, finished, None)?,
        Err(e) => catalog.finish_run(&run_id, "failed", None, None, None, finished, Some(&e.to_string()))?,
    }
    drop(catalog);

    match result? {
        None => Ok(JobOutcome::Cancelled),
        Some((_, docs_in, docs_out)) => {
            let removed = docs_in - docs_out;
            let pct = if docs_in == 0 { 0.0 } else { removed as f64 / docs_in as f64 * 100.0 };
            let message =
                format!("Kept {docs_out} of {docs_in} documents ({removed} removed, {pct:.1}%) → \"{output_name}\"");
            Ok(JobOutcome::Produced(message, aeon_shared::JobLink::Run(run_id)))
        }
    }
}

struct Names<'a> {
    run_id: &'a str,
    input_name: &'a str,
    output_name: &'a str,
}

/// Reads the input in batches of [`BATCH_DOCS`] and calls `f` for each. Returns `Ok(false)`
/// if the job was cancelled.
fn for_each_batch(
    reader: &DatasetReader,
    ctx: &mut JobCtx,
    mut f: impl FnMut(Vec<InputDoc>, &mut JobCtx) -> AppResult<()>,
) -> AppResult<bool> {
    let mut batch: Vec<InputDoc> = Vec::with_capacity(BATCH_DOCS);
    let mut failure: Option<AppError> = None;
    let mut cancelled = false;
    reader.scan(0, None, |row, r| {
        batch.push(InputDoc {
            row,
            id: r.id.to_string(),
            text: r.text.to_string(),
            source: r.source.to_string(),
            meta: r.meta.to_string(),
        });
        if batch.len() >= BATCH_DOCS {
            if ctx.is_cancelled() {
                cancelled = true;
                return false;
            }
            if let Err(e) = f(std::mem::take(&mut batch), ctx) {
                failure = Some(e);
                return false;
            }
        }
        true
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    if cancelled || ctx.is_cancelled() {
        return Ok(false);
    }
    if !batch.is_empty() {
        f(batch, ctx)?;
    }
    Ok(true)
}

/// Returns `Ok(None)` when cancelled, otherwise the output dataset id and document counts.
fn execute(
    ctx: &mut JobCtx,
    ws: &Workspace,
    req: &RunRequest,
    pipeline: &Pipeline,
    reader: &DatasetReader,
    names: &Names<'_>,
    started_at_ms: i64,
) -> AppResult<Option<(String, u64, u64)>> {
    let total = reader.row_count();
    let dedup_positions = pipeline.dedup_positions();
    let passes = dedup_positions.len() as u64 + 1;
    let run_temp = TempDataset::create_with_prefix(&ws.runs_dir(), names.run_id, RUN_TEMP_PREFIX)?;

    // One fingerprint pass per dedup step, each using the decisions of the ones before it.
    let mut decisions = Decisions::default();
    for (k, &pos) in dedup_positions.iter().enumerate() {
        let label = &pipeline.steps[pos].label;
        let mut bands = BandFiles::create(&run_temp.path().join(format!("dedup-{k}")), pipeline.bands(pos))?;
        let mut seen = 0u64;
        let base = k as u64 * total;
        let completed = for_each_batch(reader, ctx, |docs, ctx| {
            let fingerprints: Vec<(u64, Option<Vec<u64>>)> = docs
                .into_par_iter()
                .map(|mut d| {
                    let o = pipeline.process(std::mem::take(&mut d.text), d.row, &decisions, Some(pos));
                    (d.row, if o.reached_stop { pipeline.fingerprint(pos, &o.text) } else { None })
                })
                .collect();
            for (row, fp) in &fingerprints {
                if let Some(hashes) = fp {
                    bands.push(*row, hashes)?;
                }
            }
            seen += fingerprints.len() as u64;
            ctx.progress(
                base + seen,
                Some(total * passes),
                Some(format!("Pass {} of {passes}: fingerprinting for {label}", k + 1)),
            );
            Ok(())
        })?;
        if !completed {
            return Ok(None);
        }
        ctx.progress(base + total, Some(total * passes), Some(format!("Pass {} of {passes}: finding duplicates", k + 1)));
        decisions.set(pos, bands.resolve()?);
    }

    // Final pass: write kept documents, removed documents, and the kept index.
    let removed_dir = run_temp.path().join("removed");
    let kept_dir = run_temp.path().join("kept");
    std::fs::create_dir_all(&removed_dir)?;
    std::fs::create_dir_all(&kept_dir)?;
    let datasets_dir = ws.datasets_dir();
    let output_id = dataset::new_id(&datasets_dir);
    let out_temp = TempDataset::create(&datasets_dir, &output_id)?;

    let mut writer = DatasetWriter::new(out_temp.path())?;
    let mut removed = RemovedWriter::new(&removed_dir)?;
    let mut kept = KeptWriter::new(&kept_dir)?;
    let mut stats = StatsAccumulator::new(pipeline);
    let base = (passes - 1) * total;

    let completed = for_each_batch(reader, ctx, |docs, ctx| {
        let outcomes: Vec<_> = docs
            .into_par_iter()
            .map(|mut d| {
                let text = std::mem::take(&mut d.text);
                (pipeline.process(text, d.row, &decisions, None), d)
            })
            .collect();
        for (o, d) in &outcomes {
            stats.add(o);
            match &o.dropped {
                None => {
                    writer.push(NewDoc { id: &d.id, text: &o.text, source: &d.source, meta_json: &d.meta })?;
                    kept.push(d.row, o.modified)?;
                }
                Some(drop) => removed.push(RemovedRow {
                    input_row: d.row,
                    id: &d.id,
                    source: &d.source,
                    step_index: pipeline.steps[drop.pos].index,
                    rule: drop.rule,
                    detail: &drop.detail,
                    related_row: drop.related_row,
                    text: &o.text,
                })?,
            }
        }
        let message = format!("{} kept · {} removed", stats.docs_out, stats.docs_in - stats.docs_out);
        let message = if passes > 1 { format!("Pass {passes} of {passes}: {message}") } else { message };
        ctx.progress(base + stats.docs_in, Some(total * passes), Some(message));
        Ok(())
    })?;
    if !completed {
        return Ok(None); // temp folders are removed on drop
    }

    ctx.progress(total * passes, Some(total * passes), Some("Finishing…".into()));
    let (parts, text_stats) = writer.finish()?;
    let removed_parts = removed.finish()?;
    let kept_parts = kept.finish()?;
    let step_stats = stats.step_stats(pipeline);
    let (docs_in, docs_out) = (stats.docs_in, stats.docs_out);

    let manifest = DatasetManifest {
        format_version: dataset::MANIFEST_VERSION,
        id: output_id.clone(),
        name: names.output_name.to_string(),
        description: None,
        created_at_ms: now_ms(),
        parts,
        stats: text_stats,
        import: None,
        produced_by: Some(ProducedBy {
            run_id: names.run_id.to_string(),
            pipeline_name: pipeline.name.clone(),
            input_dataset_id: req.input_dataset_id.clone(),
            input_name: names.input_name.to_string(),
        }),
        mix: None,
    };
    dataset::write_manifest(out_temp.path(), &manifest)?;

    let report = RunReport {
        id: names.run_id.to_string(),
        pipeline: req.spec.clone(),
        input_dataset_id: req.input_dataset_id.clone(),
        input_name: names.input_name.to_string(),
        output_dataset_id: output_id.clone(),
        output_name: names.output_name.to_string(),
        started_at_ms,
        finished_at_ms: now_ms(),
        docs_in,
        docs_out,
        chars_in: stats.chars_in,
        chars_out: stats.chars_out,
        steps: step_stats,
        removed_parts,
        kept_parts,
    };
    std::fs::write(run_temp.path().join("report.json"), serde_json::to_string_pretty(&report)?)?;
    std::fs::write(run_temp.path().join("pipeline.toml"), toml::to_string_pretty(&req.spec)?)?;

    // Move both folders into place, then register the dataset (the run row already exists,
    // which the dataset's parent_run_id refers to).
    let run_dir = run_temp.commit()?;
    let out_dir = match out_temp.commit() {
        Ok(dir) => dir,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&run_dir);
            return Err(e);
        }
    };
    if let Err(e) = ws.catalog()?.insert_dataset(&summary_of(&manifest)) {
        let _ = std::fs::remove_dir_all(&out_dir);
        let _ = std::fs::remove_dir_all(&run_dir);
        return Err(e);
    }
    Ok(Some((output_id, docs_in, docs_out)))
}

pub fn read_report(ws: &Workspace, run_id: &str) -> AppResult<RunReport> {
    let path = ws.run_dir(run_id)?.join("report.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::InvalidInput(format!("Run {run_id} has no report (it did not complete)"))
        } else {
            e.into()
        }
    })?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobEvents, JobManager};
    use crate::pipeline::{RemovedReader, browse_changed};
    use aeon_shared::{
        ExactDedup, ImportFormat, ImportRequest, JobKind, JobState, JobStatus, LengthFilter, NearDedup,
        NormalizeWhitespace, PipelineStep, StepConfig, TextMode,
    };
    use std::time::{Duration, Instant};

    struct Quiet;
    impl JobEvents for Quiet {
        fn job_updated(&self, _: &JobStatus) {}
    }

    fn wait(jobs: &JobManager, id: u64) -> JobStatus {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let s = jobs.list().unwrap().into_iter().find(|s| s.id == id).unwrap();
            if s.state.is_finished() {
                return s;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Imports `texts` as a dataset (ids "0", "1", ...) and returns the workspace and dataset id.
    fn workspace_with(texts: &[String]) -> (tempfile::TempDir, Arc<Workspace>, Arc<JobManager>, String) {
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(ws_dir.path()).unwrap());
        let file = ws_dir.path().join("src.jsonl");
        let lines: String = texts
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({ "id": i.to_string(), "text": t }).to_string() + "\n")
            .collect();
        std::fs::write(&file, lines).unwrap();
        let jobs = JobManager::new(Arc::new(Quiet));
        let import = ImportRequest {
            name: "src".into(),
            description: None,
            paths: vec![file.display().to_string()],
            format: ImportFormat::Auto,
            text_field: "text".into(),
            id_field: Some("id".into()),
            text_mode: TextMode::WholeFile,
        };
        let w = ws.clone();
        let id = jobs.spawn(JobKind::Import, "import", move |ctx| crate::import::run(ctx, w, import)).unwrap();
        assert_eq!(wait(&jobs, id).state, JobState::Completed);
        let input_id = ws.catalog().unwrap().list_datasets().unwrap()[0].id.clone();
        (ws_dir, ws, jobs, input_id)
    }

    fn run_spec(ws: &Arc<Workspace>, jobs: Arc<JobManager>, input_id: &str, steps: Vec<StepConfig>) -> (JobStatus, RunReport) {
        let spec = PipelineSpec {
            name: "Clean".into(),
            description: None,
            steps: steps.into_iter().map(|config| PipelineStep { enabled: true, config }).collect(),
        };
        let w = ws.clone();
        let req = RunRequest { spec, input_dataset_id: input_id.to_string(), output_name: String::new() };
        let id = jobs.spawn(JobKind::PipelineRun, "run", move |ctx| run(ctx, w, req)).unwrap();
        let status = wait(&jobs, id);
        let run_id = ws.catalog().unwrap().list_runs().unwrap()[0].id.clone();
        let report = read_report(ws, &run_id);
        (status, report.unwrap_or_else(|e| panic!("no report: {e}")))
    }

    #[test]
    fn run_writes_output_dataset_report_removed_docs_and_kept_index() {
        let texts: Vec<String> = (0..10_000)
            .map(|i| if i % 4 == 0 { format!("short {i}") } else { format!("doc   number {i} has   enough words") })
            .collect();
        let (_dir, ws, jobs, input_id) = workspace_with(&texts);
        let (status, report) = run_spec(
            &ws,
            jobs,
            &input_id,
            vec![
                StepConfig::NormalizeWhitespace(NormalizeWhitespace::default()),
                StepConfig::LengthFilter(LengthFilter { min_words: Some(3), ..Default::default() }),
            ],
        );
        assert_eq!(status.state, JobState::Completed, "{:?}", status.message);

        let runs = ws.catalog().unwrap().list_runs().unwrap();
        let r = &runs[0];
        assert_eq!((r.state.as_str(), r.docs_in, r.docs_out), ("completed", Some(10_000), Some(7_500)));
        assert_eq!(r.output_name.as_deref(), Some("src · Clean"));
        assert_eq!(report.steps[0].modified, 7_500);
        assert_eq!(report.steps[1].drops_by_rule[0].count, 2_500);

        let out = DatasetReader::open(&ws.dataset_dir(r.output_dataset_id.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(out.row_count(), 7_500);
        assert_eq!(out.document(0).unwrap().text, "doc number 1 has enough words");
        assert_eq!(out.manifest().produced_by.as_ref().unwrap().run_id, r.id);

        let run_dir = ws.run_dir(&r.id).unwrap();
        let removed = RemovedReader::new(&run_dir.join("removed"), report.removed_parts.clone());
        let page = removed.browse(0, 5, Some(1), Some("min_words")).unwrap();
        assert_eq!((page.docs[0].input_row, page.docs[1].input_row), (0, 4));
        assert_eq!(removed.get(3).unwrap().text, "short 12");

        // Kept index: output row 0 is input row 1, changed by step 0 (whitespace).
        let changed = browse_changed(&run_dir, &report, Some(0), 0, 3).unwrap();
        assert_eq!(changed.total_kept, 7_500);
        assert_eq!((changed.items[0].output_row, changed.items[0].input_row), (0, 1));
        assert_eq!(changed.items[0].step_indices, vec![0]);
        assert!(browse_changed(&run_dir, &report, Some(1), 0, 3).unwrap().items.is_empty(), "filters change nothing");

        let toml_text = std::fs::read_to_string(run_dir.join("pipeline.toml")).unwrap();
        assert_eq!(toml::from_str::<PipelineSpec>(&toml_text).unwrap(), report.pipeline);
        assert!(!run_dir.join("dedup-0").exists());
    }

    #[test]
    fn dedup_steps_keep_the_first_copy_and_record_what_it_duplicates() {
        // Pseudo-random words, so unrelated paragraphs share (almost) no n-grams.
        let para = |seed: usize| -> String {
            (0..120)
                .map(|w| {
                    let h = xxhash_rust::xxh3::xxh3_64(&((seed * 10_000 + w) as u64).to_le_bytes());
                    format!("w{}", h % 1_000_000)
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut texts = Vec::new();
        for i in 0..3000usize {
            texts.push(match i {
                // Exact copies (differing only in case/spacing) of document 10.
                100 | 2000 => para(10).to_uppercase().replace(' ', "  "),
                // Near copies of document 20: one word changed (n-gram Jaccard ≈ 0.92).
                300 | 2500 => {
                    let mut words: Vec<String> = para(20).split(' ').map(String::from).collect();
                    words[60] = format!("changed{i}");
                    words.join(" ")
                }
                // Document 400 is an exact copy that also near-matches; exact dedup gets it first.
                400 => para(30),
                401 => para(30),
                _ => para(i),
            });
        }
        let (_dir, ws, jobs, input_id) = workspace_with(&texts);
        let (status, report) = run_spec(
            &ws,
            jobs,
            &input_id,
            vec![
                StepConfig::NormalizeWhitespace(NormalizeWhitespace::default()),
                StepConfig::ExactDedup(ExactDedup::default()),
                StepConfig::NearDedup(NearDedup::default()),
            ],
        );
        assert_eq!(status.state, JobState::Completed, "{:?}", status.message);
        assert_eq!(report.steps[1].drops_by_rule[0].rule, "exact_duplicate");
        // 100, 2000 (copies of 10) and 401 (copy of 400... which itself copies 30).
        assert_eq!(report.steps[1].dropped, 4, "{:?}", report.steps[1]);
        assert_eq!(report.steps[2].dropped, 2, "{:?}", report.steps[2]);
        assert_eq!(report.docs_out, 3000 - 6);

        let removed = RemovedReader::new(&ws.run_dir(&report.id).unwrap().join("removed"), report.removed_parts.clone());
        let page = removed.browse(0, 10, None, None).unwrap();
        let pairs: Vec<(u64, Option<u64>, &str)> =
            page.docs.iter().map(|d| (d.input_row, d.related_row, d.rule.as_str())).collect();
        assert!(pairs.contains(&(100, Some(10), "exact_duplicate")), "{pairs:?}");
        assert!(pairs.contains(&(2000, Some(10), "exact_duplicate")));
        assert!(pairs.contains(&(400, Some(30), "exact_duplicate")));
        assert!(pairs.contains(&(401, Some(30), "exact_duplicate")));
        assert!(pairs.contains(&(300, Some(20), "near_duplicate")));
        assert!(pairs.contains(&(2500, Some(20), "near_duplicate")));
        assert!(!ws.run_dir(&report.id).unwrap().join("dedup-0").exists(), "band files are cleaned up");
    }
}
