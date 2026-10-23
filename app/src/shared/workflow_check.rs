//! Workflow dependency check: which models and custom node packs a workflow
//! file needs, which of them the selected ComfyUI install is missing, and
//! commands to download a missing model or install a missing pack.
//!
//! The parsing and matching live in the core crate
//! (`arctic_downloader::workflow_deps`); this module adds the install's
//! folders, the catalog, the Comfy Registry and the platform installers.

use super::manager::install_named_custom_node;
use super::{
    recover_lock, spawn_progress_emitter, stop_comfyui_for_mutation, AppState,
    DownloadProgressEvent,
};
use crate::host::{AppHandle, State};
use crate::platform::{
    effective_download_root, ensure_git_available, python_exe_for_root, resolve_root_path,
    restart_comfyui_after_mutation,
};
use arctic_downloader::download::artifact_download_url;
use arctic_downloader::workflow_deps::{
    core_node_types, index_model_files, installed_packs, model_file_name, pack_installed,
    parse_workflow_requirements, safe_model_directory, PackSource,
};
use reqwest::Url;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Workflow files larger than this are not ComfyUI workflows.
const MAX_WORKFLOW_BYTES: u64 = 64 * 1024 * 1024;

/// A workflow saved in the install's ComfyUI workflows folder.
#[derive(Debug, Serialize)]
pub struct SavedWorkflow {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowModelStatus {
    pub name: String,
    pub directory: Option<String>,
    pub present: bool,
    pub url: Option<String>,
    /// Where `url` came from: `workflow` or `catalog`.
    pub url_source: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowPackStatus {
    pub id: String,
    pub source: PackSource,
    pub version: Option<String>,
    pub node_types: Vec<String>,
    pub installed: bool,
    /// GitHub repository the pack installs from, when known.
    pub repository: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowDependencyReport {
    pub workflow: String,
    pub models: Vec<WorkflowModelStatus>,
    pub packs: Vec<WorkflowPackStatus>,
    pub unresolved_node_types: Vec<String>,
}

/// Workflows saved in `<comfyui_root>/user/default/workflows`.
pub fn list_saved_workflows(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<Vec<SavedWorkflow>, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let dir = root.join("user").join("default").join("workflows");
    let mut found = Vec::new();
    collect_json_files(&dir, &dir, 2, &mut found);
    found.sort_by_key(|workflow| workflow.name.to_ascii_lowercase());
    Ok(found)
}

fn collect_json_files(base: &Path, dir: &Path, depth: usize, found: &mut Vec<SavedWorkflow>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 && !entry.file_name().to_string_lossy().starts_with('.') {
                collect_json_files(base, &path, depth - 1, found);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            found.push(SavedWorkflow {
                name: path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string(),
                path: path.to_string_lossy().to_string(),
            });
        }
    }
}

/// Checks a workflow file against the selected ComfyUI install.
pub async fn check_workflow_dependencies(
    state: State<'_, AppState>,
    path: String,
    comfyui_root: Option<String>,
) -> Result<WorkflowDependencyReport, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let path = PathBuf::from(path.trim());
    let size = std::fs::metadata(&path)
        .map_err(|err| format!("Cannot read {}: {err}", path.display()))?
        .len();
    if size > MAX_WORKFLOW_BYTES {
        return Err(format!(
            "{} is too large to be a ComfyUI workflow.",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("Cannot read {}: {err}", path.display()))?;
    let workflow: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("Not a valid JSON file: {err}"))?;
    let needs = parse_workflow_requirements(&workflow)?;

    let core = core_node_types(&root);
    let unresolved_node_types = needs
        .unresolved_node_types
        .into_iter()
        .filter(|node_type| !core.contains(node_type))
        .collect();

    // Models: present anywhere under the install's or the shared model folders.
    let mut model_roots = vec![root.join("models")];
    let download_models = effective_download_root(&root).join("models");
    if !model_roots.contains(&download_models) {
        model_roots.push(download_models);
    }
    let index = index_model_files(&model_roots);
    let catalog = state.context.catalog.catalog_snapshot();
    let catalog_url = |file: &str| {
        catalog
            .models
            .iter()
            .flat_map(|model| {
                model
                    .variants
                    .iter()
                    .flat_map(|variant| variant.artifacts.iter())
                    .chain(model.always.iter().flat_map(|group| group.artifacts.iter()))
            })
            .find(|artifact| artifact.file_name().eq_ignore_ascii_case(file))
            .and_then(|artifact| artifact_download_url(artifact).ok())
    };
    let models = needs
        .models
        .into_iter()
        .map(|model| {
            let file = model_file_name(&model.name);
            let (url, url_source) = match model.url {
                Some(url) => (Some(url), Some("workflow")),
                None => match catalog_url(file) {
                    Some(url) => (Some(url), Some("catalog")),
                    None => (None, None),
                },
            };
            WorkflowModelStatus {
                present: index.contains(&file.to_ascii_lowercase()),
                name: model.name,
                directory: model.directory,
                url,
                url_source,
            }
        })
        .collect();

    // Custom node packs.
    let installed = installed_packs(&root.join("custom_nodes"));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|err| err.to_string())?;
    let mut packs = Vec::new();
    for pack in needs.packs {
        let repository = match pack.source {
            PackSource::GitHub => github_repo_url(&format!("https://github.com/{}", pack.id)),
            PackSource::Registry => registry_repository(&client, &pack.id).await,
        };
        packs.push(WorkflowPackStatus {
            installed: pack_installed(&pack, repository.as_deref(), &installed),
            id: pack.id,
            source: pack.source,
            version: pack.version,
            node_types: pack.node_types,
            repository,
        });
    }

