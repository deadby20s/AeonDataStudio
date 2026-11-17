//! Documents removed by a run, stored as Parquet in `runs/<id>/removed/` so they can be
//! inspected later.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aeon_shared::{PartInfo, RemovedDoc, RemovedDocPreview, RemovedPage};
use arrow::array::{Array, ArrayRef, AsArray, StringBuilder, UInt64Builder};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, UInt64Type};
use arrow::record_batch::RecordBatch;

use crate::dataset::parts::{PartedWriter, scan_parts};
use crate::dataset::{PREVIEW_CHARS, column};
use crate::error::{AppError, AppResult};

const BATCH_ROWS: usize = 4096;
const SEARCH_TIME_BUDGET: Duration = Duration::from_secs(2);
const MAX_PAGE_SIZE: u64 = 200;
pub const MAX_SAMPLE: u64 = 50;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("input_row", DataType::UInt64, false),
        Field::new("id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("step_index", DataType::UInt64, false),
        Field::new("rule", DataType::Utf8, false),
        Field::new("detail", DataType::Utf8, false),
        // Added in M3; files from older runs don't have it.
        Field::new("related_row", DataType::UInt64, true),
        Field::new("text", DataType::Utf8, false),
    ]))
}

pub struct RemovedWriter {
    parts: PartedWriter,
    input_row: UInt64Builder,
    id: StringBuilder,
    source: StringBuilder,
    step_index: UInt64Builder,
    rule: StringBuilder,
    detail: StringBuilder,
    related_row: UInt64Builder,
    text: StringBuilder,
    rows: usize,
    bytes: usize,
}

pub struct RemovedRow<'a> {
    pub input_row: u64,
    pub id: &'a str,
    pub source: &'a str,
    pub step_index: u64,
    pub rule: &'a str,
    pub detail: &'a str,
    pub related_row: Option<u64>,
    pub text: &'a str,
}

impl RemovedWriter {
    pub fn new(dir: &Path) -> AppResult<Self> {
        Ok(Self {
            parts: PartedWriter::new(dir, "removed", schema())?,
            input_row: UInt64Builder::new(),
            id: StringBuilder::new(),
            source: StringBuilder::new(),
            step_index: UInt64Builder::new(),
            rule: StringBuilder::new(),
            detail: StringBuilder::new(),
            related_row: UInt64Builder::new(),
            text: StringBuilder::new(),
            rows: 0,
            bytes: 0,
        })
    }

    pub fn push(&mut self, r: RemovedRow<'_>) -> AppResult<()> {
        self.input_row.append_value(r.input_row);
        self.id.append_value(r.id);
        self.source.append_value(r.source);
        self.step_index.append_value(r.step_index);
        self.rule.append_value(r.rule);
        self.detail.append_value(r.detail);
        self.related_row.append_option(r.related_row);
        self.text.append_value(r.text);
        self.rows += 1;
        self.bytes += r.text.len();
        if self.rows >= BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> AppResult<()> {
        if self.rows == 0 {
            return Ok(());
        }
        let cols: Vec<ArrayRef> = vec![
            Arc::new(self.input_row.finish()),
            Arc::new(self.id.finish()),
            Arc::new(self.source.finish()),
            Arc::new(self.step_index.finish()),
            Arc::new(self.rule.finish()),
            Arc::new(self.detail.finish()),
            Arc::new(self.related_row.finish()),
            Arc::new(self.text.finish()),
        ];
        self.parts.write(&RecordBatch::try_new(schema(), cols)?, self.bytes as u64)?;
        self.rows = 0;
        self.bytes = 0;
        Ok(())
    }

    pub fn finish(mut self) -> AppResult<Vec<PartInfo>> {
        self.flush()?;
        self.parts.finish()
    }
}

pub struct RemovedReader {
    dir: PathBuf,
    parts: Vec<PartInfo>,
}

struct Row<'a> {
    input_row: u64,
    id: &'a str,
    source: &'a str,
    step_index: u64,
    rule: &'a str,
    detail: &'a str,
    related_row: Option<u64>,
    text: &'a str,
}

fn u64_column<'a>(batch: &'a RecordBatch, name: &str) -> AppResult<&'a arrow::array::PrimitiveArray<UInt64Type>> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_primitive_opt::<UInt64Type>())
        .ok_or_else(|| AppError::Internal(format!("removed-documents file is missing column {name}")))
}

impl RemovedReader {
    pub fn new(dir: &Path, parts: Vec<PartInfo>) -> Self {
        Self { dir: dir.to_path_buf(), parts }
    }

    fn total(&self) -> u64 {
        self.parts.iter().map(|p| p.rows).sum()
    }

    fn scan(&self, start: u64, limit: Option<u64>, mut visit: impl FnMut(u64, Row<'_>) -> bool) -> AppResult<()> {
        let mut failure = None;
        scan_parts(&self.dir, &self.parts, start, limit, None, |index, batch, i| {
            let read = || -> AppResult<Row<'_>> {
                let related = batch.column_by_name("related_row").and_then(|c| c.as_primitive_opt::<UInt64Type>());
                Ok(Row {
                    input_row: u64_column(batch, "input_row")?.value(i),
                    id: column(batch, "id")?.value(i),
                    source: column(batch, "source")?.value(i),
                    step_index: u64_column(batch, "step_index")?.value(i),
                    rule: column(batch, "rule")?.value(i),
                    detail: column(batch, "detail")?.value(i),
                    related_row: related.and_then(|c| c.is_valid(i).then(|| c.value(i))),
                    text: column(batch, "text")?.value(i),
                })
            };
            match read() {
                Ok(row) => visit(index, row),
                Err(e) => {
                    failure = Some(e);
                    false
                }
            }
        })?;
        failure.map_or(Ok(()), Err)
    }

