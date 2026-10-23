//! ComfyUI install orchestration and component-toggle for the Linux
//! backend: the fresh-install pipeline, the async `#[tauri::command]`
//! wrapper around it, and the enable/disable toggle for individual
//! addons/custom nodes/launch flags. This is the piece the roadmap doc
//! (`docs/cross-platform-development.md`, step 3) already flagged as
//! "genuine divergence accumulated over time, not surface duplication" when
//! it was investigated for a *shared* extraction -- that finding still
//! holds and isn't revisited here. This slice only relocates Linux's own
//! version of each function into its own file, the same as every other
//! slice; nothing about the two platforms' logic was unified or brought
//! closer together.
//!
//! Windows counterpart: `app_windows/install.rs`. Windows' `run_comfyui_install`
//! is noticeably larger (~650 lines here is ~510) -- it validates
//! SageAttention3 against RTX 50-series GPUs, branches on ROCm/XPU/CUDA
//! torch stacks, handles a legacy nested-`ComfyUI/ComfyUI` migration case,
//! and writes its own `install.log`, none of which Linux's version does at
//! all. Two differences already on record before this move (see step 3
//! above), reconfirmed here rather than fixed: `start_comfyui_install`
//! always persists `comfyui_torch_profile` here; Windows's does not set it
//! in this code path at all. `selected_attention_backend` returns `&str`
//! here; Windows' returns `Option<&str>`.

use crate::shared::{
    choose_install_folder, clear_directory_contents, emit_install_event,
    install_custom_node_and_record, is_empty_dir, is_recoverable_preclone_dir,
    normalize_optional_path, path_name_is_comfyui, recover_lock, remove_custom_node_dirs,
    stop_comfyui_for_mutation, write_install_state, write_install_summary, AppState,
    CustomNodeSpec, DownloadProgressEvent, InstallSummaryItem,
};
// `ComfyComponentToggleRequest`/`ComfyInstallRequest` are request-payload
// structs defined in the parent `app_linux` module itself (not
// `shared.rs`). General install/runtime utilities below are the same story.
use super::{
    clone_or_update_repo, custom_node_spec, detect_launch_attention_backend_for_root,
    download_http_file, enforce_torch_profile_linux, ensure_git_available,
    get_comfyui_install_recommendation, get_linux_prereq_cache_or_scan, git_latest_release_tag,
    install_comfyui_manager, install_custom_node, install_flashattention_linux,
    install_insightface, install_linux_wheel_for_profile, install_missing_linux_prereqs,
    install_named_custom_node, install_nunchaku_node_requirements, install_sageattention_linux,
    install_trellis2, is_nvidia_hopper_sm90, kill_python_processes_for_root, normalize_path,
    nunchaku_backend_present, python_for_root, refresh_linux_prereq_cache, resolve_root_path,
    resolve_uv_binary, restart_comfyui_after_mutation, run_command_env, run_command_with_retry,
    run_uv_pip_strict, selected_attention_backend, uninstall_comfyui_manager,
    uninstall_insightface, uninstall_trellis2, write_extra_model_paths_yaml,
    ComfyComponentToggleRequest, ComfyInstallRequest, CUSTOM_NODES, UV_PYTHON_VERSION,
};
use crate::host::{AppHandle, State};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

