//! Mixing several datasets into a new one, with per-dataset weights and an optional shuffle.
//!
//! Weight `w` means each document is included `floor(w)` times, plus once more with
//! probability `w - floor(w)`; the choice is a seeded hash of (dataset, row), so the same
//! request gives the same mix.
//!
//! The shuffle is a two-pass external shuffle: documents are first scattered into random
//! temporary buckets of about 64 MB, then each bucket is shuffled in memory and appended to the
//! output. Memory stays bounded while the order is uniformly random within the rounding of
//! bucket assignment.

use std::sync::Arc;

use aeon_shared::{DatasetManifest, JobLink, MixReport, MixRequest, MixSourceReport};
use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::dataset::{self, DatasetReader, DatasetWriter, NewDoc, TempDataset};
use crate::error::{AppError, AppResult, now_ms};
use crate::import::summary_of;
use crate::jobs::{JobCtx, JobOutcome};
use crate::workspace::Workspace;

const BUCKET_BYTES: u64 = 64 << 20;
const MAX_BUCKETS: u64 = 512;
/// Rows buffered per bucket writer before flushing, to bound memory with many buckets.
const BUCKET_BATCH_BYTES: usize = 2 << 20;

pub fn validate(req: &MixRequest) -> AppResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Give the mixed dataset a name".into()));
    }
    if req.sources.is_empty() {
        return Err(AppError::InvalidInput("Add at least one dataset to the mix".into()));
    }
    for s in &req.sources {
        if !(0.0..=100.0).contains(&s.weight) || s.weight.is_nan() {
            return Err(AppError::InvalidInput("Weights must be between 0 and 100".into()));
        }
    }
    let mut ids: Vec<&str> = req.sources.iter().map(|s| s.dataset_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != req.sources.len() {
        return Err(AppError::InvalidInput("Each dataset can be added to a mix only once".into()));
    }
    Ok(())
}

/// How many times to include row `row` of source `src` (deterministic for a seed).
fn copies(seed: u64, src: usize, row: u64, weight: f64) -> u64 {
    let whole = weight.floor();
    let frac = weight - whole;
    let mut n = whole as u64;
    if frac > 0.0 {
        let h = xxh3_64_with_seed(&[(src as u64).to_le_bytes(), row.to_le_bytes()].concat(), seed);
        if (h as f64 / u64::MAX as f64) < frac {
            n += 1;
        }
    }
    n
}

fn copy_key(seed: u64, src: usize, row: u64, copy: u64) -> u64 {
    xxh3_64_with_seed(&[(src as u64).to_le_bytes(), row.to_le_bytes(), copy.to_le_bytes()].concat(), seed ^ 0xB0C4E7)
}

/// Seeded Fisher–Yates.
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed | 1;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(i, (state % (i as u64 + 1)) as usize);
    }
}

struct Owned {
    id: String,
    text: String,
    source: String,
    meta: String,
}

