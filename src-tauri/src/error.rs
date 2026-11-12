use serde::{Serialize, Serializer};

/// Error type returned by every Tauri command.
///
/// It serializes to its display string, which the frontend receives as the rejection value
/// of `invoke`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("No workspace is open")]
    NoWorkspace,

    #[error("{0}")]
    InvalidInput(String),

    #[error("Job {0} not found")]
    JobNotFound(u64),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Catalog error: {0}")]
    Catalog(#[from] rusqlite::Error),

    #[error("Could not read workspace settings: {0}")]
    TomlRead(#[from] toml::de::Error),

    #[error("Could not write workspace settings: {0}")]
    TomlWrite(#[from] toml::ser::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Tauri(#[from] tauri::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Current time as milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
