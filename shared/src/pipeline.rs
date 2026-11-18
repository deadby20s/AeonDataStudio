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
    C4Filter(C4Filter),
    FineWebFilter(FineWebFilter),
    ExactDedup(ExactDedup),
    NearDedup(NearDedup),
    Plugin(PluginStep),
    LlmScore(LlmScore),
    LlmRewrite(LlmRewrite),
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

/// The C4 rules (Raffel et al., 2020) as in datatrove's C4 quality filter. Removes lines, then
/// may drop the whole document.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct C4Filter {
    /// Remove lines not ending in . ! ? " or similar.
    pub drop_lines_without_terminal_punctuation: bool,
    /// Remove lines with fewer words than this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_words_per_line: Option<u64>,
    /// Remove lines containing a word longer than this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_word_length: Option<u64>,
    /// Remove lines mentioning "javascript".
    pub drop_javascript_lines: bool,
    /// Remove lines about cookies, terms of use and privacy policies.
    pub drop_policy_lines: bool,
    /// Drop documents with fewer sentences than this (after removing lines).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_sentences: Option<u64>,
    pub drop_lorem_ipsum: bool,
    /// Drop documents containing "{" (usually code).
    pub drop_curly_brackets: bool,
}

impl C4Filter {
    pub fn recommended() -> Self {
        Self {
            drop_lines_without_terminal_punctuation: true,
            min_words_per_line: Some(3),
            max_word_length: Some(1000),
            drop_javascript_lines: true,
            drop_policy_lines: true,
            min_sentences: Some(5),
            drop_lorem_ipsum: true,
            drop_curly_brackets: true,
        }
    }
}

/// FineWeb's own quality rules (Penedo et al., 2024).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FineWebFilter {
    /// Minimum share of lines ending in punctuation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_line_punctuation_ratio: Option<f64>,
    pub short_line_length: u64,
    /// Maximum share of lines with at most `short_line_length` characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_short_line_ratio: Option<f64>,
    /// Maximum share of characters in lines that repeat an earlier line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duplicate_line_char_ratio: Option<f64>,
    /// Maximum newlines per word (catches lists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_newline_word_ratio: Option<f64>,
}

impl FineWebFilter {
    pub fn recommended() -> Self {
        Self {
            min_line_punctuation_ratio: Some(0.12),
            short_line_length: 30,
            max_short_line_ratio: Some(0.67),
            max_duplicate_line_char_ratio: Some(0.01),
            max_newline_word_ratio: Some(0.3),
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

/// Runs a plugin from the workspace's `plugins/` folder on each document. The plugin can keep,
/// drop or rewrite it. See docs/plugins.md.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginStep {
    /// Folder name inside `plugins/`.
    pub plugin: String,
    /// `key=value` lines, passed to the plugin (they override the plugin's defaults).
    pub options: Vec<String>,
}

/// What an LLM step does when a request still fails after retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmOnError {
    /// Stop the run with the error.
    #[default]
    Stop,
    /// Keep the document unchanged.
    Keep,
    /// Drop the document (recorded with rule `llm_error`).
    Drop,
}

/// What a score step does with the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScoreMode {
    /// Drop documents scoring below the minimum.
    #[default]
    Filter,
    /// Keep every document and save the score in its metadata, to choose a threshold later.
    Tag,
}

fn default_score_field() -> String {
    "llm_score".into()
}

fn yes_default() -> bool {
    true
}

/// Default server: Ollama's OpenAI-compatible endpoint on this computer.
pub const DEFAULT_LLM_ENDPOINT: &str = "http://localhost:11434/v1";
pub const DEFAULT_LLM_MODEL: &str = "llama3.2";

/// Scores each document with a language model and drops low scores (like FineWeb-Edu's
/// educational-quality annotations).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmScore {
    /// OpenAI-compatible base URL, e.g. `http://localhost:11434/v1` (Ollama) or `https://api.openai.com/v1`.
    pub endpoint: String,
    pub model: String,
    /// Name of the environment variable holding the API key (the key itself is never saved).
    pub api_key_env: String,
    /// `{text}` is replaced by the document. The answer's first number is the score.
    pub prompt: String,
    /// `filter` drops documents below `min_score`; `tag` keeps all and saves the score.
    pub mode: ScoreMode,
    pub min_score: f64,
    /// Metadata field the score is saved in (tag mode).
    #[serde(default = "default_score_field")]
    pub score_field: String,
    /// Longer documents are cut to this many characters before sending.
    pub max_input_chars: u64,
    pub on_error: LlmOnError,
    /// Reuse answers saved in the workspace for the same model, prompt and document.
    #[serde(default = "yes_default")]
    pub use_cache: bool,
}

