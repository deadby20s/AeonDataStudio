use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::jobs::JobManager;
use crate::workspace::Workspace;

const MAX_RECENT: usize = 10;

/// App-level preferences stored in the OS config dir (not inside a workspace).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Most recent first.
    #[serde(default)]
    pub recent_workspaces: Vec<String>,
}

impl AppSettings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn push_recent(&mut self, path: &str) {
        self.recent_workspaces.retain(|p| p != path);
        self.recent_workspaces.insert(0, path.to_string());
        self.recent_workspaces.truncate(MAX_RECENT);
    }

    pub fn forget(&mut self, path: &str) {
        self.recent_workspaces.retain(|p| p != path);
    }
}

pub struct AppState {
    workspace: RwLock<Option<Arc<Workspace>>>,
    settings: RwLock<AppSettings>,
    settings_path: PathBuf,
    pub jobs: Arc<JobManager>,
    /// Cancel flag of the preview running now, if any (one preview at a time).
    pub preview_cancel: std::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    /// Interrupts the running SQL query, if any.
    pub sql_cancel: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl AppState {
    pub fn new(settings_path: PathBuf, jobs: Arc<JobManager>) -> Self {
        let settings = AppSettings::load(&settings_path);
        Self {
            workspace: RwLock::new(None),
            settings: RwLock::new(settings),
            settings_path,
            jobs,
            preview_cancel: std::sync::Mutex::new(None),
            sql_cancel: std::sync::Mutex::new(None),
        }
    }

    pub fn workspace(&self) -> AppResult<Arc<Workspace>> {
        self.workspace
            .read()
            .map_err(|_| AppError::Internal("workspace lock poisoned".into()))?
            .clone()
            .ok_or(AppError::NoWorkspace)
    }

    pub fn current_workspace(&self) -> Option<Arc<Workspace>> {
        self.workspace.read().ok().and_then(|w| w.clone())
    }

    pub fn open_workspace(&self, path: &Path) -> AppResult<Arc<Workspace>> {
        let ws = Arc::new(Workspace::open_or_init(path)?);
        *self.workspace.write().map_err(|_| AppError::Internal("workspace lock poisoned".into()))? =
            Some(ws.clone());
        self.update_settings(|s| s.push_recent(&ws.root().display().to_string()))?;
        Ok(ws)
    }

    pub fn close_workspace(&self) -> AppResult<()> {
        *self.workspace.write().map_err(|_| AppError::Internal("workspace lock poisoned".into()))? = None;
        Ok(())
    }

    pub fn recent_workspaces(&self) -> Vec<String> {
        self.settings.read().map(|s| s.recent_workspaces.clone()).unwrap_or_default()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut AppSettings)) -> AppResult<()> {
        let mut settings =
            self.settings.write().map_err(|_| AppError::Internal("settings lock poisoned".into()))?;
        f(&mut settings);
        settings.save(&self.settings_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_list_is_deduplicated_and_capped() {
        let mut s = AppSettings::default();
        for i in 0..15 {
            s.push_recent(&format!("p{i}"));
        }
        s.push_recent("p10");
        assert_eq!(s.recent_workspaces.len(), MAX_RECENT);
        assert_eq!(s.recent_workspaces[0], "p10");
        assert_eq!(s.recent_workspaces.iter().filter(|p| *p == "p10").count(), 1);
    }
}
