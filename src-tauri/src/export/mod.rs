//! Exporting a dataset for training, into the workspace's `exports/<name>/` folder.
//!
//! ```text
//! exports/<name>/
//! ├── manifest.json                  ExportManifest: source dataset and lineage, settings, files + SHA-256
//! ├── data-00000.jsonl.zst ...       JSONL (optionally gzip/zstd)
//! ├── data-00000.parquet ...         or Parquet (id, text, source, meta)
//! └── tokens-00000.bin + .idx ...    or token ids (uint16/uint32 LE) and document offsets (uint64 LE)
//! ```
//!
//! For token shards, `.idx` holds `docs + 1` offsets into the `.bin` file, counted in tokens:
//! document `i` is tokens `idx[i] .. idx[i+1]` (including its end-of-text token, if appended).
//! The folder is written as `.exporting-<name>` and renamed when complete.

mod tokenize;

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aeon_shared::{
    ExportCompression, ExportFile, ExportFormat, ExportManifest, ExportRequest, ExportSummary, JobLink,
};
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::dataset::{DatasetReader, DatasetWriter, NewDoc, TempDataset};
use crate::error::{AppError, AppResult, now_ms};
use crate::jobs::{JobCtx, JobOutcome};
use crate::workspace::Workspace;
pub use tokenize::Tokenizer;

const MANIFEST_FILE: &str = "manifest.json";
const TEMP_PREFIX: &str = ".exporting-";
const BATCH_DOCS: usize = 4096;

/// `My Export v2` → `my-export-v2`.
pub fn folder_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches(['-', '.']).chars().take(80).collect();
    if out.is_empty() { "export".into() } else { out }
}

fn is_valid_folder(f: &str) -> bool {
    !f.is_empty()
        && !f.starts_with('.')
        && f.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_'))
}

pub fn export_dir(ws: &Workspace, folder: &str) -> AppResult<PathBuf> {
    if !is_valid_folder(folder) {
        return Err(AppError::InvalidInput(format!("Invalid export folder: {folder:?}")));
    }
    Ok(ws.exports_dir().join(folder))
}

