//! Step implementations. Normalizers change text; filters keep or drop documents.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use aeon_shared::{
    BlocklistFilter, C4Filter, CharacterRatioFilter, FineWebFilter, CleanCharacters, LanguageFilter, LengthFilter, MaskPii, NormalizeUnicode,
    NormalizeWhitespace, RepetitionFilter, StepConfig, UnicodeForm, WordQualityFilter,
};
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick, is_nfkc_quick};

use crate::error::{AppError, AppResult};

pub enum Verdict {
    Unchanged,
    Changed,
    Drop { rule: &'static str, detail: String },
}

pub trait Step: Send + Sync {
    /// Transforms `text` in place, or decides to drop the document.
    fn apply(&self, text: &mut String) -> Verdict;
}

/// Builds a step from its settings, checking them first.
pub fn build(config: &StepConfig) -> AppResult<Box<dyn Step>> {
    Ok(match config {
        StepConfig::NormalizeUnicode(c) => Box::new(c.clone()),
        StepConfig::CleanCharacters(c) => Box::new(c.clone()),
        StepConfig::NormalizeWhitespace(c) => Box::new(c.clone()),
        StepConfig::MaskPii(c) => Box::new(PiiMasker::new(c)),
        StepConfig::LengthFilter(c) => {
            check_order("characters", c.min_chars, c.max_chars)?;
            check_order("words", c.min_words, c.max_words)?;
            Box::new(c.clone())
        }
        StepConfig::LanguageFilter(c) => Box::new(Language::new(c)?),
        StepConfig::CharacterRatioFilter(c) => {
            for r in [c.min_alpha_ratio, c.max_digit_ratio, c.max_symbol_ratio, c.max_uppercase_ratio, c.max_replacement_ratio] {
                check_ratio(r)?;
            }
            Box::new(c.clone())
        }
        StepConfig::WordQualityFilter(c) => {
            for r in [c.min_alpha_word_fraction, c.max_symbol_word_ratio, c.max_bullet_line_fraction, c.max_ellipsis_line_fraction] {
                check_ratio(r)?;
            }
            Box::new(c.clone())
        }
        StepConfig::RepetitionFilter(c) => {
            for r in [
                c.max_duplicate_line_fraction,
                c.max_duplicate_paragraph_fraction,
                c.max_top_2gram_fraction,
                c.max_top_3gram_fraction,
                c.max_top_4gram_fraction,
                c.max_duplicate_5gram_fraction,
                c.max_duplicate_10gram_fraction,
            ] {
                check_ratio(r)?;
            }
            Box::new(c.clone())
        }
        StepConfig::BlocklistFilter(c) => Box::new(Blocklist::new(c)),
        StepConfig::C4Filter(c) => Box::new(c.clone()),
        StepConfig::FineWebFilter(c) => {
            for r in [c.min_line_punctuation_ratio, c.max_short_line_ratio, c.max_duplicate_line_char_ratio, c.max_newline_word_ratio] {
                check_ratio(r)?;
            }
            Box::new(c.clone())
        }
        StepConfig::ExactDedup(_)
        | StepConfig::NearDedup(_)
        | StepConfig::Plugin(_)
        | StepConfig::LlmScore(_)
        | StepConfig::LlmRewrite(_) => {
            return Err(AppError::Internal("dedup steps are compiled by the pipeline, not as document steps".into()));
        }
    })
}

fn check_ratio(r: Option<f64>) -> AppResult<()> {
    match r {
        Some(v) if !(0.0..=1.0).contains(&v) => Err(AppError::InvalidInput(format!("{v} is not a ratio between 0 and 1"))),
        _ => Ok(()),
    }
}

fn check_order(what: &str, min: Option<u64>, max: Option<u64>) -> AppResult<()> {
    match (min, max) {
        (Some(lo), Some(hi)) if lo > hi => Err(AppError::InvalidInput(format!("min {what} ({lo}) is above max ({hi})"))),
        _ => Ok(()),
    }
}

fn reject(rule: &'static str, detail: String) -> Verdict {
    Verdict::Drop { rule, detail }
}

fn replace_if_changed(text: &mut String, new: String) -> Verdict {
    if new == *text {
        Verdict::Unchanged
    } else {
        *text = new;
        Verdict::Changed
    }
}

// ---------------------------------------------------------------------------------------------
// Normalizers
// ---------------------------------------------------------------------------------------------

impl Step for NormalizeUnicode {
    fn apply(&self, text: &mut String) -> Verdict {
        match self.form {
            UnicodeForm::Nfc if is_nfc_quick(text.chars()) == IsNormalized::Yes => Verdict::Unchanged,
            UnicodeForm::Nfkc if is_nfkc_quick(text.chars()) == IsNormalized::Yes => Verdict::Unchanged,
            UnicodeForm::Nfc => replace_if_changed(text, text.nfc().collect()),
            UnicodeForm::Nfkc => replace_if_changed(text, text.nfkc().collect()),
        }
    }
}

/// Characters typical of UTF-8 text mis-decoded as Windows-1252.
fn mojibake_score(s: &str) -> usize {
    s.matches("Ã").count() + s.matches("â€").count() + s.matches("Â").count()
}

/// Reverses a UTF-8 → Windows-1252 mis-decoding when the whole text can be reversed and the
/// result has fewer telltale sequences. Text that mixes in characters outside Windows-1252
/// (e.g. genuine CJK) is left alone.
pub fn fix_mojibake(s: &str) -> Option<String> {
    let before = mojibake_score(s);
    if before == 0 {
        return None;
    }
    let (bytes, _, had_unmappable) = encoding_rs::WINDOWS_1252.encode(s);
    if had_unmappable {
        return None;
    }
    let fixed = std::str::from_utf8(&bytes).ok()?;
    (mojibake_score(fixed) < before).then(|| fixed.to_string())
}

fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

fn is_nbsp(c: char) -> bool {
    matches!(c, '\u{00A0}' | '\u{2007}' | '\u{202F}')
}

fn affects(cfg: &CleanCharacters, c: char) -> bool {
    (cfg.remove_control && c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
        || (cfg.remove_zero_width && is_zero_width(c))
        || (cfg.replace_nbsp && is_nbsp(c))
}

impl Step for CleanCharacters {
    fn apply(&self, text: &mut String) -> Verdict {
        let mut changed = false;
        if self.fix_mojibake
            && let Some(fixed) = fix_mojibake(text) {
                *text = fixed;
                changed = true;
            }
        if text.chars().any(|c| affects(self, c)) {
            *text = text
                .chars()
                .filter_map(|c| {
                    if self.replace_nbsp && is_nbsp(c) {
                        Some(' ')
                    } else if affects(self, c) {
                        None
                    } else {
                        Some(c)
                    }
                })
                .collect();
            changed = true;
        }
        if changed { Verdict::Changed } else { Verdict::Unchanged }
    }
}

impl Step for NormalizeWhitespace {
    fn apply(&self, text: &mut String) -> Verdict {
        let unified = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut out = String::with_capacity(unified.len());
        let mut blank_run = 0u64;
        for line in unified.split('\n') {
            let mut line = if self.collapse_spaces { collapse_spaces(line) } else { line.to_string() };
            if self.trim_lines {
                line.truncate(line.trim_end().len());
            }
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run > self.max_blank_lines {
                    continue;
                }
            } else {
                blank_run = 0;
            }
            out.push_str(&line);
            out.push('\n');
        }
        out.pop(); // the loop adds one newline too many
        if self.trim_document {
            let trimmed = out.trim();
            if trimmed.len() != out.len() {
                out = trimmed.to_string();
            }
        }
        replace_if_changed(text, out)
    }
}

/// Collapses runs of spaces/tabs after the leading indentation into single spaces.
fn collapse_spaces(line: &str) -> String {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let mut out = String::with_capacity(line.len());
    out.push_str(indent);
    let mut in_run = false;
    for c in rest.chars() {
        if c == ' ' || c == '\t' {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

struct PiiMasker {
    email: Option<regex::Regex>,
    ipv4: Option<regex::Regex>,
    ipv6: Option<regex::Regex>,
    phone: Option<regex::Regex>,
    card: Option<regex::Regex>,
}

fn digits(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn luhn(d: &[u32]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| if i % 2 == 1 { if x * 2 > 9 { x * 2 - 9 } else { x * 2 } } else { x })
        .sum();
    sum.is_multiple_of(10)
}

impl PiiMasker {
    fn new(c: &MaskPii) -> Self {
        let re = |on: bool, pattern: &str| on.then(|| regex::Regex::new(pattern).expect("valid pattern"));
        Self {
            email: re(c.emails, r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b"),
            ipv4: re(
                c.ip_addresses,
                r"\b(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}\b",
            ),
            ipv6: re(c.ip_addresses, r"(?i)\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b"),
            // Optional country code, then groups separated by space, dot or dash, or an area code in brackets.
            phone: re(c.phone_numbers, r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{1,4}\)[ .-]?)?\d{2,4}(?:[ .-]\d{2,4}){1,4}\b"),
            card: re(c.card_numbers, r"\b\d(?:[ -]?\d){12,18}\b"),
        }
    }
}

impl Step for PiiMasker {
    fn apply(&self, text: &mut String) -> Verdict {
        let mut out = std::borrow::Cow::Borrowed(text.as_str());
        if let Some(re) = &self.email
            && let std::borrow::Cow::Owned(s) = re.replace_all(&out, "<EMAIL>") {
                out = std::borrow::Cow::Owned(s);
            }
        // Cards before phones: a card number also looks like a long phone number.
        if let Some(re) = &self.card {
            let replaced = re.replace_all(&out, |c: &regex::Captures| {
                let m = &c[0];
                if luhn(&digits(m)) { "<CARD>".to_string() } else { m.to_string() }
            });
            if replaced != out {
                out = std::borrow::Cow::Owned(replaced.into_owned());
            }
        }
        if let Some(re) = &self.ipv6
            && let std::borrow::Cow::Owned(s) = re.replace_all(&out, "<IP>") {
                out = std::borrow::Cow::Owned(s);
            }
        if let Some(re) = &self.ipv4
            && let std::borrow::Cow::Owned(s) = re.replace_all(&out, "<IP>") {
                out = std::borrow::Cow::Owned(s);
            }
        if let Some(re) = &self.phone {
            let replaced = re.replace_all(&out, |c: &regex::Captures| {
                let m = &c[0];
                let n = digits(m).len();
                // Separated digit groups of phone length, excluding things like dates (2026-11-17).
                let looks_like_date = m.len() == 10 && m.as_bytes()[4] == b'-' && m.as_bytes()[7] == b'-';
                if (9..=15).contains(&n) && !looks_like_date { "<PHONE>".to_string() } else { m.to_string() }
            });
            if replaced != out {
                out = std::borrow::Cow::Owned(replaced.into_owned());
            }
        }
        match out {
            std::borrow::Cow::Borrowed(_) => Verdict::Unchanged,
            std::borrow::Cow::Owned(s) => replace_if_changed(text, s),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------------------------

impl Step for LengthFilter {
    fn apply(&self, text: &mut String) -> Verdict {
        if self.min_chars.is_some() || self.max_chars.is_some() {
            let chars = text.chars().count() as u64;
            if let Some(min) = self.min_chars.filter(|&m| chars < m) {
                return reject("min_chars", format!("{chars} chars < {min}"));
            }
            if let Some(max) = self.max_chars.filter(|&m| chars > m) {
                return reject("max_chars", format!("{chars} chars > {max}"));
            }
        }
        if self.min_words.is_some() || self.max_words.is_some() {
            let words = text.split_whitespace().count() as u64;
            if let Some(min) = self.min_words.filter(|&m| words < m) {
                return reject("min_words", format!("{words} words < {min}"));
            }
            if let Some(max) = self.max_words.filter(|&m| words > m) {
                return reject("max_words", format!("{words} words > {max}"));
            }
        }
        Verdict::Unchanged
    }
}

struct Language {
    keep: HashSet<whatlang::Lang>,
    min_confidence: f64,
}

/// Language detection looks at this many characters at most.
const LANGUAGE_SAMPLE_CHARS: usize = 2000;

impl Language {
    fn new(c: &LanguageFilter) -> AppResult<Self> {
        check_ratio(Some(c.min_confidence))?;
        let mut keep = HashSet::new();
        for code in c.keep.iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
            let lang = whatlang::Lang::from_code(&code).ok_or_else(|| {
                AppError::InvalidInput(format!("Unknown language code \"{code}\". Use ISO 639-3 codes such as eng, deu, fra."))
            })?;
            keep.insert(lang);
        }
        if keep.is_empty() {
            return Err(AppError::InvalidInput("Language filter: list at least one language to keep".into()));
        }
        Ok(Self { keep, min_confidence: c.min_confidence })
    }
}

impl Step for Language {
    fn apply(&self, text: &mut String) -> Verdict {
        let sample = match text.char_indices().nth(LANGUAGE_SAMPLE_CHARS) {
            Some((i, _)) => &text[..i],
            None => text.as_str(),
        };
        let Some(info) = whatlang::detect(sample) else {
            return reject("language_unknown", "no language detected".into());
        };
        let (lang, confidence) = (info.lang(), info.confidence());
        if !self.keep.contains(&lang) {
            return reject("language", format!("{} ({confidence:.2})", lang.code()));
        }
        if confidence < self.min_confidence {
            return reject("language_confidence", format!("{} {confidence:.2} < {:.2}", lang.code(), self.min_confidence));
        }
        Verdict::Unchanged
    }
}

impl Step for CharacterRatioFilter {
    fn apply(&self, text: &mut String) -> Verdict {
        let (mut total, mut alpha, mut digit, mut symbol, mut upper, mut replacement) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for c in text.chars().filter(|c| !c.is_whitespace()) {
            total += 1;
            if c.is_alphabetic() {
                alpha += 1;
                if c.is_uppercase() {
                    upper += 1;
                }
            } else if c.is_numeric() {
                digit += 1;
            } else {
                symbol += 1;
                if c == '\u{FFFD}' {
                    replacement += 1;
                }
            }
        }
        if total == 0 {
            return reject("no_visible_characters", "only whitespace".into());
        }
        let ratio = |n: u64, d: u64| if d == 0 { 0.0 } else { n as f64 / d as f64 };
        let checks: [(&'static str, Option<f64>, f64, bool); 5] = [
            ("min_alpha_ratio", self.min_alpha_ratio, ratio(alpha, total), true),
            ("max_digit_ratio", self.max_digit_ratio, ratio(digit, total), false),
            ("max_symbol_ratio", self.max_symbol_ratio, ratio(symbol, total), false),
            ("max_uppercase_ratio", self.max_uppercase_ratio, ratio(upper, alpha), false),
            ("max_replacement_ratio", self.max_replacement_ratio, ratio(replacement, total), false),
        ];
        threshold_checks(&checks)
    }
}

/// `(rule, threshold, value, is_minimum)` → the first violated rule, if any.
fn threshold_checks(checks: &[(&'static str, Option<f64>, f64, bool)]) -> Verdict {
    for &(rule, threshold, value, is_min) in checks {
        if let Some(t) = threshold {
            if is_min && value < t {
                return reject(rule, format!("{value:.3} < {t}"));
            }
            if !is_min && value > t {
                return reject(rule, format!("{value:.3} > {t}"));
            }
        }
    }
    Verdict::Unchanged
}

const BULLETS: [char; 8] = ['•', '‣', '◦', '⁃', '▪', '●', '-', '*'];

impl Step for WordQualityFilter {
    fn apply(&self, text: &mut String) -> Verdict {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return reject("no_words", "no words".into());
        }
        let n = words.len() as f64;
        let mean_len = words.iter().map(|w| w.chars().count()).sum::<usize>() as f64 / n;
        let alpha_words = words.iter().filter(|w| w.chars().any(char::is_alphabetic)).count() as f64 / n;
        let symbols = (text.matches('#').count() + text.matches("...").count() + text.matches('…').count()) as f64 / n;
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let line_frac = |pred: &dyn Fn(&str) -> bool| {
            if lines.is_empty() { 0.0 } else { lines.iter().filter(|l| pred(l)).count() as f64 / lines.len() as f64 }
        };
        let bullets = line_frac(&|l| l.starts_with(BULLETS));
        let ellipsis = line_frac(&|l| l.ends_with("...") || l.ends_with('…'));

        let checks: [(&'static str, Option<f64>, f64, bool); 6] = [
            ("min_mean_word_length", self.min_mean_word_length, mean_len, true),
            ("max_mean_word_length", self.max_mean_word_length, mean_len, false),
            ("min_alpha_word_fraction", self.min_alpha_word_fraction, alpha_words, true),
            ("max_symbol_word_ratio", self.max_symbol_word_ratio, symbols, false),
            ("max_bullet_line_fraction", self.max_bullet_line_fraction, bullets, false),
            ("max_ellipsis_line_fraction", self.max_ellipsis_line_fraction, ellipsis, false),
        ];
        threshold_checks(&checks)
    }
}

fn hash_words(words: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    words.hash(&mut h);
    h.finish()
}

/// Fraction of non-empty items that repeat an earlier item.
fn duplicate_fraction<'a>(items: impl Iterator<Item = &'a str>) -> f64 {
    let mut seen = HashSet::new();
    let (mut total, mut dup) = (0usize, 0usize);
    for item in items.map(str::trim).filter(|s| !s.is_empty()) {
        total += 1;
        if !seen.insert(item) {
            dup += 1;
        }
    }
    if total == 0 { 0.0 } else { dup as f64 / total as f64 }
}

/// Share of word characters taken by the single most frequent repeated n-gram.
fn top_ngram_fraction(words: &[&str], lens: &[usize], total_chars: usize, n: usize) -> f64 {
    if words.len() < n || total_chars == 0 {
        return 0.0;
    }
    let mut counts: HashMap<u64, (usize, usize)> = HashMap::new();
    for (i, gram) in words.windows(n).enumerate() {
        let chars: usize = lens[i..i + n].iter().sum();
        counts.entry(hash_words(gram)).or_insert((0, chars)).0 += 1;
    }
    counts
        .values()
        .filter(|(count, _)| *count > 1)
        .map(|(count, chars)| count * chars)
        .max()
        .map_or(0.0, |c| c as f64 / total_chars as f64)
}

/// Share of word characters covered by n-grams that occur more than once.
fn duplicate_ngram_fraction(words: &[&str], lens: &[usize], total_chars: usize, n: usize) -> f64 {
    if words.len() < n || total_chars == 0 {
        return 0.0;
    }
    let hashes: Vec<u64> = words.windows(n).map(hash_words).collect();
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for h in &hashes {
        *counts.entry(*h).or_default() += 1;
    }
    let mut covered = vec![false; words.len()];
    for (i, h) in hashes.iter().enumerate() {
        if counts[h] > 1 {
            covered[i..i + n].iter_mut().for_each(|c| *c = true);
        }
    }
    let dup_chars: usize = covered.iter().zip(lens).filter(|(c, _)| **c).map(|(_, l)| l).sum();
    dup_chars as f64 / total_chars as f64
}

impl Step for RepetitionFilter {
    fn apply(&self, text: &mut String) -> Verdict {
        if let Some(t) = self.max_duplicate_line_fraction {
            let f = duplicate_fraction(text.lines());
            if f > t {
                return reject("max_duplicate_line_fraction", format!("{f:.3} > {t}"));
            }
        }
        if let Some(t) = self.max_duplicate_paragraph_fraction {
            let f = duplicate_fraction(text.split("\n\n"));
            if f > t {
                return reject("max_duplicate_paragraph_fraction", format!("{f:.3} > {t}"));
            }
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let lens: Vec<usize> = words.iter().map(|w| w.chars().count()).collect();
        let total: usize = lens.iter().sum();
        let top = [
            ("max_top_2gram_fraction", self.max_top_2gram_fraction, 2),
            ("max_top_3gram_fraction", self.max_top_3gram_fraction, 3),
            ("max_top_4gram_fraction", self.max_top_4gram_fraction, 4),
        ];
        for (rule, threshold, n) in top {
            if let Some(t) = threshold {
                let f = top_ngram_fraction(&words, &lens, total, n);
                if f > t {
                    return reject(rule, format!("{f:.3} > {t}"));
                }
            }
        }
        let dup = [
            ("max_duplicate_5gram_fraction", self.max_duplicate_5gram_fraction, 5),
            ("max_duplicate_10gram_fraction", self.max_duplicate_10gram_fraction, 10),
        ];
        for (rule, threshold, n) in dup {
            if let Some(t) = threshold {
                let f = duplicate_ngram_fraction(&words, &lens, total, n);
                if f > t {
                    return reject(rule, format!("{f:.3} > {t}"));
                }
            }
        }
        Verdict::Unchanged
    }
}

/// Ends in sentence punctuation, allowing a closing quote or bracket after it.
fn ends_with_terminal_punctuation(line: &str) -> bool {
    let t = line.trim_end().trim_end_matches(['"', '\'', '”', '’', ')', ']', '»']);
    t.ends_with(['.', '!', '?', '…'])
}

const POLICY_PHRASES: [&str; 6] =
    ["terms of use", "privacy policy", "cookie policy", "uses cookies", "use of cookies", "use cookies"];

impl Step for C4Filter {
    fn apply(&self, text: &mut String) -> Verdict {
        let lower_doc = text.to_lowercase();
        if self.drop_lorem_ipsum && lower_doc.contains("lorem ipsum") {
            return reject("c4_lorem_ipsum", "contains lorem ipsum".into());
        }
        if self.drop_curly_brackets && text.contains('{') {
            return reject("c4_curly_bracket", "contains {".into());
        }
        let mut kept: Vec<&str> = Vec::new();
        let mut removed = 0usize;
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() {
                kept.push(line);
                continue;
            }
            let lower = t.to_lowercase();
            let words: Vec<&str> = t.split_whitespace().collect();
            let drop = (self.drop_lines_without_terminal_punctuation && !ends_with_terminal_punctuation(t))
                || self.min_words_per_line.is_some_and(|m| (words.len() as u64) < m)
                || self.max_word_length.is_some_and(|m| words.iter().any(|w| w.chars().count() as u64 > m))
                || (self.drop_javascript_lines && lower.contains("javascript"))
                || (self.drop_policy_lines && POLICY_PHRASES.iter().any(|p| lower.contains(p)));
            if drop {
                removed += 1;
            } else {
                kept.push(line);
            }
        }
        let new_text = if removed > 0 { kept.join("\n").trim().to_string() } else { text.clone() };
        if new_text.trim().is_empty() {
            return reject("c4_no_lines_left", format!("all {removed} lines removed"));
        }
        if let Some(min) = self.min_sentences {
            let sentences = new_text
                .split_inclusive(['.', '!', '?'])
                .filter(|s| s.trim_end().ends_with(['.', '!', '?']) && s.split_whitespace().count() > 0)
                .count() as u64;
            if sentences < min {
                return reject("c4_min_sentences", format!("{sentences} sentences < {min}"));
            }
        }
        replace_if_changed(text, new_text)
    }
}

impl Step for FineWebFilter {
    fn apply(&self, text: &mut String) -> Verdict {
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            return reject("fineweb_no_lines", "no text".into());
        }
        let n = lines.len() as f64;
        if let Some(t) = self.min_line_punctuation_ratio {
            let r = lines.iter().filter(|l| ends_with_terminal_punctuation(l)).count() as f64 / n;
            if r < t {
                return reject("fineweb_line_punctuation", format!("{r:.3} < {t}"));
            }
        }
        if let Some(t) = self.max_short_line_ratio {
            let r = lines.iter().filter(|l| l.chars().count() as u64 <= self.short_line_length).count() as f64 / n;
            if r > t {
                return reject("fineweb_short_lines", format!("{r:.3} > {t}"));
            }
        }
        if let Some(t) = self.max_duplicate_line_char_ratio {
            let mut seen = HashSet::new();
            let (mut dup, mut total) = (0usize, 0usize);
            for l in &lines {
                let c = l.chars().count();
                total += c;
                if !seen.insert(*l) {
                    dup += c;
                }
            }
            let r = if total == 0 { 0.0 } else { dup as f64 / total as f64 };
            if r > t {
                return reject("fineweb_duplicate_lines", format!("{r:.3} > {t}"));
            }
        }
        if let Some(t) = self.max_newline_word_ratio {
            let words = text.split_whitespace().count().max(1) as f64;
            let r = text.matches('\n').count() as f64 / words;
            if r > t {
                return reject("fineweb_list_like", format!("{r:.3} > {t}"));
            }
        }
        Verdict::Unchanged
    }
}

struct Blocklist {
    single: HashMap<String, String>,
    phrases: Vec<(Vec<String>, String)>,
    case_sensitive: bool,
    max_hits: u64,
}

fn tokens(text: &str, case_sensitive: bool) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| if case_sensitive { t.to_string() } else { t.to_lowercase() })
        .collect()
}

impl Blocklist {
    fn new(c: &BlocklistFilter) -> Self {
        let mut single = HashMap::new();
        let mut phrases = Vec::new();
        for term in c.terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            let toks = tokens(term, c.case_sensitive);
            match toks.len() {
                0 => {}
                1 => {
                    single.insert(toks[0].clone(), term.to_string());
                }
                _ => phrases.push((toks, term.to_string())),
            }
        }
        Self { single, phrases, case_sensitive: c.case_sensitive, max_hits: c.max_hits }
    }
}

impl Step for Blocklist {
    fn apply(&self, text: &mut String) -> Verdict {
        if self.single.is_empty() && self.phrases.is_empty() {
            return Verdict::Unchanged;
        }
        let toks = tokens(text, self.case_sensitive);
        let mut hits = 0u64;
        let mut first: Option<&str> = None;
        for (i, t) in toks.iter().enumerate() {
            if let Some(term) = self.single.get(t) {
                hits += 1;
                first.get_or_insert(term);
            }
            for (phrase, term) in &self.phrases {
                if toks[i..].starts_with(phrase) {
                    hits += 1;
                    first.get_or_insert(term);
                }
            }
        }
        if hits > self.max_hits {
            return reject("blocklist", format!("{hits} match(es), first “{}”", first.unwrap_or_default()));
        }
        Verdict::Unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(step: &dyn Step, text: &str) -> (String, Option<&'static str>, bool) {
        let mut t = text.to_string();
        match step.apply(&mut t) {
            Verdict::Unchanged => (t, None, false),
            Verdict::Changed => (t, None, true),
            Verdict::Drop { rule, .. } => (t, Some(rule), false),
        }
    }

    #[test]
    fn unicode_nfc_and_nfkc() {
        let decomposed = "cafe\u{301}";
        assert_eq!(run(&NormalizeUnicode { form: UnicodeForm::Nfc }, decomposed).0, "café");
        assert!(!run(&NormalizeUnicode { form: UnicodeForm::Nfc }, "café").2);
        assert_eq!(run(&NormalizeUnicode { form: UnicodeForm::Nfkc }, "ﬁne Ｆｕｌｌ").0, "fine Full");
    }

    #[test]
    fn mojibake_is_fixed_only_when_it_helps() {
        assert_eq!(fix_mojibake("cafÃ© and naÃ¯ve â€” done").as_deref(), Some("café and naïve — done"));
        assert_eq!(fix_mojibake("plain text"), None);
        // Contains a character Windows-1252 can't encode, so the text is left alone.
        assert_eq!(fix_mojibake("cafÃ© 日本"), None);
    }

    #[test]
    fn clean_characters() {
        let (t, _, changed) = run(&CleanCharacters::default(), "a\u{200B}b\u{00A0}c\u{7}d\ne\tf");
        assert!(changed);
        assert_eq!(t, "ab cd\ne\tf");
    }

    #[test]
    fn whitespace() {
        let (t, _, changed) = run(&NormalizeWhitespace::default(), "  Title  \r\n\r\n\r\n\r\n  indented   text\t\there \n\n");
        assert!(changed);
        assert_eq!(t, "Title\n\n  indented text here");
        assert!(!run(&NormalizeWhitespace::default(), "already\n\nclean").2);
    }

    #[test]
    fn length_filter() {
        let f = LengthFilter { min_words: Some(3), max_chars: Some(20), ..Default::default() };
        assert_eq!(run(&f, "two words").1, Some("min_words"));
        assert_eq!(run(&f, "this one is far too long").1, Some("max_chars"));
        assert_eq!(run(&f, "just three words").1, None);
    }

    #[test]
    fn language_filter() {
        let f = Language::new(&LanguageFilter::default()).unwrap();
        let en = "The committee will publish its findings next week after reviewing all of the evidence.";
        let de = "Der Ausschuss wird seine Ergebnisse nächste Woche veröffentlichen, nachdem er alle Beweise geprüft hat.";
        assert_eq!(run(&f, en).1, None);
        assert_eq!(run(&f, de).1, Some("language"));
        assert!(Language::new(&LanguageFilter { keep: vec!["xx".into()], min_confidence: 0.5 }).is_err());
    }

    #[test]
    fn character_ratios() {
        let f = CharacterRatioFilter::recommended();
        assert_eq!(run(&f, "Normal sentence with words.").1, None);
        assert_eq!(run(&f, "12345 67890 12 34 a").1, Some("min_alpha_ratio"));
        assert_eq!(run(&f, "THIS IS ALL SHOUTING TEXT").1, Some("max_uppercase_ratio"));
        assert_eq!(run(&f, "   \n ").1, Some("no_visible_characters"));
    }

    #[test]
    fn word_quality() {
        let f = WordQualityFilter::recommended();
        assert_eq!(run(&f, "A perfectly ordinary sentence about the weather today.").1, None);
        assert_eq!(run(&f, "a b c d e f g h").1, Some("min_mean_word_length"));
        // A menu fails several rules; the "-" bullets are also words without letters.
        assert_eq!(run(&f, "- Home\n- About\n- Contact\n- Blog").1, Some("min_alpha_word_fraction"));
        let bullets_only = WordQualityFilter { max_bullet_line_fraction: Some(0.9), ..Default::default() };
        assert_eq!(run(&bullets_only, "- Home\n- About\n- Contact\n- Blog").1, Some("max_bullet_line_fraction"));
        assert_eq!(run(&bullets_only, "- Home\nA normal line of text").1, None);
    }

    #[test]
    fn repetition() {
        let f = RepetitionFilter::recommended();
        let prose = "The river rose quickly after the storm. Farmers moved their animals to higher ground before nightfall. By morning the water had reached the old bridge.";
        assert_eq!(run(&f, prose).1, None);
        assert_eq!(run(&f, "buy now\nbuy now\nbuy now\nclick here").1, Some("max_duplicate_line_fraction"));
        let spam = "best cheap shoes ".repeat(20);
        assert!(run(&f, &spam).1.is_some());
    }

    #[test]
    fn ngram_fractions() {
        let words: Vec<&str> = "a b a b a b c".split(' ').collect();
        let lens = vec![1; words.len()];
        // "a b" appears 3 times → 3 × 2 chars / 7 chars.
        assert!((top_ngram_fraction(&words, &lens, 7, 2) - 6.0 / 7.0).abs() < 1e-9);
        // Every 2-gram except "b c" repeats; all words but "c" are covered.
        assert!((duplicate_ngram_fraction(&words, &lens, 7, 2) - 6.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn blocklist_matches_whole_words_and_phrases() {
        let f = Blocklist::new(&BlocklistFilter { terms: vec!["spam".into(), "Lorem Ipsum".into()], case_sensitive: false, max_hits: 0 });
        assert_eq!(run(&f, "Lorem ipsum dolor").1, Some("blocklist"));
        assert_eq!(run(&f, "this is SPAM!").1, Some("blocklist"));
        assert_eq!(run(&f, "spammer and lorem only").1, None);
        let lenient = Blocklist::new(&BlocklistFilter { terms: vec!["spam".into()], case_sensitive: false, max_hits: 1 });
        assert_eq!(run(&lenient, "spam once").1, None);
        assert_eq!(run(&lenient, "spam spam").1, Some("blocklist"));
    }

    #[test]
    fn pii_masking() {
        let m = PiiMasker::new(&MaskPii::default());
        let (t, _, changed) = run(
            &m,
            "Mail jane.doe@example.org or call +1 555 123 4567 / (030) 1234-5678. Server 192.168.1.20, \
             card 4111 1111 1111 1111. Meeting on 2026-11-17, order 12345, year 1999.",
        );
        assert!(changed);
        assert_eq!(
            t,
            "Mail <EMAIL> or call <PHONE> / <PHONE>. Server <IP>, card <CARD>. Meeting on 2026-11-17, order 12345, year 1999."
        );
        // Not a valid card number (fails Luhn) and too long for a phone: left alone.
        assert!(!run(&m, "Ref 1234 5678 9012 3456 7").2);
        let off = PiiMasker::new(&MaskPii { emails: false, ..MaskPii::default() });
        assert_eq!(run(&off, "a@b.co").0, "a@b.co");
        assert!(!run(&m, "Nothing personal here.").2);
    }

    #[test]
    fn c4_removes_lines_and_drops_documents() {
        let f = C4Filter::recommended();
        let doc = "Home | About\nThe garden was quiet in the morning. Birds sang in the old oak tree.\n\
                   Please enable JavaScript to view this site.\nWe water the beds every evening in summer.\n\
                   This site uses cookies to improve your experience.\nTomatoes need full sun to ripen well.\n\
                   Our neighbours grow beans and squash as well.\nShare";
        let (t, rule, changed) = run(&f, doc);
        assert_eq!(rule, None);
        assert!(changed);
        assert!(!t.contains("Home") && !t.contains("JavaScript") && !t.contains("cookies") && !t.contains("Share"));
        assert!(t.starts_with("The garden was quiet"));
        assert_eq!(run(&f, "One sentence only. Two sentences here.").1, Some("c4_min_sentences"));
        assert_eq!(run(&f, "Lorem ipsum dolor sit amet. A. B. C. D. E.").1, Some("c4_lorem_ipsum"));
        assert_eq!(run(&f, "fn main() { println!(\"hi\"); }").1, Some("c4_curly_bracket"));
        assert_eq!(run(&f, "menu\nlogin\nsearch").1, Some("c4_no_lines_left"));
        assert!(ends_with_terminal_punctuation("He said \"stop.\""));
    }

    #[test]
    fn fineweb_rules() {
        let f = FineWebFilter::recommended();
        let prose = "The committee met on Tuesday to review the budget for the coming year.\nIt agreed to fund the new library and repair the old bridge.";
        assert_eq!(run(&f, prose).1, None);
        assert_eq!(run(&f, "Products\nPricing\nBlog\nCareers\nContact us today").1, Some("fineweb_line_punctuation"));
        let short = "Yes.\nNo.\nMaybe.\nThe long sentence about the weather continues for quite a while here.";
        assert_eq!(run(&f, short).1, Some("fineweb_short_lines"));
        let dup = format!("{prose}\n{prose}");
        assert_eq!(run(&f, &dup).1, Some("fineweb_duplicate_lines"));
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let bad = StepConfig::LengthFilter(LengthFilter { min_words: Some(10), max_words: Some(5), ..Default::default() });
        assert!(build(&bad).is_err());
        let bad = StepConfig::CharacterRatioFilter(CharacterRatioFilter { max_digit_ratio: Some(1.5), ..Default::default() });
        assert!(build(&bad).is_err());
    }
}
