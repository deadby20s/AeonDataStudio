//! Export, mixing and import-preview types.

use serde::{Deserialize, Serialize};

use crate::{ImportFormat, ProducedBy};

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// One JSON object per line: `{"id", "text", "source", "meta"?}`.
    Jsonl,
    /// Columns `id`, `text`, `source`, `meta` (meta as a JSON string).
    Parquet,
    /// Token ids as flat binary shards plus document offsets, ready for a training loop.
    Tokens,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Jsonl, ExportFormat::Parquet, ExportFormat::Tokens];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jsonl => "JSONL",
            ExportFormat::Parquet => "Parquet",
            ExportFormat::Tokens => "Tokenized binary shards",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportCompression {
    None,
    Gzip,
    Zstd,
}

impl ExportCompression {
    pub const ALL: [ExportCompression; 3] = [ExportCompression::Zstd, ExportCompression::Gzip, ExportCompression::None];

    pub fn label(self) -> &'static str {
        match self {
            ExportCompression::None => "None (.jsonl)",
            ExportCompression::Gzip => "gzip (.jsonl.gz)",
            ExportCompression::Zstd => "Zstandard (.jsonl.zst)",
        }
    }
}

/// Built-in tiktoken encodings, or a Hugging Face `tokenizer.json` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenizerChoice {
    /// `cl100k_base`, `o200k_base`, `p50k_base` or `r50k_base` (GPT-2).
    Tiktoken(String),
    /// Path to a `tokenizer.json`.
    HuggingFace(String),
}

pub const TIKTOKEN_ENCODINGS: [(&str, &str); 4] = [
    ("o200k_base", "o200k_base (GPT-4o)"),
    ("cl100k_base", "cl100k_base (GPT-4, GPT-3.5)"),
    ("p50k_base", "p50k_base (Codex)"),
    ("r50k_base", "r50k_base (GPT-2)"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportRequest {
    pub dataset_id: String,
    /// Folder name under the workspace's `exports/` folder (made filesystem-safe).
    pub name: String,
    pub format: ExportFormat,
    /// JSONL only.
    pub compression: ExportCompression,
    /// Target size of each shard before compression, in MB.
    pub shard_mb: u64,
    /// JSONL / Parquet: include each document's metadata.
    pub include_meta: bool,
    /// Tokens only.
    pub tokenizer: Option<TokenizerChoice>,
    /// Tokens only: append the end-of-text token after every document.
    pub append_eos: bool,
    /// Folder to create the export in. `None` = the workspace's `exports/` folder.
    #[serde(default)]
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenizerInfo {
    pub name: String,
    pub vocab_size: u64,
    pub eos_id: Option<u32>,
    /// `uint16` or `uint32`, little-endian.
    pub dtype: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    pub file: String,
    pub docs: u64,
    pub bytes: u64,
    pub sha256: String,
    pub tokens: Option<u64>,
}

/// Stored as `manifest.json` in each export folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportManifest {
    pub format_version: u32,
    pub name: String,
    pub created_at_ms: i64,
    pub app_version: String,
    pub dataset_id: String,
    pub dataset_name: String,
    /// How the dataset was made, if by a pipeline run.
    pub dataset_produced_by: Option<ProducedBy>,
    pub request: ExportRequest,
    pub docs: u64,
    pub tokens: Option<u64>,
    pub tokenizer: Option<TokenizerInfo>,
    pub files: Vec<ExportFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSummary {
    /// Folder name of the export.
    pub folder: String,
    /// Full path; identifies the export for opening or removing it.
    pub path: String,
    /// Created outside the workspace's `exports/` folder.
    #[serde(default)]
    pub external: bool,
    pub name: String,
    pub created_at_ms: i64,
    pub format: ExportFormat,
    pub dataset_name: String,
    pub docs: u64,
    pub tokens: Option<u64>,
    pub files: u64,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------------------------
// Mixing
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixSource {
    pub dataset_id: String,
    /// 1 = every document once, 0.5 = a random half, 2 = every document twice,
    /// 2.5 = twice plus a random half a third time.
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixRequest {
    pub name: String,
    pub description: Option<String>,
    pub sources: Vec<MixSource>,
    pub shuffle: bool,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixSourceReport {
    pub dataset_id: String,
    pub dataset_name: String,
    pub weight: f64,
    pub docs_in: u64,
    pub docs_out: u64,
    pub chars_out: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixReport {
    pub request: MixRequest,
    pub sources: Vec<MixSourceReport>,
}

// ---------------------------------------------------------------------------------------------
// Import preview
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldInfo {
    pub name: String,
    /// How many of the sampled records have this field as text.
    pub text_count: u64,
    /// Average length of the field's text values, in characters.
    pub mean_chars: f64,
    pub example: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPreview {
    pub files: u64,
    pub ignored_files: u64,
    /// The file that was sampled.
    pub file: String,
    pub format: ImportFormat,
    pub records_sampled: u64,
    pub parse_errors: u64,
    /// Fields found in the sample (empty for plain text).
    pub fields: Vec<FieldInfo>,
    /// The field that looks most like document text (longest text on average).
    pub suggested_text_field: Option<String>,
    pub suggested_id_field: Option<String>,
    /// First documents as they would be imported with the current settings.
    pub sample_texts: Vec<String>,
}
