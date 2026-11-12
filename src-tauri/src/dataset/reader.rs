use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aeon_shared::{DatasetManifest, Document, DocumentPage, DocumentPreview};
use arrow::array::{AsArray, StringArray};
use arrow::record_batch::RecordBatch;

use super::parts::scan_parts;
use super::read_manifest;
use crate::error::{AppError, AppResult};

pub const PREVIEW_CHARS: usize = 400;
pub const MAX_PAGE_SIZE: u64 = 200;
/// A search request stops after this long and reports where to continue.
const SEARCH_TIME_BUDGET: Duration = Duration::from_secs(2);

pub struct Row<'a> {
    pub id: &'a str,
    pub text: &'a str,
    pub source: &'a str,
    pub meta: &'a str,
}

pub struct DatasetReader {
    dir: PathBuf,
    manifest: DatasetManifest,
}

impl DatasetReader {
    pub fn open(dir: &Path) -> AppResult<Self> {
        Ok(Self { dir: dir.to_path_buf(), manifest: read_manifest(dir)? })
    }

    pub fn manifest(&self) -> &DatasetManifest {
        &self.manifest
    }

    pub fn row_count(&self) -> u64 {
        self.manifest.parts.iter().map(|p| p.rows).sum()
    }

    /// Calls `visit` for each row from `start_row` on, until it returns `false` or rows run out.
    /// `limit` lets the Parquet reader skip decoding rows that will not be visited.
    pub fn scan(
        &self,
        start_row: u64,
        limit: Option<u64>,
        mut visit: impl FnMut(u64, Row<'_>) -> bool,
    ) -> AppResult<()> {
        let mut failure = None;
        scan_parts(&self.dir, &self.manifest.parts, start_row, limit, |row, batch, i| {
            let cols = (column(batch, "id"), column(batch, "text"), column(batch, "source"), column(batch, "meta"));
            match cols {
                (Ok(ids), Ok(texts), Ok(sources), Ok(metas)) => visit(
                    row,
                    Row { id: ids.value(i), text: texts.value(i), source: sources.value(i), meta: metas.value(i) },
                ),
                (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => {
                    failure = Some(e);
                    false
                }
            }
        })?;
        failure.map_or(Ok(()), Err)
    }

    /// One page of documents. With a `query`, only documents whose text contains it
    /// (case-insensitive) are returned, scanning forward from `start_row`.
    pub fn browse(&self, start_row: u64, limit: u64, query: Option<&str>) -> AppResult<DocumentPage> {
        let limit = limit.clamp(1, MAX_PAGE_SIZE);
        let total = self.row_count();
        let mut docs = Vec::new();

        let query = query.map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
        let Some(query) = query else {
            self.scan(start_row, Some(limit), |row, r| {
                docs.push(preview(row, &r));
                true
            })?;
            let next = start_row + docs.len() as u64;
            return Ok(DocumentPage {
                docs,
                next_row: (next < total).then_some(next),
                scanned_to: next.min(total),
                search_incomplete: false,
            });
        };

        let started = Instant::now();
        let mut position = start_row;
        let mut out_of_time = false;
        self.scan(start_row, None, |row, r| {
            position = row + 1;
            if contains_ignore_case(r.text, &query) {
                docs.push(preview(row, &r));
                if docs.len() as u64 >= limit {
                    return false;
                }
            }
            // Checking the clock is cheap next to decoding, but no need to do it every row.
            if row % 256 == 0 && started.elapsed() > SEARCH_TIME_BUDGET {
                out_of_time = true;
                return false;
            }
            true
        })?;
        let position = position.max(start_row).min(total);
        let filled = docs.len() as u64 >= limit;
        Ok(DocumentPage {
            docs,
            next_row: (position < total).then_some(position),
            scanned_to: position,
            search_incomplete: out_of_time && !filled,
        })
    }

    pub fn document(&self, row: u64) -> AppResult<Document> {
        let mut found = None;
        self.scan(row, Some(1), |row, r| {
            found = Some(Document {
                row,
                id: r.id.to_string(),
                source: r.source.to_string(),
                text: r.text.to_string(),
                meta_json: pretty_json(r.meta),
            });
            false
        })?;
        found.ok_or_else(|| AppError::InvalidInput(format!("Row {row} is past the end of the dataset")))
    }
}

pub(crate) fn column<'a>(batch: &'a RecordBatch, name: &str) -> AppResult<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_string_opt::<i32>())
        .ok_or_else(|| AppError::Internal(format!("dataset file is missing column {name}")))
}

fn preview(row: u64, r: &Row<'_>) -> DocumentPreview {
    let chars = r.text.chars().count();
    let truncated = chars > PREVIEW_CHARS;
    let preview = if truncated { r.text.chars().take(PREVIEW_CHARS).collect() } else { r.text.to_string() };
    DocumentPreview {
        row,
        id: r.id.to_string(),
        source: r.source.to_string(),
        chars: chars as u64,
        preview,
        truncated,
    }
}

fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| raw.to_string())
}

/// `query` must already be lowercase.
fn contains_ignore_case(text: &str, query: &str) -> bool {
    if text.is_ascii() && query.is_ascii() {
        let (t, q) = (text.as_bytes(), query.as_bytes());
        return q.is_empty() || t.windows(q.len()).any(|w| w.eq_ignore_ascii_case(q));
    }
    text.to_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{DatasetWriter, NewDoc, TempDataset, write_manifest};

    /// Writes `n` docs ("doc 0", "doc 1", ...) with every 10th containing "Needle".
    fn make_dataset(n: u64) -> (tempfile::TempDir, DatasetReader) {
        make_dataset_with_parts(n, 1_000_000)
    }

    fn make_dataset_with_parts(n: u64, part_rows: u64) -> (tempfile::TempDir, DatasetReader) {
        let root = tempfile::tempdir().unwrap();
        let tmp = TempDataset::create(root.path(), "ds-test").unwrap();
        let mut w = DatasetWriter::new(tmp.path()).unwrap().with_part_rows(part_rows);
        for i in 0..n {
            let text = if i % 10 == 0 { format!("doc {i} has a Needle") } else { format!("doc {i}") };
            w.push(NewDoc { id: &i.to_string(), text: &text, source: "src", meta_json: r#"{"k":1}"# }).unwrap();
        }
        let (parts, stats) = w.finish().unwrap();
        let manifest = DatasetManifest {
            format_version: 1,
            id: "ds-test".into(),
            name: "t".into(),
            description: None,
            created_at_ms: 0,
            parts,
            stats,
            import: None,
            produced_by: None,
        };
        write_manifest(tmp.path(), &manifest).unwrap();
        let dir = tmp.commit().unwrap();
        let reader = DatasetReader::open(&dir).unwrap();
        (root, reader)
    }

    #[test]
    fn pages_through_rows_in_order() {
        let (_root, r) = make_dataset(25);
        assert_eq!(r.row_count(), 25);
        let p1 = r.browse(0, 10, None).unwrap();
        assert_eq!(p1.docs.iter().map(|d| d.row).collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());
        assert_eq!(p1.next_row, Some(10));
        let p3 = r.browse(20, 10, None).unwrap();
        assert_eq!(p3.docs.len(), 5);
        assert_eq!(p3.docs[0].id, "20");
        assert_eq!(p3.next_row, None);
    }

    #[test]
    fn search_is_case_insensitive_and_continues_from_next_row() {
        let (_root, r) = make_dataset(35);
        let p1 = r.browse(0, 2, Some("needle")).unwrap();
        assert_eq!(p1.docs.iter().map(|d| d.row).collect::<Vec<_>>(), vec![0, 10]);
        let p2 = r.browse(p1.next_row.unwrap(), 2, Some("NEEDLE")).unwrap();
        assert_eq!(p2.docs.iter().map(|d| d.row).collect::<Vec<_>>(), vec![20, 30]);
        let p3 = r.browse(p2.next_row.unwrap(), 2, Some("needle")).unwrap();
        assert!(p3.docs.is_empty());
        assert_eq!(p3.next_row, None);
        assert!(!p3.search_incomplete);
    }

    #[test]
    fn reads_single_document_with_meta() {
        let (_root, r) = make_dataset(5);
        let d = r.document(3).unwrap();
        assert_eq!((d.id.as_str(), d.text.as_str()), ("3", "doc 3"));
        assert!(d.meta_json.contains("\"k\": 1"));
        assert!(r.document(5).is_err());
    }

    #[test]
    fn reads_across_part_boundaries() {
        let (_root, r) = make_dataset_with_parts(23, 5);
        assert_eq!(r.manifest().parts.len(), 5);
        assert_eq!(r.manifest().parts.iter().map(|p| p.rows).sum::<u64>(), 23);
        let page = r.browse(3, 9, None).unwrap();
        assert_eq!(page.docs.iter().map(|d| d.row).collect::<Vec<_>>(), (3..12).collect::<Vec<_>>());
        assert!(page.docs.iter().all(|d| d.id == d.row.to_string()));
        assert_eq!(r.document(17).unwrap().text, "doc 17");
        let hits = r.browse(0, 10, Some("needle")).unwrap();
        assert_eq!(hits.docs.iter().map(|d| d.row).collect::<Vec<_>>(), vec![0, 10, 20]);
        assert_eq!(r.manifest().stats.doc_count, 23);
    }

    #[test]
    fn non_ascii_search() {
        assert!(contains_ignore_case("Größe und Ähnlichkeit", "ähnlich"));
        assert!(!contains_ignore_case("abc", "abd"));
        assert!(contains_ignore_case("ABC", ""));
    }
}
