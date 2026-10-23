//! Generated from the commands the Tauri app registered (`generate_handler!`), plus
//! the tray commands. Maps each command name to its function: arguments come in as a
//! JSON object with camelCase keys (as the Tauri frontend sent them), the result goes
//! back as JSON. Sync commands run on the blocking pool, async ones on the runtime.
//!
//! When adding a command, add an arm here (and on the other platform).

#![allow(clippy::redundant_clone)]

// The request types commands take, wherever the platform module gets them.
#[allow(unused_imports)]
use super::*;
use crate::commands::{arg, blocking, reply, reply_result, reply_unit, Pending};
use crate::contracts::{
    AttentionBackendChangeRequest, ComfyInstallRequest, LaunchAttentionFlagRequest,
};
use crate::host::AppHandle;
use crate::shared::{AppState, BatchModelDownloadRequest};
use arctic_downloader::civitai_browse::BrowseQuery;
use serde_json::Value;

#[allow(unused_imports)]
use crate::host::State;

/// The command names this platform handles.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const COMMANDS: &[&str] = &[
    "apply_attention_backend_change",
    "apply_comfyui_component_toggle",
    "auto_update_startup",
    "browse_civitai_loras",
    "browse_civitai_workflows",
    "cancel_active_download",
    "cancel_comfyui_install",
    "check_updates_now",
    "check_workflow_dependencies",
    "download_civitai_lora",
    "download_civitai_workflow",
    "download_model_assets",
    "download_model_assets_batch",
    "download_workflow_asset",
    "download_workflow_model",
    "get_app_snapshot",
    "get_catalog",
    "get_catalog_status",
    "get_civitai_base_models",
    "get_civitai_lora",
    "get_civitai_workflow",
    "get_comfyui_addon_state",
    "get_comfyui_extra_model_config",
    "get_comfyui_install_recommendation",
    "get_comfyui_resume_state",
    "get_comfyui_runtime_status",
    "get_comfyui_update_status",
    "get_effective_download_destination",
    "get_hf_xet_preflight",
    "get_lora_browser_options",
    "get_platform_capabilities",
    "get_settings",
    "get_tray_status",
    "inspect_comfyui_path",
    "install_workflow_node_pack",
    "list_comfyui_installations",
    "list_installed_lora_files",
    "list_saved_workflows",
    "open_external_url",
    "open_folder",
    "open_log_folder",
    "refresh_catalog",
    "run_comfyui_preflight",
    "save_civitai_token",
    "save_hf_token",
    "set_app_quitting",
    "set_civitai_show_mature",
    "set_comfyui_custom_launch_args",
    "set_comfyui_extra_model_config",
    "set_comfyui_gpu_selection",
    "set_comfyui_install_base",
    "set_comfyui_launch_attention_backend",
    "set_comfyui_port",
    "set_comfyui_root",
    "set_comfyui_show_runtime_logs",
    "set_hf_xet_enabled",
    "start_comfyui_install",
    "start_comfyui_root",
    "stop_comfyui_root",
    "tray_restart_comfyui",
    "tray_start_comfyui",
    "tray_stop_comfyui",
    "update_selected_comfyui",
    "write_diagnostics_report",
];

