//! ComfyUI runtime start/stop plumbing for the Linux backend: resolving
//! which install a command applies to, launching/monitoring the child
//! process, locating a working Python interpreter for a given install,
//! checking the latest upstream release tag, and killing lingering
//! processes on stop/uninstall. Distinct from `app_linux/install_state.rs`
//! (read-only state reporting) and the install/toggle orchestrators that
//! remain in `app_linux.rs`.
//!
//! `resolve_root_path`, `start_comfyui_root_impl`, `spawn_comfyui_start_monitor`,
//! and `git_latest_release_tag` are referenced from `platform.rs` as
//! `crate::app_linux::{...}` -- moving their definitions here doesn't change
//! that path, since `app_linux.rs` re-exports them (`pub(crate) use
//! runtime::{...}`) the same way every other slice's public surface is
//! re-exported.
//!
//! `pip_has_package` deliberately did *not* move here despite living right
//! in the middle of this code in the original file (between
//! `python_exe_for_root` and the GitHub-tag helpers): it's already consumed
//! by `install_state.rs` and `addons.rs` via `use super::pip_has_package;`,
//! and nothing in this module actually calls it, so moving it would only
//! have added churn to two already-verified files for no benefit.

use crate::shared::{
    append_launch_settings_args, append_manager_launch_arg, apply_torch_allocator_env_compat,
    builtin_manager_installed, builtin_manager_requirements, comfyui_runtime_running,
    comfyui_supported_launch_flags, drop_unsupported_launch_flags, emit_comfyui_runtime_event,
    emit_comfyui_runtime_log_event, ensure_comfyui_port_available, kill_managed_comfyui_child,
    normalize_release_version, parse_custom_launch_args, parse_semver_triplet,
    wait_for_comfyui_start, ComfyEndpoint,
};
use arctic_downloader::app::{drain_lossy_lines, AppContext};
// General command/env/python-runtime utilities and other cross-cutting
// helpers defined in the parent `app_linux` module itself (not
// `shared.rs`), used well beyond this runtime-plumbing set.
use super::{
    apply_python_tls_environment, build_command, comfyui_launch_args, command_available,
    detect_launch_attention_backend_for_root, is_forbidden_install_path, nerdstats_enabled,
    normalize_canonical_path, nunchaku_backend_present, python_module_importable,
    python_runtime_env_for_root, recover_lock, run_command_capture, update_tray_comfy_status,
    AppState,
};
use crate::host::AppHandle;
use serde::Deserialize;
use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
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
        Some(normalize_canonical_path(&canonical).to_path_buf())
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

    let root = normalize_canonical_path(&std::fs::canonicalize(&root).unwrap_or(root));
    let main_py = root.join("main.py");
    if !main_py.exists() {
        return Err(format!("ComfyUI main.py not found in {}", root.display()));
    }

    let py_exe = resolve_start_python_exe(app, state, &root)?;
    let settings = state.context.config.settings();
    let custom_launch_args = parse_custom_launch_args(&settings.comfyui_custom_launch_args)?;

    let configured_root_matches = settings
        .comfyui_root
        .as_ref()
        .map(|configured_root| {
            normalize_canonical_path(
                &std::fs::canonicalize(configured_root).unwrap_or_else(|_| configured_root.clone()),
            ) == root
        })
        .unwrap_or(false);

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
                if nunchaku_backend_present(&root) {
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
    let main_py_string = main_py.to_string_lossy().to_string();
    let mut args_owned = vec![
        "-W".to_string(),
        "ignore::FutureWarning".to_string(),
        main_py_string,
    ];
    let mut launch_args = comfyui_launch_args(
        settings.comfyui_listen_enabled,
        settings.comfyui_pinned_memory_enabled,
        effective_attention.as_deref(),
        settings.comfyui_lowvram_enabled,
        settings.comfyui_bf16_unet_enabled,
        settings.comfyui_async_offload_enabled,
        settings.comfyui_disable_smart_memory_enabled,
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
    for arg in launch_args {
        args_owned.push(arg);
    }
    let arg_refs: Vec<&str> = args_owned.iter().map(String::as_str).collect();
    let py_exe_string = py_exe.to_string_lossy().to_string();
    let python_envs = python_runtime_env_for_root(&root);
    let env_refs: Vec<(&str, &str)> = python_envs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut cmd = build_command(&py_exe_string, &arg_refs, Some(&root), &env_refs)?;
    apply_torch_allocator_env_compat(&mut cmd);

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
    if !nerdstats_enabled() && settings.comfyui_show_runtime_logs {
        if let Some(stdout) = child.stdout.take() {
            spawn_comfyui_runtime_log_stream(app.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_comfyui_runtime_log_stream(app.clone(), "stderr", stderr);
        }
    }
    *recover_lock(state.comfyui_process_root.lock()) = Some(root.clone());
    *recover_lock(state.comfyui_process.lock()) = Some(child);
    Ok(())
}

pub(crate) fn stop_comfyui_root_impl(state: &AppState) -> Result<bool, String> {
    let configured_root = state.context.config.settings().comfyui_root;
    let mut stopped_any = kill_managed_comfyui_child(state)?;

    // After app restart, or when Flatpak launches ComfyUI on the host, we may no
    // longer have a child handle that can stop the real server process. In that case,
    // stop the host listener process by its selected ComfyUI root.
    if let Some(root) = configured_root {
        let root = normalize_canonical_path(&std::fs::canonicalize(&root).unwrap_or(root));
        let port = ComfyEndpoint::from_settings(&state.context.config.settings()).port;
        if kill_host_comfyui_for_root(&root, port)? {
            stopped_any = true;
        }
    }

    Ok(stopped_any)
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

fn comfyui_listener_running(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(180)).is_ok()
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
    let linux_dot_venv_py = root.join(".venv").join("bin").join("python");
    let legacy_linux_dot_venv_py = install_dir.join(".venv").join("bin").join("python");
    let linux_venv_py = root.join("venv").join("bin").join("python");
    let legacy_linux_venv_py = install_dir.join("venv").join("bin").join("python");

    let mut cmd = if linux_dot_venv_py.exists() {
        std::process::Command::new(linux_dot_venv_py)
    } else if legacy_linux_dot_venv_py.exists() {
        std::process::Command::new(legacy_linux_dot_venv_py)
    } else if linux_venv_py.exists() {
        std::process::Command::new(linux_venv_py)
    } else if legacy_linux_venv_py.exists() {
        std::process::Command::new(legacy_linux_venv_py)
    } else if command_available("python3", &["--version"]) {
        std::process::Command::new("python3")
    } else {
        std::process::Command::new("python")
    };
    apply_python_tls_environment(&mut cmd);
    if !nerdstats_enabled() {
        super::apply_background_command_flags(&mut cmd);
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
        root.join(".venv").join("bin").join("python"),
        install_dir.join(".venv").join("bin").join("python"),
        root.join("venv").join("bin").join("python"),
        install_dir.join("venv").join("bin").join("python"),
    ]
}

fn python_exe_works(py_exe: &Path, root: &Path) -> bool {
    if !py_exe.exists() {
        return false;
    }
    let mut cmd = std::process::Command::new(py_exe);
    apply_python_tls_environment(&mut cmd);
    cmd.arg("--version");
    cmd.current_dir(root);
    super::apply_background_command_flags(&mut cmd);
    apply_torch_allocator_env_compat(&mut cmd);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn resolve_start_python_exe(
    _app: &AppHandle,
    _state: &AppState,
    root: &Path,
) -> Result<PathBuf, String> {
    let candidates = python_exe_candidates_for_root(root);
    for candidate in &candidates {
        if python_exe_works(candidate, root) {
            return Ok(candidate.clone());
        }
    }

    if candidates.iter().any(|c| c.exists()) {
        return Err(
            "Detected Python runtime candidates, but none are executable. Reinstall ComfyUI runtime."
                .to_string(),
        );
    }

    if command_available("python3", &["--version"]) {
        return Ok(PathBuf::from("python3"));
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
        root.join(".venv").join("bin").join("python"),
        install_dir.join(".venv").join("bin").join("python"),
        root.join("venv").join("bin").join("python"),
        install_dir.join("venv").join("bin").join("python"),
    ];
    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Python executable for this ComfyUI install was not found.".to_string())
}

#[derive(Debug, Deserialize)]
struct GithubTagEntry {
    name: String,
}

fn comfyui_origin_github_repo(root: &Path) -> Option<(String, String)> {
    let config = std::fs::read_to_string(root.join(".git").join("config")).ok()?;
    let mut in_origin = false;

    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[remote ") {
            in_origin = trimmed == r#"[remote "origin"]"#;
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some(url) = trimmed.strip_prefix("url =") {
            return parse_github_repo_from_url(url.trim());
        }
    }

    None
}

fn parse_github_repo_from_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/');
    let path = if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("http://github.com/") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        rest
    } else {
        trimmed.strip_prefix("git@github.com:")?
    };

    let mut parts = path.trim_end_matches(".git").split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

fn github_latest_release_tag(owner: &str, repo: &str) -> Option<(String, String)> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!(
            "ComfyPilot/{} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_NAME")
        ))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;

    let mut best: Option<((u64, u64, u64), String, String)> = None;
    for page in 1..=5 {
        let url =
            format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=100&page={page}");
        let response = client.get(&url).send().ok()?.error_for_status().ok()?;
        let tags: Vec<GithubTagEntry> = response.json().ok()?;
        if tags.is_empty() {
            break;
        }
        for entry in tags {
            let tag = entry.name;
            let Some(version) = normalize_release_version(&tag) else {
                continue;
            };
            let Some(parsed) = parse_semver_triplet(&version) else {
                continue;
            };
            match &best {
                Some((current, _, _)) if *current >= parsed => {}
                _ => best = Some((parsed, tag, version)),
            }
        }
    }

    best.map(|(_, tag, version)| (tag, version))
}

