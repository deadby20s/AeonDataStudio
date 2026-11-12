//! Reading raw records from source files.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aeon_shared::{ImportFormat, TextMode};
use arrow::array::{Array, AsArray};
use arrow::compute::cast;
use arrow::datatypes::DataType;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
}

/// A file to import, with its format resolved.
#[derive(Debug, Clone)]
pub struct InputFile {
    pub path: PathBuf,
    /// Name stored in each document's `source` column, e.g. `corpus/part1.jsonl`.
    pub source: String,
    pub format: ImportFormat,
    pub compression: Compression,
    /// Tab-separated (only meaningful for CSV).
    pub tsv: bool,
    pub size: u64,
}

pub struct RawRecord {
    pub id: Option<String>,
    /// `None` when the text field is missing or not a string.
    pub text: Option<String>,
    pub meta: Map<String, Value>,
}

pub enum Item {
    Record(RawRecord),
    /// A line or row that could not be parsed, with a short description.
    ParseError(String),
}

pub struct ReadOptions<'a> {
    pub text_field: &'a str,
    pub id_field: Option<&'a str>,
    pub text_mode: TextMode,
}

/// Splits a file name into its data format and compression, looking through `.gz` / `.zst`.
fn detect(path: &Path) -> (Option<ImportFormat>, Compression, bool) {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (stem, compression) = if let Some(s) = name.strip_suffix(".gz") {
        (s.to_string(), Compression::Gzip)
    } else if let Some(s) = name.strip_suffix(".zst").or_else(|| name.strip_suffix(".zstd")) {
        (s.to_string(), Compression::Zstd)
    } else {
        (name, Compression::None)
    };
    let ext = stem.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let format = match ext {
        "jsonl" | "ndjson" => Some(ImportFormat::Jsonl),
        "json" => Some(ImportFormat::Json),
        "txt" | "text" | "md" | "markdown" => Some(ImportFormat::Text),
        "csv" | "tsv" => Some(ImportFormat::Csv),
        "parquet" => Some(ImportFormat::Parquet),
        _ => None,
    };
    (format, compression, ext == "tsv")
}