pub fn run(ctx: &mut JobCtx, ws: Arc<Workspace>, req: MixRequest) -> AppResult<JobOutcome> {
    validate(&req)?;
    let readers: Vec<DatasetReader> =
        req.sources.iter().map(|s| DatasetReader::open(&ws.dataset_dir(&s.dataset_id)?)).collect::<AppResult<_>>()?;
    let total_rows: u64 = readers.iter().map(|r| r.row_count()).sum();
    // Estimate output text size to choose the number of shuffle buckets.
    let est_bytes: f64 = readers
        .iter()
        .zip(&req.sources)
        .map(|(r, s)| r.manifest().stats.text_bytes as f64 * s.weight)
        .sum();
    let buckets = ((est_bytes / BUCKET_BYTES as f64).ceil() as u64).clamp(1, MAX_BUCKETS);
    let passes = if req.shuffle { 2 } else { 1 };

    let datasets_dir = ws.datasets_dir();
    let id = dataset::new_id(&datasets_dir);
    let out_temp = TempDataset::create(&datasets_dir, &id)?;
    let mut writer = DatasetWriter::new(out_temp.path())?;
    let mut reports: Vec<MixSourceReport> = Vec::new();

    // When shuffling, the first pass writes into bucket datasets inside the temp folder.
    let bucket_dir = out_temp.path().join(".buckets");
    let mut bucket_writers: Vec<DatasetWriter> = Vec::new();
    if req.shuffle {
        for b in 0..buckets {
            let dir = bucket_dir.join(format!("b{b:04}"));
            std::fs::create_dir_all(&dir)?;
            bucket_writers.push(DatasetWriter::new(&dir)?.with_batch_bytes(BUCKET_BATCH_BYTES));
        }
    }

    let mut processed = 0u64;
    for (src, (reader, s)) in readers.iter().zip(&req.sources).enumerate() {
        let name = reader.manifest().name.clone();
        let mut report = MixSourceReport {
            dataset_id: s.dataset_id.clone(),
            dataset_name: name.clone(),
            weight: s.weight,
            docs_in: reader.row_count(),
            docs_out: 0,
            chars_out: 0,
        };
        let mut failure = None;
        let mut cancelled = false;
        reader.scan(0, None, |row, r| {
            processed += 1;
            for c in 0..copies(req.seed, src, row, s.weight) {
                let id = if c == 0 { r.id.to_string() } else { format!("{}#{}", r.id, c + 1) };
                let source = format!("{name}/{}", r.source);
                let doc = NewDoc { id: &id, text: r.text, source: &source, meta_json: r.meta };
                let result = if req.shuffle {
                    let b = (copy_key(req.seed, src, row, c) % buckets) as usize;
                    bucket_writers[b].push(doc)
                } else {
                    writer.push(doc)
                };
                if let Err(e) = result {
                    failure = Some(e);
                    return false;
                }
                report.docs_out += 1;
                report.chars_out += r.text.chars().count() as u64;
            }
            if processed.is_multiple_of(4096) {
                if ctx.is_cancelled() {
                    cancelled = true;
                    return false;
                }
                ctx.progress(
                    processed,
                    Some(total_rows * passes),
                    Some(format!("{}reading {name}", if req.shuffle { "Pass 1 of 2: " } else { "" })),
                );
            }
            true
        })?;
        if let Some(e) = failure {
            return Err(e);
        }
        if cancelled || ctx.is_cancelled() {
            return Ok(JobOutcome::Cancelled);
        }
        reports.push(report);
    }

    if req.shuffle {
        let bucket_parts: Vec<_> =
            bucket_writers.into_iter().map(|w| w.finish().map(|(parts, _)| parts)).collect::<AppResult<_>>()?;
        let out_docs: u64 = reports.iter().map(|r| r.docs_out).sum();
        let mut written = 0u64;
        for (b, parts) in bucket_parts.into_iter().enumerate() {
            if ctx.is_cancelled() {
                return Ok(JobOutcome::Cancelled);
            }
            let dir = bucket_dir.join(format!("b{b:04}"));
            let mut docs: Vec<Owned> = Vec::new();
            dataset::parts::scan_parts(&dir, &parts, 0, None, None, |_, batch, i| {
                let col = |n: &str| dataset::column(batch, n).map(|c| c.value(i).to_string());
                if let (Ok(id), Ok(text), Ok(source), Ok(meta)) = (col("id"), col("text"), col("source"), col("meta")) {
                    docs.push(Owned { id, text, source, meta });
                }
                true
            })?;
            shuffle(&mut docs, req.seed.wrapping_add(b as u64));
            for d in &docs {
                writer.push(NewDoc { id: &d.id, text: &d.text, source: &d.source, meta_json: &d.meta })?;
            }
            written += docs.len() as u64;
            drop(docs);
            std::fs::remove_dir_all(&dir)?;
            let done = total_rows + written * total_rows / out_docs.max(1);
            ctx.progress(done, Some(total_rows * 2), Some(format!("Pass 2 of 2: shuffling bucket {} of {buckets}", b + 1)));
        }
        let _ = std::fs::remove_dir_all(&bucket_dir);
    }

    let (parts, stats) = writer.finish()?;
    if stats.doc_count == 0 {
        return Err(AppError::InvalidInput("The mix is empty: all weights select no documents".into()));
    }
    let manifest = DatasetManifest {
        format_version: dataset::MANIFEST_VERSION,
        id: id.clone(),
        name: req.name.trim().to_string(),
        description: req.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(String::from),
        created_at_ms: now_ms(),
        parts,
        stats,
        import: None,
        produced_by: None,
        mix: Some(MixReport { request: req.clone(), sources: reports }),
    };
    dataset::write_manifest(out_temp.path(), &manifest)?;
    let dir = out_temp.commit()?;
    if let Err(e) = ws.catalog()?.insert_dataset(&summary_of(&manifest)) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(JobOutcome::Produced(
        format!("Mixed {} documents into \"{}\"", crate::export::thousands(manifest.stats.doc_count), manifest.name),
        JobLink::Dataset(id),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobEvents, JobManager};
    use aeon_shared::{ImportFormat, ImportRequest, JobKind, JobState, JobStatus, MixSource, TextMode};
    use std::time::{Duration, Instant};

    struct Quiet;
    impl JobEvents for Quiet {
        fn job_updated(&self, _: &JobStatus) {}
    }

    fn wait(jobs: &Arc<JobManager>, id: u64) -> JobStatus {
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

    fn import(ws: &Arc<Workspace>, jobs: &Arc<JobManager>, dir: &std::path::Path, name: &str, n: usize) -> String {
        let file = dir.join(format!("{name}.jsonl"));
        let lines: String =
            (0..n).map(|i| serde_json::json!({"id": format!("{name}{i}"), "text": format!("{name} text {i}")}).to_string() + "\n").collect();
        std::fs::write(&file, lines).unwrap();
        let req = ImportRequest {
            name: name.into(),
            description: None,
            paths: vec![file.display().to_string()],
            format: ImportFormat::Auto,
            text_field: "text".into(),
            id_field: Some("id".into()),
            text_mode: TextMode::WholeFile,
        };
        let w = ws.clone();
        let id = jobs.spawn(JobKind::Import, "i", move |ctx| crate::import::run(ctx, w, req)).unwrap();
        assert_eq!(wait(jobs, id).state, JobState::Completed);
        ws.catalog().unwrap().list_datasets().unwrap().into_iter().find(|d| d.name == name).unwrap().id
    }

    #[test]
    fn weights_select_copies_and_shuffle_keeps_every_document() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(dir.path()).unwrap());
        let src = tempfile::tempdir().unwrap();
        let jobs = JobManager::new(Arc::new(Quiet));
        let a = import(&ws, &jobs, src.path(), "alpha", 4000);
        let b = import(&ws, &jobs, src.path(), "beta", 1000);
        let req = MixRequest {
            name: "Mix".into(),
            description: None,
            sources: vec![MixSource { dataset_id: a, weight: 0.5 }, MixSource { dataset_id: b, weight: 2.0 }],
            shuffle: true,
            seed: 7,
        };
        let w = ws.clone();
        let r = req.clone();
        let id = jobs.spawn(JobKind::Mix, "m", move |ctx| run(ctx, w, r)).unwrap();
        let status = wait(&jobs, id);
        assert_eq!(status.state, JobState::Completed, "{:?}", status.message);

        let mixed = ws.catalog().unwrap().list_datasets().unwrap().into_iter().find(|d| d.name == "Mix").unwrap();
        let reader = DatasetReader::open(&ws.dataset_dir(&mixed.id).unwrap()).unwrap();
        let m = reader.manifest().mix.clone().unwrap();
        let (alpha_out, beta_out) = (m.sources[0].docs_out, m.sources[1].docs_out);
        assert!((1800..=2200).contains(&alpha_out), "about half of 4000: {alpha_out}");
        assert_eq!(beta_out, 2000);
        assert_eq!(reader.row_count(), alpha_out + beta_out);

        let mut ids = Vec::new();
        reader
            .scan(0, None, |_, r| {
                ids.push((r.id.to_string(), r.source.to_string()));
                true
            })
            .unwrap();
        // Every beta document appears twice (second copy marked #2), sources are prefixed.
        assert!(ids.iter().any(|(id, s)| id == "beta7#2" && s == "beta/beta.jsonl"));
        let unique: std::collections::HashSet<_> = ids.iter().map(|(i, _)| i).collect();
        assert_eq!(unique.len(), ids.len());
        // Shuffled: the first 100 rows are not all from one source.
        let first_alpha = ids[..100].iter().filter(|(i, _)| i.starts_with("alpha")).count();
        assert!((10..90).contains(&first_alpha), "{first_alpha}");
        // No temp buckets left.
        assert!(!ws.dataset_dir(&mixed.id).unwrap().join(".buckets").exists());

        // Same seed, same selection.
        let w = ws.clone();
        let r = MixRequest { name: "Mix 2".into(), ..req };
        let id = jobs.spawn(JobKind::Mix, "m", move |ctx| run(ctx, w, r)).unwrap();
        wait(&jobs, id);
        let again = ws.catalog().unwrap().list_datasets().unwrap().into_iter().find(|d| d.name == "Mix 2").unwrap();
        assert_eq!(again.doc_count, mixed.doc_count);
    }

    #[test]
    fn validation() {
        let ok = MixRequest {
            name: "m".into(),
            description: None,
            sources: vec![MixSource { dataset_id: "a".into(), weight: 1.0 }],
            shuffle: false,
            seed: 1,
        };
        assert!(validate(&ok).is_ok());
        assert!(validate(&MixRequest { sources: vec![], ..ok.clone() }).is_err());
        let dup = vec![MixSource { dataset_id: "a".into(), weight: 1.0 }, MixSource { dataset_id: "a".into(), weight: 2.0 }];
        assert!(validate(&MixRequest { sources: dup, ..ok.clone() }).is_err());
        let neg = vec![MixSource { dataset_id: "a".into(), weight: -1.0 }];
        assert!(validate(&MixRequest { sources: neg, ..ok }).is_err());
    }

    #[test]
    fn copies_follow_weight() {
        let n: u64 = (0..10_000).map(|r| copies(3, 0, r, 0.3)).sum();
        assert!((2700..3300).contains(&n), "{n}");
        assert_eq!((0..100).map(|r| copies(3, 0, r, 2.0)).sum::<u64>(), 200);
        assert_eq!(copies(3, 0, 5, 0.0), 0);
    }
}