fn run_comfyui_install(
    app: &AppHandle,
    request: &ComfyInstallRequest,
    shared_runtime_root: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    run_comfyui_install_linux(app, request, shared_runtime_root, cancel)
}
fn run_comfyui_install_linux(
    app: &AppHandle,
    request: &ComfyInstallRequest,
    shared_runtime_root: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    let mut summary: Vec<InstallSummaryItem> = Vec::new();
    let include_insight_face = request.include_insight_face || request.include_nunchaku;
    let selected_attention = [
        request.include_sage_attention,
        request.include_sage_attention3,
        request.include_flash_attention,
        request.include_nunchaku,
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if selected_attention > 1 {
        return Err(
            "Choose only one of SageAttention, SageAttention3, FlashAttention, or Nunchaku."
                .to_string(),
        );
    }

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }

    let base_root = normalize_path(&request.install_root)?;
    let extra_model_root = normalize_optional_path(request.extra_model_root.as_deref())?;
    let selected_comfy_root = path_name_is_comfyui(&base_root);
    let comfy_dir = if selected_comfy_root {
        base_root.clone()
    } else {
        choose_install_folder(&base_root, request.force_fresh)
    };
    let install_root = comfy_dir.clone();

    std::fs::create_dir_all(&install_root).map_err(|err| err.to_string())?;
    write_install_state(&install_root, "in_progress", "init");
    emit_install_event(
        app,
        "info",
        &format!("Install folder selected: {}", install_root.display()),
    );

    let mut scan = get_linux_prereq_cache_or_scan()?;
    let distro = scan.distro.clone();
    emit_install_event(
        app,
        "step",
        &format!("Detected Linux distribution family: {distro}."),
    );
    write_install_state(&install_root, "in_progress", "linux_packages");
    if scan.missing_required.is_empty() && scan.missing_optional.is_empty() {
        emit_install_event(app, "info", "Linux system prerequisites already installed.");
    } else {
        emit_install_event(
            app,
            "step",
            &format!(
                "Installing missing Linux prerequisites for {}...",
                scan.distro
            ),
        );
        install_missing_linux_prereqs(&scan)?;
        scan = refresh_linux_prereq_cache()?;
        if !scan.missing_required.is_empty() {
            return Err(format!(
                "Required Linux packages are still missing after install attempt: {}",
                scan.missing_required.join(", ")
            ));
        }
    }

    ensure_git_available(app)?;
    if !comfy_dir.join("main.py").exists() {
        write_install_state(&install_root, "in_progress", "clone_comfyui");
        emit_install_event(app, "step", "Cloning ComfyUI...");
        if comfy_dir.exists() && !is_empty_dir(&comfy_dir) {
            if is_recoverable_preclone_dir(&comfy_dir) {
                clear_directory_contents(&comfy_dir)?;
            } else {
                return Err(format!(
                    "Selected ComfyUI folder already exists and is not empty: {}. Choose a new base folder or remove existing files.",
                    comfy_dir.display()
                ));
            }
        }
        run_command_with_retry(
            "git",
            &[
                "clone",
                "https://github.com/Comfy-Org/ComfyUI.git",
                &comfy_dir.to_string_lossy(),
            ],
            Some(&install_root),
            2,
        )?;
        // Pin fresh installs to latest release tag so users do not see an
        // immediate update prompt after a clean install.
        if let Some((latest_tag, latest_version)) = git_latest_release_tag(&comfy_dir) {
            if let Err(err) = run_command_with_retry(
                "git",
                &["checkout", "-B", "master", &latest_tag],
                Some(&comfy_dir),
                1,
            ) {
                emit_install_event(
                    app,
                    "warn",
                    &format!(
                        "ComfyUI cloned, but failed to pin to release tag {} (v{}): {}",
                        latest_tag, latest_version, err
                    ),
                );
            } else {
                emit_install_event(
                    app,
                    "info",
                    &format!(
                        "Pinned fresh ComfyUI install to latest release tag {} (v{}).",
                        latest_tag, latest_version
                    ),
                );
            }
        } else {
            emit_install_event(
                app,
                "warn",
                "ComfyUI cloned, but latest release tag could not be resolved during install.",
            );
        }
        summary.push(InstallSummaryItem {
            name: "ComfyUI core".to_string(),
            status: "ok".to_string(),
            detail: "ComfyUI cloned successfully.".to_string(),
        });
    } else {
        summary.push(InstallSummaryItem {
            name: "ComfyUI core".to_string(),
            status: "skipped".to_string(),
            detail: "Existing ComfyUI folder reused.".to_string(),
        });
    }

    if let Some(extra_root) = extra_model_root.as_ref() {
        write_install_state(&install_root, "in_progress", "extra_model_paths");
        emit_install_event(
            app,
            "step",
            &format!(
                "Configuring ComfyUI extra model paths from {}...",
                extra_root.display()
            ),
        );
        let config_path =
            write_extra_model_paths_yaml(&comfy_dir, extra_root, request.extra_model_use_default)?;
        summary.push(InstallSummaryItem {
            name: "extra_model_paths".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "Configured {} with base path {}.",
                config_path.display(),
                extra_root.display()
            ),
        });
    }

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }

    write_install_state(&install_root, "in_progress", "python_venv");
    emit_install_event(app, "step", "Preparing uv-managed Python + local .venv...");
    let uv_bin = resolve_uv_binary(shared_runtime_root, app)?;
    let python_store = shared_runtime_root.join(".python");
    std::fs::create_dir_all(&python_store).map_err(|err| err.to_string())?;
    let python_store_s = python_store.to_string_lossy().to_string();
    run_command_env(
        &uv_bin,
        &["python", "install", UV_PYTHON_VERSION],
        Some(&comfy_dir),
        &[
            ("UV_PYTHON_INSTALL_DIR", &python_store_s),
            ("UV_PYTHON_INSTALL_BIN", "false"),
        ],
    )?;

    let venv_dir = comfy_dir.join(".venv");
    let py_exe = venv_dir.join("bin").join("python");
    if !py_exe.exists() {
        let venv_s = venv_dir.to_string_lossy().to_string();
        run_command_env(
            &uv_bin,
            &["venv", "--seed", "--python", UV_PYTHON_VERSION, &venv_s],
            Some(&comfy_dir),
            &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
        )?;
    } else {
        emit_install_event(app, "step", "Existing .venv found; reusing.");
    }
    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &["install", "--upgrade", "pip", "setuptools", "wheel"],
        Some(&comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
    )?;

    let recommendation = get_comfyui_install_recommendation(None);
    let selected_profile = request
        .torch_profile
        .clone()
        .unwrap_or(recommendation.torch_profile);
    let hopper_sm90 = is_nvidia_hopper_sm90();
    write_install_state(&install_root, "in_progress", "torch_stack");
    emit_install_event(app, "step", "Installing Torch stack...");
    enforce_torch_profile_linux(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &comfy_dir,
        &selected_profile,
        &python_store_s,
    )?;

    write_install_state(&install_root, "in_progress", "comfy_requirements");
    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &[
            "install",
            "-r",
            &comfy_dir.join("requirements.txt").to_string_lossy(),
        ],
        Some(&comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
    )?;
    // Re-apply selected torch stack because requirements can drift torch/torchvision.
    enforce_torch_profile_linux(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &comfy_dir,
        &selected_profile,
        &python_store_s,
    )?;
    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &["install", "--upgrade", "pyyaml", "nvidia-ml-py"],
        Some(&comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
    )?;

    let addon_root = comfy_dir.join("custom_nodes");
    std::fs::create_dir_all(&addon_root).map_err(|err| err.to_string())?;

    if request.include_sage_attention {
        write_install_state(&install_root, "in_progress", "addon_sageattention");
        emit_install_event(app, "step", "Installing SageAttention...");
        install_sageattention_linux(
            &comfy_dir,
            &py_exe.to_string_lossy(),
            &selected_profile,
            hopper_sm90,
        )?;
    }
    if include_insight_face {
        write_install_state(&install_root, "in_progress", "addon_insightface");
        if request.include_nunchaku && !request.include_insight_face {
            emit_install_event(
                app,
                "step",
                "Installing InsightFace (required by Nunchaku)...",
            );
        } else {
            emit_install_event(app, "step", "Installing InsightFace...");
        }
        install_insightface(
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
        )?;
    }

    if request.include_flash_attention {
        write_install_state(&install_root, "in_progress", "addon_flashattention");
        emit_install_event(app, "step", "Installing FlashAttention...");
        install_flashattention_linux(
            &comfy_dir,
            &py_exe.to_string_lossy(),
            &selected_profile,
            hopper_sm90,
        )?;
        summary.push(InstallSummaryItem {
            name: "flash-attention".to_string(),
            status: "ok".to_string(),
            detail: "Installed using Linux wheel stack.".to_string(),
        });
    }
    if request.include_sage_attention3 {
        write_install_state(&install_root, "in_progress", "addon_sageattention3");
        emit_install_event(app, "step", "Installing SageAttention3...");
        install_linux_wheel_for_profile(
            &comfy_dir,
            &py_exe.to_string_lossy(),
            &selected_profile,
            "sage3",
            hopper_sm90,
            true,
        )?;
        // Keep sageattention installed for ComfyUI --use-sage-attention compatibility checks.
        install_sageattention_linux(
            &comfy_dir,
            &py_exe.to_string_lossy(),
            &selected_profile,
            hopper_sm90,
        )?;
        summary.push(InstallSummaryItem {
            name: "sageattention3".to_string(),
            status: "ok".to_string(),
            detail: "Installed using Linux wheel stack.".to_string(),
        });
    }
    if request.include_nunchaku {
        write_install_state(&install_root, "in_progress", "addon_nunchaku");
        emit_install_event(app, "step", "Installing Nunchaku...");
        ensure_git_available(app)?;
        std::fs::create_dir_all(&addon_root).map_err(|err| err.to_string())?;
        let nunchaku_node = addon_root.join("ComfyUI-nunchaku");
        for folder in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
            let path = addon_root.join(folder);
            if path.exists() {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        clone_or_update_repo(
            &comfy_dir,
            &nunchaku_node,
            "https://github.com/nunchaku-ai/ComfyUI-nunchaku",
        )?;
        let versions_json = nunchaku_node.join("nunchaku_versions.json");
        let _ = download_http_file(
            "https://nunchaku.tech/cdn/nunchaku_versions.json",
            &versions_json,
        );
        install_nunchaku_node_requirements(
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
            &nunchaku_node,
        )?;
        install_linux_wheel_for_profile(
            &comfy_dir,
            &py_exe.to_string_lossy(),
            &selected_profile,
            "nunchaku",
            hopper_sm90,
            true,
        )?;
        if !nunchaku_backend_present(&comfy_dir) {
            return Err(
                "Nunchaku install incomplete: module or custom node not detected after install."
                    .to_string(),
            );
        }
        summary.push(InstallSummaryItem {
            name: "nunchaku".to_string(),
            status: "ok".to_string(),
            detail: "Installed Linux nunchaku wheel and ComfyUI-nunchaku node.".to_string(),
        });
    }
    if request.include_trellis2 {
        write_install_state(&install_root, "in_progress", "addon_trellis2");
        emit_install_event(app, "step", "Installing Trellis2...");
        let custom_nodes_dir = comfy_dir.join("custom_nodes");
        std::fs::create_dir_all(&custom_nodes_dir).map_err(|err| err.to_string())?;
        let trellis_dir = custom_nodes_dir.join("ComfyUI-TRELLIS2");
        clone_or_update_repo(
            &comfy_dir,
            &trellis_dir,
            "https://github.com/ArcticLatent/ComfyUI-TRELLIS2",
        )?;
        let trellis_req = trellis_dir.join("requirements.txt");
        if trellis_req.exists() {
            run_uv_pip_strict(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &["install", "-r", &trellis_req.to_string_lossy()],
                Some(&comfy_dir),
                &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
            )?;
        }

        let geometry_dir = custom_nodes_dir.join("ComfyUI-GeometryPack");
        clone_or_update_repo(
            &comfy_dir,
            &geometry_dir,
            "https://github.com/PozzettiAndrea/ComfyUI-GeometryPack",
        )?;
        let geometry_req = geometry_dir.join("requirements.txt");
        if geometry_req.exists() {
            run_uv_pip_strict(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &["install", "-r", &geometry_req.to_string_lossy()],
                Some(&comfy_dir),
                &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
            )?;
        }
        run_uv_pip_strict(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &["install", "--upgrade", "tomli"],
            Some(&comfy_dir),
            &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
        )?;

        let ultrashape_dir = custom_nodes_dir.join("ComfyUI-UltraShape1");
        clone_or_update_repo(
            &comfy_dir,
            &ultrashape_dir,
            "https://github.com/jtydhr88/ComfyUI-UltraShape1",
        )?;
        let ultrashape_req = ultrashape_dir.join("requirements.txt");
        if ultrashape_req.exists() {
            run_uv_pip_strict(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &["install", "-r", &ultrashape_req.to_string_lossy()],
                Some(&ultrashape_dir),
                &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
            )?;
            run_uv_pip_strict(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &["install", "-U", "accelerate"],
                Some(&ultrashape_dir),
                &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
            )?;
        }

        let ultrashape_models_dir = comfy_dir.join("models").join("UltraShape");
        std::fs::create_dir_all(&ultrashape_models_dir).map_err(|err| err.to_string())?;
        let ultrashape_model_file = ultrashape_models_dir.join("ultrashape_v1.pt");
        if !ultrashape_model_file.exists() {
            download_http_file(
                "https://huggingface.co/infinith/UltraShape/resolve/main/ultrashape_v1.pt",
                &ultrashape_model_file,
            )?;
        }
        summary.push(InstallSummaryItem {
            name: "trellis2".to_string(),
            status: "ok".to_string(),
            detail: "Installed TRELLIS2 + GeometryPack + UltraShape1 Linux flow.".to_string(),
        });
    }

    let requested_custom_nodes: [(bool, &CustomNodeSpec); 6] = [
        (request.node_comfyui_manager, &CUSTOM_NODES[0]),
        (request.node_comfyui_easy_use, &CUSTOM_NODES[1]),
        (request.node_rgthree_comfy, &CUSTOM_NODES[2]),
        (request.node_comfyui_gguf, &CUSTOM_NODES[3]),
        (request.node_comfyui_kjnodes, &CUSTOM_NODES[4]),
        (request.node_comfyui_crystools, &CUSTOM_NODES[5]),
    ];
    for (requested, spec) in requested_custom_nodes {
        if !requested {
            continue;
        }
        install_custom_node_and_record(app, &install_root, spec, &mut summary, || {
            if spec.flag_key == "node_comfyui_manager" {
                return install_comfyui_manager(app, &comfy_dir, &py_exe).map(|_| ());
            }
            install_custom_node(
                app,
                &comfy_dir,
                &addon_root,
                &py_exe,
                spec.repo_url,
                spec.install_folder_name,
            )
        });
    }

    // Final guard: custom-node requirements can drift torch deps.
    // Re-assert the selected stack before first launch.
    write_install_state(&install_root, "in_progress", "finalize_torch_stack");
    emit_install_event(
        app,
        "step",
        "Finalizing Torch stack for selected profile...",
    );
    enforce_torch_profile_linux(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &comfy_dir,
        &selected_profile,
        &python_store_s,
    )?;

    write_install_summary(&install_root, &summary);
    write_install_state(&install_root, "completed", "done");
    Ok(comfy_dir)
}