pub fn validate(req: &ExportRequest) -> AppResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Give the export a name".into()));
    }
    if req.shard_mb == 0 || req.shard_mb > 100_000 {
        return Err(AppError::InvalidInput("Shard size must be between 1 MB and 100 GB".into()));
    }
    if req.format == ExportFormat::Tokens && req.tokenizer.is_none() {
        return Err(AppError::InvalidInput("Choose a tokenizer".into()));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> AppResult<String> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Documents read from the dataset, in batches, with cancellation.
fn for_each_batch(
    reader: &DatasetReader,
    ctx: &mut JobCtx,
    mut f: impl FnMut(&[(String, String, String, String)], &mut JobCtx) -> AppResult<()>,
) -> AppResult<bool> {
    let mut batch = Vec::with_capacity(BATCH_DOCS);
    let mut failure = None;
    let mut cancelled = false;
    reader.scan(0, None, |_, r| {
        batch.push((r.id.to_string(), r.text.to_string(), r.source.to_string(), r.meta.to_string()));
        if batch.len() >= BATCH_DOCS {
            if ctx.is_cancelled() {
                cancelled = true;
                return false;
            }
            if let Err(e) = f(&batch, ctx) {
                failure = Some(e);
                return false;
            }
            batch.clear();
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
        f(&batch, ctx)?;
    }
    Ok(true)
}

/// Rotating writer for JSONL or token shards.
struct ShardFiles {
    dir: PathBuf,
    files: Vec<ExportFile>,
}

impl ShardFiles {
    fn next_name(&self, prefix: &str, ext: &str) -> String {
        format!("{prefix}-{:05}.{ext}", self.files.len())
    }
}

struct JsonlShard {
    file: String,
    out: Box<dyn Write>,
    docs: u64,
    bytes: u64,
}

fn open_jsonl(dir: &Path, file: &str, compression: ExportCompression) -> AppResult<Box<dyn Write>> {
    let raw = BufWriter::with_capacity(1 << 20, File::create(dir.join(file))?);
    Ok(match compression {
        ExportCompression::None => Box::new(raw),
        ExportCompression::Gzip => Box::new(flate2::write::GzEncoder::new(raw, flate2::Compression::default())),
        ExportCompression::Zstd => Box::new(zstd::stream::write::Encoder::new(raw, 3)?.auto_finish()),
    })
}

fn export_jsonl(
    ctx: &mut JobCtx,
    reader: &DatasetReader,
    req: &ExportRequest,
    dir: &Path,
) -> AppResult<Option<(Vec<ExportFile>, u64)>> {
    let ext = match req.compression {
        ExportCompression::None => "jsonl",
        ExportCompression::Gzip => "jsonl.gz",
        ExportCompression::Zstd => "jsonl.zst",
    };
    let shard_bytes = req.shard_mb << 20;
    let total = reader.row_count();
    let mut shards = ShardFiles { dir: dir.to_path_buf(), files: Vec::new() };
    let mut current: Option<JsonlShard> = None;
    let mut done = 0u64;

    let close = |shard: JsonlShard, shards: &mut ShardFiles| -> AppResult<()> {
        let JsonlShard { file, mut out, docs, .. } = shard;
        out.flush()?;
        drop(out); // finishes the compressor
        let path = shards.dir.join(&file);
        let bytes = std::fs::metadata(&path)?.len();
        shards.files.push(ExportFile { file, docs, bytes, sha256: String::new(), tokens: None });
        Ok(())
    };

    let completed = for_each_batch(reader, ctx, |batch, ctx| {
        let lines: Vec<String> = batch
            .par_iter()
            .map(|(id, text, source, meta)| {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), id.as_str().into());
                obj.insert("text".into(), text.as_str().into());
                obj.insert("source".into(), source.as_str().into());
                if req.include_meta {
                    let m = serde_json::from_str::<serde_json::Value>(meta).unwrap_or_default();
                    if m.as_object().is_some_and(|o| !o.is_empty()) {
                        obj.insert("meta".into(), m);
                    }
                }
                serde_json::Value::Object(obj).to_string()
            })
            .collect();
        for line in lines {
            if current.is_none() {
                let file = shards.next_name("data", ext);
                let out = open_jsonl(&shards.dir, &file, req.compression)?;
                current = Some(JsonlShard { file, out, docs: 0, bytes: 0 });
            }
            let shard = current.as_mut().expect("shard is open");
            shard.out.write_all(line.as_bytes())?;
            shard.out.write_all(b"\n")?;
            shard.docs += 1;
            shard.bytes += line.len() as u64 + 1;
            if shard.bytes >= shard_bytes {
                close(current.take().expect("shard is open"), &mut shards)?;
            }
        }
        done += batch.len() as u64;
        ctx.progress(done, Some(total), Some(format!("{} shard(s) written", shards.files.len())));
        Ok(())
    })?;
    if !completed {
        return Ok(None);
    }
    if let Some(shard) = current.take() {
        close(shard, &mut shards)?;
    }
    Ok(Some((shards.files, done)))
}

fn export_parquet(
    ctx: &mut JobCtx,
    reader: &DatasetReader,
    req: &ExportRequest,
    dir: &Path,
) -> AppResult<Option<(Vec<ExportFile>, u64)>> {
    let total = reader.row_count();
    let mut writer = DatasetWriter::for_export(dir, "data", req.shard_mb << 20)?;
    let mut done = 0u64;
    let completed = for_each_batch(reader, ctx, |batch, ctx| {
        for (id, text, source, meta) in batch {
            let meta = if req.include_meta { meta.as_str() } else { "{}" };
            writer.push(NewDoc { id, text, source, meta_json: meta })?;
        }
        done += batch.len() as u64;
        ctx.progress(done, Some(total), None);
        Ok(())
    })?;
    if !completed {
        return Ok(None);
    }
    let (parts, _) = writer.finish()?;
    let files = parts
        .into_iter()
        .map(|p| ExportFile { file: p.file, docs: p.rows, bytes: p.bytes, sha256: String::new(), tokens: None })
        .collect();
    Ok(Some((files, done)))
}

struct TokenShard {
    name: String,
    bin: BufWriter<File>,
    idx: BufWriter<File>,
    docs: u64,
    tokens: u64,
}

fn export_tokens(
    ctx: &mut JobCtx,
    reader: &DatasetReader,
    req: &ExportRequest,
    dir: &Path,
    tok: &Tokenizer,
) -> AppResult<Option<(Vec<ExportFile>, u64, u64)>> {
    let info = tok.info().clone();
    let eos = if req.append_eos {
        Some(info.eos_id.ok_or_else(|| {
            AppError::InvalidInput(format!(
                "{} has no recognizable end-of-text token; turn off \"Append end-of-text token\"",
                info.name
            ))
        })?)
    } else {
        None
    };
    let wide = info.dtype == "uint32";
    let bytes_per_token = if wide { 4 } else { 2 };
    let shard_tokens = ((req.shard_mb << 20) / bytes_per_token).max(1);
    let total = reader.row_count();
    let mut files: Vec<ExportFile> = Vec::new();
    let mut current: Option<TokenShard> = None;
    let mut done = 0u64;
    let mut all_tokens = 0u64;

    let close = |shard: TokenShard, files: &mut Vec<ExportFile>| -> AppResult<()> {
        let TokenShard { name, mut bin, mut idx, docs, tokens } = shard;
        bin.flush()?;
        idx.flush()?;
        drop((bin, idx));
        for (ext, doc_count, tok_count) in [("bin", docs, Some(tokens)), ("idx", docs, None)] {
            let file = format!("{name}.{ext}");
            let bytes = std::fs::metadata(dir.join(&file))?.len();
            files.push(ExportFile { file, docs: doc_count, bytes, sha256: String::new(), tokens: tok_count });
        }
        Ok(())
    };

    let completed = for_each_batch(reader, ctx, |batch, ctx| {
        let encoded: Vec<AppResult<Vec<u32>>> = batch.par_iter().map(|(_, text, _, _)| tok.encode(text)).collect();
        for ids in encoded {
            let mut ids = ids?;
            if let Some(e) = eos {
                ids.push(e);
            }
            if current.is_none() {
                let name = format!("tokens-{:05}", files.len() / 2);
                let mut idx = BufWriter::new(File::create(dir.join(format!("{name}.idx")))?);
                idx.write_all(&0u64.to_le_bytes())?;
                let bin = BufWriter::with_capacity(1 << 20, File::create(dir.join(format!("{name}.bin")))?);
                current = Some(TokenShard { name, bin, idx, docs: 0, tokens: 0 });
            }
            let shard = current.as_mut().expect("shard is open");
            for id in &ids {
                if wide {
                    shard.bin.write_all(&id.to_le_bytes())?;
                } else {
                    let narrow = u16::try_from(*id).map_err(|_| AppError::Internal(format!("token id {id} does not fit uint16")))?;
                    shard.bin.write_all(&narrow.to_le_bytes())?;
                }
            }
            shard.tokens += ids.len() as u64;
            shard.docs += 1;
            shard.idx.write_all(&shard.tokens.to_le_bytes())?;
            all_tokens += ids.len() as u64;
            if shard.tokens >= shard_tokens {
                close(current.take().expect("shard is open"), &mut files)?;
            }
        }
        done += batch.len() as u64;
        ctx.progress(done, Some(total), Some(format!("{} tokens", crate::export::thousands(all_tokens))));
        Ok(())
    })?;
    if !completed {
        return Ok(None);
    }
    if let Some(shard) = current.take() {
        close(shard, &mut files)?;
    }
    Ok(Some((files, done, all_tokens)))
}

pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn run(ctx: &mut JobCtx, ws: Arc<Workspace>, req: ExportRequest, app_version: String) -> AppResult<JobOutcome> {
    validate(&req)?;
    let reader = DatasetReader::open(&ws.dataset_dir(&req.dataset_id)?)?;
    let folder = folder_name(&req.name);
    let exports = ws.exports_dir();
    if exports.join(&folder).exists() {
        return Err(AppError::InvalidInput(format!("An export folder named \"{folder}\" already exists")));
    }
    // Load the tokenizer before writing anything, so a bad tokenizer fails fast.
    let tokenizer = match (&req.format, &req.tokenizer) {
        (ExportFormat::Tokens, Some(choice)) => {
            ctx.progress(0, None, Some("Loading tokenizer…".into()));
            Some(Tokenizer::load(choice)?)
        }
        _ => None,
    };
    let temp = TempDataset::create_with_prefix(&exports, &folder, TEMP_PREFIX)?;

    let result = match req.format {
        ExportFormat::Jsonl => export_jsonl(ctx, &reader, &req, temp.path())?.map(|(f, d)| (f, d, None)),
        ExportFormat::Parquet => export_parquet(ctx, &reader, &req, temp.path())?.map(|(f, d)| (f, d, None)),
        ExportFormat::Tokens => {
            let tok = tokenizer.as_ref().expect("loaded above");
            export_tokens(ctx, &reader, &req, temp.path(), tok)?.map(|(f, d, t)| (f, d, Some(t)))
        }
    };
    let Some((mut files, docs, tokens)) = result else {
        return Ok(JobOutcome::Cancelled);
    };

    ctx.progress(docs, Some(docs), Some("Computing checksums…".into()));
    let hashes: Vec<AppResult<String>> = files.par_iter().map(|f| sha256_file(&temp.path().join(&f.file))).collect();
    for (f, h) in files.iter_mut().zip(hashes) {
        f.sha256 = h?;
    }

    let manifest = ExportManifest {
        format_version: 1,
        name: req.name.trim().to_string(),
        created_at_ms: now_ms(),
        app_version,
        dataset_id: req.dataset_id.clone(),
        dataset_name: reader.manifest().name.clone(),
        dataset_produced_by: reader.manifest().produced_by.clone(),
        request: req.clone(),
        docs,
        tokens,
        tokenizer: tokenizer.map(|t| t.info().clone()),
        files,
    };
    std::fs::write(temp.path().join(MANIFEST_FILE), serde_json::to_string_pretty(&manifest)?)?;
    std::fs::write(temp.path().join("SHA256SUMS"), checksum_file(&manifest))?;
    temp.commit()?;

    let size: u64 = manifest.files.iter().map(|f| f.bytes).sum();
    let mut message = format!(
        "Exported {} documents to exports/{folder} ({} file{}, {:.1} MB)",
        thousands(docs),
        manifest.files.len(),
        if manifest.files.len() == 1 { "" } else { "s" },
        size as f64 / 1_048_576.0
    );
    if let Some(t) = tokens {
        message.push_str(&format!(", {} tokens", thousands(t)));
    }
    Ok(JobOutcome::Produced(message, JobLink::Export(folder)))
}

/// `sha256sum`-compatible listing, so `sha256sum -c SHA256SUMS` verifies the export.
fn checksum_file(m: &ExportManifest) -> String {
    m.files.iter().map(|f| format!("{}  {}\n", f.sha256, f.file)).collect()
}

pub fn read_manifest(dir: &Path) -> AppResult<ExportManifest> {
    Ok(serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST_FILE))?)?)
}

