//! Running a pipeline over a dataset as a background job.
//!
//! ```text
//! runs/<run-id>/
//! ├── report.json        RunReport: frozen recipe, per-step stats, removed-document parts
//! ├── pipeline.toml      the recipe exactly as run
//! └── removed/           removed-00000.parquet ... (input_row, id, source, step_index, rule, detail, text)
//! ```
//!
//! The run folder and the output dataset are written to temp folders and only moved into
//! place when the run succeeds.

use std::sync::Arc;

use aeon_shared::{DatasetManifest, PipelineSpec, ProducedBy, RunReport};
use rayon::prelude::*;

use super::removed::{RemovedRow, RemovedWriter};
use super::{Pipeline, RUN_TEMP_PREFIX, StatsAccumulator};
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

    let result = execute(ctx, &ws, &req, &pipeline, &reader, &run_id, &input_name, &output_name, started_at_ms);

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

/// Returns `Ok(None)` when cancelled, otherwise the output dataset id and document counts.
#[allow(clippy::too_many_arguments)]
fn execute(
    ctx: &mut JobCtx,
    ws: &Workspace,
    req: &RunRequest,
    pipeline: &Pipeline,
    reader: &DatasetReader,
    run_id: &str,
    input_name: &str,
    output_name: &str,
    started_at_ms: i64,
) -> AppResult<Option<(String, u64, u64)>> {
    let total = reader.row_count();
    let run_temp = TempDataset::create_with_prefix(&ws.runs_dir(), run_id, RUN_TEMP_PREFIX)?;
    let removed_dir = run_temp.path().join("removed");
    std::fs::create_dir_all(&removed_dir)?;
    let datasets_dir = ws.datasets_dir();
    let output_id = dataset::new_id(&datasets_dir);
    let out_temp = TempDataset::create(&datasets_dir, &output_id)?;

    let mut writer = DatasetWriter::new(out_temp.path())?;
    let mut removed = RemovedWriter::new(&removed_dir)?;
    let mut stats = StatsAccumulator::new(pipeline);
    let mut batch: Vec<InputDoc> = Vec::with_capacity(BATCH_DOCS);
    let mut failure: Option<AppError> = None;
    let mut cancelled = false;

    let mut flush = |batch: &mut Vec<InputDoc>, ctx: &mut JobCtx| -> AppResult<()> {
        let docs: Vec<InputDoc> = std::mem::take(batch);
        let outcomes: Vec<_> = docs
            .into_par_iter()
            .map(|mut d| {
                let text = std::mem::take(&mut d.text);
                (pipeline.process(text), d)
            })
            .collect();
        for (o, d) in &outcomes {
            stats.add(o);
            match &o.dropped {
                None => writer.push(NewDoc { id: &d.id, text: &o.text, source: &d.source, meta_json: &d.meta })?,
                Some(drop) => removed.push(RemovedRow {
                    input_row: d.row,
                    id: &d.id,
                    source: &d.source,
                    step_index: pipeline.steps[drop.pos].index,
                    rule: drop.rule,
                    detail: &drop.detail,
                    text: &o.text,
                })?,
            }
        }
        ctx.progress(
            stats.docs_in,
            Some(total),
            Some(format!("{} kept · {} removed", stats.docs_out, stats.docs_in - stats.docs_out)),
        );
        Ok(())
    };

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
            if let Err(e) = flush(&mut batch, ctx) {
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
        return Ok(None); // temp folders are removed on drop
    }
    flush(&mut batch, ctx)?;
    drop(flush);

    ctx.progress(total, Some(total), Some("Finishing…".into()));
    let (parts, text_stats) = writer.finish()?;
    let removed_parts = removed.finish()?;
    let step_stats = stats.step_stats(pipeline);
    let (docs_in, docs_out) = (stats.docs_in, stats.docs_out);

    let manifest = DatasetManifest {
        format_version: dataset::MANIFEST_VERSION,
        id: output_id.clone(),
        name: output_name.to_string(),
        description: None,
        created_at_ms: now_ms(),
        parts,
        stats: text_stats,
        import: None,
        produced_by: Some(ProducedBy {
            run_id: run_id.to_string(),
            pipeline_name: pipeline.name.clone(),
            input_dataset_id: req.input_dataset_id.clone(),
            input_name: input_name.to_string(),
        }),
    };
    dataset::write_manifest(out_temp.path(), &manifest)?;

    let report = RunReport {
        id: run_id.to_string(),
        pipeline: req.spec.clone(),
        input_dataset_id: req.input_dataset_id.clone(),
        input_name: input_name.to_string(),
        output_dataset_id: output_id.clone(),
        output_name: output_name.to_string(),
        started_at_ms,
        finished_at_ms: now_ms(),
        docs_in,
        docs_out,
        chars_in: stats.chars_in,
        chars_out: stats.chars_out,
        steps: step_stats,
        removed_parts,
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
    use crate::pipeline::RemovedReader;
    use aeon_shared::{
        ImportFormat, ImportRequest, JobKind, JobState, JobStatus, LengthFilter, NormalizeWhitespace, PipelineStep,
        StepConfig, TextMode,
    };
    use std::time::{Duration, Instant};

    struct Quiet;
    impl JobEvents for Quiet {
        fn job_updated(&self, _: &JobStatus) {}
    }

    fn wait(jobs: &JobManager, id: u64) -> JobStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let s = jobs.list().unwrap().into_iter().find(|s| s.id == id).unwrap();
            if s.state.is_finished() {
                return s;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn run_writes_output_dataset_report_and_removed_docs() {
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(ws_dir.path()).unwrap());
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("d.jsonl");
        let mut lines = String::new();
        for i in 0..10_000 {
            let text = if i % 4 == 0 { format!("short {i}") } else { format!("doc   number {i} has   enough words") };
            lines.push_str(&serde_json::json!({ "id": i.to_string(), "text": text }).to_string());
            lines.push('\n');
        }
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

        let spec = PipelineSpec {
            name: "Clean".into(),
            description: None,
            steps: vec![
                PipelineStep { enabled: true, config: StepConfig::NormalizeWhitespace(NormalizeWhitespace::default()) },
                PipelineStep {
                    enabled: true,
                    config: StepConfig::LengthFilter(LengthFilter { min_words: Some(3), ..Default::default() }),
                },
            ],
        };
        let w = ws.clone();
        let req = RunRequest { spec, input_dataset_id: input_id.clone(), output_name: String::new() };
        let id = jobs.spawn(JobKind::PipelineRun, "run", move |ctx| run(ctx, w, req)).unwrap();
        let status = wait(&jobs, id);
        assert_eq!(status.state, JobState::Completed, "{:?}", status.message);

        let runs = ws.catalog().unwrap().list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        let r = &runs[0];
        assert_eq!((r.state.as_str(), r.docs_in, r.docs_out), ("completed", Some(10_000), Some(7_500)));
        assert_eq!(r.output_name.as_deref(), Some("src · Clean"));

        let report = read_report(&ws, &r.id).unwrap();
        assert_eq!(report.steps[0].modified, 7_500);
        assert_eq!(report.steps[1].drops_by_rule[0].count, 2_500);

        let out = DatasetReader::open(&ws.dataset_dir(r.output_dataset_id.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(out.row_count(), 7_500);
        assert_eq!(out.document(0).unwrap().text, "doc number 1 has enough words");
        assert_eq!(out.manifest().produced_by.as_ref().unwrap().run_id, r.id);

        let removed = RemovedReader::new(&ws.run_dir(&r.id).unwrap().join("removed"), report.removed_parts.clone());
        let page = removed.browse(0, 5, Some(1), Some("min_words")).unwrap();
        assert_eq!(page.docs.len(), 5);
        assert_eq!((page.docs[0].input_row, page.docs[0].id.as_str()), (0, "0"));
        assert_eq!(page.docs[1].input_row, 4);
        assert!(removed.browse(0, 5, Some(0), None).unwrap().docs.is_empty());
        assert_eq!(removed.get(3).unwrap().text, "short 12");

        let toml_text = std::fs::read_to_string(ws.run_dir(&r.id).unwrap().join("pipeline.toml")).unwrap();
        let parsed: PipelineSpec = toml::from_str(&toml_text).unwrap();
        assert_eq!(parsed, report.pipeline);
    }
}
