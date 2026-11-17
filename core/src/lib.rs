//! The AeonDataStudio engine, without any UI: workspaces, the catalog, import, cleaning
//! pipelines, export and mixing, the SQL explorer, and the background job runner.
//!
//! The desktop app (`src-tauri`), the `aeon` command-line tool, the local HTTP API and the MCP
//! server are all thin layers over [`state::AppState`] and the modules here.

pub mod api;
pub mod catalog;
pub mod dataset;
pub mod demo;
pub mod error;
pub mod export;
pub mod import;
pub mod jobs;
pub mod llm;
pub mod mcp;
pub mod mix;
pub mod ops;
pub mod pipeline;
pub mod plugins;
#[cfg(test)]
mod scale;
pub mod sql;
pub mod state;
pub mod workspace;

pub use error::{AppError, AppResult};
pub use state::AppState;
