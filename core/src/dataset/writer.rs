use std::path::Path;
use std::sync::Arc;

use aeon_shared::{PartInfo, TextStats};
use arrow::array::{ArrayRef, StringBuilder};
use arrow::record_batch::RecordBatch;

use super::parts::PartedWriter;
use super::schema;
use super::stats::StatsBuilder;
use crate::error::AppResult;

/// Rows buffered before a record batch is handed to the Parquet writer.
const BATCH_ROWS: usize = 4096;
/// Text bytes buffered before a record batch is flushed, whichever comes first.
const BATCH_BYTES: usize = 32 << 20;

pub struct NewDoc<'a> {
    pub id: &'a str,
    pub text: &'a str,
    pub source: &'a str,
    pub meta_json: &'a str,
}

/// Streams documents into Parquet part files and collects text statistics.
pub struct DatasetWriter {
    parts: PartedWriter,
    id: StringBuilder,
    text: StringBuilder,
    source: StringBuilder,
    meta: StringBuilder,
    rows: usize,
    bytes: usize,
    batch_rows: usize,
    batch_bytes: usize,
    stats: StatsBuilder,
}

impl DatasetWriter {
    pub fn new(dir: &Path) -> AppResult<Self> {
        Ok(Self {
            parts: PartedWriter::new(dir, "part", schema())?,
            id: StringBuilder::new(),
            text: StringBuilder::new(),
            source: StringBuilder::new(),
            meta: StringBuilder::new(),
            rows: 0,
            bytes: 0,
            batch_rows: BATCH_ROWS,
            batch_bytes: BATCH_BYTES,
            stats: StatsBuilder::default(),
        })
    }

    /// Flush after this many buffered text bytes (smaller saves memory with many writers open).
    pub fn with_batch_bytes(mut self, bytes: usize) -> Self {
        self.batch_bytes = bytes.max(1);
        self
    }

    /// Writer for an export: files `<prefix>-00000.parquet`, ..., each about `part_bytes`
    /// of text; with `include_meta` false the `meta` column is written as `{}`.
    pub fn for_export(dir: &Path, prefix: &'static str, part_bytes: u64) -> AppResult<Self> {
        let mut w = Self::new(dir)?;
        w.parts = PartedWriter::new(dir, prefix, schema())?;
        w.parts.set_part_bytes(part_bytes);
        Ok(w)
    }

    /// Small parts, so tests can cover reads that cross part boundaries.
    #[cfg(test)]
    pub fn with_part_rows(mut self, rows: u64) -> Self {
        self.parts.set_part_rows(rows);
        self.batch_rows = rows as usize;
        self
    }

    pub fn push(&mut self, doc: NewDoc<'_>) -> AppResult<()> {
        self.stats.add(doc.text);
        self.id.append_value(doc.id);
        self.text.append_value(doc.text);
        self.source.append_value(doc.source);
        self.meta.append_value(doc.meta_json);
        self.rows += 1;
        self.bytes += doc.text.len();
        if self.rows >= self.batch_rows || self.bytes >= self.batch_bytes {
            self.flush()?;
        }
        Ok(())
    }

    pub fn doc_count(&self) -> u64 {
        self.parts.rows() + self.rows as u64
    }

    fn flush(&mut self) -> AppResult<()> {
        if self.rows == 0 {
            return Ok(());
        }
        let cols: Vec<ArrayRef> = vec![
            Arc::new(self.id.finish()),
            Arc::new(self.text.finish()),
            Arc::new(self.source.finish()),
            Arc::new(self.meta.finish()),
        ];
        let batch = RecordBatch::try_new(schema(), cols)?;
        self.parts.write(&batch, self.bytes as u64)?;
        self.rows = 0;
        self.bytes = 0;
        Ok(())
    }

    /// Writes remaining rows and closes the last part.
    pub fn finish(mut self) -> AppResult<(Vec<PartInfo>, TextStats)> {
        self.flush()?;
        Ok((self.parts.finish()?, self.stats.finish()))
    }
}
