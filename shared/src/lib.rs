//! Types exchanged between the frontend (WASM) and the backend (Tauri).
//!
//! Keep this crate free of platform-specific dependencies: it must compile for both
//! `wasm32-unknown-unknown` and the native target.

use serde::{Deserialize, Serialize};

pub mod export;
pub mod pipeline;
pub use export::*;
pub use pipeline::*;

/// Name of the Tauri event emitted whenever a job's status changes.
/// The payload is a [`JobStatus`].
pub const JOB_UPDATE_EVENT: &str = "job://update";

/// Startup information for the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    /// Whether this build can update itself (it was built with the update signing key).
    #[serde(default)]
    pub updates_configured: bool,
    pub workspace: Option<WorkspaceInfo>,
    /// Recently opened workspace folders, most recent first.
    pub recent_workspaces: Vec<String>,
}

/// An available update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// The currently open workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub path: String,
    pub created_at_ms: i64,
    pub dataset_count: u64,
    pub run_count: u64,
}

/// One row of the dataset catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at_ms: i64,
    pub doc_count: Option<u64>,
    /// Total characters of document text.
    pub char_count: Option<u64>,
    /// Size of the dataset's files on disk.
    pub byte_size: Option<u64>,
    /// Short description of where the data came from, e.g. "3 JSONL files".
    pub source_desc: Option<String>,
    /// The run that produced this dataset, if it was not imported directly.
    pub parent_run_id: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImportFormat {
    /// Decide per file from its extension (`.gz` / `.zst` suffixes are looked through).
    Auto,
    /// One JSON object per line (`.jsonl`, `.ndjson`).
    Jsonl,
    /// A JSON array of objects (`.json`).
    Json,
    /// Plain text (`.txt`, `.md`, `.text`), split according to [`TextMode`].
    Text,
    /// Comma-separated (`.csv`) or tab-separated (`.tsv`) with a header row.
    Csv,
    Parquet,
    /// Web pages (`.html`, `.htm`): one document per file, main text extracted.
    Html,
    /// Web archives (`.warc`, usually `.warc.gz`, e.g. Common Crawl): one document per HTML page.
    Warc,
}

impl ImportFormat {
    pub const ALL: [ImportFormat; 8] = [
        ImportFormat::Auto,
        ImportFormat::Jsonl,
        ImportFormat::Json,
        ImportFormat::Text,
        ImportFormat::Csv,
        ImportFormat::Parquet,
        ImportFormat::Html,
        ImportFormat::Warc,
    ];

    /// Formats where the text is extracted rather than taken from a field.
    pub fn has_fields(self) -> bool {
        !matches!(self, ImportFormat::Text | ImportFormat::Html | ImportFormat::Warc)
    }

    pub fn label(self) -> &'static str {
        match self {
            ImportFormat::Auto => "Detect from extension",
            ImportFormat::Jsonl => "JSONL",
            ImportFormat::Json => "JSON array",
            ImportFormat::Text => "Plain text",
            ImportFormat::Csv => "CSV / TSV",
            ImportFormat::Parquet => "Parquet",
            ImportFormat::Html => "HTML pages",
            ImportFormat::Warc => "WARC web archive",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            ImportFormat::Auto => "auto",
            ImportFormat::Jsonl => "JSONL",
            ImportFormat::Json => "JSON",
            ImportFormat::Text => "text",
            ImportFormat::Csv => "CSV",
            ImportFormat::Parquet => "Parquet",
            ImportFormat::Html => "HTML",
            ImportFormat::Warc => "WARC",
        }
    }
}

/// How plain-text files are split into documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextMode {
    WholeFile,
    PerLine,
    /// Split on blank lines.
    PerParagraph,
}

impl TextMode {
    pub const ALL: [TextMode; 3] = [TextMode::WholeFile, TextMode::PerLine, TextMode::PerParagraph];