impl Default for LlmScore {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_LLM_ENDPOINT.into(),
            model: DEFAULT_LLM_MODEL.into(),
            api_key_env: String::new(),
            prompt: EDU_SCORE_PROMPT.into(),
            mode: ScoreMode::Filter,
            min_score: 3.0,
            score_field: default_score_field(),
            max_input_chars: 6000,
            on_error: LlmOnError::Stop,
            use_cache: true,
        }
    }
}

/// Replaces each document with a language model's answer: rephrasing, translation,
/// summaries, or synthetic data such as question-answer pairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmRewrite {
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    /// `{text}` is replaced by the document.
    pub prompt: String,
    pub temperature: f64,
    pub max_tokens: u64,
    pub max_input_chars: u64,
    pub on_error: LlmOnError,
    /// Reuse answers saved in the workspace for the same model, prompt and document.
    #[serde(default = "yes_default")]
    pub use_cache: bool,
}

impl Default for LlmRewrite {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_LLM_ENDPOINT.into(),
            model: DEFAULT_LLM_MODEL.into(),
            api_key_env: String::new(),
            prompt: REPHRASE_PROMPT.into(),
            temperature: 0.7,
            max_tokens: 1024,
            max_input_chars: 6000,
            on_error: LlmOnError::Stop,
            use_cache: true,
        }
    }
}

pub const EDU_SCORE_PROMPT: &str = "Below is an extract from a web page. Rate how useful it would be for teaching at school or university, on a scale from 0 to 5:

0: no educational value (ads, navigation, spam, unreadable text)
1: some basic facts, mixed with irrelevant or promotional content
2: touches on an educational topic, but superficial or disorganized
3: coherent and useful for learning, like an introductory text
4: clear, focused and highly relevant for teaching, like a good textbook section
5: outstanding: thorough, accurate and easy to follow

Extract:
{text}

Answer with the score only, as a single number.";

pub const REPHRASE_PROMPT: &str = "Rewrite the following text in clear, high-quality English, in the style of an encyclopedia article. Keep every fact; do not add information, and leave out ads, navigation and other clutter. Answer with the rewritten text only.

Text:
{text}";

pub const QA_PROMPT: &str = "Read the following text and write 3 to 5 question-and-answer pairs that a student could learn from. Answer each question using only the text. Use this format, with a blank line between pairs:

Question: ...
Answer: ...

Text:
{text}";

pub const SUMMARY_PROMPT: &str = "Write a short, factual summary (3 to 5 sentences) of the following text. Answer with the summary only.

