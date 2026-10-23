//! ComfyUI install orchestration and component-toggle for the Windows
//! backend. Windows counterpart to `app_linux/install.rs` -- see that
//! file's doc comment for the scope rationale (this slice only relocates
//! each platform's own version of these functions; nothing was unified).
//!
//! `run_comfyui_install` here is noticeably larger than Linux's (~650 lines
//! vs ~510): it validates SageAttention3 against RTX 50-series GPUs,
//! branches on ROCm/XPU/CUDA torch stacks, handles a legacy nested
//! `ComfyUI/ComfyUI` migration case, and writes its own `install.log`, none
//! of which Linux's version does. Two differences already on record before
//! this move (`docs/cross-platform-development.md`, step 3), reconfirmed
//! here rather than fixed: `start_comfyui_install` uses
//! `spawn_blocking` (Linux uses `spawn`, with a comment here explaining
//! why) and never sets `comfyui_torch_profile` in this code path (Linux
//! always does). `selected_attention_backend` returns `Option<&str>` here;
//! Linux's returns `&str`. `apply_comfyui_component_toggle` also carries
//! Windows-only CUDA-only-addon validation and a Trellis2 torch-profile
//! check that Linux's doesn't have at all.

use crate::shared::{
    choose_install_folder, clear_directory_contents, custom_node_exists, emit_install_event,
    install_custom_node_and_record, is_empty_dir, is_recoverable_preclone_dir,
    normalize_optional_path, path_name_is_comfyui, recover_lock, remove_custom_node_dirs,
    stop_comfyui_for_mutation, write_install_state, write_install_summary, AppState,
    CustomNodeSpec, DownloadProgressEvent, InstallSummaryItem,
};
// `ComfyComponentToggleRequest`/`ComfyInstallRequest` are request-payload
// structs, and everything else below is a general install/runtime utility,
// defined in the parent `app_windows` module itself (not `shared.rs`).
use super::{
    attention_wheel_url, custom_node_spec, detect_torch_profile_for_root, ensure_git_available,
    ensure_insightface_runtime_compat, ensure_uv_python_installed, ensure_venv_pip,
    finalize_nunchaku_install, get_comfyui_install_recommendation, git_latest_release_tag,
    install_comfyui_manager, install_custom_node, install_insightface, install_named_custom_node,
    install_nunchaku_node_requirements, install_trellis2, install_wheel_no_deps,
    is_forbidden_install_path, is_non_cuda_profile, is_rocm_profile, is_xpu_profile,
    kill_python_processes_for_root, normalize_path, pip_has_package, profile_from_torch_env,
    python_for_root, reassert_torch_stack_for_profile, resolve_root_path, resolve_uv_binary,
    restart_comfyui_after_mutation, run_command, run_command_with_retry, run_uv_pip_strict,
    selected_attention_backend, uninstall_comfyui_manager, uninstall_insightface,
    uninstall_trellis2, uv_pip_uninstall_best_effort, write_extra_model_paths_yaml,
    ComfyComponentToggleRequest, ComfyInstallRequest, CUSTOM_NODES,
};
use crate::host::{AppHandle, State};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

