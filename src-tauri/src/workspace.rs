//! A workspace is a folder the user picks; all datasets, runs and settings live inside it.
//!
//! ```text
//! workspace/
//! ├── studio.toml       settings and format version
//! ├── catalog.sqlite    datasets, runs, lineage
//! ├── pipelines/        saved recipes
//! ├── datasets/         one folder per dataset
//! ├── runs/             one folder per run
//! └── exports/
//! ```

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aeon_shared::WorkspaceInfo;
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::error::{AppError, AppResult, now_ms};

pub const SETTINGS_FILE: &str = "studio.toml";
pub const CATALOG_FILE: &str = "catalog.sqlite";
pub const SUBDIRS: &[&str] = &["pipelines", "datasets", "runs", "exports"];

/// Bumped when the on-disk layout changes in a way older versions cannot read.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub name: String,
    pub format_version: u32,
    pub created_at_ms: i64,
}

pub struct Workspace {
    root: PathBuf,
    settings: WorkspaceSettings,
    catalog: Mutex<Catalog>,
}

impl Workspace {
    /// Opens an existing workspace, or initializes a new one in an empty folder.
    ///
    /// A non-empty folder without `studio.toml` is rejected so that picking the wrong folder
    /// (for example "Documents") never scatters workspace files into it.
    pub fn open_or_init(root: &Path) -> AppResult<Self> {
        if !root.is_dir() {
            return Err(AppError::InvalidInput(format!("{} is not a folder", root.display())));
        }
        let settings_path = root.join(SETTINGS_FILE);
        let settings = if settings_path.is_file() {
            let settings: WorkspaceSettings = toml::from_str(&std::fs::read_to_string(&settings_path)?)?;
            if settings.format_version > FORMAT_VERSION {
                return Err(AppError::InvalidInput(format!(
                    "This workspace was created by a newer version of AeonDataStudio (format {}, this app supports {})",
                    settings.format_version, FORMAT_VERSION
                )));
            }
            settings
        } else if is_empty_dir(root)? {
            let settings = WorkspaceSettings {
                name: root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Workspace".into()),
                format_version: FORMAT_VERSION,
                created_at_ms: now_ms(),
            };
            std::fs::write(&settings_path, toml::to_string_pretty(&settings)?)?;
            settings
        } else {
            return Err(AppError::InvalidInput(format!(
                "{} is not a workspace and is not empty. Choose an empty folder to create a new workspace.",
                root.display()
            )));
        };

        for dir in SUBDIRS {
            std::fs::create_dir_all(root.join(dir))?;
        }
        crate::dataset::remove_stale_temp_dirs(&root.join("datasets"), crate::dataset::TEMP_PREFIX);
        crate::dataset::remove_stale_temp_dirs(&root.join("runs"), crate::pipeline::RUN_TEMP_PREFIX);
        let catalog = Catalog::open(&root.join(CATALOG_FILE))?;
        catalog.fail_interrupted_runs(now_ms())?;

        Ok(Self { root: root.to_path_buf(), settings, catalog: Mutex::new(catalog) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn datasets_dir(&self) -> PathBuf {
        self.root.join("datasets")
    }

    /// Folder of a dataset. The id is validated so it can't point outside the workspace.
    pub fn dataset_dir(&self, id: &str) -> AppResult<PathBuf> {
        if !crate::dataset::is_valid_id(id) {
            return Err(AppError::InvalidInput(format!("Invalid dataset id: {id:?}")));
        }
        Ok(self.datasets_dir().join(id))
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    pub fn pipelines_dir(&self) -> PathBuf {
        self.root.join("pipelines")
    }

    /// Folder of a run. The id is validated so it can't point outside the workspace.
    pub fn run_dir(&self, id: &str) -> AppResult<PathBuf> {
        if !crate::dataset::is_valid_id(id) {
            return Err(AppError::InvalidInput(format!("Invalid run id: {id:?}")));
        }
        Ok(self.runs_dir().join(id))
    }

    pub fn catalog(&self) -> AppResult<std::sync::MutexGuard<'_, Catalog>> {
        self.catalog.lock().map_err(|_| AppError::Internal("catalog lock poisoned".into()))
    }

    pub fn info(&self) -> AppResult<WorkspaceInfo> {
        let catalog = self.catalog()?;
        Ok(WorkspaceInfo {
            name: self.settings.name.clone(),
            path: self.root.display().to_string(),
            created_at_ms: self.settings.created_at_ms,
            dataset_count: catalog.dataset_count()?,
            run_count: catalog.run_count()?,
        })
    }
}

fn is_empty_dir(path: &Path) -> AppResult<bool> {
    Ok(std::fs::read_dir(path)?.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_empty_folder_and_reopens_it() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_init(dir.path()).unwrap();
        for sub in SUBDIRS {
            assert!(dir.path().join(sub).is_dir());
        }
        assert!(dir.path().join(SETTINGS_FILE).is_file());
        let created = ws.info().unwrap().created_at_ms;
        drop(ws);

        let reopened = Workspace::open_or_init(dir.path()).unwrap();
        assert_eq!(reopened.info().unwrap().created_at_ms, created);
    }

    #[test]
    fn rejects_non_empty_folder_without_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let err = Workspace::open_or_init(dir.path()).err().unwrap();
        assert!(matches!(err, AppError::InvalidInput(_)));
        assert!(!dir.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn rejects_newer_format() {
        let dir = tempfile::tempdir().unwrap();
        let settings = WorkspaceSettings { name: "x".into(), format_version: FORMAT_VERSION + 1, created_at_ms: 0 };
        std::fs::write(dir.path().join(SETTINGS_FILE), toml::to_string(&settings).unwrap()).unwrap();
        assert!(Workspace::open_or_init(dir.path()).is_err());
    }
}