    /// A page of removed documents, optionally only those removed by one step and/or rule.
    pub fn browse(&self, start: u64, limit: u64, step_index: Option<u64>, rule: Option<&str>) -> AppResult<RemovedPage> {
        let limit = limit.clamp(1, MAX_PAGE_SIZE);
        let total = self.total();
        let filtered = step_index.is_some() || rule.is_some();
        let started = Instant::now();
        let mut docs = Vec::new();
        let mut position = start;
        let mut out_of_time = false;
        self.scan(start, (!filtered).then_some(limit), |index, r| {
            position = index + 1;
            if step_index.is_none_or(|s| s == r.step_index) && rule.is_none_or(|ru| ru == r.rule) {
                let chars = r.text.chars().count();
                let truncated = chars > PREVIEW_CHARS;
                docs.push(RemovedDocPreview {
                    index,
                    input_row: r.input_row,
                    id: r.id.to_string(),
                    source: r.source.to_string(),
                    step_index: r.step_index,
                    rule: r.rule.to_string(),
                    detail: r.detail.to_string(),
                    related_row: r.related_row,
                    chars: chars as u64,
                    preview: if truncated { r.text.chars().take(PREVIEW_CHARS).collect() } else { r.text.to_string() },
                    truncated,
                });
                if docs.len() as u64 >= limit {
                    return false;
                }
            }
            if index % 256 == 0 && started.elapsed() > SEARCH_TIME_BUDGET {
                out_of_time = true;
                return false;
            }
            true
        })?;
        let position = position.max(start).min(total);
        let filled = docs.len() as u64 >= limit;
        Ok(RemovedPage {
            docs,
            next_index: (position < total).then_some(position),
            scanned_to: position,
            search_incomplete: out_of_time && !filled,
        })
    }

    pub fn get(&self, index: u64) -> AppResult<RemovedDoc> {
        let mut found = None;
        self.scan(index, Some(1), |index, r| {
            found = Some(RemovedDoc {
                index,
                input_row: r.input_row,
                id: r.id.to_string(),
                source: r.source.to_string(),
                step_index: r.step_index,
                rule: r.rule.to_string(),
                detail: r.detail.to_string(),
                related_row: r.related_row,
                text: r.text.to_string(),
            });
            false
        })?;
        found.ok_or_else(|| AppError::InvalidInput(format!("No removed document at position {index}")))
    }

    /// Up to `count` random documents removed by `step_index`/`rule`, in log order. The same
    /// `seed` gives the same sample. Only the small columns are read to find candidates.
    pub fn sample(&self, step_index: u64, rule: &str, count: u64, seed: u64) -> AppResult<Vec<RemovedDoc>> {
        let mut candidates = Vec::new();
        let mut failure = None;
        scan_parts(&self.dir, &self.parts, 0, None, Some(&["step_index", "rule"]), |index, batch, i| {
            match (u64_column(batch, "step_index"), column(batch, "rule")) {
                (Ok(steps), Ok(rules)) => {
                    if steps.value(i) == step_index && rules.value(i) == rule {
                        candidates.push(index);
                    }
                    true
                }
                (Err(e), _) | (_, Err(e)) => {
                    failure = Some(e);
                    false
                }
            }
        })?;
        if let Some(e) = failure {
            return Err(e);
        }
        let mut chosen = pick(&candidates, count.min(MAX_SAMPLE) as usize, seed);
        chosen.sort_unstable();
        chosen.into_iter().map(|i| self.get(i)).collect()
    }
}

/// `count` distinct items chosen pseudo-randomly (partial Fisher–Yates with a seeded generator).
fn pick(items: &[u64], count: usize, seed: u64) -> Vec<u64> {
    let mut items = items.to_vec();
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let n = count.min(items.len());
    for i in 0..n {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = i + (state % (items.len() - i) as u64) as usize;
        items.swap(i, j);
    }
    items.truncate(n);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_row_round_trips_and_sampling_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = RemovedWriter::new(dir.path()).unwrap();
        for i in 0..300u64 {
            let rule = if i % 3 == 0 { "near_duplicate" } else { "min_words" };
            w.push(RemovedRow {
                input_row: i * 2,
                id: &i.to_string(),
                source: "s",
                step_index: if i % 3 == 0 { 9 } else { 2 },
                rule,
                detail: "d",
                related_row: (i % 3 == 0).then_some(i),
                text: "text",
            })
            .unwrap();
        }
        let parts = w.finish().unwrap();
        let r = RemovedReader::new(dir.path(), parts);
        assert_eq!(r.get(3).unwrap().related_row, Some(3));
        assert_eq!(r.get(4).unwrap().related_row, None);

        let a = r.sample(9, "near_duplicate", 5, 42).unwrap();
        let b = r.sample(9, "near_duplicate", 5, 42).unwrap();
        assert_eq!(a.len(), 5);
        assert_eq!(a.iter().map(|d| d.index).collect::<Vec<_>>(), b.iter().map(|d| d.index).collect::<Vec<_>>());
        assert!(a.iter().all(|d| d.rule == "near_duplicate" && d.step_index == 9));
        assert_ne!(
            a.iter().map(|d| d.index).collect::<Vec<_>>(),
            r.sample(9, "near_duplicate", 5, 7).unwrap().iter().map(|d| d.index).collect::<Vec<_>>()
        );
        assert!(r.sample(9, "no_such_rule", 5, 1).unwrap().is_empty());
    }

    #[test]
    fn pick_returns_distinct_items() {
        let items: Vec<u64> = (0..100).collect();
        let mut p = pick(&items, 20, 3);
        p.sort_unstable();
        p.dedup();
        assert_eq!(p.len(), 20);
        assert_eq!(pick(&items[..3], 10, 1).len(), 3);
    }
}
