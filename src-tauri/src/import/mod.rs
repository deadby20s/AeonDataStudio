//! Importing source files into a new dataset.

mod sources;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aeon_shared::{
    DatasetManifest, DatasetSummary, ImportFormat, ImportReport, ImportRequest, SkipCounts, SourceFileReport,
};

use crate::dataset::{self, DatasetWriter, NewDoc, TempDataset};
use crate::error::{AppError, AppResult, now_ms};
use crate::jobs::{JobCtx, JobOutcome};
use crate::workspace::Workspace;

pub use sources::collect_inputs;
use sources::{Item, ReadOptions, read_file};

const MAX_SAMPLE_ERRORS: usize = 10;
/// Progress is reported (and cancellation checked) every this many records.
const PROGRESS_EVERY: u64 = 1000;

/// Checks a request before a job is started, so obvious mistakes show up immediately.
pub fn validate(req: &ImportRequest) -> AppResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Give the dataset a name".into()));
    }
    if req.paths.is_empty() {
        return Err(AppError::InvalidInput("Add at least one file or folder".into()));
    }
    if req.format != ImportFormat::Text && req.text_field.trim().is_empty() {
        return Err(AppError::InvalidInput("Enter the name of the text field".into()));
    }
    Ok(())
}

pub fn run(ctx: &mut JobCtx, ws: Arc<Workspace>, req: ImportRequest) -> AppResult<JobOutcome> {
    validate(&req)?;
    ctx.progress(0, None, Some("Finding files…".into()));
    let (files, ignored_files) = collect_inputs(&req.paths, req.format)?;
    if files.is_empty() {
        return Err(AppError::InvalidInput("No importable files found in the chosen folders".into()));
    }
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();

    let datasets_dir = ws.datasets_dir();
    let id = dataset::new_id(&datasets_dir);
    let temp = TempDataset::create(&datasets_dir, &id)?;
    let mut writer = DatasetWriter::new(temp.path())?;

    let text_field = req.text_field.trim();
    let id_field = req.id_field.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let opts = ReadOptions { text_field, id_field, text_mode: req.text_mode };
    let bytes_read = Arc::new(AtomicU64::new(0));
    let mut skipped = SkipCounts { ignored_files, ..SkipCounts::default() };
    let mut sample_errors = Vec::new();
    let mut reports = Vec::with_capacity(files.len());
    let mut records_seen = 0u64;
    let mut cancelled = false;

    for (file_idx, file) in files.iter().enumerate() {
        let mut report = SourceFileReport {
            source: file.source.clone(),
            format: file.format,
            docs: 0,
            skipped: SkipCounts::default(),
        };
        let mut record_no = 0u64;
        let mut meta_buf = String::new();
        read_file(file, &opts, bytes_read.clone(), &mut |item| {
            record_no += 1;
            records_seen += 1;
            match item {
                Item::ParseError(msg) => {
                    report.skipped.parse_errors += 1;
                    if sample_errors.len() < MAX_SAMPLE_ERRORS {
                        sample_errors.push(format!("{}: {msg}", file.source));
                    }
                }
                Item::Record(rec) => match rec.text {
                    None => report.skipped.missing_text += 1,
                    Some(text) if text.trim().is_empty() => report.skipped.empty_text += 1,
                    Some(text) => {
                        let id = rec.id.unwrap_or_else(|| format!("{}:{record_no}", file.source));
                        meta_buf.clear();
                        if !rec.meta.is_empty() {
                            meta_buf = serde_json::to_string(&rec.meta)?;
                        }
                        writer.push(NewDoc {
                            id: &id,
                            text: &text,
                            source: &file.source,
                            meta_json: if meta_buf.is_empty() { "{}" } else { &meta_buf },
                        })?;
                        report.docs += 1;
                    }
                },
            }
            if records_seen.is_multiple_of(PROGRESS_EVERY) {
                if ctx.is_cancelled() {
                    cancelled = true;
                    return Ok(false);
                }
                ctx.progress(
                    bytes_read.load(Ordering::Relaxed).min(total_bytes),
                    Some(total_bytes),
                    Some(format!(
                        "File {} of {} · {} documents",
                        file_idx + 1,
                        files.len(),
                        writer.doc_count()
                    )),
                );
            }
            Ok(true)
        })?;
        skipped.add(&report.skipped);
        reports.push(report);
        if cancelled || ctx.is_cancelled() {
            return Ok(JobOutcome::Cancelled); // `temp` is dropped and removed
        }
    }

    ctx.progress(total_bytes, Some(total_bytes), Some("Finishing…".into()));
    let (parts, stats) = writer.finish()?;
    if stats.doc_count == 0 {
        let hint = sample_errors.first().map(|e| format!(" First problem: {e}")).unwrap_or_default();
        return Err(AppError::InvalidInput(format!(
            "No documents found: {} records without text field \"{text_field}\", {} empty, {} unreadable.{hint}",
            skipped.missing_text, skipped.empty_text, skipped.parse_errors
        )));
    }

    let manifest = DatasetManifest {
        format_version: dataset::MANIFEST_VERSION,
        id: id.clone(),
        name: req.name.trim().to_string(),
        description: req.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(String::from),
        created_at_ms: now_ms(),
        parts,
        stats,
        import: Some(ImportReport { request: req, files: reports, skipped, sample_errors }),
    };
    dataset::write_manifest(temp.path(), &manifest)?;
    let final_dir = temp.commit()?;

    let summary = summary_of(&manifest);
    if let Err(e) = ws.catalog()?.insert_dataset(&summary) {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(e);
    }

    let s = &manifest.stats;
    let skipped_total = manifest.import.as_ref().map_or(0, |i| i.skipped.records());
    let mut message = format!("Imported {} documents into \"{}\"", s.doc_count, manifest.name);
    if skipped_total > 0 {
        message.push_str(&format!(", skipped {skipped_total} records"));
    }
    Ok(JobOutcome::Completed(Some(message)))
}