    Ok(WorkflowDependencyReport {
        workflow: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        models,
        packs,
        unresolved_node_types,
    })
}

/// The GitHub repository of a Comfy Registry pack.
async fn registry_repository(client: &reqwest::Client, id: &str) -> Option<String> {
    let valid_id = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid_id {
        return None;
    }
    let response = client
        .get(format!("https://api.comfy.org/nodes/{id}"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let body: serde_json::Value = response.json().await.ok()?;
    github_repo_url(body.get("repository")?.as_str()?)
}

/// `raw` as `https://github.com/<owner>/<repo>`, or `None` for anything else:
/// pack sources come from workflow files, so only plain GitHub repositories
/// are cloned.
fn github_repo_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let [owner, repo] = segments.as_slice() else {
        return None;
    };
    let repo = repo.trim_end_matches(".git");
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(repo)).then(|| format!("https://github.com/{owner}/{repo}"))
}

/// Downloads a model a workflow needs into the install's model folder
/// `<download root>/models/<directory>`.
pub async fn download_workflow_model(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    directory: String,
    name: String,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let url = Url::parse(url.trim()).map_err(|err| format!("Invalid download URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http(s) download URLs are supported.".to_string());
    }
    let directory = safe_model_directory(&directory)
        .ok_or_else(|| format!("Unsupported model folder '{directory}'."))?
        .to_string();
    let file_name = model_file_name(&name).trim().to_string();
    if file_name.is_empty() {
        return Err("The model has no file name.".to_string());
    }
    let dest_dir = effective_download_root(&root)
        .join("models")
        .join(&directory);

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
        url.to_string(),
        dest_dir,
        file_name.clone(),
        tx,
        Some(cancel),
    );
    *recover_lock(state.active_abort.lock()) = Some(handle.abort_handle());
    spawn_progress_emitter(app.clone(), "model".to_string(), rx);
    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = handle.await;
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.active_cancel.lock()) = None;
        *recover_lock(managed.active_abort.lock()) = None;
        let (phase, message) = match result {
            Ok(Ok(_)) => (
                "batch_finished",
                format!("Downloaded {file_name} to models/{directory}."),
            ),
            Ok(Err(err)) if err.to_string().to_ascii_lowercase().contains("cancel") => {
                ("cancelled", format!("{file_name} download cancelled."))
            }
            Ok(Err(err)) => (
                "batch_failed",
                format!("{file_name} download failed: {err}"),
            ),
            Err(err) => (
                "batch_failed",
                format!("{file_name} download task failed: {err}"),
            ),
        };
        let _ = app_for_task.emit(
            "download-progress",
            DownloadProgressEvent {
                kind: "model".to_string(),
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
    Ok(())
}

/// Clones a missing custom node pack (a GitHub repository) into the
/// install's `custom_nodes` and installs its requirements. ComfyUI is
/// stopped for the install and restarted if it was running.
pub async fn install_workflow_node_pack(
    app: AppHandle,
    state: State<'_, AppState>,
    repository: String,
    comfyui_root: Option<String>,
) -> Result<String, String> {
    let repo = github_repo_url(&repository).ok_or_else(|| {
        "Only GitHub repositories (https://github.com/owner/repo) can be installed.".to_string()
    })?;
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let folder = repo.rsplit('/').next().unwrap_or_default().to_string();
    if root.join("custom_nodes").join(&folder).exists() {
        return Err(format!("custom_nodes/{folder} already exists."));
    }
    let py_exe = python_exe_for_root(&root)?;
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let app_for_task = app.clone();
    let (repo_for_task, folder_for_task) = (repo.clone(), folder.clone());
    let result = crate::host::async_runtime::spawn_blocking(move || {
        ensure_git_available(&app_for_task)?;
        install_named_custom_node(
            &app_for_task,
            &root,
            &py_exe,
            &repo_for_task,
            &folder_for_task,
        )
    })
    .await
    .map_err(|err| format!("Install task failed: {err}"))?;
    restart_comfyui_after_mutation(&app, &state, was_running)?;
    result.map(|()| format!("Installed {folder} from {repo}."))
}

#[cfg(test)]
mod tests {
    use super::github_repo_url;

    #[test]
    fn only_plain_github_repositories_are_accepted() {
        assert_eq!(
            github_repo_url("https://github.com/kijai/ComfyUI-KJNodes.git").as_deref(),
            Some("https://github.com/kijai/ComfyUI-KJNodes")
        );
        assert_eq!(
            github_repo_url("https://github.com/city96/ComfyUI-GGUF/").as_deref(),
            Some("https://github.com/city96/ComfyUI-GGUF")
        );
        for bad in [
            "http://github.com/a/b",
            "https://gitlab.com/a/b",
            "https://github.com/a",
            "https://github.com/a/b/tree/main",
            "https://github.com/../b",
            "file:///etc/passwd",
            "https://github.com/a/b;rm -rf",
        ] {
            assert_eq!(github_repo_url(bad), None, "{bad}");
        }
    }
}