/// Runs `name` with `args`, or `None` if there is no such command.
pub(crate) fn dispatch(handle: &AppHandle, name: &str, args: Value) -> Option<Pending> {
    let h = handle.clone();
    let _ = &args;
    Some(match name {
        "get_platform_capabilities" => Box::pin(async move {
            blocking(move || reply(crate::platform::get_platform_capabilities())).await
        }),
        "get_app_snapshot" => Box::pin(async move {
            blocking(move || reply(super::get_app_snapshot(h.state::<AppState>()))).await
        }),
        "get_catalog" => Box::pin(async move {
            reply_result(crate::shared::get_catalog(h.state::<AppState>()).await)
        }),
        "refresh_catalog" => Box::pin(async move {
            reply_result(crate::shared::refresh_catalog(h.state::<AppState>()).await)
        }),
        "get_catalog_status" => Box::pin(async move {
            blocking(move || reply(crate::shared::get_catalog_status(h.state::<AppState>()))).await
        }),
        "get_settings" => Box::pin(async move {
            blocking(move || reply(crate::shared::get_settings(h.state::<AppState>()))).await
        }),
        "inspect_comfyui_path" => Box::pin(async move {
            let path: String = arg(&args, "path")?;
            blocking(move || reply_result(super::inspect_comfyui_path(path))).await
        }),
        "list_comfyui_installations" => Box::pin(async move {
            let base_path: Option<String> = arg(&args, "basePath")?;
            blocking(move || {
                reply_result(super::install_state::list_comfyui_installations(
                    h.state::<AppState>(),
                    base_path,
                ))
            })
            .await
        }),
        "get_comfyui_install_recommendation" => Box::pin(async move {
            let gpu_selection: Option<String> = arg(&args, "gpuSelection")?;
            blocking(move || reply(super::get_comfyui_install_recommendation(gpu_selection))).await
        }),
        "set_comfyui_gpu_selection" => Box::pin(async move {
            let gpu_selection: String = arg(&args, "gpuSelection")?;
            blocking(move || {
                reply_result(crate::shared::set_comfyui_gpu_selection(
                    h.state::<AppState>(),
                    gpu_selection,
                ))
            })
            .await
        }),
        "get_comfyui_resume_state" => Box::pin(async move {
            let install_base: Option<String> = arg(&args, "installBase")?;
            blocking(move || {
                reply_result(crate::shared::get_comfyui_resume_state(
                    h.state::<AppState>(),
                    install_base,
                ))
            })
            .await
        }),
        "get_comfyui_addon_state" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(super::install_state::get_comfyui_addon_state(
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "apply_attention_backend_change" => Box::pin(async move {
            let request: AttentionBackendChangeRequest = arg(&args, "request")?;
            blocking(move || {
                reply_result(super::apply_attention_backend_change(
                    h.clone(),
                    h.state::<AppState>(),
                    request,
                ))
            })
            .await
        }),
        "set_comfyui_launch_attention_backend" => Box::pin(async move {
            let request: LaunchAttentionFlagRequest = arg(&args, "request")?;
            blocking(move || {
                reply_result(super::set_comfyui_launch_attention_backend(
                    h.clone(),
                    h.state::<AppState>(),
                    request,
                ))
            })
            .await
        }),
        "apply_comfyui_component_toggle" => Box::pin(async move {
            let request: ComfyComponentToggleRequest = arg(&args, "request")?;
            reply_result(
                super::install::apply_comfyui_component_toggle(
                    h.clone(),
                    h.state::<AppState>(),
                    request,
                )
                .await,
            )
        }),
        "get_comfyui_update_status" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::get_comfyui_update_status(h.state::<AppState>(), comfyui_root).await,
            )
        }),
        "update_selected_comfyui" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                super::update_selected_comfyui(h.clone(), h.state::<AppState>(), comfyui_root)
                    .await,
            )
        }),
        "run_comfyui_preflight" => Box::pin(async move {
            let request: ComfyInstallRequest = arg(&args, "request")?;
            blocking(move || reply(super::run_comfyui_preflight(h.state::<AppState>(), request)))
                .await
        }),
        "get_hf_xet_preflight" => {
            Box::pin(async move { reply_result(super::get_hf_xet_preflight(h.clone()).await) })
        }
        "set_hf_xet_enabled" => Box::pin(async move {
            let enabled: bool = arg(&args, "enabled")?;
            blocking(move || {
                reply_result(super::set_hf_xet_enabled(
                    h.clone(),
                    h.state::<AppState>(),
                    enabled,
                ))
            })
            .await
        }),
        "set_comfyui_root" => Box::pin(async move {
            let comfyui_root: String = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(super::set_comfyui_root(h.state::<AppState>(), comfyui_root))
            })
            .await
        }),
        "set_comfyui_install_base" => Box::pin(async move {
            let comfyui_install_base: String = arg(&args, "comfyuiInstallBase")?;
            blocking(move || {
                reply_result(super::install_state::set_comfyui_install_base(
                    h.state::<AppState>(),
                    comfyui_install_base,
                ))
            })
            .await
        }),
        "get_comfyui_extra_model_config" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(crate::shared::get_comfyui_extra_model_config(
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "get_effective_download_destination" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(crate::shared::get_effective_download_destination(
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "set_comfyui_extra_model_config" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            let extra_model_root: Option<String> = arg(&args, "extraModelRoot")?;
            let use_as_default: bool = arg(&args, "useAsDefault")?;
            blocking(move || {
                reply_result(crate::shared::set_comfyui_extra_model_config(
                    h.state::<AppState>(),
                    comfyui_root,
                    extra_model_root,
                    use_as_default,
                ))
            })
            .await
        }),
        "set_comfyui_custom_launch_args" => Box::pin(async move {
            let custom_launch_args: String = arg(&args, "customLaunchArgs")?;
            blocking(move || {
                reply_result(crate::shared::set_comfyui_custom_launch_args(
                    h.state::<AppState>(),
                    custom_launch_args,
                ))
            })
            .await
        }),
        "set_comfyui_port" => Box::pin(async move {
            let port: u16 = arg(&args, "port")?;
            blocking(move || {
                reply_result(crate::shared::set_comfyui_port(h.state::<AppState>(), port))
            })
            .await
        }),
        "set_comfyui_show_runtime_logs" => Box::pin(async move {
            let enabled: bool = arg(&args, "enabled")?;
            blocking(move || {
                reply_result(crate::shared::set_comfyui_show_runtime_logs(
                    h.state::<AppState>(),
                    enabled,
                ))
            })
            .await
        }),
        "save_civitai_token" => Box::pin(async move {
            let token: String = arg(&args, "token")?;
            blocking(move || {
                reply_result(crate::shared::save_civitai_token(
                    h.state::<AppState>(),
                    token,
                ))
            })
            .await
        }),
        "save_hf_token" => Box::pin(async move {
            let token: String = arg(&args, "token")?;
            blocking(move || {
                reply_result(crate::shared::save_hf_token(h.state::<AppState>(), token))
            })
            .await
        }),
        "write_diagnostics_report" => Box::pin(async move {
            blocking(move || {
                reply(crate::shared::diagnostics::write_diagnostics_report(
                    h.state::<AppState>(),
                ))
            })
            .await
        }),
        "open_log_folder" => Box::pin(async move {
            blocking(move || reply_result(crate::shared::diagnostics::open_log_folder())).await
        }),
        "list_saved_workflows" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(crate::shared::workflow_check::list_saved_workflows(
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "check_workflow_dependencies" => Box::pin(async move {
            let path: String = arg(&args, "path")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::workflow_check::check_workflow_dependencies(
                    h.state::<AppState>(),
                    path,
                    comfyui_root,
                )
                .await,
            )
        }),
        "download_workflow_model" => Box::pin(async move {
            let url: String = arg(&args, "url")?;
            let directory: String = arg(&args, "directory")?;
            let name: String = arg(&args, "name")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::workflow_check::download_workflow_model(
                    h.clone(),
                    h.state::<AppState>(),
                    url,
                    directory,
                    name,
                    comfyui_root,
                )
                .await,
            )
        }),
        "install_workflow_node_pack" => Box::pin(async move {
            let repository: String = arg(&args, "repository")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::workflow_check::install_workflow_node_pack(
                    h.clone(),
                    h.state::<AppState>(),
                    repository,
                    comfyui_root,
                )
                .await,
            )
        }),
        "check_updates_now" => {
            Box::pin(
                async move { reply_result(super::check_updates_now(h.state::<AppState>()).await) },
            )
        }
        "auto_update_startup" => Box::pin(async move {
            reply_result(super::auto_update_startup(h.clone(), h.state::<AppState>()).await)
        }),
        "download_model_assets" => Box::pin(async move {
            let model_id: String = arg(&args, "modelId")?;
            let variant_id: String = arg(&args, "variantId")?;
            let ram_tier: Option<String> = arg(&args, "ramTier")?;
            let vram_tier: Option<String> = arg(&args, "vramTier")?;
            let selected_artifact_keys: Option<Vec<String>> = arg(&args, "selectedArtifactKeys")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::download_model_assets(
                    h.clone(),
                    h.state::<AppState>(),
                    model_id,
                    variant_id,
                    ram_tier,
                    vram_tier,
                    selected_artifact_keys,
                    comfyui_root,
                )
                .await,
            )
        }),
        "download_model_assets_batch" => Box::pin(async move {
            let request: BatchModelDownloadRequest = arg(&args, "request")?;
            reply_result(
                crate::shared::download_model_assets_batch(
                    h.clone(),
                    h.state::<AppState>(),
                    request,
                )
                .await,
            )
        }),
        "download_workflow_asset" => Box::pin(async move {
            let workflow_id: String = arg(&args, "workflowId")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::download_workflow_asset(
                    h.clone(),
                    h.state::<AppState>(),
                    workflow_id,
                    comfyui_root,
                )
                .await,
            )
        }),
        "get_lora_browser_options" => Box::pin(async move {
            blocking(move || {
                reply(crate::shared::lora_browser::get_lora_browser_options(
                    h.state::<AppState>(),
                ))
            })
            .await
        }),
        "browse_civitai_loras" => Box::pin(async move {
            let query: BrowseQuery = arg(&args, "query")?;
            reply_result(
                crate::shared::lora_browser::browse_civitai_loras(h.state::<AppState>(), query)
                    .await,
            )
        }),
        "get_civitai_lora" => Box::pin(async move {
            let model_id: u64 = arg(&args, "modelId")?;
            reply_result(
                crate::shared::lora_browser::get_civitai_lora(h.state::<AppState>(), model_id)
                    .await,
            )
        }),
        "get_civitai_base_models" => Box::pin(async move {
            reply_result(crate::shared::lora_browser::get_civitai_base_models().await)
        }),
        "set_civitai_show_mature" => Box::pin(async move {
            let enabled: bool = arg(&args, "enabled")?;
            blocking(move || {
                reply_result(crate::shared::lora_browser::set_civitai_show_mature(
                    h.state::<AppState>(),
                    enabled,
                ))
            })
            .await
        }),
        "list_installed_lora_files" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply(crate::shared::lora_browser::list_installed_lora_files(
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "browse_civitai_workflows" => Box::pin(async move {
            let query: BrowseQuery = arg(&args, "query")?;
            reply_result(
                crate::shared::workflow_browser::browse_civitai_workflows(
                    h.state::<AppState>(),
                    query,
                )
                .await,
            )
        }),
        "get_civitai_workflow" => Box::pin(async move {
            let model_id: u64 = arg(&args, "modelId")?;
            reply_result(
                crate::shared::workflow_browser::get_civitai_workflow(
                    h.state::<AppState>(),
                    model_id,
                )
                .await,
            )
        }),
        "download_civitai_workflow" => Box::pin(async move {
            let model_id: u64 = arg(&args, "modelId")?;
            let version_id: u64 = arg(&args, "versionId")?;
            let file_id: u64 = arg(&args, "fileId")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::workflow_browser::download_civitai_workflow(
                    h.clone(),
                    h.state::<AppState>(),
                    model_id,
                    version_id,
                    file_id,
                    comfyui_root,
                )
                .await,
            )
        }),
        "download_civitai_lora" => Box::pin(async move {
            let model_id: u64 = arg(&args, "modelId")?;
            let version_id: u64 = arg(&args, "versionId")?;
            let file_id: Option<u64> = arg(&args, "fileId")?;
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            reply_result(
                crate::shared::lora_browser::download_civitai_lora(
                    h.clone(),
                    h.state::<AppState>(),
                    model_id,
                    version_id,
                    file_id,
                    comfyui_root,
                )
                .await,
            )
        }),
        "start_comfyui_install" => Box::pin(async move {
            let request: ComfyInstallRequest = arg(&args, "request")?;
            reply_result(
                super::install::start_comfyui_install(h.clone(), h.state::<AppState>(), request)
                    .await,
            )
        }),
        "cancel_comfyui_install" => Box::pin(async move {
            blocking(move || {
                reply_result(crate::shared::cancel_comfyui_install(h.state::<AppState>()))
            })
            .await
        }),
        "start_comfyui_root" => Box::pin(async move {
            let comfyui_root: Option<String> = arg(&args, "comfyuiRoot")?;
            blocking(move || {
                reply_result(crate::shared::start_comfyui_root(
                    h.clone(),
                    h.state::<AppState>(),
                    comfyui_root,
                ))
            })
            .await
        }),
        "stop_comfyui_root" => Box::pin(async move {
            blocking(move || {
                reply_result(crate::shared::stop_comfyui_root(
                    h.clone(),
                    h.state::<AppState>(),
                ))
            })
            .await
        }),
        "get_comfyui_runtime_status" => Box::pin(async move {
            blocking(move || {
                reply(crate::shared::get_comfyui_runtime_status(
                    h.state::<AppState>(),
                ))
            })
            .await
        }),
        "open_folder" => Box::pin(async move {
            let path: String = arg(&args, "path")?;
            blocking(move || reply_result(super::open_folder(path))).await
        }),
        "open_external_url" => Box::pin(async move {
            let url: String = arg(&args, "url")?;
            blocking(move || reply_result(super::open_external_url(url))).await
        }),
        "cancel_active_download" => Box::pin(async move {
            blocking(move || reply_result(super::cancel_active_download(h.state::<AppState>())))
                .await
        }),
        "get_tray_status" => Box::pin(async move {
            blocking(move || reply(crate::shared::tray::get_tray_status(h.state::<AppState>())))
                .await
        }),
        "tray_start_comfyui" => Box::pin(async move {
            blocking(move || {
                crate::shared::tray::tray_start_comfyui(h.clone());
                reply_unit(())
            })
            .await
        }),
        "tray_restart_comfyui" => Box::pin(async move {
            blocking(move || {
                crate::shared::tray::tray_restart_comfyui(h.clone());
                reply_unit(())
            })
            .await
        }),
        "tray_stop_comfyui" => Box::pin(async move {
            blocking(move || {
                crate::shared::tray::tray_stop_comfyui(h.clone());
                reply_unit(())
            })
            .await
        }),
        "set_app_quitting" => Box::pin(async move {
            blocking(move || {
                crate::shared::tray::set_app_quitting(h.state::<AppState>());
                reply_unit(())
            })
            .await
        }),
        _ => return None,
    })
}