pub fn summary_of(m: &DatasetManifest) -> DatasetSummary {
    DatasetSummary {
        id: m.id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        created_at_ms: m.created_at_ms,
        doc_count: Some(m.stats.doc_count),
        char_count: Some(m.stats.char_count),
        byte_size: Some(m.byte_size()),
        source_desc: m.import.as_ref().map(source_desc),
        parent_run_id: None,
    }
}

/// e.g. "2 JSONL files, 1 CSV file".
fn source_desc(report: &ImportReport) -> String {
    let mut by_format: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &report.files {
        *by_format.entry(f.format.short()).or_default() += 1;
    }
    by_format
        .into_iter()
        .map(|(fmt, n)| format!("{n} {fmt} file{}", if n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::DatasetReader;
    use crate::jobs::{JobEvents, JobManager};
    use aeon_shared::{JobState, JobStatus, TextMode};
    use std::time::{Duration, Instant};

    struct Quiet;
    impl JobEvents for Quiet {
        fn job_updated(&self, _: &JobStatus) {}
    }

    fn request(paths: Vec<String>) -> ImportRequest {
        ImportRequest {
            name: "Test set".into(),
            description: None,
            paths,
            format: ImportFormat::Auto,
            text_field: "text".into(),
            id_field: Some("id".into()),
            text_mode: TextMode::PerLine,
        }
    }

    fn run_to_end(ws: Arc<Workspace>, req: ImportRequest) -> JobStatus {
        let jobs = JobManager::new(Arc::new(Quiet));
        let id = jobs.spawn(aeon_shared::JobKind::Import, "import", move |ctx| run(ctx, ws, req)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
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
    fn imports_mixed_files_into_catalog() {
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(ws_dir.path()).unwrap());
        let src = tempfile::tempdir().unwrap();
        let jsonl = src.path().join("a.jsonl");
        std::fs::write(&jsonl, "{\"id\":\"x1\",\"text\":\"first doc\"}\n{\"text\":\"  \"}\nbroken\n{\"text\":\"third\"}\n").unwrap();
        let txt = src.path().join("b.txt");
        std::fs::write(&txt, "line one\nline two\n").unwrap();

        let status =
            run_to_end(ws.clone(), request(vec![jsonl.display().to_string(), txt.display().to_string()]));
        assert_eq!(status.state, JobState::Completed, "{:?}", status.message);

        let list = ws.catalog().unwrap().list_datasets().unwrap();
        assert_eq!(list.len(), 1);
        let d = &list[0];
        assert_eq!(d.doc_count, Some(4));
        assert_eq!(d.source_desc.as_deref(), Some("1 JSONL file, 1 text file"));

        let reader = DatasetReader::open(&ws.dataset_dir(&d.id).unwrap()).unwrap();
        let report = reader.manifest().import.as_ref().unwrap();
        assert_eq!(report.skipped.empty_text, 1);
        assert_eq!(report.skipped.parse_errors, 1);
        let page = reader.browse(0, 10, None).unwrap();
        let ids: Vec<_> = page.docs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["x1", "a.jsonl:4", "b.txt:1", "b.txt:2"]);
        // No temp folders left behind.
        let leftovers = std::fs::read_dir(ws.datasets_dir())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn fails_clearly_when_text_field_is_wrong_and_leaves_nothing() {
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(ws_dir.path()).unwrap());
        let src = tempfile::tempdir().unwrap();
        let jsonl = src.path().join("a.jsonl");
        std::fs::write(&jsonl, "{\"content\":\"hello\"}\n").unwrap();

        let status = run_to_end(ws.clone(), request(vec![jsonl.display().to_string()]));
        assert_eq!(status.state, JobState::Failed);
        assert!(status.message.unwrap().contains("without text field \"text\""));
        assert_eq!(ws.catalog().unwrap().dataset_count().unwrap(), 0);
        assert_eq!(std::fs::read_dir(ws.datasets_dir()).unwrap().count(), 0);
    }

    #[test]
    fn validate_catches_missing_name_and_paths() {
        assert!(validate(&ImportRequest { name: " ".into(), ..request(vec!["x".into()]) }).is_err());
        assert!(validate(&request(vec![])).is_err());
    }
}