pub(crate) fn git_latest_release_tag(root: &Path) -> Option<(String, String)> {
    if let Ok((stdout, _)) = run_command_capture(
        "git",
        &["ls-remote", "--tags", "--refs", "origin"],
        Some(root),
    ) {
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
        if let Some((_, tag, version)) = best {
            return Some((tag, version));
        }
    }

    let (owner, repo) = comfyui_origin_github_repo(root)
        .unwrap_or_else(|| ("Comfy-Org".to_string(), "ComfyUI".to_string()));
    github_latest_release_tag(&owner, &repo)
}

fn host_comfyui_running_for_needle(needle: &str) -> bool {
    match run_command_capture("pgrep", &["-f", needle], None) {
        Ok((stdout, _)) => stdout.lines().any(|line| !line.trim().is_empty()),
        Err(_) => false,
    }
}

/// The install among `roots` whose ComfyUI (`<root>/main.py`) runs as a
/// process this app didn't start: one from before it opened, or a Flatpak's
/// host process.
pub(crate) fn host_comfyui_root_among(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find(|root| host_comfyui_running_for_needle(&root.join("main.py").to_string_lossy()))
        .cloned()
}

fn signal_host_pids(pids: &[String], signal: &str) {
    for pid in pids {
        let _ = run_command_capture("kill", &[signal, pid.as_str()], None);
    }
}

