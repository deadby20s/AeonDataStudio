//! Writing and reading a sequence of Parquet part files (`<prefix>-00000.parquet`, ...).

use std::fs::File;
use std::path::{Path, PathBuf};

use aeon_shared::PartInfo;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::{ArrowWriter, ProjectionMask};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::error::AppResult;

/// A new part file starts after this many rows or text bytes.
const PART_ROWS: u64 = 250_000;
const PART_BYTES: u64 = 512 << 20;
/// Smaller row groups make paging to an offset cheaper.
const ROW_GROUP_ROWS: usize = 16_384;

struct OpenPart {
    file: String,
    path: PathBuf,
    writer: ArrowWriter<File>,
    rows: u64,
    bytes: u64,
}

pub struct PartedWriter {
    dir: PathBuf,
    prefix: &'static str,
    schema: SchemaRef,
    props: WriterProperties,
    current: Option<OpenPart>,
    parts: Vec<PartInfo>,
    part_rows: u64,
}

impl PartedWriter {
    pub fn new(dir: &Path, prefix: &'static str, schema: SchemaRef) -> AppResult<Self> {
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .build();
        Ok(Self {
            dir: dir.to_path_buf(),
            prefix,
            schema,
            props,
            current: None,
            parts: Vec::new(),
            part_rows: PART_ROWS,
        })
    }

    #[cfg(test)]
    pub fn set_part_rows(&mut self, rows: u64) {
        self.part_rows = rows;
    }

    /// Rows written so far, including the open part.
    pub fn rows(&self) -> u64 {
        self.parts.iter().map(|p| p.rows).sum::<u64>() + self.current.as_ref().map_or(0, |p| p.rows)
    }

    /// Writes one batch; `text_bytes` counts toward the part size limit.
    pub fn write(&mut self, batch: &RecordBatch, text_bytes: u64) -> AppResult<()> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        if self.current.is_none() {
            let file = format!("{}-{:05}.parquet", self.prefix, self.parts.len());
            let path = self.dir.join(&file);
            let writer = ArrowWriter::try_new(File::create(&path)?, self.schema.clone(), Some(self.props.clone()))?;
            self.current = Some(OpenPart { file, path, writer, rows: 0, bytes: 0 });
        }
        let part = self.current.as_mut().expect("part was just opened");
        part.writer.write(batch)?;
        part.rows += batch.num_rows() as u64;
        part.bytes += text_bytes;
        if part.rows >= self.part_rows || part.bytes >= PART_BYTES {
            self.close_part()?;
        }
        Ok(())
    }

    fn close_part(&mut self) -> AppResult<()> {
        if let Some(part) = self.current.take() {
            part.writer.close()?;
            let bytes = std::fs::metadata(&part.path)?.len();
            self.parts.push(PartInfo { file: part.file, rows: part.rows, bytes });
        }
        Ok(())
    }

    pub fn finish(mut self) -> AppResult<Vec<PartInfo>> {
        self.close_part()?;
        Ok(self.parts)
    }
}

/// Calls `visit(row, batch, index_in_batch)` for each row from `start_row` on, across parts,
/// until it returns `false`. `limit` lets the reader skip decoding rows that won't be visited;
/// `columns` reads only those columns (names missing from a file are ignored).
pub fn scan_parts(
    dir: &Path,
    parts: &[PartInfo],
    start_row: u64,
    limit: Option<u64>,
    columns: Option<&[&str]>,
    mut visit: impl FnMut(u64, &RecordBatch, usize) -> bool,
) -> AppResult<()> {
    let mut part_start = 0u64;
    let mut remaining = limit;
    for part in parts {
        let part_end = part_start + part.rows;
        if start_row >= part_end {
            part_start = part_end;
            continue;
        }
        if remaining == Some(0) {
            return Ok(());
        }
        let offset = start_row.saturating_sub(part_start);
        let mut builder = ParquetRecordBatchReaderBuilder::try_new(File::open(dir.join(&part.file))?)?
            .with_batch_size(1024)
            .with_offset(offset as usize);
        if let Some(n) = remaining {
            builder = builder.with_limit(n as usize);
        }
        if let Some(names) = columns {
            let indices: Vec<usize> = names.iter().filter_map(|n| builder.schema().index_of(n).ok()).collect();
            let mask = ProjectionMask::roots(builder.parquet_schema(), indices);
            builder = builder.with_projection(mask);
        }
        let mut row = part_start + offset;
        for batch in builder.build()? {
            let batch = batch?;
            for i in 0..batch.num_rows() {
                if !visit(row, &batch, i) {
                    return Ok(());
                }
                row += 1;
                if let Some(n) = remaining.as_mut() {
                    *n = n.saturating_sub(1);
                }
            }
        }
        part_start = part_end;
    }
    Ok(())
}