pub(crate) async fn start_comfyui_install(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ComfyInstallRequest,
) -> Result<(), String> {
    {
        let mut active = recover_lock(state.install_cancel.lock());
        if active.is_some() {
            return Err("ComfyUI installation is already active.".to_string());
        }
        *active = Some(CancellationToken::new());
    }

    let cancel = recover_lock(state.install_cancel.lock())
        .as_ref()
        .cloned()
        .ok_or_else(|| "Failed to initialize install cancellation token.".to_string())?;
    let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");

    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = run_comfyui_install(&app_for_task, &request, &shared_runtime_root, &cancel);
        match result {
            Ok(comfy_root) => {
                let install_dir = comfy_root
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| comfy_root.clone());
                let managed = app_for_task.state::<AppState>();
                let normalized_shared_models =
                    normalize_optional_path(request.extra_model_root.as_deref())
                        .ok()
                        .flatten();
                let _ = managed.context.config.update_settings(|settings| {
                    settings.comfyui_root = Some(comfy_root.clone());
                    settings.comfyui_last_install_dir = Some(install_dir.clone());
                    settings.comfyui_pinned_memory_enabled = request.include_pinned_memory;
                    settings.comfyui_torch_profile =
                        Some(request.torch_profile.clone().unwrap_or_else(|| {
                            get_comfyui_install_recommendation(None).torch_profile
                        }));
                    settings.comfyui_attention_backend =
                        Some(selected_attention_backend(&request).to_string());
                    settings.shared_models_root = normalized_shared_models.clone();
                    settings.shared_models_use_default = normalized_shared_models
                        .as_ref()
                        .is_some_and(|_| request.extra_model_use_default);
                });
                let _ = app_for_task.emit(
                    "comfyui-install-progress",
                    DownloadProgressEvent {
                        kind: "comfyui_install".to_string(),
                        phase: "finished".to_string(),
                        artifact: Some(install_dir.to_string_lossy().to_string()),
                        index: None,
                        total: None,
                        received: None,
                        size: None,
                        folder: Some(comfy_root.to_string_lossy().to_string()),
                        message: Some(format!(
                            "ComfyUI installation completed. Root set to {}",
                            comfy_root.display()
                        )),
                    },
                );
            }
            Err(err) => emit_install_event(&app_for_task, "failed", &err),
        }
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.install_cancel.lock()) = None;
    });

    Ok(())
}

