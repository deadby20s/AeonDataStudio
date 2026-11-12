//! Index of a run's kept documents: for each output row, its input row and which steps changed
//! it. Stored in `runs/<id>/kept/`. Powers the Inspector's before/after view.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aeon_shared::{ChangedPage, ChangedRef, PartInfo, RemovedDoc, RunReport};
use arrow::array::{ArrayRef, AsArray, UInt64Builder};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, UInt64Type};
use arrow::record_batch::RecordBatch;

use super::removed::RemovedReader;
use crate::dataset::parts::{PartedWriter, scan_parts};
use crate::error::{AppError, AppResult};

const BATCH_ROWS: usize = 16_384;
const MAX_PAGE_SIZE: u64 = 200;
const SCAN_TIME_BUDGET: Duration = Duration::from_secs(2);

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("input_row", DataType::UInt64, false),
        // Bit i set: the step at position i of the compiled pipeline (report.steps[i]) changed it.
        Field::new("modified", DataType::UInt64, false),
    ]))
}

pub struct KeptWriter {
    parts: PartedWriter,
    input_row: UInt64Builder,
    modified: UInt64Builder,
    rows: usize,
}

impl KeptWriter {
    pub fn new(dir: &Path) -> AppResult<Self> {
        Ok(Self { parts: PartedWriter::new(dir, "kept", schema())?, input_row: UInt64Builder::new(), modified: UInt64Builder::new(), rows: 0 })
    }

    pub fn push(&mut self, input_row: u64, modified: u64) -> AppResult<()> {
        self.input_row.append_value(input_row);
        self.modified.append_value(modified);
        self.rows += 1;
        if self.rows >= BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> AppResult<()> {
        if self.rows == 0 {
            return Ok(());
        }
        let cols: Vec<ArrayRef> = vec![Arc::new(self.input_row.finish()), Arc::new(self.modified.finish())];
        self.parts.write(&RecordBatch::try_new(schema(), cols)?, 0)?;
        self.rows = 0;
        Ok(())
    }

    pub fn finish(mut self) -> AppResult<Vec<PartInfo>> {
        self.flush()?;
        self.parts.finish()
    }
}

/// Kept documents changed by normalization, from output row `start` on. With `step_index`,
/// only documents changed by that step.
pub fn browse_changed(
    run_dir: &Path,
    report: &RunReport,
    step_index: Option<u64>,
    start: u64,
    limit: u64,
) -> AppResult<ChangedPage> {
    let total_kept: u64 = report.kept_parts.iter().map(|p| p.rows).sum();
    if report.kept_parts.is_empty() {
        return Err(AppError::InvalidInput(
            "This run was made before before/after tracking existed. Run the pipeline again to compare documents.".into(),
        ));
    }
    let limit = limit.clamp(1, MAX_PAGE_SIZE);
    let mask = match step_index {
        None => u64::MAX,
        Some(idx) => {
            let pos = report
                .steps
                .iter()
                .position(|s| s.index == idx)
                .ok_or_else(|| AppError::InvalidInput(format!("Step {} was not part of this run", idx + 1)))?;
            1u64 << pos
        }
    };
    let started = Instant::now();
    let mut items = Vec::new();
    let mut position = start;
    let mut failure = None;
    scan_parts(&run_dir.join("kept"), &report.kept_parts, start, None, None, |row, batch, i| {
        position = row + 1;
        let cols = (
            batch.column_by_name("input_row").and_then(|c| c.as_primitive_opt::<UInt64Type>()),
            batch.column_by_name("modified").and_then(|c| c.as_primitive_opt::<UInt64Type>()),
        );
        let (Some(inputs), Some(mods)) = cols else {
            failure = Some(AppError::Internal("kept index is missing columns".into()));
            return false;
        };
        let m = mods.value(i);
        if m & mask != 0 {
            let step_indices = (0..report.steps.len()).filter(|p| m & (1 << p) != 0).map(|p| report.steps[p].index).collect();
            items.push(ChangedRef { output_row: row, input_row: inputs.value(i), step_indices });
            if items.len() as u64 >= limit {
                return false;
            }
        }
        !(row % 1024 == 0 && started.elapsed() > SCAN_TIME_BUDGET)
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    let position = position.max(start).min(total_kept);
    Ok(ChangedPage { items, next_start: (position < total_kept).then_some(position), scanned_to: position, total_kept })
}

pub fn sample_removed(
    run_dir: &Path,
    report: &RunReport,
    step_index: u64,
    rule: &str,
    count: u64,
    seed: u64,
) -> AppResult<Vec<RemovedDoc>> {
    RemovedReader::new(&run_dir.join("removed"), report.removed_parts.clone()).sample(step_index, rule, count, seed)
}