fn run_comfyui_install(
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
    if request.include_sage_attention3 {
        let gpu = super::detect_nvidia_gpu_details();
        let is_50_series = gpu
            .name
            .as_deref()
            .map(|name| name.to_ascii_lowercase().contains("rtx 50"))
            .unwrap_or(false);
        if !is_50_series {
            return Err(
                "SageAttention3 is available only for NVIDIA RTX 50-series GPUs.".to_string(),
            );
        }
    }

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }

    let base_root = normalize_path(&request.install_root)?;
    let extra_model_root = normalize_optional_path(request.extra_model_root.as_deref())?;
    if is_forbidden_install_path(&base_root) {
        return Err(
            "Install folder is not allowed. Avoid C:\\, Windows, or Program Files.".to_string(),
        );
    }
    let selected_comfy_root = path_name_is_comfyui(&base_root);
    let mut comfy_dir = if selected_comfy_root {
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

    let log_path = install_root.join("install.log");
    let mut log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|err| err.to_string())?;
    let _ = writeln!(log_file, "Starting install");

    let recommendation = get_comfyui_install_recommendation(None);
    let selected_profile = request
        .torch_profile
        .clone()
        .unwrap_or(recommendation.torch_profile);
    if is_non_cuda_profile(&selected_profile)
        && (request.include_sage_attention
            || request.include_sage_attention3
            || request.include_flash_attention
            || request.include_nunchaku
            || request.include_trellis2)
    {
        return Err(
            "SageAttention, SageAttention3, FlashAttention, Nunchaku, and Trellis2 are CUDA-only and are not available with the Windows ROCm/XPU profiles."
                .to_string(),
        );
    }
    if request.include_trellis2 && !matches!(selected_profile.as_str(), "torch280_cu128") {
        return Err(
            "Trellis2 currently requires Torch 2.8.0 + cu128 (Torch280 wheel set).".to_string(),
        );
    }
    emit_install_event(
        app,
        "info",
        &format!("Using {} ({})", selected_profile, recommendation.reason),
    );

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }
    ensure_git_available(app)?;
    // Migration fallback: older builds sometimes created ComfyUI/ComfyUI.
    let nested_legacy = comfy_dir.join("ComfyUI").join("main.py");
    if !comfy_dir.join("main.py").exists() && nested_legacy.exists() {
        comfy_dir = comfy_dir.join("ComfyUI");
        emit_install_event(
            app,
            "info",
            &format!(
                "Detected existing nested ComfyUI; using {}",
                comfy_dir.display()
            ),
        );
    }

    if !comfy_dir.join("main.py").exists() {
        write_install_state(&install_root, "in_progress", "clone_comfyui");
        emit_install_event(app, "step", "Cloning ComfyUI...");
        if comfy_dir.exists() && !is_empty_dir(&comfy_dir) {
            if is_recoverable_preclone_dir(&comfy_dir) {
                emit_install_event(
                    app,
                    "info",
                    "Cleaning previous partial install artifacts before clone...",
                );
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
                "https://github.com/Comfy-Org/ComfyUI",
                &comfy_dir.to_string_lossy(),
            ],
            Some(&install_root),
            2,
        )?;
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
        emit_install_event(
            app,
            "step",
            "ComfyUI folder already exists, skipping clone.",
        );
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
    emit_install_event(app, "step", "Preparing uv-managed Python + local .venv...");
    emit_install_event(
        app,
        "info",
        &format!("Shared uv runtime path: {}", shared_runtime_root.display()),
    );
    write_install_state(&install_root, "in_progress", "python_venv");
    let uv_bin = resolve_uv_binary(shared_runtime_root, app)?;
    let python_store = shared_runtime_root.join(".python");
    std::fs::create_dir_all(&python_store).map_err(|err| err.to_string())?;
    let python_store_s = python_store.to_string_lossy().to_string();
    let resolved_python = ensure_uv_python_installed(&uv_bin, Some(&comfy_dir), &python_store_s)?;

    let venv_dir = comfy_dir.join(".venv");
    let py_exe = venv_dir.join("Scripts").join("python.exe");
    if !py_exe.exists() {
        let venv_s = venv_dir.to_string_lossy().to_string();
        super::run_command_env(
            &uv_bin,
            &["venv", "--seed", "--python", &resolved_python, &venv_s],
            Some(&comfy_dir),
            &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
        )?;
    } else {
        emit_install_event(app, "step", "Existing .venv found; reusing.");
    }

    emit_install_event(app, "step", "Verifying uv pip in local .venv...");
    ensure_venv_pip(&uv_bin, &py_exe, &comfy_dir, &python_store_s)?;

    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &[
            "install",
            "--upgrade",
            "pip",
            "setuptools",
            "wheel",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(&comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
    )?;

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }
    emit_install_event(app, "step", "Installing Torch stack...");
    write_install_state(&install_root, "in_progress", "torch_stack");
    // Installs, verifies and pins the stack (the same steps a later reassert
    // takes when the stack doesn't match).
    reassert_torch_stack_for_profile(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &comfy_dir,
        &python_store_s,
        &selected_profile,
    )?;

    if cancel.is_cancelled() {
        return Err("Installation cancelled.".to_string());
    }
    emit_install_event(app, "step", "Installing ComfyUI requirements...");
    write_install_state(&install_root, "in_progress", "comfy_requirements");
    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &[
            "install",
            "-r",
            &comfy_dir.join("requirements.txt").to_string_lossy(),
            "--no-cache",
        ],
        Some(&comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
    )?;
    if is_rocm_profile(&selected_profile) || is_xpu_profile(&selected_profile) {
        run_uv_pip_strict(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &[
                "install",
                "onnxruntime",
                "onnx",
                "stringzilla==3.12.6",
                "transformers==4.57.6",
                "--no-cache-dir",
                "--timeout=1000",
                "--retries",
                "10",
            ],
            Some(&comfy_dir),
            &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
        )?;
    } else {
        run_uv_pip_strict(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &[
                "install",
                "onnxruntime-gpu",
                "onnx",
                "stringzilla==3.12.6",
                "transformers==4.57.6",
                "--no-cache-dir",
                "--timeout=1000",
                "--retries",
                "10",
            ],
            Some(&comfy_dir),
            &[("UV_PYTHON_INSTALL_DIR", &python_store_s)],
        )?;
    }

    let addon_root = comfy_dir.join("custom_nodes");
    std::fs::create_dir_all(&addon_root).map_err(|err| err.to_string())?;

    // Keep only the selected high-performance attention backend by uninstalling others first.
    let selected_attention_choice = if request.include_nunchaku {
        Some("nunchaku")
    } else if request.include_sage_attention || request.include_sage_attention3 {
        Some("sage")
    } else if request.include_flash_attention {
        Some("flash")
    } else {
        None
    };
    if selected_attention_choice.is_some() {
        write_install_state(&install_root, "in_progress", "cleanup_attention_backends");
        emit_install_event(
            app,
            "step",
            "Cleaning previous attention backend packages...",
        );
        uv_pip_uninstall_best_effort(
            &uv_bin,
            &py_exe,
            &comfy_dir,
            &python_store_s,
            &[
                "sageattention",
                "sageattn3",
                "flash-attn",
                "flash_attn",
                "nunchaku",
            ],
        )?;
        if !request.include_nunchaku {
            for folder in ["nunchaku_nodes", "ComfyUI-nunchaku"] {
                let nunchaku_node = addon_root.join(folder);
                if nunchaku_node.exists() {
                    let _ = std::fs::remove_dir_all(nunchaku_node);
                }
            }
        }
    }

    if request.include_nunchaku {
        write_install_state(&install_root, "in_progress", "addon_nunchaku");
        emit_install_event(app, "step", "Installing Nunchaku...");
        let nunchaku_node = addon_root.join("ComfyUI-nunchaku");
        for folder in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
            let stale = addon_root.join(folder);
            if stale.exists() {
                let _ = std::fs::remove_dir_all(stale);
            }
        }
        run_command(
            "git",
            &[
                "clone",
                "https://github.com/nunchaku-ai/ComfyUI-nunchaku",
                &nunchaku_node.to_string_lossy(),
            ],
            Some(&comfy_dir),
        )?;

        let nunchaku_whl = attention_wheel_url(&selected_profile, "nunchaku").ok_or_else(|| {
            format!("No Nunchaku wheel available for torch profile {selected_profile}")
        })?;
        install_wheel_no_deps(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &comfy_dir,
            &python_store_s,
            nunchaku_whl,
            true,
        )?;
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
                app,
                &comfy_dir,
                &uv_bin,
                &py_exe.to_string_lossy(),
                &python_store_s,
            )?;
        }
        install_nunchaku_node_requirements(
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
            &nunchaku_node,
        )?;
        emit_install_event(
            app,
            "step",
            "Reasserting CUDA Torch stack after Nunchaku dependencies...",
        );
        reassert_torch_stack_for_profile(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &comfy_dir,
            &python_store_s,
            &selected_profile,
        )?;

        finalize_nunchaku_install(
            app,
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
            &nunchaku_node,
        )?;
    }
    if request.include_trellis2 {
        write_install_state(&install_root, "in_progress", "addon_trellis2");
        emit_install_event(app, "step", "Installing Trellis2...");
        install_trellis2(
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
        )?;
    }
    if request.include_sage_attention {
        write_install_state(&install_root, "in_progress", "addon_sageattention");
        emit_install_event(app, "step", "Installing SageAttention...");
        let whl = attention_wheel_url(&selected_profile, "sage").ok_or_else(|| {
            format!("No SageAttention wheel available for torch profile {selected_profile}")
        })?;
        install_wheel_no_deps(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &comfy_dir,
            &python_store_s,
            whl,
            true,
        )?;
    }
    if request.include_sage_attention3 {
        write_install_state(&install_root, "in_progress", "addon_sageattention3");
        emit_install_event(app, "step", "Installing SageAttention3...");
        let whl = attention_wheel_url(&selected_profile, "sage3").ok_or_else(|| {
            format!("No SageAttention3 wheel available for torch profile {selected_profile}")
        })?;

        install_wheel_no_deps(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &comfy_dir,
            &python_store_s,
            whl,
            false,
        )?;
        if let Some(sage_whl) = attention_wheel_url(&selected_profile, "sage") {
            install_wheel_no_deps(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &comfy_dir,
                &python_store_s,
                sage_whl,
                true,
            )?;
        }
    }
    if request.include_flash_attention {
        write_install_state(&install_root, "in_progress", "addon_flashattention");
        emit_install_event(app, "step", "Installing FlashAttention...");
        let whl = attention_wheel_url(&selected_profile, "flash").ok_or_else(|| {
            format!("No FlashAttention wheel available for torch profile {selected_profile}")
        })?;
        install_wheel_no_deps(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &comfy_dir,
            &python_store_s,
            whl,
            false,
        )?;
    }
    if include_insight_face && !request.include_nunchaku {
        write_install_state(&install_root, "in_progress", "addon_insightface");
        emit_install_event(app, "step", "Installing InsightFace...");
        install_insightface(
            app,
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
        )?;
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

    if include_insight_face || request.include_nunchaku {
        emit_install_event(
            app,
            "step",
            "Finalizing InsightFace runtime compatibility...",
        );
        ensure_insightface_runtime_compat(
            &comfy_dir,
            &uv_bin,
            &py_exe.to_string_lossy(),
            &python_store_s,
        )?;
    }

    emit_install_event(
        app,
        "step",
        "Reasserting selected Torch stack after add-on and custom-node installs...",
    );
    reassert_torch_stack_for_profile(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &comfy_dir,
        &python_store_s,
        &selected_profile,
    )?;

    write_install_summary(&install_root, &summary);
    let failed_count = summary.iter().filter(|x| x.status == "failed").count();
    if failed_count > 0 {
        emit_install_event(
            app,
            "warn",
            &format!(
                "Install completed with {failed_count} custom-node failures. See install-summary.json."
            ),
        );
    }

    let _attention_backend = selected_attention_backend(request);

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
    // `run_comfyui_install` is fully synchronous (git/pip/HTTP calls with no
    // `.await` points) and can run for many minutes; `spawn_blocking` (not
    // `spawn`) keeps it off the async runtime's worker threads so it can't
    // starve other concurrent async work (update checks, other downloads).
    crate::host::async_runtime::spawn_blocking(move || {
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
                    settings.comfyui_attention_backend =
                        selected_attention_backend(&request).map(|value| value.to_string());
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
        let torch_profile = request.torch_profile.clone();
        crate::host::async_runtime::spawn_blocking(move || -> Result<String, String> {
            let resolved_profile = torch_profile
                .clone()
                .or_else(|| detect_torch_profile_for_root(&root_clone))
                .unwrap_or_default();
            if is_non_cuda_profile(&resolved_profile)
                && matches!(
                    component_clone.as_str(),
                    "addon_sageattention"
                        | "sageattention"
                        | "addon_sageattention3"
                        | "sageattention3"
                        | "addon_flashattention"
                        | "flashattention"
                        | "addon_nunchaku"
                        | "nunchaku"
                        | "addon_trellis2"
                        | "trellis2"
                )
            {
                return Err(
                    "SageAttention, SageAttention3, FlashAttention, Nunchaku, and Trellis2 are CUDA-only and are not available with the Windows ROCm/XPU profiles."
                        .to_string(),
                );
            }
            match component_clone.as_str() {
                "addon_insightface" | "insightface" => {
                    if enabled {
                        install_insightface(
                            &app_clone,
                            &root_clone,
                            &uv_bin_clone,
                            &py_path_clone,
                            &uv_python_install_dir_clone,
                        )?;
                        Ok("Installed InsightFace.".to_string())
                    } else {
                        let nunchaku_active = pip_has_package(&root_clone, "nunchaku")
                            || custom_node_exists(&root_clone, "ComfyUI-nunchaku")
                            || custom_node_exists(&root_clone, "nunchaku_nodes");
                        if nunchaku_active {
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
                        let profile = if let Some(profile) = torch_profile {
                            profile
                        } else {
                            profile_from_torch_env(&root_clone)?
                        };
                        if !matches!(profile.as_str(), "torch280_cu128") {
                            return Err(
                                "Trellis2 currently requires Torch 2.8.0 + cu128 (Torch280 wheel set)."
                                    .to_string(),
                            );
                        }
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
