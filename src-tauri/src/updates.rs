//! In-app updates from GitHub releases.
//!
//! Updates are signed: the release workflow signs the installers with a private key kept as a
//! GitHub secret, and the app checks them against the public key in `updater-pubkey.txt`.
//! When that file is empty (a build made before the key was set up), updates are reported as
//! not configured instead of failing.

use aeon_shared::UpdateInfo;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use aeon_core::{AppError, AppResult};

const PUBKEY: &str = include_str!("../updater-pubkey.txt");

fn pubkey() -> AppResult<&'static str> {
    let key = PUBKEY.trim();
    if key.is_empty() {
        return Err(AppError::InvalidInput(
            "Updates are not set up in this build yet. Download new versions from the Releases page.".into(),
        ));
    }
    Ok(key)
}

pub fn configured() -> bool {
    !PUBKEY.trim().is_empty()
}

pub async fn check(app: &AppHandle) -> AppResult<Option<UpdateInfo>> {
    let updater = app
        .updater_builder()
        .pubkey(pubkey()?)
        .build()
        .map_err(|e| AppError::Internal(format!("Updater: {e}")))?;
    let update = updater.check().await.map_err(|e| AppError::Internal(format!("Could not check for updates: {e}")))?;
    Ok(update.map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone(), date: u.date.map(|d| d.to_string()) }))
}

/// Downloads and installs the available update, then restarts the app.
pub async fn install(app: &AppHandle) -> AppResult<()> {
    let updater = app
        .updater_builder()
        .pubkey(pubkey()?)
        .build()
        .map_err(|e| AppError::Internal(format!("Updater: {e}")))?;
    let Some(update) = updater.check().await.map_err(|e| AppError::Internal(format!("Could not check for updates: {e}")))?
    else {
        return Err(AppError::InvalidInput("No update is available".into()));
    };
    log::info!("installing update {}", update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| AppError::Internal(format!("Could not install the update: {e}")))?;
    app.restart();
}
