//! Pipeline recipes, step settings, and run reports.
//!
//! A pipeline is stored as TOML, for example:
//!
//! ```toml
//! name = "Web cleanup"
//!
//! [[steps]]
//! kind = "normalize_unicode"
//! form = "nfc"
//!
//! [[steps]]
//! kind = "length_filter"
//! enabled = false
//! min_words = 50
//! ```
//!
//! Every step type has a [`StepKindInfo`] describing its settings, which the UI uses to build
//! the settings form, so adding a step type needs no new UI code.

use serde::{Deserialize, Serialize};

use crate::PartInfo;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub steps: Vec<PipelineStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineStep {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(flatten)]
    pub config: StepConfig,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepConfig {
    NormalizeUnicode(NormalizeUnicode),
    CleanCharacters(CleanCharacters),
    NormalizeWhitespace(NormalizeWhitespace),
    MaskPii(MaskPii),
    LengthFilter(LengthFilter),
    LanguageFilter(LanguageFilter),
    CharacterRatioFilter(CharacterRatioFilter),
    WordQualityFilter(WordQualityFilter),
    RepetitionFilter(RepetitionFilter),
    BlocklistFilter(BlocklistFilter),
    ExactDedup(ExactDedup),
    NearDedup(NearDedup),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnicodeForm {
    #[default]
    Nfc,
    Nfkc,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NormalizeUnicode {
    pub form: UnicodeForm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanCharacters {
    pub fix_mojibake: bool,
    pub remove_control: bool,
    pub remove_zero_width: bool,
    pub replace_nbsp: bool,
}

impl Default for CleanCharacters {
    fn default() -> Self {
        Self { fix_mojibake: true, remove_control: true, remove_zero_width: true, replace_nbsp: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NormalizeWhitespace {
    pub trim_lines: bool,
    pub collapse_spaces: bool,
    pub max_blank_lines: u64,
    pub trim_document: bool,
}

impl Default for NormalizeWhitespace {
    fn default() -> Self {
        Self { trim_lines: true, collapse_spaces: true, max_blank_lines: 1, trim_document: true }
    }
}

/// Replaces personal information with placeholders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskPii {
    pub emails: bool,
    pub phone_numbers: bool,
    pub ip_addresses: bool,
    pub card_numbers: bool,
}

impl Default for MaskPii {
    fn default() -> Self {
        Self { emails: true, phone_numbers: true, ip_addresses: true, card_numbers: true }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LengthFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_chars: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_words: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_words: Option<u64>,
}

impl LengthFilter {
    /// Thresholds used when the step is added in the editor.
    pub fn recommended() -> Self {
        Self { min_chars: None, max_chars: None, min_words: Some(50), max_words: Some(100_000) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageFilter {
    /// ISO 639-3 codes, e.g. `eng`, `deu`, `fra`.
    pub keep: Vec<String>,
    pub min_confidence: f64,
}

impl Default for LanguageFilter {
    fn default() -> Self {
        Self { keep: vec!["eng".into()], min_confidence: 0.5 }
    }
}

/// Ratios are measured over non-whitespace characters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterRatioFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_alpha_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_digit_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_symbol_ratio: Option<f64>,
    /// Over letters only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uppercase_ratio: Option<f64>,
    /// U+FFFD, left behind by broken encodings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_replacement_ratio: Option<f64>,
}

impl CharacterRatioFilter {
    /// Thresholds used when the step is added in the editor.
    pub fn recommended() -> Self {
        Self {
            min_alpha_ratio: Some(0.5),
            max_digit_ratio: Some(0.3),
            max_symbol_ratio: Some(0.3),
            max_uppercase_ratio: Some(0.5),
            max_replacement_ratio: Some(0.01),
        }
    }
}

/// Heuristics from DeepMind's Gopher paper (Rae et al., 2021).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WordQualityFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_mean_word_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_mean_word_length: Option<f64>,
    /// Fraction of words that must contain at least one letter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_alpha_word_fraction: Option<f64>,
    /// `#` and `...` per word.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_symbol_word_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bullet_line_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ellipsis_line_fraction: Option<f64>,
}

impl WordQualityFilter {
    /// Thresholds used when the step is added in the editor.
    pub fn recommended() -> Self {
        Self {
            min_mean_word_length: Some(3.0),
            max_mean_word_length: Some(10.0),
            min_alpha_word_fraction: Some(0.8),
            max_symbol_word_ratio: Some(0.1),
            max_bullet_line_fraction: Some(0.9),
            max_ellipsis_line_fraction: Some(0.3),
        }
    }
}

/// Repetition heuristics from the Gopher paper.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RepetitionFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duplicate_line_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duplicate_paragraph_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_top_2gram_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_top_3gram_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_top_4gram_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duplicate_5gram_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duplicate_10gram_fraction: Option<f64>,
}

impl RepetitionFilter {
    /// Thresholds used when the step is added in the editor.
    pub fn recommended() -> Self {
        Self {
            max_duplicate_line_fraction: Some(0.3),
            max_duplicate_paragraph_fraction: Some(0.3),
            max_top_2gram_fraction: Some(0.2),
            max_top_3gram_fraction: Some(0.18),
            max_top_4gram_fraction: Some(0.16),
            max_duplicate_5gram_fraction: Some(0.15),
            max_duplicate_10gram_fraction: Some(0.1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlocklistFilter {
    /// Words or phrases. Matched as whole words.
    pub terms: Vec<String>,
    pub case_sensitive: bool,
    /// Documents with more matches than this are dropped (0 = any match drops).
    pub max_hits: u64,
}

impl Default for BlocklistFilter {
    fn default() -> Self {
        Self {
            terms: ["lorem ipsum", "javascript is disabled", "enable javascript", "enable cookies"]
                .map(String::from)
                .to_vec(),
            case_sensitive: false,
            max_hits: 0,
        }
    }
}

/// Removes documents whose text is identical to an earlier document's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExactDedup {
    /// Compare lowercased text with whitespace collapsed, so trivial variants match too.
    pub normalize: bool,
}

impl Default for ExactDedup {
    fn default() -> Self {
        Self { normalize: true }
    }
}

/// MinHash + LSH: removes documents that share most of their word n-grams with an earlier one.
/// With `bands` × `rows_per_band` hashes, a pair with Jaccard similarity `s` is caught with
/// probability `1 - (1 - s^rows)^bands`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NearDedup {
    pub ngram_size: u64,
    pub bands: u64,
    pub rows_per_band: u64,
}

impl Default for NearDedup {
    fn default() -> Self {
        Self { ngram_size: 5, bands: 14, rows_per_band: 8 }
    }
}

impl NearDedup {
    /// Probability that a pair with Jaccard similarity `s` is detected.
    pub fn detection_probability(&self, s: f64) -> f64 {
        1.0 - (1.0 - s.powi(self.rows_per_band as i32)).powi(self.bands as i32)
    }
}

// ---------------------------------------------------------------------------------------------
// Step descriptions for the UI
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepCategory {
    /// Changes text, never drops documents (except ones left empty).
    Normalize,
    /// Keeps or drops documents, never changes text.
    Filter,
    /// Drops documents that repeat earlier ones. Needs a pass over the whole dataset.
    Dedup,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    Bool,
    /// Non-negative integer, required.
    Count,
    /// Non-negative integer; empty turns the rule off.
    OptCount,
    /// Ratio between 0 and 1; empty turns the rule off.
    OptRatio,
    /// Ratio between 0 and 1, required.
    Ratio,
    /// Any number; empty turns the rule off.
    OptNumber,
    /// One of the given `(value, label)` pairs.
    Choice(&'static [(&'static str, &'static str)]),
    /// List of strings, one per line in the UI.
    List,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub help: &'static str,
}

pub struct StepKindInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub category: StepCategory,
    pub description: &'static str,
    pub fields: &'static [FieldSpec],
    pub default: fn() -> StepConfig,
}

const fn field(key: &'static str, label: &'static str, kind: FieldKind, help: &'static str) -> FieldSpec {
    FieldSpec { key, label, kind, help }
}

use FieldKind::*;

pub static STEP_KINDS: &[StepKindInfo] = &[
    StepKindInfo {
        kind: "normalize_unicode",
        label: "Normalize Unicode",
        category: StepCategory::Normalize,
        description: "Make equivalent characters identical. NFC is safe; NFKC also folds compatibility forms (full-width letters, ligatures, superscripts).",
        fields: &[field("form", "Form", Choice(&[("nfc", "NFC (canonical)"), ("nfkc", "NFKC (compatibility)")]), "")],
        default: || StepConfig::NormalizeUnicode(NormalizeUnicode::default()),
    },
    StepKindInfo {
        kind: "clean_characters",
        label: "Clean characters",
        category: StepCategory::Normalize,
        description: "Repair common encoding damage and remove invisible characters.",
        fields: &[
            field("fix_mojibake", "Fix mojibake", Bool, "Repairs UTF-8 text that was decoded as Windows-1252 (e.g. “cafÃ©” → “café”). Only applied when the repair clearly helps."),
            field("remove_control", "Remove control characters", Bool, "Keeps newlines and tabs."),
            field("remove_zero_width", "Remove zero-width characters", Bool, "Zero-width spaces and joiners, byte-order marks."),
            field("replace_nbsp", "Replace non-breaking spaces", Bool, "With ordinary spaces."),
        ],
        default: || StepConfig::CleanCharacters(CleanCharacters::default()),
    },
    StepKindInfo {
        kind: "normalize_whitespace",
        label: "Normalize whitespace",
        category: StepCategory::Normalize,
        description: "Tidy spaces, line endings and blank lines. Line endings always become \\n.",
        fields: &[
            field("trim_lines", "Trim trailing spaces", Bool, ""),
            field("collapse_spaces", "Collapse runs of spaces and tabs", Bool, "Leading indentation is kept."),
            field("max_blank_lines", "Max consecutive blank lines", Count, ""),
            field("trim_document", "Trim start and end of document", Bool, ""),
        ],
        default: || StepConfig::NormalizeWhitespace(NormalizeWhitespace::default()),
    },
    StepKindInfo {
        kind: "mask_pii",
        label: "Mask personal information",
        category: StepCategory::Normalize,
        description: "Replace personal information with placeholders: <EMAIL>, <PHONE>, <IP>, <CARD>. Pattern-based: catches common formats, not names or addresses.",
        fields: &[
            field("emails", "Email addresses", Bool, ""),
            field("phone_numbers", "Phone numbers", Bool, "9 or more digits in a phone-like format (+1 555 123 4567, (030) 1234-5678)."),
            field("ip_addresses", "IP addresses", Bool, "IPv4, and IPv6 written in full."),
            field("card_numbers", "Payment card numbers", Bool, "13–19 digits that pass the Luhn checksum."),
        ],
        default: || StepConfig::MaskPii(MaskPii::default()),
    },
    StepKindInfo {
        kind: "length_filter",
        label: "Length filter",
        category: StepCategory::Filter,
        description: "Drop documents that are too short or too long.",
        fields: &[
            field("min_chars", "Min characters", OptCount, ""),
            field("max_chars", "Max characters", OptCount, ""),
            field("min_words", "Min words", OptCount, "Gopher uses 50."),
            field("max_words", "Max words", OptCount, "Gopher uses 100,000."),
        ],
        default: || StepConfig::LengthFilter(LengthFilter::recommended()),
    },
    StepKindInfo {
        kind: "language_filter",
        label: "Language filter",
        category: StepCategory::Filter,
        description: "Keep documents detected as one of the given languages (detection uses the first 2,000 characters).",
        fields: &[
            field("keep", "Languages to keep", List, "ISO 639-3 codes, one per line: eng, deu, fra, spa, ita, por, nld, rus, cmn, jpn, kor, ara, hin…"),
            field("min_confidence", "Min confidence", Ratio, "0–1. Documents detected with lower confidence are dropped."),
        ],
        default: || StepConfig::LanguageFilter(LanguageFilter::default()),
    },
    StepKindInfo {
        kind: "character_ratio_filter",
        label: "Character ratio filter",
        category: StepCategory::Filter,
        description: "Drop documents dominated by digits, symbols or capitals, or with broken encoding. Ratios are over non-whitespace characters.",
        fields: &[
            field("min_alpha_ratio", "Min letter ratio", OptRatio, ""),
            field("max_digit_ratio", "Max digit ratio", OptRatio, ""),
            field("max_symbol_ratio", "Max symbol ratio", OptRatio, "Punctuation and other symbols."),
            field("max_uppercase_ratio", "Max uppercase ratio", OptRatio, "Over letters only."),
            field("max_replacement_ratio", "Max replacement-character ratio", OptRatio, "U+FFFD (�), left by broken encodings."),
        ],
        default: || StepConfig::CharacterRatioFilter(CharacterRatioFilter::recommended()),
    },
    StepKindInfo {
        kind: "word_quality_filter",
        label: "Word quality filter",
        category: StepCategory::Filter,
        description: "Gopher quality rules: catches word lists, link farms, menus and other non-prose.",
        fields: &[
            field("min_mean_word_length", "Min mean word length", OptNumber, "Gopher: 3"),
            field("max_mean_word_length", "Max mean word length", OptNumber, "Gopher: 10"),
            field("min_alpha_word_fraction", "Min fraction of words with a letter", OptRatio, "Gopher: 0.8"),
            field("max_symbol_word_ratio", "Max # and … per word", OptRatio, "Gopher: 0.1"),
            field("max_bullet_line_fraction", "Max fraction of bullet lines", OptRatio, "Gopher: 0.9"),
            field("max_ellipsis_line_fraction", "Max fraction of lines ending in …", OptRatio, "Gopher: 0.3"),
        ],
        default: || StepConfig::WordQualityFilter(WordQualityFilter::recommended()),
    },
    StepKindInfo {
        kind: "repetition_filter",
        label: "Repetition filter",
        category: StepCategory::Filter,
        description: "Gopher repetition rules: drop documents that repeat lines, paragraphs or phrases. N-gram fractions are measured in characters.",
        fields: &[
            field("max_duplicate_line_fraction", "Max duplicate line fraction", OptRatio, "Gopher: 0.3"),
            field("max_duplicate_paragraph_fraction", "Max duplicate paragraph fraction", OptRatio, "Gopher: 0.3"),
            field("max_top_2gram_fraction", "Max top 2-gram fraction", OptRatio, "Gopher: 0.20"),
            field("max_top_3gram_fraction", "Max top 3-gram fraction", OptRatio, "Gopher: 0.18"),
            field("max_top_4gram_fraction", "Max top 4-gram fraction", OptRatio, "Gopher: 0.16"),
            field("max_duplicate_5gram_fraction", "Max duplicate 5-gram fraction", OptRatio, "Gopher: 0.15"),
            field("max_duplicate_10gram_fraction", "Max duplicate 10-gram fraction", OptRatio, "Gopher: 0.10"),
        ],
        default: || StepConfig::RepetitionFilter(RepetitionFilter::recommended()),
    },
    StepKindInfo {
        kind: "blocklist_filter",
        label: "Blocklist filter",
        category: StepCategory::Filter,
        description: "Drop documents containing listed words or phrases (whole-word match).",
        fields: &[
            field("terms", "Words and phrases", List, "One per line."),
            field("case_sensitive", "Case-sensitive", Bool, ""),
            field("max_hits", "Allowed matches", Count, "Documents with more matches are dropped. 0 drops on the first match."),
        ],
        default: || StepConfig::BlocklistFilter(BlocklistFilter::default()),
    },
    StepKindInfo {
        kind: "exact_dedup",
        label: "Exact deduplication",
        category: StepCategory::Dedup,
        description: "Keep the first copy of each text and remove later identical copies. Adds a pass over the dataset when running.",
        fields: &[field("normalize", "Ignore case and whitespace", Bool, "Lowercases and collapses whitespace before comparing.")],
        default: || StepConfig::ExactDedup(ExactDedup::default()),
    },
    StepKindInfo {
        kind: "near_dedup",
        label: "Near-duplicate removal",
        category: StepCategory::Dedup,
        description: "MinHash LSH over word n-grams: keep the first of each group of near-identical documents. With 14 bands × 8 rows, pairs with 80% n-gram overlap are caught about 92% of the time, 90% overlap almost always, 70% overlap about half the time. Adds a pass over the dataset when running.",
        fields: &[
            field("ngram_size", "Words per n-gram", Count, "5 is common for web text."),
            field("bands", "Bands", Count, "More bands catch less similar pairs (lower threshold)."),
            field("rows_per_band", "Rows per band", Count, "More rows require more similarity (higher threshold)."),
        ],
        default: || StepConfig::NearDedup(NearDedup::default()),
    },
];

impl StepConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            StepConfig::NormalizeUnicode(_) => "normalize_unicode",
            StepConfig::CleanCharacters(_) => "clean_characters",
            StepConfig::NormalizeWhitespace(_) => "normalize_whitespace",
            StepConfig::MaskPii(_) => "mask_pii",
            StepConfig::LengthFilter(_) => "length_filter",
            StepConfig::LanguageFilter(_) => "language_filter",
            StepConfig::CharacterRatioFilter(_) => "character_ratio_filter",
            StepConfig::WordQualityFilter(_) => "word_quality_filter",
            StepConfig::RepetitionFilter(_) => "repetition_filter",
            StepConfig::BlocklistFilter(_) => "blocklist_filter",
            StepConfig::ExactDedup(_) => "exact_dedup",
            StepConfig::NearDedup(_) => "near_dedup",
        }
    }

    pub fn info(&self) -> &'static StepKindInfo {
        step_kind(self.kind()).expect("every StepConfig variant is listed in STEP_KINDS")
    }
}

pub fn step_kind(kind: &str) -> Option<&'static StepKindInfo> {
    STEP_KINDS.iter().find(|k| k.kind == kind)
}

impl PipelineSpec {
    /// A starting point: every step with its default settings.
    pub fn template(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some("Normalization, Gopher-style quality filters and deduplication for web text.".into()),
            steps: STEP_KINDS.iter().map(|k| PipelineStep { enabled: true, config: (k.default)() }).collect(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Saved pipelines, previews, and runs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineSummary {
    /// File name inside the workspace's `pipelines/` folder, e.g. `web-cleanup.toml`.
    pub file: String,
    pub name: String,
    pub description: Option<String>,
    pub step_count: u64,
    pub modified_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleCount {
    pub rule: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepStats {
    /// Position in the pipeline's step list (disabled steps included).
    pub index: u64,
    pub kind: String,
    pub label: String,
    pub docs_in: u64,
    pub dropped: u64,
    /// Documents whose text this step changed.
    pub modified: u64,
    pub chars_in: u64,
    pub chars_out: u64,
    /// Most frequent first.
    pub drops_by_rule: Vec<RuleCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropExample {
    pub step_index: u64,
    pub rule: String,
    pub detail: String,
    pub id: String,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeExample {
    pub step_index: u64,
    pub id: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelinePreview {
    pub sample_docs: u64,
    pub docs_out: u64,
    pub steps: Vec<StepStats>,
    pub drop_examples: Vec<DropExample>,
    pub change_examples: Vec<ChangeExample>,
}

/// One row of the runs list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub pipeline_name: String,
    pub input_dataset_id: Option<String>,
    pub input_name: Option<String>,
    pub output_dataset_id: Option<String>,
    pub output_name: Option<String>,
    /// "running", "completed", "failed" or "cancelled".
    pub state: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub docs_in: Option<u64>,
    pub docs_out: Option<u64>,
    pub error: Option<String>,
}

/// Stored as `runs/<id>/report.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub id: String,
    pub pipeline: PipelineSpec,
    pub input_dataset_id: String,
    pub input_name: String,
    pub output_dataset_id: String,
    pub output_name: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub docs_in: u64,
    pub docs_out: u64,
    pub chars_in: u64,
    pub chars_out: u64,
    pub steps: Vec<StepStats>,
    /// Parquet files with the removed documents, in `runs/<id>/removed/`.
    pub removed_parts: Vec<PartInfo>,
    /// Parquet files mapping each kept document to its input row and the steps that changed
    /// it, in `runs/<id>/kept/`. Empty for runs made before this was recorded.
    #[serde(default)]
    pub kept_parts: Vec<PartInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemovedDocPreview {
    /// Position in the run's removed-documents list.
    pub index: u64,
    /// Row in the input dataset.
    pub input_row: u64,
    pub id: String,
    pub source: String,
    pub step_index: u64,
    pub rule: String,
    pub detail: String,
    /// For duplicates: the input row of the document this one duplicates.
    #[serde(default)]
    pub related_row: Option<u64>,
    pub chars: u64,
    pub preview: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemovedPage {
    pub docs: Vec<RemovedDocPreview>,
    pub next_index: Option<u64>,
    pub scanned_to: u64,
    pub search_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemovedDoc {
    pub index: u64,
    pub input_row: u64,
    pub id: String,
    pub source: String,
    pub step_index: u64,
    pub rule: String,
    pub detail: String,
    /// For duplicates: the input row of the document this one duplicates.
    #[serde(default)]
    pub related_row: Option<u64>,
    /// Text as it was when the document was dropped (after earlier normalization steps).
    pub text: String,
}

/// A kept document that normalization changed, for the Inspector's before/after view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedRef {
    pub output_row: u64,
    pub input_row: u64,
    /// Recipe indices of the steps that changed it.
    pub step_indices: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedPage {
    pub items: Vec<ChangedRef>,
    pub next_start: Option<u64>,
    pub scanned_to: u64,
    /// Total kept documents in the run.
    pub total_kept: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_is_described_and_defaults_match_their_kind() {
        for info in STEP_KINDS {
            let cfg = (info.default)();
            assert_eq!(cfg.kind(), info.kind);
            assert!(std::ptr::eq(cfg.info(), info));
        }
        assert_eq!(STEP_KINDS.len(), 12);
    }

    #[test]
    fn steps_roundtrip_through_json_with_flattened_kind() {
        let spec = PipelineSpec::template("t");
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""kind":"length_filter""#));
        assert!(json.contains(r#""enabled":true"#));
        assert_eq!(serde_json::from_str::<PipelineSpec>(&json).unwrap(), spec);
    }

    #[test]
    fn near_dedup_detection_probability_matches_the_description() {
        let d = NearDedup::default();
        assert!((d.detection_probability(0.8) - 0.92).abs() < 0.01);
        assert!(d.detection_probability(0.9) > 0.99);
        assert!((d.detection_probability(0.7) - 0.56).abs() < 0.02);
    }

    #[test]
    fn missing_thresholds_mean_rule_off() {
        let step: PipelineStep = serde_json::from_str(r#"{"kind":"length_filter","min_chars":10}"#).unwrap();
        assert!(step.enabled);
        match step.config {
            StepConfig::LengthFilter(f) => {
                assert_eq!(f.min_chars, Some(10));
                assert_eq!(f.min_words, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rules_switched_off_stay_off_after_a_roundtrip() {
        let mut f = LengthFilter::recommended();
        f.min_words = None;
        let step = PipelineStep { enabled: false, config: StepConfig::LengthFilter(f) };
        let json = serde_json::to_string(&step).unwrap();
        assert!(!json.contains("min_words"));
        assert_eq!(serde_json::from_str::<PipelineStep>(&json).unwrap(), step);
    }
}