/// Expands picked paths into input files. Folders are walked recursively in name order;
/// hidden entries are skipped. Returns the files and the number of folder files ignored
/// because their type is unknown.
pub fn collect_inputs(paths: &[String], format: ImportFormat) -> AppResult<(Vec<InputFile>, u64)> {
    let mut files = Vec::new();
    let mut ignored = 0;
    for raw in paths {
        let path = PathBuf::from(raw.trim());
        if path.is_dir() {
            let root_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut found = Vec::new();
            walk(&path, &mut found)?;
            found.sort();
            for file in found {
                let rel = file.strip_prefix(&path).unwrap_or(&file);
                let source = format!("{root_name}/{}", rel.to_string_lossy().replace('\\', "/"));
                match resolve(&file, source, format, false)? {
                    Some(f) => files.push(f),
                    None => ignored += 1,
                }
            }
        } else if path.is_file() {
            let source = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            files.push(resolve(&path, source, format, true)?.expect("strict resolve returns a file or an error"));
        } else {
            return Err(AppError::InvalidInput(format!("{} does not exist", path.display())));
        }
    }
    Ok((files, ignored))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> AppResult<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// With `strict`, an unknown file type is an error (the user picked it explicitly);
/// otherwise it is skipped.
fn resolve(path: &Path, source: String, format: ImportFormat, strict: bool) -> AppResult<Option<InputFile>> {
    let (detected, compression, tsv) = detect(path);
    let format = match (format, detected) {
        (ImportFormat::Auto, Some(f)) => f,
        (ImportFormat::Auto, None) if strict => {
            return Err(AppError::InvalidInput(format!(
                "Can't tell the format of {source} from its extension. Choose a format instead of \"detect\"."
            )));
        }
        (ImportFormat::Auto, None) => return Ok(None),
        (explicit, _) => explicit,
    };
    if format == ImportFormat::Parquet && compression != Compression::None {
        return Err(AppError::InvalidInput(format!("{source}: compressed Parquet files are not supported")));
    }
    let size = std::fs::metadata(path)?.len();
    Ok(Some(InputFile { path: path.to_path_buf(), source, format, compression, tsv, size }))
}

/// Counts bytes read from the underlying (possibly compressed) file, for progress.
struct Counting<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn open_stream(file: &InputFile, bytes_read: Arc<AtomicU64>) -> AppResult<Box<dyn BufRead>> {
    let raw = Counting { inner: File::open(&file.path)?, count: bytes_read };
    let raw = BufReader::with_capacity(1 << 20, raw);
    Ok(match file.compression {
        Compression::None => Box::new(raw),
        Compression::Gzip => Box::new(BufReader::new(flate2::bufread::MultiGzDecoder::new(raw))),
        Compression::Zstd => Box::new(BufReader::new(zstd::stream::read::Decoder::with_buffer(raw)?)),
    })
}

/// Reads every record of `file`, calling `emit` for each. Stops early if `emit` returns
/// `Ok(false)`. Problems with single records are reported as [`Item::ParseError`];
/// problems with the file as a whole (unreadable, missing column) are returned as errors.
pub fn read_file(
    file: &InputFile,
    opts: &ReadOptions<'_>,
    bytes_read: Arc<AtomicU64>,
    emit: &mut dyn FnMut(Item) -> AppResult<bool>,
) -> AppResult<()> {
    let result = match file.format {
        ImportFormat::Jsonl => read_jsonl(open_stream(file, bytes_read)?, opts, emit),
        ImportFormat::Json => read_json_array(open_stream(file, bytes_read)?, opts, emit),
        ImportFormat::Text => read_text(open_stream(file, bytes_read)?, opts.text_mode, emit),
        ImportFormat::Csv => read_csv(open_stream(file, bytes_read)?, file.tsv, opts, emit),
        ImportFormat::Parquet => {
            let r = read_parquet(&file.path, opts, emit);
            bytes_read.fetch_add(file.size, Ordering::Relaxed);
            r
        }
        ImportFormat::Auto => Err(AppError::Internal("format was not resolved".into())),
    };
    result.map_err(|e| AppError::InvalidInput(format!("{}: {e}", file.source)))
}

fn record_from_object(mut obj: Map<String, Value>, opts: &ReadOptions<'_>) -> RawRecord {
    let text = match obj.remove(opts.text_field) {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            obj.insert(opts.text_field.to_string(), other);
            None
        }
        None => None,
    };
    let id = opts.id_field.and_then(|field| match obj.remove(field) {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => {
            obj.insert(field.to_string(), other);
            None
        }
        None => None,
    });
    RawRecord { id, text, meta: obj }
}

fn read_jsonl(
    mut reader: Box<dyn BufRead>,
    opts: &ReadOptions<'_>,
    emit: &mut dyn FnMut(Item) -> AppResult<bool>,
) -> AppResult<()> {
    let mut buf = Vec::new();
    let mut line_no = 0u64;
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(());
        }
        line_no += 1;
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(obj)) => Item::Record(record_from_object(obj, opts)),
            Ok(_) => Item::ParseError(format!("line {line_no}: not a JSON object")),
            Err(e) => Item::ParseError(format!("line {line_no}: {e}")),
        };
        if !emit(item)? {
            return Ok(());
        }
    }
}

fn read_json_array(
    reader: Box<dyn BufRead>,
    opts: &ReadOptions<'_>,
    emit: &mut dyn FnMut(Item) -> AppResult<bool>,
) -> AppResult<()> {
    let items = match serde_json::from_reader::<_, Value>(reader)? {
        Value::Array(items) => items,
        Value::Object(obj) => vec![Value::Object(obj)],
        _ => return Err(AppError::InvalidInput("expected a JSON array of objects".into())),
    };
    for (i, item) in items.into_iter().enumerate() {
        let item = match item {
            Value::Object(obj) => Item::Record(record_from_object(obj, opts)),
            _ => Item::ParseError(format!("element {}: not a JSON object", i + 1)),
        };
        if !emit(item)? {
            return Ok(());
        }
    }
    Ok(())
}