Text:
{text}";

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
    /// Calls something outside the app: a plugin program or a language model.
    External,
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
    /// A required number.
    Number,
    /// One line of text.
    Text,
    /// Several lines of text, such as a prompt.
    LongText,
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
        kind: "c4_filter",
        label: "C4 filter",
        category: StepCategory::Filter,
        description: "The C4 rules (Raffel et al., 2020): removes lines without end punctuation, very short lines, javascript notices and cookie/policy lines, then drops documents with too few sentences, lorem ipsum or code. Removes lines, so it also changes text. Sentences are counted by end punctuation (C4 used a sentence splitter).",
        fields: &[
            field("drop_lines_without_terminal_punctuation", "Remove lines without end punctuation", Bool, "C4: on. FineWeb turned this off."),
            field("min_words_per_line", "Min words per line", OptCount, "C4: 3"),
            field("max_word_length", "Max word length", OptCount, "C4: 1000"),
            field("drop_javascript_lines", "Remove lines mentioning javascript", Bool, ""),
            field("drop_policy_lines", "Remove cookie / privacy / terms lines", Bool, ""),
            field("min_sentences", "Min sentences", OptCount, "C4: 5"),
            field("drop_lorem_ipsum", "Drop lorem ipsum documents", Bool, ""),
            field("drop_curly_brackets", "Drop documents containing {", Bool, "Usually code."),
        ],
        default: || StepConfig::C4Filter(C4Filter::recommended()),
    },
    StepKindInfo {
        kind: "fineweb_filter",
        label: "FineWeb filter",
        category: StepCategory::Filter,
        description: "FineWeb's own rules (Penedo et al., 2024): enough lines ending in punctuation, not too many short lines, little repeated-line text, not list-like.",
        fields: &[
            field("min_line_punctuation_ratio", "Min share of lines ending in punctuation", OptRatio, "FineWeb: 0.12"),
            field("short_line_length", "Short line: at most this many characters", Count, "FineWeb: 30"),
            field("max_short_line_ratio", "Max share of short lines", OptRatio, "FineWeb: 0.67"),
            field("max_duplicate_line_char_ratio", "Max share of characters in repeated lines", OptRatio, "FineWeb: 0.01"),
            field("max_newline_word_ratio", "Max newlines per word", OptRatio, "FineWeb: 0.3"),
        ],
        default: || StepConfig::FineWebFilter(FineWebFilter::recommended()),
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
    StepKindInfo {
        kind: "plugin",
        label: "Plugin",
        category: StepCategory::External,
        description: "Run a plugin from the workspace's plugins folder on each document: a program (Python, a binary, anything) that reads documents as JSON lines and answers keep, drop or new text. Use it for quality classifiers like fastText or FineWeb-Edu, custom filters or any tool you already have. Settings → Plugins lists the installed ones.",
        fields: &[
            field("plugin", "Plugin", Text, "Folder name inside plugins/, e.g. wordlist-filter."),
            field("options", "Options", List, "key=value, one per line. Overrides the plugin's defaults."),
        ],
        default: || StepConfig::Plugin(PluginStep::default()),
    },
    StepKindInfo {
        kind: "llm_score",
        label: "LLM quality score",
        category: StepCategory::External,
        description: "Ask a language model to score each document and drop low scores, like FineWeb-Edu's educational-quality filter. Works with any OpenAI-compatible server: Ollama, llama.cpp, LM Studio, vLLM, or a hosted API. Each document is one request, so place it after deduplication and cheap filters. Previews use at most 50 documents.",
        fields: &[
            field("endpoint", "Server URL", Text, "OpenAI-compatible base URL. Ollama: http://localhost:11434/v1"),
            field("model", "Model", Text, ""),
            field("api_key_env", "API key variable", Text, "Name of the environment variable with the API key, e.g. OPENAI_API_KEY. Empty for local servers. The key itself is never saved."),
            field("prompt", "Prompt", LongText, "{text} is replaced by the document. The first number in the answer is the score."),
            field("mode", "Mode", Choice(&[("filter", "Drop low scores"), ("tag", "Keep all, save the score")]), "Save the score to try thresholds later (SQL → Score histogram) without asking the model again."),
            field("min_score", "Min score", Number, "Drop low scores: documents scoring lower are dropped."),
            field("score_field", "Score field", Text, "Keep all: the metadata field the score is saved in, e.g. llm_score (meta->>'llm_score' in SQL)."),
            field("max_input_chars", "Max characters sent", Count, "Longer documents are cut."),
            field("on_error", "When a request fails", Choice(&[("stop", "Stop the run"), ("keep", "Keep the document"), ("drop", "Drop the document")]), "Requests are retried 3 times first."),
            field("use_cache", "Reuse saved answers", Bool, "Answers are saved in the workspace (cache/llm.sqlite); re-running with the same model, prompt and document costs nothing."),
        ],
        default: || StepConfig::LlmScore(LlmScore::default()),
    },
    StepKindInfo {
        kind: "llm_rewrite",
        label: "LLM rewrite",
        category: StepCategory::External,
        description: "Replace each document with a language model's answer to your prompt: rephrase web text (as in WRAP), summarize, translate, or generate synthetic data such as question-answer pairs. Any OpenAI-compatible server works. Each document is one request, so place it after deduplication and filters. Previews use at most 50 documents.",
        fields: &[
            field("endpoint", "Server URL", Text, "OpenAI-compatible base URL. Ollama: http://localhost:11434/v1"),
            field("model", "Model", Text, ""),
            field("api_key_env", "API key variable", Text, "Name of the environment variable with the API key. Empty for local servers."),
            field("prompt", "Prompt", LongText, "{text} is replaced by the document."),
            field("temperature", "Temperature", Number, "0 is most predictable; 0.7 gives more varied text."),
            field("max_tokens", "Max answer tokens", Count, ""),
            field("max_input_chars", "Max characters sent", Count, "Longer documents are cut."),
            field("on_error", "When a request fails", Choice(&[("stop", "Stop the run"), ("keep", "Keep the document"), ("drop", "Drop the document")]), "Requests are retried 3 times first."),
            field("use_cache", "Reuse saved answers", Bool, "Answers are saved in the workspace (cache/llm.sqlite). Turn off to get a fresh answer each run (with temperature above 0, answers differ)."),
        ],
        default: || StepConfig::LlmRewrite(LlmRewrite::default()),
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
            StepConfig::C4Filter(_) => "c4_filter",
            StepConfig::FineWebFilter(_) => "fineweb_filter",
            StepConfig::ExactDedup(_) => "exact_dedup",
            StepConfig::NearDedup(_) => "near_dedup",
            StepConfig::Plugin(_) => "plugin",
            StepConfig::LlmScore(_) => "llm_score",
            StepConfig::LlmRewrite(_) => "llm_rewrite",
        }
    }

    /// Steps that send each document to a language model.
    pub fn is_llm(&self) -> bool {
        matches!(self, StepConfig::LlmScore(_) | StepConfig::LlmRewrite(_))
    }

    pub fn info(&self) -> &'static StepKindInfo {
        step_kind(self.kind()).expect("every StepConfig variant is listed in STEP_KINDS")
    }
}

