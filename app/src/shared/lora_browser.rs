//! Commands for the LoRAs tab's Civitai browser: listing, details, the
//! base-model filter, installed-file marks, the mature-content setting and
//! downloads (through the existing LoRA downloader).

use super::{recover_lock, spawn_progress_emitter, AppState, DownloadProgressEvent};
use crate::host::{AppHandle, State};
use crate::platform::{effective_download_root, resolve_root_path};
use arctic_downloader::civitai_browse::{
    self, BrowsePage, BrowseQuery, Kind, LoraDetails, PERIODS, SORTS,
};
use arctic_downloader::config::AppSettings;
use arctic_downloader::model::LoraDefinition;
use serde::Serialize;
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Sort and period choices for the browser's dropdowns.
#[derive(Debug, Serialize)]
pub struct LoraBrowserOptions {
    pub sorts: Vec<&'static str>,
    pub periods: Vec<&'static str>,
    pub show_mature: bool,
}

pub fn get_lora_browser_options(state: State<'_, AppState>) -> LoraBrowserOptions {
    LoraBrowserOptions {
        sorts: SORTS.to_vec(),
        periods: PERIODS.to_vec(),
        show_mature: state.context.config.settings().civitai_show_mature,
    }
}

pub async fn browse_civitai_loras(
    state: State<'_, AppState>,
    query: BrowseQuery,
) -> Result<BrowsePage, String> {
    let include_mature = state.context.config.settings().civitai_show_mature;
    civitai_browse::browse(Kind::Loras, &query, include_mature)
        .await
        .map_err(|err| format!("{err:#}"))
}

pub async fn get_civitai_lora(
    state: State<'_, AppState>,
    model_id: u64,
) -> Result<LoraDetails, String> {
    let include_mature = state.context.config.settings().civitai_show_mature;
    civitai_browse::details(Kind::Loras, model_id, include_mature)
        .await
        .map_err(|err| format!("{err:#}"))
}

pub async fn get_civitai_base_models() -> Result<Vec<String>, String> {
    civitai_browse::base_models()
        .await
        .map_err(|err| format!("{err:#}"))
}

pub fn set_civitai_show_mature(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AppSettings, String> {
    state
        .context
        .config
        .update_settings(|settings| settings.civitai_show_mature = enabled)
        .map_err(|err| err.to_string())
}

/// Lowercased names of the files under `models/loras` (any depth up to 4),
/// so the browser can mark LoRAs that are already installed.
pub fn list_installed_lora_files(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Vec<String> {
    let Ok(root) = resolve_root_path(&state.context, comfyui_root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect_file_names(
        &effective_download_root(&root).join("models").join("loras"),
        4,
        &mut names,
    );
    names
}

fn collect_file_names(dir: &Path, depth: usize, names: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_file_names(&path, depth - 1, names);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            names.push(name.to_ascii_lowercase());
        }
    }
}

/// Downloads one file of a Civitai LoRA version into
/// `models/loras/<base model>/`, emitting `download-progress` events.
pub async fn download_civitai_lora(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: u64,
    version_id: u64,
    file_id: Option<u64>,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let effective_root = effective_download_root(&root);
    let settings = state.context.config.settings();
    let details = civitai_browse::details(Kind::Loras, model_id, settings.civitai_show_mature)
        .await
        .map_err(|err| format!("{err:#}"))?;
    let version = details
        .versions
        .iter()
        .find(|v| v.id == version_id)
        .ok_or_else(|| "That LoRA version is no longer on Civitai.".to_string())?;
    let file = match file_id {
        Some(id) => version.files.iter().find(|f| f.id == id),
        None => version
            .files
            .iter()
            .find(|f| f.primary)
            .or(version.files.first()),
    }
    .ok_or_else(|| "That LoRA version has no downloadable file.".to_string())?;
    let lora = LoraDefinition {
        id: format!("civitai-{model_id}"),
        display_name: details.name.clone(),
        family: version.base_model.clone(),
        download_url: file.download_url.clone(),
        note: None,
        file_name: Some(file.name.clone()),
    };

    let cancel = CancellationToken::new();
    {
        let mut active = recover_lock(state.active_cancel.lock());
        if active.is_some() {
            return Err("A download is already active. Cancel it first.".to_string());
        }
        *active = Some(cancel.clone());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = state.context.downloads.download_lora_with_cancel(
        effective_root,
        lora,
        settings.civitai_token.clone(),
        tx,
        Some(cancel),
    );
    *recover_lock(state.active_abort.lock()) = Some(handle.abort_handle());
    spawn_progress_emitter(app.clone(), "lora".to_string(), rx);
    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = handle.await;
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.active_cancel.lock()) = None;
        *recover_lock(managed.active_abort.lock()) = None;

        let (phase, message, total) = match result {
            Ok(Ok(outcome)) => (
                "batch_finished",
                format!("LoRA saved to {}", outcome.destination.display()),
                Some(1),
            ),
            Ok(Err(err)) => {
                let message = err.to_string();
                let phase = if message.to_ascii_lowercase().contains("cancel") {
                    "cancelled"
                } else {
                    "batch_failed"
                };
                (phase, message, None)
            }
            Err(join_err) => (
                if join_err.is_cancelled() {
                    "cancelled"
                } else {
                    "batch_failed"
                },
                join_err.to_string(),
                None,
            ),
        };
        let _ = app_for_task.emit(
            "download-progress",
            DownloadProgressEvent {
                kind: "lora".to_string(),
                phase: phase.to_string(),
                artifact: None,
                index: None,
                total,
                received: None,
                size: None,
                folder: None,
                message: Some(message),
            },
        );
    });

    Ok(())
}
