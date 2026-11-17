//! Datasets on disk.
//!
//! ```text
//! datasets/<id>/
//! ├── manifest.json        DatasetManifest: parts, stats, import report
//! ├── part-00000.parquet   columns: id, text, source, meta (all UTF-8; meta is a JSON object)
//! └── part-00001.parquet ...
//! ```
//!
//! Datasets are written into `datasets/.importing-<id>/` and renamed into place only when
//! complete, so a crash or cancel never leaves a half-written dataset.

pub mod parts;
mod reader;
mod stats;
mod writer;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aeon_shared::DatasetManifest;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use crate::error::{AppError, AppResult, now_ms};

pub use reader::{DatasetReader, PREVIEW_CHARS};
pub(crate) use reader::column;
pub use writer::{DatasetWriter, NewDoc};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const MANIFEST_VERSION: u32 = 1;
/// Prefix of folders for datasets still being written.
pub const TEMP_PREFIX: &str = ".importing-";

pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("meta", DataType::Utf8, false),
    ]))
}

/// Dataset ids are used as folder names, so they are restricted to `[a-z0-9-]`.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A new, unused dataset id such as `ds-mhw3k2x1`.
pub fn new_id(datasets_dir: &Path) -> String {
    new_prefixed_id(datasets_dir, "ds", TEMP_PREFIX)
}

/// A new id `<prefix>-<base36 time>` not yet used as a folder (or temp folder) in `dir`.
pub fn new_prefixed_id(dir: &Path, prefix: &str, temp_prefix: &str) -> String {
    let base = format!("{prefix}-{}", to_base36(now_ms().max(0) as u64));
    let taken = |id: &str| dir.join(id).exists() || dir.join(format!("{temp_prefix}{id}")).exists();
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).expect("unbounded range")
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

pub fn read_manifest(dir: &Path) -> AppResult<DatasetManifest> {
    let path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::DatasetNotFound(dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        } else {
            e.into()
        }
    })?;
    let manifest: DatasetManifest = serde_json::from_str(&text)?;
    if manifest.format_version > MANIFEST_VERSION {
        return Err(AppError::InvalidInput(format!(
            "Dataset {} was written by a newer version of AeonDataStudio",
            manifest.id
        )));
    }
    Ok(manifest)
}

pub fn write_manifest(dir: &Path, manifest: &DatasetManifest) -> AppResult<()> {
    std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_string_pretty(manifest)?)?;
    Ok(())
}

/// Removes leftovers of imports or runs that were interrupted by a crash.
pub fn remove_stale_temp_dirs(dir: &Path, temp_prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(temp_prefix)
            && let Err(e) = std::fs::remove_dir_all(entry.path()) {
                log::warn!("could not remove {}: {e}", entry.path().display());
            }
    }
}

/// A dataset folder being written. Deleted on drop unless [`TempDataset::commit`] succeeds.
pub struct TempDataset {
    temp_dir: PathBuf,
    final_dir: PathBuf,
    committed: bool,
}

impl TempDataset {
    pub fn create(datasets_dir: &Path, id: &str) -> AppResult<Self> {
        Self::create_with_prefix(datasets_dir, id, TEMP_PREFIX)
    }

    /// A folder `<dir>/<temp_prefix><id>` that becomes `<dir>/<id>` on commit.
    pub fn create_with_prefix(dir: &Path, id: &str, temp_prefix: &str) -> AppResult<Self> {
        let temp_dir = dir.join(format!("{temp_prefix}{id}"));
        std::fs::create_dir_all(&temp_dir)?;
        Ok(Self { temp_dir, final_dir: dir.join(id), committed: false })
    }

    pub fn path(&self) -> &Path {
        &self.temp_dir
    }

    pub fn commit(mut self) -> AppResult<PathBuf> {
        std::fs::rename(&self.temp_dir, &self.final_dir)?;
        self.committed = true;
        Ok(self.final_dir.clone())
    }
}

impl Drop for TempDataset {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.temp_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_valid_and_unique() {
        let dir = tempfile::tempdir().unwrap();
        let a = new_id(dir.path());
        assert!(is_valid_id(&a), "{a}");
        std::fs::create_dir(dir.path().join(&a)).unwrap();
        let b = new_id(dir.path());
        assert_ne!(a, b);
        assert!(is_valid_id(&b));
    }

    #[test]
    fn rejects_path_like_ids() {
        for bad in ["", "..", "../x", "a/b", "A", "x y", "ds\\1"] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn temp_dataset_is_removed_unless_committed() {
        let dir = tempfile::tempdir().unwrap();
        let t = TempDataset::create(dir.path(), "ds-a").unwrap();
        let temp = t.path().to_path_buf();
        drop(t);
        assert!(!temp.exists());

        let t = TempDataset::create(dir.path(), "ds-b").unwrap();
        let final_dir = t.commit().unwrap();
        assert!(final_dir.is_dir());
    }
}
