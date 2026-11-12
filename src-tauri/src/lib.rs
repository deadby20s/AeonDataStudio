mod catalog;
mod commands;
mod error;
mod jobs;
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
            if let Some(last) = state.recent_workspaces().first().cloned() {
                if let Err(e) = state.open_workspace(std::path::Path::new(&last)) {
                    log::warn!("could not reopen workspace {last}: {e}");
                }
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
            commands::list_jobs,
            commands::cancel_job,
            commands::clear_finished_jobs,
            commands::start_self_test_job,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