pub fn step_kind(kind: &str) -> Option<&'static StepKindInfo> {
    STEP_KINDS.iter().find(|k| k.kind == kind)
}

fn on(config: StepConfig) -> PipelineStep {
    PipelineStep { enabled: true, config }
}

/// A ready-made recipe offered when creating a pipeline.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Data recipes use a language model (an OpenAI-compatible server).
    pub uses_llm: bool,
    pub build: fn() -> PipelineSpec,
}

/// Normalization and cheap filters that data recipes run before the model, so it only sees
/// clean, unique documents.
fn recipe_prep() -> Vec<PipelineStep> {
    vec![
        on(StepConfig::NormalizeUnicode(NormalizeUnicode::default())),
        on(StepConfig::CleanCharacters(CleanCharacters::default())),
        on(StepConfig::NormalizeWhitespace(NormalizeWhitespace::default())),
        on(StepConfig::LengthFilter(LengthFilter { min_words: Some(50), max_words: Some(5000), ..Default::default() })),
        on(StepConfig::ExactDedup(ExactDedup::default())),
    ]
}

fn recipe(name: &str, description: &str, last: StepConfig) -> PipelineSpec {
    let mut steps = recipe_prep();
    steps.push(on(last));
    PipelineSpec { name: name.into(), description: Some(description.into()), steps }
}

