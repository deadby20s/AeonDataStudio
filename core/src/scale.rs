//! Scale test: the whole workflow on millions of documents, with timings, peak memory and disk
//! use. Opt-in (ignored by default); runs in CI via `.github/workflows/scale-test.yml`:
//!
//! ```text
//! SCALE_DOCS=2000000 cargo test --release -p aeondatastudio --no-default-features scale_test -- --ignored --nocapture
//! ```
//!
//! Documents are generated from a synthetic vocabulary so they are mostly unique (real dedup
//! work rather than one huge duplicate cluster); 5% are exact and 5% near copies of earlier ones.
//! The language filter is turned off because the vocabulary is not a real language.

use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aeon_shared::{
    ExportCompression, ExportFormat, ExportRequest, ImportFormat, ImportRequest, JobKind, JobState, JobStatus,
    PipelineSpec, TextMode, TokenizerChoice,
};

use crate::jobs::{JobEvents, JobManager};
use crate::workspace::Workspace;

struct Quiet;
impl JobEvents for Quiet {
    fn job_updated(&self, _: &JobStatus) {}
}

fn wait(jobs: &Arc<JobManager>, id: u64) -> JobStatus {
    let deadline = Instant::now() + Duration::from_secs(4 * 3600);
    loop {
        let s = jobs.list().unwrap().into_iter().find(|s| s.id == id).unwrap();
        if s.state.is_finished() {
            return s;
        }
        assert!(Instant::now() < deadline, "job {id} took more than 4 hours");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .map(|it| {
            it.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    _ => e.metadata().map(|m| m.len()).unwrap_or(0),
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Peak resident memory of this process (Linux only).
fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status.lines().find(|l| l.starts_with("VmHWM:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn generate(path: &Path, n: u64) -> u64 {
    const SYL: [&str; 24] = [
        "ka", "lo", "mi", "ten", "ra", "vo", "sel", "dun", "pa", "ri", "mor", "ta", "ne", "gul", "fa", "sto", "be", "lin",
        "cor", "da", "pe", "nu", "shi", "zo",
    ];
    let mut rng = Rng(42);
    let vocab: Vec<String> = (0..2000)
        .map(|_| (0..2 + rng.below(2)).map(|_| SYL[rng.below(SYL.len())]).collect::<String>())
        .collect();
    let mut recent: Vec<String> = Vec::with_capacity(1000);
    let mut out = BufWriter::with_capacity(1 << 22, std::fs::File::create(path).unwrap());
    let mut bytes = 0u64;
    for i in 0..n {
        let roll = rng.below(100);
        let text = if roll < 5 && !recent.is_empty() {
            recent[rng.below(recent.len())].clone()
        } else if roll < 10 && !recent.is_empty() {
            let mut words: Vec<String> = recent[rng.below(recent.len())].split(' ').map(String::from).collect();
            let k = rng.below(words.len());
            words[k] = vocab[rng.below(vocab.len())].clone();
            words.join(" ")
        } else {
            let len = 80 + rng.below(320);
            let mut s = String::with_capacity(len * 7);
            for w in 0..len {
                if w > 0 {
                    s.push(' ');
                }
                let word = &vocab[rng.below(vocab.len())];
                if w % 14 == 0 {
                    let mut c = word.chars();
                    s.extend(c.next().map(|f| f.to_ascii_uppercase()));
                    s.push_str(c.as_str());
                } else {
                    s.push_str(word);
                }
                if w % 14 == 13 {
                    s.push('.');
                }
            }
            s.push('.');
            if recent.len() < 1000 {
                recent.push(s.clone());
            } else {
                let k = rng.below(1000);
                recent[k] = s.clone();
            }
            s
        };
        let line = serde_json::json!({"id": format!("s{i}"), "text": text}).to_string();
        bytes += line.len() as u64 + 1;
        out.write_all(line.as_bytes()).unwrap();
        out.write_all(b"\n").unwrap();
    }
    out.flush().unwrap();
    bytes
}

#[test]
#[ignore]
fn scale_test() {
    let n: u64 = std::env::var("SCALE_DOCS").ok().and_then(|v| v.parse().ok()).unwrap_or(1_000_000);
    let keep = tempfile::tempdir().unwrap();
    let base = std::env::var("SCALE_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| keep.path().to_path_buf());
    std::fs::create_dir_all(&base).unwrap();
    let ws_dir = base.join("workspace");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut phase = |name: &str, started: Instant, detail: String| {
        let line = format!("{:.1} s", started.elapsed().as_secs_f64());
        println!("{name}: {line} — {detail}");
        rows.push((name.to_string(), format!("{line} | {detail}")));
    };

    let t = Instant::now();
    let src = base.join("source.jsonl");
    let src_bytes = generate(&src, n);
    phase("Generate", t, format!("{n} docs, {:.2} GB JSONL", src_bytes as f64 / 1e9));

    let ws = Arc::new(Workspace::open_or_init(&ws_dir).unwrap());
    let jobs = JobManager::new(Arc::new(Quiet));

    let t = Instant::now();
    let req = ImportRequest {
        name: "Scale".into(),
        description: None,
        paths: vec![src.display().to_string()],
        format: ImportFormat::Jsonl,
        text_field: "text".into(),
        id_field: Some("id".into()),
        text_mode: TextMode::WholeFile,
    };
    let w = ws.clone();
    let s = wait(&jobs, jobs.spawn(JobKind::Import, "import", move |ctx| crate::import::run(ctx, w, req)).unwrap());
    assert_eq!(s.state, JobState::Completed, "{:?}", s.message);
    let input = ws.catalog().unwrap().list_datasets().unwrap()[0].clone();
    phase(
        "Import",
        t,
        format!("{:.0} MB/s of JSONL, dataset {:.2} GB on disk", src_bytes as f64 / 1e6 / t.elapsed().as_secs_f64(), input.byte_size.unwrap_or(0) as f64 / 1e9),
    );
    std::fs::remove_file(&src).unwrap();

    let t = Instant::now();
    let mut spec = PipelineSpec::template("Scale template");
    for step in &mut spec.steps {
        if step.config.kind() == "language_filter" {
            step.enabled = false;
        }
    }
    let w = ws.clone();
    let id = input.id.clone();
    let s = wait(
        &jobs,
        jobs.spawn(JobKind::PipelineRun, "run", move |ctx| {
            crate::pipeline::run(ctx, w, crate::pipeline::RunRequest { spec, input_dataset_id: id, output_name: String::new() })
        })
        .unwrap(),
    );
    assert_eq!(s.state, JobState::Completed, "{:?}", s.message);
    let run = ws.catalog().unwrap().list_runs().unwrap()[0].clone();
    let report = crate::pipeline::read_report(&ws, &run.id).unwrap();
    let by_step: Vec<String> =
        report.steps.iter().filter(|s| s.dropped > 0).map(|s| format!("{} −{}", s.label, s.dropped)).collect();
    phase(
        "Pipeline (template incl. PII masking, exact + near dedup)",
        t,
        format!(
            "{:.0} docs/s, kept {} of {}; {}",
            n as f64 / t.elapsed().as_secs_f64(),
            report.docs_out,
            report.docs_in,
            by_step.join(", ")
        ),
    );

    let t = Instant::now();
    let req = ExportRequest {
        dataset_id: run.output_dataset_id.clone().unwrap(),
        name: "scale tokens".into(),
        format: ExportFormat::Tokens,
        compression: ExportCompression::None,
        shard_mb: 512,
        include_meta: false,
        tokenizer: Some(TokenizerChoice::Tiktoken("r50k_base".into())),
        append_eos: true,
        output_dir: None,
    };
    let w = ws.clone();
    let s = wait(&jobs, jobs.spawn(JobKind::Export, "export", move |ctx| crate::export::run(ctx, w, req, "scale".into())).unwrap());
    assert_eq!(s.state, JobState::Completed, "{:?}", s.message);
    phase("Token export (r50k)", t, s.message.unwrap_or_default());

    let disk = ["datasets", "runs", "exports"]
        .iter()
        .map(|d| format!("{d} {:.2} GB", dir_size(&ws_dir.join(d)) as f64 / 1e9))
        .collect::<Vec<_>>()
        .join(", ");
    let mem = peak_rss_mb().map_or("n/a".to_string(), |m| format!("{m:.0} MB"));
    println!("Peak memory: {mem}; disk: {disk}");

    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        let mut md = format!(
            "## Scale test: {n} documents\n\nRunner: {} CPUs. Peak memory: **{mem}**. Disk: {disk}.\n\n| Phase | Time | Detail |\n|---|---|---|\n",
            std::thread::available_parallelism().map_or(0, |p| p.get())
        );
        for (name, rest) in &rows {
            md.push_str(&format!("| {name} | {rest} |\n"));
        }
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(summary).unwrap();
        f.write_all(md.as_bytes()).unwrap();
    }
}
