//! Importing source files into a new dataset.

mod html;
mod sources;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aeon_shared::{
    DatasetManifest, DatasetSummary, FieldInfo, ImportFormat, ImportPreview, ImportRequest, ImportReport, SkipCounts,
    SourceFileReport, TextMode,
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

/// Records read for a preview.
const PREVIEW_RECORDS: usize = 50;
const PREVIEW_TEXTS: usize = 5;
const PREVIEW_CHARS: usize = 300;
/// A field name no real file has, so previews keep every field in `meta`.
const NO_FIELD: &str = "\u{0}";

fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().nth(n).is_some() {
        out.push('…');
    }
    out
}

/// Samples the first file of an import: its fields (with a guess at the text field) and the
/// first documents as they'd be imported with `text_field`.
pub fn preview(paths: &[String], format: ImportFormat, text_field: &str, text_mode: TextMode) -> AppResult<ImportPreview> {
    if paths.is_empty() {
        return Err(AppError::InvalidInput("Add a file or folder first".into()));
    }
    let (files, ignored_files) = collect_inputs(paths, format)?;
    let first = files.first().ok_or_else(|| AppError::InvalidInput("No importable files found".into()))?;
    let opts = ReadOptions { text_field: NO_FIELD, id_field: None, text_mode, allow_missing_text: true };
    let mut records = Vec::new();
    let mut parse_errors = 0u64;
    read_file(first, &opts, Arc::new(AtomicU64::new(0)), &mut |item| {
        match item {
            Item::Record(r) => records.push(r),
            Item::ParseError(_) => parse_errors += 1,
        }
        Ok(records.len() + (parse_errors as usize) < PREVIEW_RECORDS)
    })?;

    let mut fields: Vec<FieldInfo> = Vec::new();
    
    let sample_texts: Vec<String> = if !first.format.has_fields() {
        records.iter().filter_map(|r| r.text.as_deref()).take(PREVIEW_TEXTS).map(|t| clip(t, PREVIEW_CHARS)).collect()
    } else {
        let mut chars: Vec<u64> = Vec::new();
        for r in &records {
            for (k, v) in &r.meta {
                let i = match fields.iter().position(|f| &f.name == k) {
                    Some(i) => i,
                    None => {
                        fields.push(FieldInfo { name: k.clone(), text_count: 0, mean_chars: 0.0, example: String::new() });
                        chars.push(0);
                        fields.len() - 1
                    }
                };
                let f = &mut fields[i];
                if let serde_json::Value::String(s) = v {
                    f.text_count += 1;
                    chars[i] += s.chars().count() as u64;
                    if f.example.is_empty() && !s.trim().is_empty() {
                        f.example = clip(s.trim(), 120);
                    }
                } else if f.example.is_empty() && !v.is_null() {
                    f.example = clip(&v.to_string(), 120);
                }
            }
        }
        for (f, c) in fields.iter_mut().zip(&chars) {
            f.mean_chars = if f.text_count == 0 { 0.0 } else { *c as f64 / f.text_count as f64 };
        }
        records
            .iter()
            .filter_map(|r| r.meta.get(text_field).and_then(|v| v.as_str()))
            .take(PREVIEW_TEXTS)
            .map(|t| clip(t, PREVIEW_CHARS))
            .collect()
    };
    let half = (records.len() as u64).div_ceil(2).max(1);
    let suggested_text_field = fields
        .iter()
        .filter(|f| f.text_count >= half)
        .max_by(|a, b| a.mean_chars.total_cmp(&b.mean_chars))
        .map(|f| f.name.clone());
    let suggested_id_field = ["id", "doc_id", "document_id", "uuid", "_id", "url"]
        .iter()
        .find(|c| fields.iter().any(|f| f.name == **c))
        .map(|c| c.to_string());
    Ok(ImportPreview {
        files: files.len() as u64,
        ignored_files,
        file: first.source.clone(),
        format: first.format,
        records_sampled: records.len() as u64,
        parse_errors,
        fields,
        suggested_text_field,
        suggested_id_field,
        sample_texts,
    })
}

/// Checks a request before a job is started, so obvious mistakes show up immediately.
pub fn validate(req: &ImportRequest) -> AppResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Give the dataset a name".into()));
    }
    if req.paths.is_empty() {
        return Err(AppError::InvalidInput("Add at least one file or folder".into()));
    }
    if req.format.has_fields() && req.text_field.trim().is_empty() {
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
    let opts = ReadOptions { text_field, id_field, text_mode: req.text_mode, allow_missing_text: false };
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
        produced_by: None,
        mix: None,
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
    Ok(JobOutcome::Produced(message, aeon_shared::JobLink::Dataset(manifest.id.clone())))
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
        source_desc: m
            .import
            .as_ref()
            .map(source_desc)
            .or_else(|| m.produced_by.as_ref().map(|p| format!("{} on {}", p.pipeline_name, p.input_name)))
            .or_else(|| {
                m.mix.as_ref().map(|mix| {
                    let names: Vec<&str> = mix.sources.iter().map(|s| s.dataset_name.as_str()).collect();
                    format!("Mix of {}", names.join(", "))
                })
            }),
        parent_run_id: m.produced_by.as_ref().map(|p| p.run_id.clone()),
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
    fn preview_lists_fields_and_suggests_the_text_field() {
        let src = tempfile::tempdir().unwrap();
        let csv = src.path().join("forum.csv");
        std::fs::write(&csv, "post_id,body,score\n1,\"A fairly long forum post about gardening.\",5\n2,Another post here,3\n").unwrap();
        let p = preview(&[csv.display().to_string()], ImportFormat::Auto, "text", TextMode::WholeFile).unwrap();
        assert_eq!(p.format, ImportFormat::Csv);
        assert_eq!(p.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["post_id", "body", "score"]);
        assert_eq!(p.suggested_text_field.as_deref(), Some("body"));
        assert!(p.sample_texts.is_empty(), "no field named text");
        let p = preview(&[csv.display().to_string()], ImportFormat::Auto, "body", TextMode::WholeFile).unwrap();
        assert_eq!(p.sample_texts[0], "A fairly long forum post about gardening.");

        let jsonl = src.path().join("d.jsonl");
        std::fs::write(&jsonl, "{\"id\":1,\"content\":\"hello there world\",\"n\":3}\nbad\n").unwrap();
        let p = preview(&[jsonl.display().to_string()], ImportFormat::Auto, "content", TextMode::WholeFile).unwrap();
        assert_eq!((p.records_sampled, p.parse_errors), (1, 1));
        assert_eq!(p.suggested_text_field.as_deref(), Some("content"));
        assert_eq!(p.suggested_id_field.as_deref(), Some("id"));
        assert_eq!(p.fields.iter().find(|f| f.name == "n").unwrap().example, "3");

        let txt = src.path().join("a.txt");
        std::fs::write(&txt, "line one\nline two\n").unwrap();
        let p = preview(&[txt.display().to_string()], ImportFormat::Auto, "text", TextMode::PerLine).unwrap();
        assert_eq!(p.sample_texts, vec!["line one", "line two"]);
    }

    #[test]
    fn validate_catches_missing_name_and_paths() {
        assert!(validate(&ImportRequest { name: " ".into(), ..request(vec!["x".into()]) }).is_err());
        assert!(validate(&request(vec![])).is_err());
    }
}
