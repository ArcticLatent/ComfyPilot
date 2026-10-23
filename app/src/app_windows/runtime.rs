//! ComfyUI runtime start/stop plumbing for the Windows backend. Windows
//! counterpart to `app_linux/runtime.rs` -- see that file's doc comment for
//! the scope rationale and the `platform.rs` re-export note (applies
//! identically here).
//!
//! Real, pre-existing divergence physically relocated but not unified:
//! - `resolve_start_python_exe` self-heals here (bootstraps a uv-managed
//!   Python runtime via `resolve_uv_binary`/`ensure_uv_python_installed` when
//!   no working interpreter is found); Linux's just returns an error.
//! - `start_comfyui_root_impl` calls Windows-only `apply_intel_xpu_launch_env`
//!   and the Job Object guard in sibling module `process_guard`; neither has
//!   a Linux equivalent.
//! - `restart_comfyui_after_mutation` does not wait for ComfyUI to finish
//!   starting before updating tray status; Linux's calls
//!   `wait_for_comfyui_start` first.
//! - There is no Windows equivalent of Linux's `comfyui_origin_github_repo`/
//!   `parse_github_repo_from_url`/`github_latest_release_tag`/
//!   `GithubTagEntry` GitHub-API fallback chain -- `git_latest_release_tag`
//!   here only ever reads `git ls-remote` and returns `None` if that yields
//!   nothing usable.
//! - **Two `#[cfg(target_os = ...)]` variants of `kill_python_processes_for_root`
//!   exist in the original file** (a real PowerShell `Get-CimInstance`-based
//!   implementation under `#[cfg(target_os = "windows")]`, and a `Ok(false)`
//!   no-op under `#[cfg(not(target_os = "windows"))]`), exactly the
//!   cfg-gated-duplicate trap `docs/cross-platform-development.md` already
//!   warns about from an earlier extraction pass. Both moved here together,
//!   with their original `#[cfg(...)]` attributes intact.

use crate::shared::{
    append_launch_settings_args, append_manager_launch_arg, apply_torch_allocator_env_compat,
    builtin_manager_installed, builtin_manager_requirements, comfyui_external_running,
    comfyui_runtime_running, comfyui_supported_launch_flags, drop_unsupported_launch_flags,
    emit_comfyui_runtime_event, emit_comfyui_runtime_log_event, ensure_comfyui_port_available,
    kill_managed_comfyui_child, normalize_release_version, parse_custom_launch_args,
    parse_semver_triplet, wait_for_comfyui_start, AppState, ComfyEndpoint,
};
use arctic_downloader::app::{drain_lossy_lines, AppContext};
// General command/env/python-runtime utilities defined in the parent
// `app_windows` module itself (not `shared.rs`), used well beyond this
// runtime-plumbing set.
use super::{
    apply_background_command_flags, apply_intel_xpu_launch_env, comfyui_launch_args,
    command_available, detect_launch_attention_backend_for_root, detect_torch_profile_for_root,
    ensure_uv_python_installed, is_forbidden_install_path, nerdstats_enabled,
    python_module_importable, recover_lock, release_comfy_job_object, resolve_uv_binary,
    run_command_capture, strip_windows_verbatim_prefix, track_comfy_job_object,
    update_tray_comfy_status,
};
use crate::host::AppHandle;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub(crate) fn resolve_root_path(
    context: &AppContext,
    comfyui_root: Option<String>,
) -> Result<std::path::PathBuf, String> {
    fn normalize_existing(path: std::path::PathBuf) -> Option<std::path::PathBuf> {
        let absolute = if path.is_absolute() {
            path
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(path)
        } else {
            path
        };
        if !absolute.exists() {
            return None;
        }
        let canonical = std::fs::canonicalize(&absolute).ok().or(Some(absolute))?;
        Some(strip_windows_verbatim_prefix(&canonical).to_path_buf())
    }

    if let Some(root) = comfyui_root {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            let path = std::path::PathBuf::from(trimmed);
            if let Some(normalized) = normalize_existing(path) {
                // `comfyui_root` here is caller-supplied (any invoke from the
                // webview can pass an arbitrary existing path, not just the
                // app's own configured root), and this function feeds
                // filesystem-mutating commands (model/workflow downloads,
                // extra_model_paths.yaml writes). Refuse system directories
                // even though the path exists, same as the install-time
                // guard in `is_forbidden_install_path`.
                if is_forbidden_install_path(&normalized) {
                    return Err(
                        "That folder can't be used as a ComfyUI root (system directory)."
                            .to_string(),
                    );
                }
                return Ok(normalized);
            }
        }
    }

    if let Some(path) = context.config.settings().comfyui_root {
        if let Some(normalized) = normalize_existing(path) {
            return Ok(normalized);
        }
    }

    Err("Select a valid ComfyUI root folder first.".to_string())
}