pub(crate) fn kill_host_comfyui_for_root(root: &Path, port: u16) -> Result<bool, String> {
    let main_py = root.join("main.py");
    if !main_py.exists() {
        return Ok(false);
    }
    let needle = main_py.to_string_lossy().to_string();
    let (stdout, _) = match run_command_capture("pgrep", &["-f", &needle], None) {
        Ok(output) => output,
        Err(_) => return Ok(false),
    };

    let pids: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if pids.is_empty() {
        return Ok(false);
    }

    signal_host_pids(&pids, "-TERM");

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let still_running = host_comfyui_running_for_needle(&needle);
        if !still_running || !comfyui_listener_running(port) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let remaining: Vec<String> = match run_command_capture("pgrep", &["-f", &needle], None) {
        Ok((stdout, _)) => stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    };
    if remaining.is_empty() || !comfyui_listener_running(port) {
        return Ok(true);
    }

    signal_host_pids(&remaining, "-KILL");
    let force_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if !host_comfyui_running_for_needle(&needle) || !comfyui_listener_running(port) {
            return Ok(true);
        }
        if Instant::now() >= force_deadline {
            return Err("Failed to stop host ComfyUI process cleanly.".to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub(crate) fn kill_python_processes_for_root(_root: &Path, py_exe: &Path) -> Result<bool, String> {
    // Match the installation's exact virtual-environment interpreter rather
    // than the broader root path, so unrelated Python processes cannot be
    // selected merely because one of their arguments mentions the folder.
    //
    // `py_exe` can be a bare command name like "python3" when no venv
    // interpreter exists for this root (see `python_for_root`'s system
    // fallback). Canonicalizing a bare name that doesn't resolve against the
    // current directory would previously fall back to that same unqualified
    // string, turning the `pgrep -f` needle below into a wildcard that
    // matches every process on the machine whose command line happens to
    // contain "python3" (IDEs, language servers, unrelated scripts) --
    // refuse instead of ever degrading to that.
    let Ok(interpreter) = std::fs::canonicalize(py_exe) else {
        log::warn!(
            "Refusing to search for lingering ComfyUI Python processes: '{}' is not a resolvable interpreter path (no venv found for this install).",
            py_exe.display()
        );
        return Ok(false);
    };
    if !interpreter.is_absolute() {
        return Ok(false);
    }
    let needle = interpreter.to_string_lossy().to_string();
    if needle.trim().is_empty() {
        return Ok(false);
    }

    let matching_pids = || -> Vec<String> {
        run_command_capture("pgrep", &["-f", &needle], None)
            .map(|(stdout, _)| {
                stdout
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    let pids = matching_pids();
    if pids.is_empty() {
        return Ok(false);
    }
    signal_host_pids(&pids, "-TERM");

    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if matching_pids().is_empty() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let remaining = matching_pids();
    signal_host_pids(&remaining, "-KILL");
    std::thread::sleep(Duration::from_millis(250));
    if matching_pids().is_empty() {
        Ok(true)
    } else {
        Err("Failed to stop lingering ComfyUI Python processes.".to_string())
    }
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
    wait_for_comfyui_start(state, Duration::from_secs(45))?;
    update_tray_comfy_status(app, true);
    emit_comfyui_runtime_event(
        app,
        "restarted_after_changes",
        "ComfyUI restarted after install/remove operation.",
    );
    Ok(())
}
