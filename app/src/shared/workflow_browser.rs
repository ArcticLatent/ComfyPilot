//! Commands for the Workflows tab's Civitai browser: ComfyUI workflows on
//! Civitai that come as a `.json` file, their details, and downloading one
//! into the install's `user/default/workflows` so it can be checked and
//! opened in ComfyUI. Sorts, periods, base models and the mature-content
//! setting are shared with the LoRA browser (`lora_browser`).

use super::{recover_lock, spawn_progress_emitter, AppState, DownloadProgressEvent};
use crate::host::{AppHandle, State};
use crate::platform::resolve_root_path;
use arctic_downloader::civitai_browse::{self, BrowsePage, BrowseQuery, Kind, LoraDetails};
use arctic_downloader::download::sanitize_file_name;
use tokio_util::sync::CancellationToken;

/// Workflow files larger than this are not ComfyUI workflows (as the check's limit).
const MAX_WORKFLOW_BYTES: u64 = 64 * 1024 * 1024;

pub async fn browse_civitai_workflows(
    state: State<'_, AppState>,
    query: BrowseQuery,
) -> Result<BrowsePage, String> {
    let include_mature = state.context.config.settings().civitai_show_mature;
    civitai_browse::browse(Kind::Workflows, &query, include_mature)
        .await
        .map_err(|err| format!("{err:#}"))
}

pub async fn get_civitai_workflow(
    state: State<'_, AppState>,
    model_id: u64,
) -> Result<LoraDetails, String> {
    let include_mature = state.context.config.settings().civitai_show_mature;
    civitai_browse::details(Kind::Workflows, model_id, include_mature)
        .await
        .map_err(|err| format!("{err:#}"))
}

/// Whether `value` is a ComfyUI workflow: the editor's format (`nodes`) or
/// the API format (nodes keyed by id, each with a `class_type`).
fn looks_like_workflow(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("nodes").is_some_and(|nodes| nodes.is_array()) {
        return true;
    }
    !object.is_empty()
        && object
            .values()
            .all(|node| node.get("class_type").is_some_and(|c| c.is_string()))
}

/// Downloads a workflow's `.json` file into `<comfyui_root>/user/default/workflows`,
/// emitting `download-progress` events (kind `workflow`). Returns the path it
/// saves to. A download that isn't JSON (an error page, say) is deleted and
/// reported as failed.
pub async fn download_civitai_workflow(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: u64,
    version_id: u64,
    file_id: u64,
    comfyui_root: Option<String>,
) -> Result<String, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let include_mature = state.context.config.settings().civitai_show_mature;
    let details = civitai_browse::details(Kind::Workflows, model_id, include_mature)
        .await
        .map_err(|err| format!("{err:#}"))?;
    let file = details
        .versions
        .iter()
        .find(|v| v.id == version_id)
        .and_then(|v| v.files.iter().find(|f| f.id == file_id))
        .ok_or_else(|| "That workflow file is no longer on Civitai.".to_string())?;
    if file
        .size_bytes
        .is_some_and(|size| size > MAX_WORKFLOW_BYTES)
    {
        return Err(format!(
            "{} is too large to be a ComfyUI workflow.",
            file.name
        ));
    }
    let file_name = sanitize_file_name(&file.name);
    if !file_name.to_ascii_lowercase().ends_with(".json") {
        return Err(format!("{} is not a .json workflow.", file.name));
    }
    let workflows_dir = root.join("user").join("default").join("workflows");
    let dest = workflows_dir.join(&file_name);

    let cancel = CancellationToken::new();
    {
        let mut active = recover_lock(state.active_cancel.lock());
        if active.is_some() {
            return Err("A download is already active. Cancel it first.".to_string());
        }
        *active = Some(cancel.clone());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = state.context.downloads.download_file_with_cancel(
        file.download_url.clone(),
        workflows_dir,
        file_name.clone(),
        tx,
        Some(cancel),
    );
    *recover_lock(state.active_abort.lock()) = Some(handle.abort_handle());
    spawn_progress_emitter(app.clone(), "workflow".to_string(), rx);
    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = handle.await;
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.active_cancel.lock()) = None;
        *recover_lock(managed.active_abort.lock()) = None;
        let (phase, message) = match result {
            Ok(Ok(path)) => {
                match std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .filter(looks_like_workflow)
                {
                    Some(_) => (
                        "batch_finished",
                        format!("Saved {file_name} to ComfyUI's workflows."),
                    ),
                    None => {
                        let _ = std::fs::remove_file(&path);
                        (
                        "batch_failed",
                        format!("{file_name} from Civitai isn't a ComfyUI workflow; it was not kept."),
                    )
                    }
                }
            }
            Ok(Err(err)) if err.to_string().to_ascii_lowercase().contains("cancel") => {
                ("cancelled", format!("{file_name} download cancelled."))
            }
            Ok(Err(err)) => (
                "batch_failed",
                format!("{file_name} download failed: {err:#}"),
            ),
            Err(err) => (
                "batch_failed",
                format!("{file_name} download task failed: {err}"),
            ),
        };
        let _ = app_for_task.emit(
            "download-progress",
            DownloadProgressEvent {
                kind: "workflow".to_string(),
                phase: phase.to_string(),
                artifact: None,
                index: None,
                total: Some(1),
                received: None,
                size: None,
                folder: None,
                message: Some(message),
            },
        );
    });
    Ok(dest.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::looks_like_workflow;
    use serde_json::json;

    #[test]
    fn only_comfyui_workflows_are_kept() {
        assert!(looks_like_workflow(&json!({"nodes": [], "links": []})));
        assert!(looks_like_workflow(
            &json!({"3": {"class_type": "KSampler", "inputs": {}}})
        ));
        assert!(!looks_like_workflow(
            &json!({"error": "Unauthorized", "message": "login"})
        ));
        assert!(!looks_like_workflow(&json!({})));
        assert!(!looks_like_workflow(&json!([1, 2])));
        assert!(!looks_like_workflow(&json!({"nodes": "x"})));
    }
}