pub fn list(ws: &Workspace) -> AppResult<Vec<ExportSummary>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(ws.exports_dir())? {
        let entry = entry?;
        let folder = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() || !is_valid_folder(&folder) {
            continue;
        }
        let Ok(m) = read_manifest(&entry.path()) else { continue };
        out.push(ExportSummary {
            folder,
            path: entry.path().display().to_string(),
            name: m.name,
            created_at_ms: m.created_at_ms,
            format: m.request.format,
            dataset_name: m.dataset_name,
            docs: m.docs,
            tokens: m.tokens,
            files: m.files.len() as u64,
            bytes: m.files.iter().map(|f| f.bytes).sum(),
        });
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.created_at_ms));
    Ok(out)
}

pub fn delete(ws: &Workspace, folder: &str) -> AppResult<()> {
    let dir = export_dir(ws, folder)?;
    if !dir.join(MANIFEST_FILE).is_file() {
        return Err(AppError::InvalidInput(format!("exports/{folder} is not an export made by AeonDataStudio")));
    }
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

pub fn remove_stale(exports: &Path) {
    crate::dataset::remove_stale_temp_dirs(exports, TEMP_PREFIX);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobEvents, JobManager};
    use aeon_shared::{ImportFormat, ImportRequest, JobKind, JobState, JobStatus, TextMode, TokenizerChoice};
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

    fn setup(n: usize) -> (tempfile::TempDir, Arc<Workspace>, Arc<JobManager>, String) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(Workspace::open_or_init(&dir.path().join("ws")).unwrap_or_else(|_| {
            std::fs::create_dir_all(dir.path().join("ws")).unwrap();
            Workspace::open_or_init(&dir.path().join("ws")).unwrap()
        }));
        let file = dir.path().join("src.jsonl");
        let lines: String = (0..n)
            .map(|i| {
                serde_json::json!({"id": format!("d{i}"), "text": format!("Document number {i} says hello."), "lang": "en"})
                    .to_string()
                    + "\n"
            })
            .collect();
        std::fs::write(&file, lines).unwrap();
        let jobs = JobManager::new(Arc::new(Quiet));
        let req = ImportRequest {
            name: "Src".into(),
            description: None,
            paths: vec![file.display().to_string()],
            format: ImportFormat::Auto,
            text_field: "text".into(),
            id_field: Some("id".into()),
            text_mode: TextMode::WholeFile,
        };
        let w = ws.clone();
        let id = jobs.spawn(JobKind::Import, "i", move |ctx| crate::import::run(ctx, w, req)).unwrap();
        assert_eq!(wait(&jobs, id).state, JobState::Completed);
        let ds = ws.catalog().unwrap().list_datasets().unwrap()[0].id.clone();
        (dir, ws, jobs, ds)
    }

    fn export(ws: &Arc<Workspace>, jobs: &Arc<JobManager>, req: ExportRequest) -> JobStatus {
        let w = ws.clone();
        let id = jobs.spawn(JobKind::Export, "e", move |ctx| run(ctx, w, req, "test".into())).unwrap();
        wait(jobs, id)
    }

    fn request(ds: &str, name: &str, format: ExportFormat) -> ExportRequest {
        ExportRequest {
            dataset_id: ds.into(),
            name: name.into(),
            format,
            compression: ExportCompression::Zstd,
            shard_mb: 1,
            include_meta: true,
            tokenizer: None,
            append_eos: true,
        }
    }

    #[test]
    fn jsonl_export_shards_compresses_and_checksums() {
        let (_d, ws, jobs, ds) = setup(60_000);
        let s = export(&ws, &jobs, request(&ds, "My Export", ExportFormat::Jsonl));
        assert_eq!(s.state, JobState::Completed, "{:?}", s.message);
        let dir = ws.exports_dir().join("my-export");
        let m = read_manifest(&dir).unwrap();
        assert!(m.files.len() >= 3, "~4.5 MB of JSON in 1 MB shards: {}", m.files.len());
        assert_eq!(m.files.iter().map(|f| f.docs).sum::<u64>(), 60_000);
        assert_eq!(m.docs, 60_000);
        let first = &m.files[0];
        assert!(first.file.ends_with(".jsonl.zst"));
        assert_eq!(first.sha256, sha256_file(&dir.join(&first.file)).unwrap());
        let text = String::from_utf8(zstd::decode_all(File::open(dir.join(&first.file)).unwrap()).unwrap()).unwrap();
        let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["id"], "d0");
        assert_eq!(line["meta"]["lang"], "en");
        assert!(std::fs::read_to_string(dir.join("SHA256SUMS")).unwrap().contains(&first.sha256));
        // Same name again is refused rather than overwritten.
        assert_eq!(export(&ws, &jobs, request(&ds, "my export", ExportFormat::Jsonl)).state, JobState::Failed);
        assert_eq!(list(&ws).unwrap().len(), 1);
        delete(&ws, "my-export").unwrap();
        assert!(list(&ws).unwrap().is_empty());
    }

    #[test]
    fn parquet_export_without_meta() {
        let (_d, ws, jobs, ds) = setup(1000);
        let mut req = request(&ds, "pq", ExportFormat::Parquet);
        req.include_meta = false;
        assert_eq!(export(&ws, &jobs, req).state, JobState::Completed);
        let m = read_manifest(&ws.exports_dir().join("pq")).unwrap();
        assert_eq!(m.files[0].file, "data-00000.parquet");
        assert_eq!(m.docs, 1000);
    }

    #[test]
    fn token_export_writes_bin_and_idx_that_decode_back() {
        let (_d, ws, jobs, ds) = setup(500);
        let mut req = request(&ds, "tok", ExportFormat::Tokens);
        req.tokenizer = Some(TokenizerChoice::Tiktoken("r50k_base".into()));
        let s = export(&ws, &jobs, req);
        assert_eq!(s.state, JobState::Completed, "{:?}", s.message);
        let dir = ws.exports_dir().join("tok");
        let m = read_manifest(&dir).unwrap();
        assert_eq!(m.tokenizer.as_ref().unwrap().dtype, "uint16");
        let bin = std::fs::read(dir.join("tokens-00000.bin")).unwrap();
        let idx = std::fs::read(dir.join("tokens-00000.idx")).unwrap();
        let ids: Vec<u32> = bin.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect();
        let offs: Vec<u64> = idx.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(offs.len(), 501);
        assert_eq!(*offs.last().unwrap() as usize, ids.len());
        assert_eq!(m.tokens, Some(ids.len() as u64));
        let tok = Tokenizer::load(&TokenizerChoice::Tiktoken("r50k_base".into())).unwrap();
        let doc1 = &ids[offs[1] as usize..offs[2] as usize];
        assert_eq!(*doc1.last().unwrap(), 50_256, "ends with <|endoftext|>");
        assert_eq!(tok.decode(&doc1[..doc1.len() - 1]), "Document number 1 says hello.");
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("My Export v2!"), "my-export-v2");
        assert_eq!(folder_name("../../etc"), "etc");
        assert_eq!(folder_name("  "), "export");
        assert!(!is_valid_folder("../x") && !is_valid_folder(".exporting-a"));
    }
}