pub static PRESETS: &[Preset] = &[
    Preset {
        id: "gopher",
        label: "Gopher-style (recommended start)",
        description: "Normalization, PII masking, Gopher quality and repetition rules, blocklist, exact and near dedup.",
        uses_llm: false,
        build: || PipelineSpec::template("Web cleanup"),
    },
    Preset {
        id: "fineweb",
        label: "FineWeb-style",
        description: "Close to FineWeb: English (≥ 0.65), Gopher repetition and quality, C4 rules without the end-punctuation line rule, FineWeb rules, MinHash dedup (5-grams, 14 × 8), email and IP masking. FineWeb also extracted text with trafilatura and deduplicated per crawl.",
        uses_llm: false,
        build: || PipelineSpec {
            name: "FineWeb-style".into(),
            description: Some("Approximates the FineWeb filtering pipeline (Penedo et al., 2024).".into()),
            steps: vec![
                on(StepConfig::LanguageFilter(LanguageFilter { keep: vec!["eng".into()], min_confidence: 0.65 })),
                on(StepConfig::RepetitionFilter(RepetitionFilter::recommended())),
                on(StepConfig::LengthFilter(LengthFilter { min_words: Some(50), max_words: Some(100_000), ..Default::default() })),
                on(StepConfig::WordQualityFilter(WordQualityFilter::recommended())),
                on(StepConfig::C4Filter(C4Filter { drop_lines_without_terminal_punctuation: false, ..C4Filter::recommended() })),
                on(StepConfig::FineWebFilter(FineWebFilter::recommended())),
                on(StepConfig::NearDedup(NearDedup::default())),
                on(StepConfig::MaskPii(MaskPii { emails: true, ip_addresses: true, phone_numbers: false, card_numbers: false })),
            ],
        },
    },
    Preset {
        id: "c4",
        label: "C4-style",
        description: "Close to C4: English, the C4 line and document rules, exact dedup. C4 also used a bad-words list (add your own terms to a blocklist step) and removed repeated 3-sentence spans across the corpus.",
        uses_llm: false,
        build: || PipelineSpec {
            name: "C4-style".into(),
            description: Some("Approximates the C4 cleaning rules (Raffel et al., 2020).".into()),
            steps: vec![
                on(StepConfig::NormalizeWhitespace(NormalizeWhitespace::default())),
                on(StepConfig::LanguageFilter(LanguageFilter { keep: vec!["eng".into()], min_confidence: 0.9 })),
                on(StepConfig::C4Filter(C4Filter::recommended())),
                on(StepConfig::ExactDedup(ExactDedup::default())),
            ],
        },
    },
    Preset {
        id: "minimal",
        label: "Minimal",
        description: "Only normalization and exact dedup: keeps everything else, for data that is already clean.",
        uses_llm: false,
        build: || PipelineSpec {
            name: "Minimal cleanup".into(),
            description: Some("Normalization and exact deduplication only.".into()),
            steps: vec![
                on(StepConfig::NormalizeUnicode(NormalizeUnicode::default())),
                on(StepConfig::CleanCharacters(CleanCharacters::default())),
                on(StepConfig::NormalizeWhitespace(NormalizeWhitespace::default())),
                on(StepConfig::ExactDedup(ExactDedup::default())),
            ],
        },
    },
    Preset {
        id: "recipe_edu",
        label: "Recipe: educational quality (LLM)",
        description: "Normalize, length filter and dedup, then a language model scores each document 0–5 for educational value and keeps 3 and up, like FineWeb-Edu. Needs an OpenAI-compatible server (Ollama by default).",
        uses_llm: true,
        build: || recipe(
            "Educational quality (LLM)",
            "Keeps documents a language model scores 3 or more for educational value (FineWeb-Edu style).",
            StepConfig::LlmScore(LlmScore::default()),
        ),
    },
    Preset {
        id: "recipe_rephrase",
        label: "Recipe: rephrase web text (LLM)",
        description: "Normalize, filter and dedup, then a language model rewrites each document in clean encyclopedia style, keeping the facts (as in WRAP, Maini et al., 2024). Needs an OpenAI-compatible server.",
        uses_llm: true,
        build: || recipe(
            "Rephrase web text (LLM)",
            "Rewrites documents in encyclopedia style with a language model (WRAP-style synthetic rephrasing).",
            StepConfig::LlmRewrite(LlmRewrite::default()),
        ),
    },
    Preset {
        id: "recipe_qa",
        label: "Recipe: synthetic Q&A (LLM)",
        description: "Normalize, filter and dedup, then a language model turns each document into question-answer pairs grounded in its text. Needs an OpenAI-compatible server.",
        uses_llm: true,
        build: || recipe(
            "Synthetic Q&A (LLM)",
            "Generates question-answer pairs from each document with a language model.",
            StepConfig::LlmRewrite(LlmRewrite { prompt: QA_PROMPT.into(), temperature: 0.3, ..LlmRewrite::default() }),
        ),
    },
    Preset {
        id: "recipe_summary",
        label: "Recipe: summaries (LLM)",
        description: "Normalize, filter and dedup, then a language model writes a short factual summary of each document. Needs an OpenAI-compatible server.",
        uses_llm: true,
        build: || recipe(
            "Summaries (LLM)",
            "Replaces each document with a short summary written by a language model.",
            StepConfig::LlmRewrite(LlmRewrite { prompt: SUMMARY_PROMPT.into(), temperature: 0.2, max_tokens: 300, ..LlmRewrite::default() }),
        ),
    },
];

impl PipelineSpec {
    /// The default recipe: Gopher-style quality filtering for web text.
    pub fn template(name: impl Into<String>) -> Self {
        let kinds = [
            "normalize_unicode",
            "clean_characters",
            "normalize_whitespace",
            "mask_pii",
            "length_filter",
            "language_filter",
            "character_ratio_filter",
            "word_quality_filter",
            "repetition_filter",
            "blocklist_filter",
            "exact_dedup",
            "near_dedup",
        ];
        Self {
            name: name.into(),
            description: Some("Normalization, Gopher-style quality filters and deduplication for web text.".into()),
            steps: kinds.iter().map(|k| on((step_kind(k).expect("listed").default)())).collect(),
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
        assert_eq!(STEP_KINDS.len(), 17);
        assert_eq!(PipelineSpec::template("t").steps.len(), 12);
        for p in PRESETS {
            let spec = (p.build)();
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(serde_json::from_str::<PipelineSpec>(&json).unwrap(), spec, "{}", p.id);
        }
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
