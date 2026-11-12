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

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("input_row", DataType::UInt64, false),
        Field::new("id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("step_index", DataType::UInt64, false),
        Field::new("rule", DataType::Utf8, false),
        Field::new("detail", DataType::Utf8, false),
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
    text: &'a str,
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
        scan_parts(&self.dir, &self.parts, start, limit, |index, batch, i| {
            let read = || -> AppResult<Row<'_>> {
                let u64col = |name: &str| {
                    batch
                        .column_by_name(name)
                        .and_then(|c| c.as_primitive_opt::<UInt64Type>())
                        .ok_or_else(|| AppError::Internal(format!("removed-documents file is missing column {name}")))
                };
                let (input_row, step_index) = (u64col("input_row")?, u64col("step_index")?);
                debug_assert!(input_row.is_valid(i));
                Ok(Row {
                    input_row: input_row.value(i),
                    id: column(batch, "id")?.value(i),
                    source: column(batch, "source")?.value(i),
                    step_index: step_index.value(i),
                    rule: column(batch, "rule")?.value(i),
                    detail: column(batch, "detail")?.value(i),
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
                text: r.text.to_string(),
            });
            false
        })?;
        found.ok_or_else(|| AppError::InvalidInput(format!("No removed document at position {index}")))
    }
}
