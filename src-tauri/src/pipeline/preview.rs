//! Trying a pipeline on a sample of a dataset, to tune settings before a full run.

use std::collections::HashMap;

use aeon_shared::{ChangeExample, DropExample, PipelinePreview, PipelineSpec};
use rayon::prelude::*;

use super::{Pipeline, StatsAccumulator};
use crate::dataset::DatasetReader;
use crate::error::AppResult;

/// The sample is taken in this many evenly spaced chunks, so it covers different parts
/// (and usually different source files) of the dataset.
const SAMPLE_CHUNKS: u64 = 20;
const EXAMPLES_PER_RULE: usize = 3;
const EXAMPLES_PER_STEP: usize = 3;
const EXAMPLE_CHARS: usize = 300;
pub const MAX_SAMPLE: u64 = 20_000;

pub fn preview(spec: &PipelineSpec, reader: &DatasetReader, sample_size: u64) -> AppResult<PipelinePreview> {
    let pipeline = Pipeline::compile(spec)?;
    let total = reader.row_count();
    let sample_size = sample_size.clamp(1, MAX_SAMPLE).min(total);

    let docs = sample(reader, sample_size)?;

    let results: Vec<_> = docs
        .into_par_iter()
        .map(|(id, text)| {
            let outcome = pipeline.process(text.clone());
            (id, text, outcome)
        })
        .collect();

    let mut stats = StatsAccumulator::new(&pipeline);
    let mut drop_examples = Vec::new();
    let mut per_rule: HashMap<(usize, &'static str), usize> = HashMap::new();
    let mut change_examples = Vec::new();
    let mut per_step: HashMap<usize, usize> = HashMap::new();

    for (id, original, o) in &results {
        stats.add(o);
        if let Some(d) = &o.dropped {
            let n = per_rule.entry((d.pos, d.rule)).or_default();
            if *n < EXAMPLES_PER_RULE {
                *n += 1;
                drop_examples.push(DropExample {
                    step_index: pipeline.steps[d.pos].index,
                    rule: d.rule.to_string(),
                    detail: d.detail.clone(),
                    id: id.clone(),
                    preview: o.text.chars().take(EXAMPLE_CHARS).collect(),
                });
            }
        }
        // Show what normalizers changed: re-run the steps up to the changing one to get the
        // before/after text of that step alone.
        for pos in (0..pipeline.steps.len()).filter(|p| o.modified & (1 << p) != 0) {
            let n = per_step.entry(pos).or_default();
            if *n >= EXAMPLES_PER_STEP {
                continue;
            }
            *n += 1;
            let mut text = original.clone();
            for s in &pipeline.steps[..pos] {
                let _ = s.step.apply(&mut text);
            }
            let before = text.clone();
            let _ = pipeline.steps[pos].step.apply(&mut text);
            let (b, a) = diff_window(&before, &text);
            change_examples.push(ChangeExample { step_index: pipeline.steps[pos].index, id: id.clone(), before: b, after: a });
        }
    }

    Ok(PipelinePreview {
        sample_docs: stats.docs_in,
        docs_out: stats.docs_out,
        steps: stats.step_stats(&pipeline),
        drop_examples,
        change_examples,
    })
}

/// `size` distinct documents as `(id, text)`, spread across the dataset.
///
/// When the sample is at least half the dataset, the dataset is read whole (at most
/// 2 × [`MAX_SAMPLE`] rows) and every k-th row is kept. Otherwise [`SAMPLE_CHUNKS`] evenly
/// spaced runs of rows are read, each contiguously (cheap with Parquet row groups); the runs
/// can't overlap because the gap between run starts is at least twice the run length.
fn sample(reader: &DatasetReader, size: u64) -> AppResult<Vec<(String, String)>> {
    let total = reader.row_count();
    let size = size.min(total);
    let mut docs = Vec::with_capacity(size as usize);
    if size == 0 {
        return Ok(docs);
    }
    if size * 2 >= total {
        // Row i * total / size for i in 0..size: strictly increasing, so all distinct.
        let mut next_i = 0u64;
        reader.scan(0, None, |row, r| {
            if row == next_i * total / size {
                docs.push((r.id.to_string(), r.text.to_string()));
                next_i += 1;
            }
            next_i < size
        })?;
    } else {
        let chunks = SAMPLE_CHUNKS.min(size);
        for c in 0..chunks {
            let start = total * c / chunks;
            // Spread the remainder over the first chunks so the sizes add up exactly.
            let n = size / chunks + u64::from(c < size % chunks);
            reader.scan(start, Some(n), |_, r| {
                docs.push((r.id.to_string(), r.text.to_string()));
                true
            })?;
        }
    }
    Ok(docs)
}

/// Cuts both texts to a window starting a little before their first difference, and makes
/// invisible characters visible so the change can be seen.
fn diff_window(before: &str, after: &str) -> (String, String) {
    let common = before.chars().zip(after.chars()).take_while(|(a, b)| a == b).count();
    let start = common.saturating_sub(60);
    let cut = |s: &str| {
        let body: String = s.chars().skip(start).take(EXAMPLE_CHARS).map(visible).collect();
        if start > 0 { format!("…{body}") } else { body }
    };
    (cut(before), cut(after))
}

fn visible(c: char) -> String {
    match c {
        '\n' => "⏎\n".into(),
        '\r' => "␍".into(),
        '\t' => "⇥".into(),
        '\u{00A0}' => "⍽".into(),
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' => "␣".into(),
        c if c.is_control() => format!("\\u{{{:04x}}}", c as u32),
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{DatasetWriter, NewDoc, TempDataset, write_manifest};
    use aeon_shared::DatasetManifest;

    fn dataset(n: u64) -> (tempfile::TempDir, DatasetReader) {
        let root = tempfile::tempdir().unwrap();
        let tmp = TempDataset::create(root.path(), "ds-s").unwrap();
        let mut w = DatasetWriter::new(tmp.path()).unwrap();
        for i in 0..n {
            w.push(NewDoc { id: &i.to_string(), text: "t", source: "s", meta_json: "{}" }).unwrap();
        }
        let (parts, stats) = w.finish().unwrap();
        let m = DatasetManifest {
            format_version: 1,
            id: "ds-s".into(),
            name: "s".into(),
            description: None,
            created_at_ms: 0,
            parts,
            stats,
            import: None,
            produced_by: None,
        };
        write_manifest(tmp.path(), &m).unwrap();
        let dir = tmp.commit().unwrap();
        let r = DatasetReader::open(&dir).unwrap();
        (root, r)
    }

    fn ids(docs: &[(String, String)]) -> Vec<u64> {
        docs.iter().map(|(id, _)| id.parse().unwrap()).collect()
    }

    #[test]
    fn sample_has_no_duplicates_and_the_requested_size() {
        for (total, size) in [(21, 100), (21, 21), (25, 21), (1000, 100), (1000, 999), (1003, 7)] {
            let (_root, r) = dataset(total);
            let docs = sample(&r, size).unwrap();
            let got = ids(&docs);
            let mut unique = got.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), got.len(), "duplicates for total={total} size={size}");
            assert_eq!(got.len() as u64, size.min(total), "size for total={total} size={size}");
        }
        // Spread across the dataset, not just the start.
        let (_root, r) = dataset(1000);
        assert!(ids(&sample(&r, 100).unwrap()).iter().any(|&i| i > 900));
    }

    #[test]
    fn diff_window_starts_near_first_change_and_shows_invisible_chars() {
        let before = format!("{}a\u{00A0}b", "x".repeat(100));
        let after = format!("{}a b", "x".repeat(100));
        let (b, a) = diff_window(&before, &after);
        assert!(b.starts_with('…') && b.ends_with("a⍽b"), "{b}");
        assert!(a.ends_with("a b"));
    }
}