    pub fn label(self) -> &'static str {
        match self {
            TextMode::WholeFile => "One document per file",
            TextMode::PerLine => "One document per line",
            TextMode::PerParagraph => "One document per paragraph (split on blank lines)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRequest {
    pub name: String,
    pub description: Option<String>,
    /// Files and/or folders. Folders are searched recursively.
    pub paths: Vec<String>,
    pub format: ImportFormat,
    /// Field or column holding the text (JSON, CSV, Parquet).
    pub text_field: String,
    /// Field or column holding a document id. When absent, ids are `<source>:<record number>`.
    pub id_field: Option<String>,
    pub text_mode: TextMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkipCounts {
    /// Records without the text field, or where it is not a string.
    pub missing_text: u64,
    /// Records whose text is empty or whitespace only.
    pub empty_text: u64,
    /// Lines or rows that could not be parsed.
    pub parse_errors: u64,
    /// Files in picked folders that were ignored because their type is not supported.
    pub ignored_files: u64,
}

impl SkipCounts {
    pub fn records(&self) -> u64 {
        self.missing_text + self.empty_text + self.parse_errors
    }

    pub fn add(&mut self, other: &SkipCounts) {
        self.missing_text += other.missing_text;
        self.empty_text += other.empty_text;
        self.parse_errors += other.parse_errors;
        self.ignored_files += other.ignored_files;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceFileReport {
    /// Path shown to the user and stored in each document's `source` column.
    pub source: String,
    pub format: ImportFormat,
    pub docs: u64,
    pub skipped: SkipCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    pub request: ImportRequest,
    pub files: Vec<SourceFileReport>,
    pub skipped: SkipCounts,
    /// A few example problems, for showing to the user.
    pub sample_errors: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Datasets
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistogramBin {
    pub lo: u64,
    /// Exclusive upper bound; `None` for the last, open-ended bin.
    pub hi: Option<u64>,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TextStats {
    pub doc_count: u64,
    pub char_count: u64,
    /// UTF-8 bytes of document text (before compression).
    pub text_bytes: u64,
    pub min_chars: Option<u64>,
    pub max_chars: Option<u64>,
    /// Document length in characters.
    pub length_histogram: Vec<HistogramBin>,
}

impl TextStats {
    pub fn mean_chars(&self) -> Option<f64> {
        (self.doc_count > 0).then(|| self.char_count as f64 / self.doc_count as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartInfo {
    pub file: String,
    pub rows: u64,
    pub bytes: u64,
}

/// Stored as `manifest.json` in each dataset folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub format_version: u32,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at_ms: i64,
    pub parts: Vec<PartInfo>,
    pub stats: TextStats,
    /// Present when the dataset was created by an import.
    pub import: Option<ImportReport>,
    /// Present when the dataset was created by a pipeline run.
    #[serde(default)]
    pub produced_by: Option<ProducedBy>,
    /// Present when the dataset was created by mixing other datasets.
    #[serde(default)]
    pub mix: Option<MixReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProducedBy {
    pub run_id: String,
    pub pipeline_name: String,
    pub input_dataset_id: String,
    pub input_name: String,
}

impl DatasetManifest {
    pub fn byte_size(&self) -> u64 {
        self.parts.iter().map(|p| p.bytes).sum()
    }
}

/// A document in a list: text cut to a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentPreview {
    /// Position in the dataset, starting at 0.
    pub row: u64,
    pub id: String,
    pub source: String,
    pub chars: u64,
    pub preview: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentPage {
    pub docs: Vec<DocumentPreview>,
    /// Row to pass as `start_row` for the next page, or `None` at the end.
    pub next_row: Option<u64>,
    /// For searches: rows examined so far (from the start of the dataset).
    pub scanned_to: u64,
    /// For searches: the time budget ran out before the page filled. Continue from `next_row`.
    pub search_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub row: u64,
    pub id: String,
    pub source: String,
    pub text: String,
    /// Extra fields kept from the source, as pretty-printed JSON (`{}` when none).
    pub meta_json: String,
}

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobKind {
    /// Diagnostic job that only reports progress; used to check the job runner.
    SelfTest,
    Import,
    PipelineRun,
    Export,
    Mix,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::SelfTest => "Self-test",
            JobKind::Import => "Import",
            JobKind::PipelineRun => "Pipeline run",
            JobKind::Export => "Export",
            JobKind::Mix => "Mix",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Completed | JobState::Failed | JobState::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            JobState::Queued => "Queued",
            JobState::Running => "Running",
            JobState::Completed => "Completed",
            JobState::Failed => "Failed",
            JobState::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: u64,
    /// `None` when the total amount of work is not known yet.
    pub total: Option<u64>,
}

impl Progress {
    /// Fraction complete in `0.0..=1.0`, or `None` if the total is unknown.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(0) => Some(1.0),
            Some(total) => Some((self.done as f64 / total as f64).clamp(0.0, 1.0)),
            None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: JobId,
    pub kind: JobKind,
    pub title: String,
    pub state: JobState,
    pub progress: Progress,
    /// Latest human-readable status line, or the error message for failed jobs.
    pub message: Option<String>,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// What a completed job produced, for an "Open" button.
    #[serde(default)]
    pub link: Option<JobLink>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobLink {
    Dataset(String),
    Run(String),
    /// An export folder name inside the workspace's `exports/`.
    Export(String),
}