fn text_record(text: String) -> Item {
    Item::Record(RawRecord { id: None, text: Some(text), meta: Map::new() })
}

fn read_text(
    mut reader: Box<dyn BufRead>,
    mode: TextMode,
    emit: &mut dyn FnMut(Item) -> AppResult<bool>,
) -> AppResult<()> {
    match mode {
        TextMode::WholeFile => {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            emit(text_record(String::from_utf8_lossy(&bytes).into_owned()))?;
            Ok(())
        }
        TextMode::PerLine | TextMode::PerParagraph => {
            let per_line = mode == TextMode::PerLine;
            let mut buf = Vec::new();
            let mut paragraph = String::new();
            loop {
                buf.clear();
                let eof = reader.read_until(b'\n', &mut buf)? == 0;
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                if per_line {
                    if !eof && !line.trim().is_empty() && !emit(text_record(line.to_string()))? {
                        return Ok(());
                    }
                } else if eof || line.trim().is_empty() {
                    if !paragraph.trim().is_empty() && !emit(text_record(std::mem::take(&mut paragraph)))? {
                        return Ok(());
                    }
                    paragraph.clear();
                } else {
                    if !paragraph.is_empty() {
                        paragraph.push('\n');
                    }
                    paragraph.push_str(line);
                }
                if eof {
                    return Ok(());
                }
            }
        }
    }
}

fn read_csv(
    reader: Box<dyn BufRead>,
    tsv: bool,
    opts: &ReadOptions<'_>,
    emit: &mut dyn FnMut(Item) -> AppResult<bool>,
) -> AppResult<()> {
    let mut csv = csv::ReaderBuilder::new()
        .delimiter(if tsv { b'\t' } else { b',' })
        .flexible(true)
        .has_headers(true)
        .from_reader(reader);
    let headers: Vec<String> =
        csv.byte_headers()?.iter().map(|h| String::from_utf8_lossy(h).trim().to_string()).collect();
    let text_idx = headers.iter().position(|h| h == opts.text_field).ok_or_else(|| missing_column(opts.text_field, &headers))?;
    let id_idx = opts.id_field.and_then(|f| headers.iter().position(|h| h == f));

    let mut record = csv::ByteRecord::new();
    let mut row_no = 1u64; // header is row 1
    loop {
        row_no += 1;
        match csv.read_byte_record(&mut record) {
            Ok(false) => return Ok(()),
            Ok(true) => {}
            Err(e) => {
                if !emit(Item::ParseError(format!("row {row_no}: {e}")))? {
                    return Ok(());
                }
                continue;
            }
        }
        let field = |i: usize| record.get(i).map(|b| String::from_utf8_lossy(b).into_owned());
        let mut meta = Map::new();
        for (i, name) in headers.iter().enumerate() {
            if i != text_idx && Some(i) != id_idx
                && let Some(v) = field(i) {
                    meta.insert(name.clone(), Value::String(v));
                }
        }
        let rec = RawRecord { id: id_idx.and_then(field), text: field(text_idx), meta };
        if !emit(Item::Record(rec))? {
            return Ok(());
        }
    }
}

