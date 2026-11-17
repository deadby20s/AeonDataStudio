mod commands;
mod updates;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aeon_core::api::ApiServer;
use aeon_core::jobs::{JobEvents, JobManager};
use aeon_core::state::AppState;
use aeon_shared::{JOB_UPDATE_EVENT, JobStatus};
use tauri::{Emitter, Manager};

/// Forwards job updates to the window.
struct WindowEvents(tauri::AppHandle);

impl JobEvents for WindowEvents {
    fn job_updated(&self, status: &JobStatus) {
        if let Err(e) = self.0.emit(JOB_UPDATE_EVENT, status) {
            log::warn!("failed to emit job update: {e}");
        }
    }
}

/// The engine state shared by the commands and the local API.
pub struct Studio(pub Arc<AppState>);

/// The local API server while it runs.
#[derive(Default)]
pub struct Api {
    pub server: Mutex<Option<ApiServer>>,
    pub error: Mutex<Option<String>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            // Writes AeonDataStudio.log to the OS log folder (Settings → Open log folder).
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                    file_name: Some("AeonDataStudio".into()),
                }))
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout))
                .level(log::LevelFilter::Info)
                .max_file_size(5 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .setup(|app| {
            let settings_path: PathBuf = app.path().app_config_dir()?.join("settings.json");
            let jobs = JobManager::new(Arc::new(WindowEvents(app.handle().clone())));
            let version = app.package_info().version.to_string();
            let state = Arc::new(AppState::new(settings_path, jobs, version.clone()));

            // Reopen the last workspace. If it has moved or is broken, start without one;
            // it stays in the recent list so the user sees the error when they pick it.
            if let Some(last) = state.recent_workspaces().first().cloned()
                && let Err(e) = state.open_workspace(std::path::Path::new(&last))
            {
                log::warn!("could not reopen workspace {last}: {e}");
            }

            let api = Api::default();
            if state.settings().api.enabled {
                commands::start_api_server(&state, &api);
            }

            log::info!("AeonDataStudio {version} started");
            app.manage(Studio(state));
            app.manage(api);
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
            commands::cancel_preview,
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
            commands::pick_export_folder,
            commands::start_mix,
            commands::preview_import,
            commands::create_demo_dataset,
            commands::open_log_folder,
            commands::check_for_update,
            commands::install_update,
            commands::sql_tables,
            commands::run_sql,
            commands::cancel_sql,
            commands::check_sql,
            commands::list_jobs,
            commands::cancel_job,
            commands::clear_finished_jobs,
            commands::start_self_test_job,
            commands::list_plugins,
            commands::open_plugins_folder,
            commands::get_theme_settings,
            commands::set_theme_settings,
            commands::api_status,
            commands::set_api_enabled,
            commands::new_api_token,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