pub(crate) fn start_comfyui_root_impl(
    app: &AppHandle,
    state: &AppState,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    if comfyui_runtime_running(state) {
        return Ok(());
    }
    ensure_comfyui_port_available(ComfyEndpoint::from_settings(
        &state.context.config.settings(),
    ))?;

    let root = if let Some(raw) = comfyui_root {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            state
                .context
                .config
                .settings()
                .comfyui_root
                .ok_or_else(|| "ComfyUI root is not configured.".to_string())?
        } else {
            PathBuf::from(trimmed)
        }
    } else {
        state
            .context
            .config
            .settings()
            .comfyui_root
            .ok_or_else(|| "ComfyUI root is not configured.".to_string())?
    };

    let root = strip_windows_verbatim_prefix(&std::fs::canonicalize(&root).unwrap_or(root));
    let main_py = root.join("main.py");
    if !main_py.exists() {
        return Err(format!("ComfyUI main.py not found in {}", root.display()));
    }

    let py_exe = resolve_start_python_exe(app, state, &root)?;
    let settings = state.context.config.settings();
    let custom_launch_args = parse_custom_launch_args(&settings.comfyui_custom_launch_args)?;
    let mut cmd = std::process::Command::new(&py_exe);
    if !nerdstats_enabled() {
        apply_background_command_flags(&mut cmd);
    }
    apply_torch_allocator_env_compat(&mut cmd);
    let configured_root_matches = settings
        .comfyui_root
        .as_ref()
        .map(|configured_root| {
            strip_windows_verbatim_prefix(
                &std::fs::canonicalize(configured_root)
                    .unwrap_or_else(|_| PathBuf::from(configured_root)),
            ) == root
        })
        .unwrap_or(false);
    let effective_profile = detect_torch_profile_for_root(&root).or_else(|| {
        if configured_root_matches {
            settings.comfyui_torch_profile.clone()
        } else {
            None
        }
    });
    let effective_attention = {
        let configured = if configured_root_matches {
            settings.comfyui_attention_backend.clone()
        } else {
            None
        };
        match configured.as_deref() {
            Some("none") => None,
            Some("sage3") => {
                if python_module_importable(&root, "sageattn3") {
                    Some("sage3".to_string())
                } else {
                    return Err(
                        "SageAttention3 is selected but not importable in this install. Re-apply SageAttention3 for this ComfyUI root."
                            .to_string(),
                    );
                }
            }
            Some("sage") => {
                if python_module_importable(&root, "sageattention")
                    || python_module_importable(&root, "sageattn3")
                {
                    Some("sage".to_string())
                } else {
                    return Err(
                        "SageAttention is selected but not importable in this install. Re-apply SageAttention for this ComfyUI root."
                            .to_string(),
                    );
                }
            }
            Some("flash") => {
                if python_module_importable(&root, "flash_attn") {
                    Some("flash".to_string())
                } else {
                    return Err(
                        "FlashAttention is selected but not importable in this install. Re-apply FlashAttention for this ComfyUI root."
                            .to_string(),
                    );
                }
            }
            Some("nunchaku") => {
                if super::nunchaku_backend_present(&root) {
                    Some("nunchaku".to_string())
                } else {
                    return Err(
                        "Nunchaku is selected but backend is not installed correctly for this ComfyUI root. Re-apply Nunchaku."
                            .to_string(),
                    );
                }
            }
            _ => detect_launch_attention_backend_for_root(&root),
        }
    };
    cmd.arg("-W").arg("ignore::FutureWarning").arg(main_py);
    let mut launch_args = comfyui_launch_args(
        settings.comfyui_listen_enabled,
        settings.comfyui_pinned_memory_enabled,
        settings.comfyui_lowvram_enabled,
        settings.comfyui_bf16_unet_enabled,
        settings.comfyui_async_offload_enabled,
        settings.comfyui_disable_smart_memory_enabled,
        effective_attention.as_deref(),
        &custom_launch_args,
    );
    // Older checkouts don't know the flag, even with the package installed.
    append_manager_launch_arg(
        &mut launch_args,
        builtin_manager_requirements(&root).is_some() && builtin_manager_installed(&py_exe),
    );
    append_launch_settings_args(&mut launch_args, &settings);
    if let Some(supported) = comfyui_supported_launch_flags(&root) {
        let dropped =
            drop_unsupported_launch_flags(&mut launch_args, &supported, &custom_launch_args);
        if !dropped.is_empty() {
            emit_comfyui_runtime_event(
                app,
                "launch_args",
                format!(
                    "Skipping options this ComfyUI version doesn't support: {}",
                    dropped.join(" ")
                ),
            );
        }
    }
    emit_comfyui_runtime_event(
        app,
        "launch_args",
        format!(
            "Launching with attention backend: {}",
            effective_attention
                .as_deref()
                .unwrap_or("PyTorch attention")
        ),
    );
    cmd.args(launch_args);
    if let Some(profile) = effective_profile.as_deref() {
        apply_intel_xpu_launch_env(&mut cmd, profile);
    }
    cmd.current_dir(&root);
    if nerdstats_enabled() {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else if settings.comfyui_show_runtime_logs {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to start ComfyUI: {err}"))?;
    track_comfy_job_object(&child);
    if !nerdstats_enabled() && settings.comfyui_show_runtime_logs {
        if let Some(stdout) = child.stdout.take() {
            spawn_comfyui_runtime_log_stream(app.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_comfyui_runtime_log_stream(app.clone(), "stderr", stderr);
        }
    }
    *recover_lock(state.comfyui_process_root.lock()) = Some(root);
    *recover_lock(state.comfyui_process.lock()) = Some(child);
    Ok(())
}

/// Which install a ComfyUI this app didn't start belongs to: not looked up on
/// Windows (that takes PowerShell; the app's own ComfyUI is known, and it
/// ends with the app through the Job Object).
pub(crate) fn host_comfyui_root_among(_roots: &[PathBuf]) -> Option<PathBuf> {
    None
}

pub(crate) fn stop_comfyui_root_impl(state: &AppState) -> Result<bool, String> {
    let mut stopped_any = kill_managed_comfyui_child(state)?;
    // Drop (and thus close/kill-on-close) the Job Object so any lingering
    // grandchild process ComfyUI spawned is cleaned up too, not just the
    // one process `kill_managed_comfyui_child` holds a handle to.
    release_comfy_job_object();

    // After app restart, we may no longer have a child handle but ComfyUI can still
    // be running and listening on its port. In that case, stop the listener process.
    // Only a Python process of the configured install is stopped: the port
    // may belong to an unrelated program.
    if comfyui_external_running(state) {
        #[cfg(target_os = "windows")]
        {
            let settings = state.context.config.settings();
            let port = ComfyEndpoint::from_settings(&settings).port;
            if let Some(root) = settings.comfyui_root.as_deref() {
                if let Ok(py_exe) = python_exe_for_root(root) {
                    if kill_listener_process_on_port(port, root, &py_exe)? {
                        stopped_any = true;
                    }
                }
            }
        }
    }

    Ok(stopped_any)
}

/// Stops the process listening on `port` if it is this install's ComfyUI:
/// a Python process running the install's interpreter or with a command
/// line inside the install folder. Anything else is left running.
#[cfg(target_os = "windows")]
fn kill_listener_process_on_port(port: u16, root: &Path, py_exe: &Path) -> Result<bool, String> {
    let root_norm =
        strip_windows_verbatim_prefix(&std::fs::canonicalize(root).unwrap_or(root.to_path_buf()))
            .to_string_lossy()
            .replace('\'', "''");
    let py_norm = strip_windows_verbatim_prefix(
        &std::fs::canonicalize(py_exe).unwrap_or(py_exe.to_path_buf()),
    )
    .to_string_lossy()
    .replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $root = '{root_norm}'.TrimEnd('\\') + '\\'; \
         $py = '{py_norm}'; \
         $ownerPids = Get-NetTCPConnection -LocalPort {port} -State Listen | Select-Object -ExpandProperty OwningProcess -Unique; \
         if (-not $ownerPids) {{ exit 3 }}; \
         $matched = 0; \
         foreach ($ownerPid in $ownerPids) {{ \
           $p = Get-CimInstance Win32_Process -Filter \"ProcessId=$ownerPid\"; \
           $exe = [string]$p.ExecutablePath; \
           $cmd = [string]$p.CommandLine; \
           $isPython = $p.Name -match '^python'; \
           $matchPy = $exe -and ($exe.ToLowerInvariant() -eq $py.ToLowerInvariant()); \
           $matchRoot = $cmd -and $cmd.ToLowerInvariant().Contains($root.ToLowerInvariant()); \
           if ($isPython -and ($matchPy -or $matchRoot)) {{ \
             Stop-Process -Id $ownerPid -Force -ErrorAction SilentlyContinue; \
             $matched++; \
           }} \
         }}; \
         if ($matched -eq 0) {{ exit 4 }}; \
         Start-Sleep -Milliseconds 180; \
         $left = Get-NetTCPConnection -LocalPort {port} -State Listen; \
         if ($left) {{ exit 2 }} else {{ exit 0 }}"
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    apply_background_command_flags(&mut cmd);
    let status = cmd
        .status()
        .map_err(|err| format!("Failed to stop ComfyUI listener on port {port}: {err}"))?;
    if status.success() {
        return Ok(true);
    }
    if status.code() == Some(3) {
        return Ok(false);
    }
    if status.code() == Some(4) {
        return Err(format!(
            "Port {port} is held by a program that isn't this ComfyUI install, so it was not stopped."
        ));
    }
    if status.code() == Some(2) {
        return Err(format!(
            "ComfyUI listener is still active on port {port} after stop attempt."
        ));
    }
    Err(format!(
        "Failed stopping ComfyUI listener process on port {port} (exit code {:?}).",
        status.code()
    ))
}

pub(crate) fn spawn_comfyui_start_monitor(app: &AppHandle, instance_name: String) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let state = app_handle.state::<AppState>();
        match wait_for_comfyui_start(&state, Duration::from_secs(45)) {
            Ok(()) => {
                update_tray_comfy_status(&app_handle, true);
                emit_comfyui_runtime_event(
                    &app_handle,
                    "started",
                    format!("{instance_name} started."),
                );
                let url = ComfyEndpoint::from_settings(&state.context.config.settings()).url();
                if let Err(err) = open::that(url) {
                    log::warn!("Failed to open ComfyUI in browser: {err}");
                }
            }
            Err(err) => {
                let running = comfyui_runtime_running(&state);
                update_tray_comfy_status(&app_handle, running);
                emit_comfyui_runtime_event(
                    &app_handle,
                    "start_failed",
                    format!("{instance_name} start failed: {err}"),
                );
            }
        }
    });
}

fn spawn_comfyui_runtime_log_stream(
    app: AppHandle,
    stream_name: &'static str,
    reader: impl std::io::Read + Send + 'static,
) {
    std::thread::spawn(move || {
        let _ = drain_lossy_lines(reader, |text| {
            emit_comfyui_runtime_log_event(&app, stream_name, text)
        });
    });
}

pub(crate) fn python_for_root(root: &Path) -> std::process::Command {
    let install_dir = root
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    let venv_py = root.join(".venv").join("Scripts").join("python.exe");
    let legacy_venv_py = install_dir.join(".venv").join("Scripts").join("python.exe");
    let embed_py = root.join("python_embeded").join("python.exe");
    let legacy_embed_py = install_dir.join("python_embeded").join("python.exe");

    let mut cmd = if venv_py.exists() {
        std::process::Command::new(venv_py)
    } else if legacy_venv_py.exists() {
        std::process::Command::new(legacy_venv_py)
    } else if embed_py.exists() {
        std::process::Command::new(embed_py)
    } else if legacy_embed_py.exists() {
        std::process::Command::new(legacy_embed_py)
    } else {
        std::process::Command::new("python")
    };
    if !nerdstats_enabled() {
        apply_background_command_flags(&mut cmd);
    }
    apply_torch_allocator_env_compat(&mut cmd);
    cmd
}

fn python_exe_candidates_for_root(root: &Path) -> Vec<PathBuf> {
    let install_dir = root
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    vec![
        root.join(".venv").join("Scripts").join("python.exe"),
        install_dir.join(".venv").join("Scripts").join("python.exe"),
        root.join("python_embeded").join("python.exe"),
        install_dir.join("python_embeded").join("python.exe"),
    ]
}

fn python_exe_works(py_exe: &Path, root: &Path) -> bool {
    if !py_exe.exists() {
        return false;
    }
    let mut cmd = std::process::Command::new(py_exe);
    cmd.arg("--version");
    cmd.current_dir(root);
    apply_background_command_flags(&mut cmd);
    apply_torch_allocator_env_compat(&mut cmd);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn resolve_start_python_exe(
    app: &AppHandle,
    state: &AppState,
    root: &Path,
) -> Result<PathBuf, String> {
    let candidates = python_exe_candidates_for_root(root);
    for candidate in &candidates {
        if python_exe_works(candidate, root) {
            return Ok(candidate.clone());
        }
    }

    if candidates.iter().any(|c| c.exists()) {
        emit_comfyui_runtime_event(
            app,
            "preparing_runtime",
            "Preparing local Python runtime for this ComfyUI installation...",
        );
        let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
        let uv_bin = resolve_uv_binary(&shared_runtime_root, app)?;
        let python_store = shared_runtime_root.join(".python");
        std::fs::create_dir_all(&python_store).map_err(|err| err.to_string())?;
        let python_store_s = python_store.to_string_lossy().to_string();
        let _ = ensure_uv_python_installed(&uv_bin, Some(root), &python_store_s)?;

        for candidate in &candidates {
            if python_exe_works(candidate, root) {
                return Ok(candidate.clone());
            }
        }
    }

    if command_available("python", &["--version"]) {
        return Ok(PathBuf::from("python"));
    }

    Err(
        "No working Python executable found for this ComfyUI install. Reinstall or run Install New once to bootstrap runtime."
            .to_string(),
    )
}

pub(crate) fn python_exe_for_root(root: &Path) -> Result<PathBuf, String> {
    let install_dir = root
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    let candidates = [
        root.join(".venv").join("Scripts").join("python.exe"),
        install_dir.join(".venv").join("Scripts").join("python.exe"),
        root.join("python_embeded").join("python.exe"),
        install_dir.join("python_embeded").join("python.exe"),
    ];
    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Python executable for this ComfyUI install was not found.".to_string())
}

pub(crate) fn git_latest_release_tag(root: &Path) -> Option<(String, String)> {
    let (stdout, _) = run_command_capture(
        "git",
        &["ls-remote", "--tags", "--refs", "origin"],
        Some(root),
    )
    .ok()?;
    let mut best: Option<((u64, u64, u64), String, String)> = None;

    for line in stdout.lines() {
        let mut cols = line.split_whitespace();
        let Some(_sha) = cols.next() else {
            continue;
        };
        let Some(ref_name) = cols.next() else {
            continue;
        };
        let Some(tag) = ref_name.strip_prefix("refs/tags/") else {
            continue;
        };
        let Some(version) = normalize_release_version(tag) else {
            continue;
        };
        let Some(parsed) = parse_semver_triplet(&version) else {
            continue;
        };

        match &best {
            Some((current, _, _)) if *current >= parsed => {}
            _ => best = Some((parsed, tag.to_string(), version)),
        }
    }

    best.map(|(_, tag, version)| (tag, version))
}

#[cfg(target_os = "windows")]
pub(crate) fn kill_python_processes_for_root(root: &Path, py_exe: &Path) -> Result<bool, String> {
    let root =
        strip_windows_verbatim_prefix(&std::fs::canonicalize(root).unwrap_or(root.to_path_buf()));
    let py_exe = strip_windows_verbatim_prefix(
        &std::fs::canonicalize(py_exe).unwrap_or(py_exe.to_path_buf()),
    );
    let root_norm = root.to_string_lossy().replace('\'', "''");
    let py_norm = py_exe.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $root='{}'.TrimEnd('\\') + '\\'; \
         $py='{}'; \
         $killed=0; \
         $procs = Get-CimInstance Win32_Process -Filter \"Name='python.exe'\"; \
         foreach ($p in $procs) {{ \
           $exe = [string]$p.ExecutablePath; \
           $cmd = [string]$p.CommandLine; \
           $matchPy = $exe -and ($exe.ToLowerInvariant() -eq $py.ToLowerInvariant()); \
           $matchRoot = $cmd -and ($cmd.ToLowerInvariant().Contains($root.ToLowerInvariant())); \
           if ($matchPy -or $matchRoot) {{ \
             Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue; \
             $killed++; \
           }} \
         }}; \
         if ($killed -gt 0) {{ Start-Sleep -Milliseconds 250 }}; \
         Write-Output $killed",
        root_norm, py_norm
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    apply_background_command_flags(&mut cmd);
    let out = cmd
        .output()
        .map_err(|err| format!("Failed to stop lingering Python processes: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "Failed stopping lingering Python processes (exit code {:?}).",
            out.status.code()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let killed = text.trim().parse::<u64>().unwrap_or(0);
    Ok(killed > 0)
}
#[cfg(not(target_os = "windows"))]
pub(crate) fn kill_python_processes_for_root(_root: &Path, _py_exe: &Path) -> Result<bool, String> {
    Ok(false)
}

pub(crate) fn restart_comfyui_after_mutation(
    app: &AppHandle,
    state: &AppState,
    was_running: bool,
) -> Result<(), String> {
    if !was_running {
        return Ok(());
    }
    start_comfyui_root_impl(app, state, None)?;
    update_tray_comfy_status(app, true);
    emit_comfyui_runtime_event(
        app,
        "restarted_after_changes",
        "ComfyUI restarted after install/remove operation.",
    );
    Ok(())
}