fn read_parquet(path: &Path, opts: &ReadOptions<'_>, emit: &mut dyn FnMut(Item) -> AppResult<bool>) -> AppResult<()> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let names: Vec<String> = builder.schema().fields().iter().map(|f| f.name().clone()).collect();
    let text_idx = names.iter().position(|n| n == opts.text_field).ok_or_else(|| missing_column(opts.text_field, &names))?;
    let id_idx = opts.id_field.and_then(|f| names.iter().position(|n| n == f));
    let meta_idx: Vec<usize> = (0..names.len()).filter(|&i| i != text_idx && Some(i) != id_idx).collect();

    for batch in builder.with_batch_size(2048).build()? {
        let batch = batch?;
        let text = cast(batch.column(text_idx), &DataType::Utf8)
            .map_err(|e| AppError::InvalidInput(format!("column {} can't be read as text: {e}", opts.text_field)))?;
        let text = text.as_string::<i32>();
        let ids = id_idx.map(|i| cast(batch.column(i), &DataType::Utf8)).transpose()?;
        let ids = ids.as_ref().map(|a| a.as_string::<i32>());

        // Other columns become each document's metadata, converted via Arrow's JSON writer.
        let mut metas: Vec<Map<String, Value>> = vec![Map::new(); batch.num_rows()];
        if !meta_idx.is_empty() {
            let projected = batch.project(&meta_idx)?;
            let mut writer = arrow::json::LineDelimitedWriter::new(Vec::new());
            writer.write(&projected)?;
            writer.finish()?;
            let json = writer.into_inner();
            for (slot, line) in metas.iter_mut().zip(json.split(|&b| b == b'\n')) {
                if let Ok(Value::Object(obj)) = serde_json::from_slice(line) {
                    *slot = obj;
                }
            }
        }

        for (i, meta) in metas.into_iter().enumerate() {
            let rec = RawRecord {
                id: ids.and_then(|a| a.is_valid(i).then(|| a.value(i).to_string())),
                text: text.is_valid(i).then(|| text.value(i).to_string()),
                meta,
            };
            if !emit(Item::Record(rec))? {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn missing_column(name: &str, available: &[String]) -> AppError {
    AppError::InvalidInput(format!(
        "no field named \"{name}\". Available: {}",
        if available.is_empty() { "(none)".to_string() } else { available.join(", ") }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn opts() -> ReadOptions<'static> {
        ReadOptions { text_field: "text", id_field: Some("id"), text_mode: TextMode::PerLine }
    }

    fn read_all(path: &Path, opts: &ReadOptions<'_>) -> AppResult<Vec<Item>> {
        let (files, _) = collect_inputs(&[path.display().to_string()], ImportFormat::Auto)?;
        let mut items = Vec::new();
        for f in &files {
            read_file(f, opts, Arc::new(AtomicU64::new(0)), &mut |item| {
                items.push(item);
                Ok(true)
            })?;
        }
        Ok(items)
    }

    fn texts(items: &[Item]) -> Vec<Option<String>> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Record(r) => Some(r.text.clone()),
                Item::ParseError(_) => None,
            })
            .collect()
    }

    #[test]
    fn detects_formats_through_compression_suffixes() {
        assert_eq!(detect(Path::new("a.JSONL.gz")), (Some(ImportFormat::Jsonl), Compression::Gzip, false));
        assert_eq!(detect(Path::new("a.tsv.zst")), (Some(ImportFormat::Csv), Compression::Zstd, true));
        assert_eq!(detect(Path::new("a.bin")), (None, Compression::None, false));
    }

    #[test]
    fn jsonl_keeps_id_and_meta_and_reports_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("d.jsonl");
        std::fs::write(&p, "{\"id\":7,\"text\":\"hello\",\"url\":\"u\"}\n\nnot json\n[1]\n{\"body\":\"x\"}\n").unwrap();
        let items = read_all(&p, &opts()).unwrap();
        assert_eq!(items.len(), 4);
        match &items[0] {
            Item::Record(r) => {
                assert_eq!(r.id.as_deref(), Some("7"));
                assert_eq!(r.text.as_deref(), Some("hello"));
                assert_eq!(r.meta.get("url"), Some(&Value::String("u".into())));
            }
            _ => panic!("expected record"),
        }
        assert!(matches!(&items[1], Item::ParseError(m) if m.starts_with("line 3")));
        assert!(matches!(&items[2], Item::ParseError(m) if m.contains("not a JSON object")));
        assert!(matches!(&items[3], Item::Record(r) if r.text.is_none()));
    }

    #[test]
    fn gzip_and_zstd_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"{\"text\":\"a\"}\n{\"text\":\"b\"}\n";
        let gz = dir.path().join("x.jsonl.gz");
        let mut enc = flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap();
        let zs = dir.path().join("y.jsonl.zst");
        std::fs::write(&zs, zstd::encode_all(&data[..], 3).unwrap()).unwrap();
        for p in [gz, zs] {
            assert_eq!(texts(&read_all(&p, &opts()).unwrap()), vec![Some("a".into()), Some("b".into())]);
        }
    }

    #[test]
    fn text_modes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.txt");
        std::fs::write(&p, "one\r\ntwo\n\n\nthree\nfour\n").unwrap();
        let lines = read_all(&p, &opts()).unwrap();
        assert_eq!(texts(&lines).len(), 4);
        let para = ReadOptions { text_mode: TextMode::PerParagraph, ..opts() };
        assert_eq!(texts(&read_all(&p, &para).unwrap()), vec![Some("one\ntwo".into()), Some("three\nfour".into())]);
        let whole = ReadOptions { text_mode: TextMode::WholeFile, ..opts() };
        assert_eq!(texts(&read_all(&p, &whole).unwrap()).len(), 1);
    }

    #[test]
    fn csv_and_tsv_with_missing_column_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("d.csv");
        std::fs::write(&p, "id,text,lang\n1,\"hello, world\",en\n2,bye,fr\n").unwrap();
        let items = read_all(&p, &opts()).unwrap();
        assert_eq!(texts(&items), vec![Some("hello, world".into()), Some("bye".into())]);
        assert!(matches!(&items[0], Item::Record(r) if r.id.as_deref() == Some("1") && r.meta.contains_key("lang")));

        let t = dir.path().join("d.tsv");
        std::fs::write(&t, "content\tx\nhi\t1\n").unwrap();
        let err = read_all(&t, &opts()).err().unwrap().to_string();
        assert!(err.contains("no field named \"text\"") && err.contains("content"), "{err}");
        let content = ReadOptions { text_field: "content", ..opts() };
        assert_eq!(texts(&read_all(&t, &content).unwrap()), vec![Some("hi".into())]);
    }

    #[test]
    fn parquet_with_extra_columns() {
        use arrow::array::{ArrayRef, Int64Array, StringArray};
        use arrow::datatypes::{Field, Schema};
        use arrow::record_batch::RecordBatch;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("d.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("text", DataType::Utf8, true),
            Field::new("score", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("a"), None])) as ArrayRef,
                Arc::new(Int64Array::from(vec![5, 6])) as ArrayRef,
            ],
        )
        .unwrap();
        let mut w = parquet::arrow::ArrowWriter::try_new(File::create(&p).unwrap(), schema, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let items = read_all(&p, &opts()).unwrap();
        assert_eq!(texts(&items), vec![Some("a".into()), None]);
        assert!(matches!(&items[0], Item::Record(r) if r.meta.get("score") == Some(&Value::from(5))));
    }

    #[test]
    fn folders_are_walked_and_unknown_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("b.txt"), "x").unwrap();
        std::fs::write(root.join("sub").join("a.jsonl"), "{}").unwrap();
        std::fs::write(root.join("image.png"), "x").unwrap();
        std::fs::write(root.join(".hidden.txt"), "x").unwrap();
        let (files, ignored) = collect_inputs(&[root.display().to_string()], ImportFormat::Auto).unwrap();
        let sources: Vec<_> = files.iter().map(|f| f.source.as_str()).collect();
        assert_eq!(sources, vec!["corpus/b.txt", "corpus/sub/a.jsonl"]);
        assert_eq!(ignored, 1);
    }
}
