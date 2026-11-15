mod catalog;
mod commands;
mod dataset;
mod error;
mod export;
mod import;
mod jobs;
mod mix;
mod pipeline;
mod state;
mod workspace;

use std::path::PathBuf;

use tauri::Manager;

use crate::jobs::JobManager;
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let settings_path: PathBuf = app.path().app_config_dir()?.join("settings.json");
            let jobs = JobManager::new(std::sync::Arc::new(app.handle().clone()));
            let state = AppState::new(settings_path, jobs);

            // Reopen the last workspace. If it has moved or is broken, start without one;
            // it stays in the recent list so the user sees the error when they pick it.
            if let Some(last) = state.recent_workspaces().first().cloned()
                && let Err(e) = state.open_workspace(std::path::Path::new(&last)) {
                    log::warn!("could not reopen workspace {last}: {e}");
                }

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::pick_folder,
            commands::open_workspace,
            commands::close_workspace,
            commands::forget_recent_workspace,
            commands::list_datasets,
            commands::pick_import_files,
            commands::pick_import_folder,
            commands::start_import,
            commands::get_dataset,
            commands::browse_documents,
            commands::get_document,
            commands::delete_dataset,
            commands::list_pipelines,
            commands::get_pipeline,
            commands::save_pipeline,
            commands::delete_pipeline,
            commands::preview_pipeline,
            commands::start_pipeline_run,
            commands::list_runs,
            commands::get_run,
            commands::browse_removed,
            commands::get_removed,
            commands::delete_run,
            commands::sample_removed,
            commands::browse_changed,
            commands::start_export,
            commands::list_exports,
            commands::delete_export,
            commands::open_export_folder,
            commands::pick_tokenizer_file,
            commands::start_mix,
            commands::preview_import,
            commands::list_jobs,
            commands::cancel_job,
            commands::clear_finished_jobs,
            commands::start_self_test_job,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