pub(crate) async fn apply_comfyui_component_toggle(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ComfyComponentToggleRequest,
) -> Result<String, String> {
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let component = request.component.trim().to_ascii_lowercase();

    let result = if matches!(
        component.as_str(),
        "addon_pinned_memory"
            | "pinned_memory"
            | "launch_listen"
            | "addon_launch_listen"
            | "launch_lowvram"
            | "addon_launch_lowvram"
            | "launch_bf16_unet"
            | "addon_launch_bf16_unet"
            | "launch_async_offload"
            | "addon_launch_async_offload"
            | "launch_disable_smart_memory"
            | "addon_launch_disable_smart_memory"
            | "launch_disable_dynamic_vram"
            | "launch_fast_disk"
    ) {
        match component.as_str() {
            "addon_pinned_memory" | "pinned_memory" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_pinned_memory_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("Pinned memory enabled.".to_string())
                } else {
                    Ok("Pinned memory disabled.".to_string())
                }
            }
            "launch_listen" | "addon_launch_listen" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_listen_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --listen enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --listen.".to_string())
                }
            }
            "launch_lowvram" | "addon_launch_lowvram" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_lowvram_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --lowvram enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --lowvram.".to_string())
                }
            }
            "launch_bf16_unet" | "addon_launch_bf16_unet" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_bf16_unet_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --bf16-unet enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --bf16-unet.".to_string())
                }
            }
            "launch_async_offload" | "addon_launch_async_offload" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_async_offload_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --async-offload enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --async-offload.".to_string())
                }
            }
            "launch_disable_smart_memory" | "addon_launch_disable_smart_memory" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| {
                        settings.comfyui_disable_smart_memory_enabled = enabled
                    })
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --disable-smart-memory enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --disable-smart-memory.".to_string())
                }
            }
            "launch_disable_dynamic_vram" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| {
                        settings.comfyui_disable_dynamic_vram_enabled = enabled
                    })
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --disable-dynamic-vram enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --disable-dynamic-vram.".to_string())
                }
            }
            "launch_fast_disk" => {
                let enabled = request.enabled;
                state
                    .context
                    .config
                    .update_settings(|settings| settings.comfyui_fast_disk_enabled = enabled)
                    .map_err(|err| err.to_string())?;
                if enabled {
                    Ok("ComfyUI will start with --fast-disk enabled.".to_string())
                } else {
                    Ok("ComfyUI will start without --fast-disk.".to_string())
                }
            }
            _ => Err("Unknown component toggle target.".to_string()),
        }
    } else {
        let root = resolve_root_path(&state.context, request.comfyui_root)?;
        let py_path = {
            let probe = python_for_root(&root);
            probe.get_program().to_string_lossy().to_string()
        };
        let py_exe = PathBuf::from(&py_path);
        let _ = kill_python_processes_for_root(&root, &py_exe);

        let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
        let uv_bin = resolve_uv_binary(&shared_runtime_root, &app)?;
        let uv_python_install_dir = shared_runtime_root
            .join(".python")
            .to_string_lossy()
            .to_string();
        let app_clone = app.clone();
        let root_clone = root.clone();
        let py_path_clone = py_path.clone();
        let py_exe_clone = py_exe.clone();
        let component_clone = component.clone();
        let uv_bin_clone = uv_bin.clone();
        let uv_python_install_dir_clone = uv_python_install_dir.clone();
        let enabled = request.enabled;
        crate::host::async_runtime::spawn_blocking(move || -> Result<String, String> {
            match component_clone.as_str() {
                "addon_insightface" | "insightface" => {
                    if enabled {
                        install_insightface(
                            &root_clone,
                            &uv_bin_clone,
                            &py_path_clone,
                            &uv_python_install_dir_clone,
                        )?;
                        Ok("Installed InsightFace.".to_string())
                    } else {
                        if detect_launch_attention_backend_for_root(&root_clone).as_deref()
                            == Some("nunchaku")
                        {
                            return Err(
                                "Cannot remove InsightFace while Nunchaku is selected. Switch attention backend first."
                                    .to_string(),
                            );
                        }
                        uninstall_insightface(
                            &root_clone,
                            &uv_bin_clone,
                            &py_path_clone,
                            &uv_python_install_dir_clone,
                        )?;
                        Ok("Removed InsightFace.".to_string())
                    }
                }
                "addon_trellis2" | "trellis2" => {
                    if enabled {
                        ensure_git_available(&app_clone)?;
                        install_trellis2(
                            &root_clone,
                            &uv_bin_clone,
                            &py_path_clone,
                            &uv_python_install_dir_clone,
                        )?;
                        Ok("Installed Trellis2.".to_string())
                    } else {
                        uninstall_trellis2(
                            &root_clone,
                            &uv_bin_clone,
                            &py_path_clone,
                            &uv_python_install_dir_clone,
                        )?;
                        Ok("Removed Trellis2.".to_string())
                    }
                }
                key @ ("node_comfyui_manager" | "node_comfyui_easy_use" | "node_rgthree_comfy"
                    | "node_comfyui_gguf" | "node_comfyui_kjnodes" | "node_comfyui_crystools") => {
                    let spec = custom_node_spec(key)
                        .expect("key matched one of CUSTOM_NODES' flag_key values above");
                    if key == "node_comfyui_manager" {
                        return if enabled {
                            ensure_git_available(&app_clone)?;
                            install_comfyui_manager(&app_clone, &root_clone, &py_exe_clone)
                        } else {
                            uninstall_comfyui_manager(&app_clone, &root_clone, &py_exe_clone)?;
                            Ok(format!("Removed {}.", spec.display_name))
                        };
                    }
                    if enabled {
                        ensure_git_available(&app_clone)?;
                        install_named_custom_node(
                            &app_clone,
                            &root_clone,
                            &py_exe_clone,
                            spec.repo_url,
                            spec.install_folder_name,
                        )?;
                        Ok(format!("Installed {}.", spec.display_name))
                    } else {
                        remove_custom_node_dirs(&root_clone, spec.known_folder_names);
                        Ok(format!("Removed {}.", spec.display_name))
                    }
                }
                _ => Err("Unknown component toggle target.".to_string()),
            }
        })
        .await
        .map_err(|err| format!("Component operation task failed: {err}"))?
    }?;

    restart_comfyui_after_mutation(&app, &state, was_running)?;
    Ok(result)
}
