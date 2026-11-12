use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aeon_shared::{PartInfo, TextStats};
use arrow::array::{ArrayRef, StringBuilder};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use super::schema;
use super::stats::StatsBuilder;
use crate::error::AppResult;

/// Rows buffered before a record batch is handed to the Parquet writer.
const BATCH_ROWS: usize = 4096;
/// Text bytes buffered before a record batch is flushed, whichever comes first.
const BATCH_BYTES: usize = 32 << 20;
/// A new part file starts after this many rows or text bytes.
const PART_ROWS: u64 = 250_000;
const PART_BYTES: u64 = 512 << 20;
/// Smaller row groups make paging to an offset cheaper.
const ROW_GROUP_ROWS: usize = 16_384;

pub struct NewDoc<'a> {
    pub id: &'a str,
    pub text: &'a str,
    pub source: &'a str,
    pub meta_json: &'a str,
}

struct Buffer {
    id: StringBuilder,
    text: StringBuilder,
    source: StringBuilder,
    meta: StringBuilder,
    rows: usize,
    bytes: usize,
}

impl Buffer {
    fn new() -> Self {
        Self {
            id: StringBuilder::new(),
            text: StringBuilder::new(),
            source: StringBuilder::new(),
            meta: StringBuilder::new(),
            rows: 0,
            bytes: 0,
        }
    }

    fn take_batch(&mut self) -> AppResult<RecordBatch> {
        let cols: Vec<ArrayRef> = vec![
            Arc::new(self.id.finish()),
            Arc::new(self.text.finish()),
            Arc::new(self.source.finish()),
            Arc::new(self.meta.finish()),
        ];
        self.rows = 0;
        self.bytes = 0;
        Ok(RecordBatch::try_new(schema(), cols)?)
    }
}

struct Part {
    file: String,
    path: PathBuf,
    writer: ArrowWriter<File>,
    rows: u64,
    bytes: u64,
}

/// Streams documents into Parquet part files and collects text statistics.
pub struct DatasetWriter {
    dir: PathBuf,
    props: WriterProperties,
    buffer: Buffer,
    part: Option<Part>,
    parts: Vec<PartInfo>,
    stats: StatsBuilder,
    part_rows: u64,
    batch_rows: usize,
}

impl DatasetWriter {
    pub fn new(dir: &Path) -> AppResult<Self> {
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .build();
        Ok(Self {
            dir: dir.to_path_buf(),
            props,
            buffer: Buffer::new(),
            part: None,
            parts: Vec::new(),
            stats: StatsBuilder::default(),
            part_rows: PART_ROWS,
            batch_rows: BATCH_ROWS,
        })
    }

    /// Small parts, so tests can cover reads that cross part boundaries.
    #[cfg(test)]
    pub fn with_part_rows(mut self, rows: u64) -> Self {
        self.part_rows = rows;
        self.batch_rows = rows as usize;
        self
    }

    pub fn push(&mut self, doc: NewDoc<'_>) -> AppResult<()> {
        self.stats.add(doc.text);
        let b = &mut self.buffer;
        b.id.append_value(doc.id);
        b.text.append_value(doc.text);
        b.source.append_value(doc.source);
        b.meta.append_value(doc.meta_json);
        b.rows += 1;
        b.bytes += doc.text.len();
        if b.rows >= self.batch_rows || b.bytes >= BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    pub fn doc_count(&self) -> u64 {
        self.parts.iter().map(|p| p.rows).sum::<u64>()
            + self.part.as_ref().map_or(0, |p| p.rows)
            + self.buffer.rows as u64
    }

    fn flush(&mut self) -> AppResult<()> {
        if self.buffer.rows == 0 {
            return Ok(());
        }
        let rows = self.buffer.rows as u64;
        let bytes = self.buffer.bytes as u64;
        let batch = self.buffer.take_batch()?;
        if self.part.is_none() {
            let file = format!("part-{:05}.parquet", self.parts.len());
            let path = self.dir.join(&file);
            let writer = ArrowWriter::try_new(File::create(&path)?, schema(), Some(self.props.clone()))?;
            self.part = Some(Part { file, path, writer, rows: 0, bytes: 0 });
        }
        let part = self.part.as_mut().expect("part was just created");
        part.writer.write(&batch)?;
        part.rows += rows;
        part.bytes += bytes;
        if part.rows >= self.part_rows || part.bytes >= PART_BYTES {
            self.close_part()?;
        }
        Ok(())
    }

    fn close_part(&mut self) -> AppResult<()> {
        if let Some(part) = self.part.take() {
            part.writer.close()?;
            let bytes = std::fs::metadata(&part.path)?.len();
            self.parts.push(PartInfo { file: part.file, rows: part.rows, bytes });
        }
        Ok(())
    }

    /// Writes remaining rows and closes the last part.
    pub fn finish(mut self) -> AppResult<(Vec<PartInfo>, TextStats)> {
        self.flush()?;
        self.close_part()?;
        Ok((self.parts, self.stats.finish()))
    }
}
