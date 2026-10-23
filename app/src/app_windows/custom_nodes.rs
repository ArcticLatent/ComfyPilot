//! Generic custom-node install primitives for the Windows backend.
//!
//! Windows counterpart to `app_linux/custom_nodes.rs` -- same scope, same
//! reasoning for stopping here rather than pulling in the per-addon
//! installers or the install/toggle orchestrators (see that file's doc
//! comment). One genuine behavioral difference, not just cosmetic: Windows'
//! `install_custom_node` re-pins the `requests` dependency stack
//! (`reassert_requests_dependency_stack`) after every custom-node install;
//! Linux's does not. That's real, pre-existing divergence -- preserved here,
//! not "fixed" into parity with Linux.

use crate::shared::{emit_install_event, AppState, CustomNodeSpec};
// These are general command-running utilities defined in the parent
// `app_windows` module itself (not `shared.rs`), used well beyond
// custom-node installs.
use super::{
    resolve_uv_binary, run_command, run_command_with_retry, run_uv_pip_strict,
    uv_pip_uninstall_best_effort,
};
use crate::host::AppHandle;
use std::path::{Path, PathBuf};

pub(crate) fn install_custom_node(
    app: &AppHandle,
    install_root: &Path,
    custom_nodes_root: &Path,
    py_exe: &Path,
    repo_url: &str,
    folder_name: &str,
) -> Result<(), String> {
    emit_install_event(
        app,
        "step",
        &format!("Installing custom node: {folder_name}..."),
    );
    let node_dir = custom_nodes_root.join(folder_name);
    if node_dir.exists() {
        let _ = std::fs::remove_dir_all(&node_dir);
    }
    run_command_with_retry(
        "git",
        &["clone", repo_url, &node_dir.to_string_lossy()],
        Some(install_root),
        2,
    )?;

    // Check out the catalog's known-good commit when it pins this repository.
    let pin = app
        .state::<AppState>()
        .context
        .catalog
        .catalog_snapshot()
        .custom_node_pin(repo_url)
        .map(str::to_string);
    if let Some(commit) = pin {
        let node_dir_s = node_dir.to_string_lossy().to_string();
        match run_command("git", &["-C", &node_dir_s, "checkout", "--quiet", &commit], Some(install_root)) {
            Ok(_) => emit_install_event(
                app,
                "info",
                &format!("Pinned {folder_name} to commit {}.", &commit[..commit.len().min(12)]),
            ),
            Err(err) => emit_install_event(
                app,
                "warn",
                &format!(
                    "Could not check out pinned commit {commit} for {folder_name}; using its latest commit. {err}"
                ),
            ),
        }
    }

    let req = node_dir.join("requirements.txt");
    if req.exists() {
        let non_empty = std::fs::metadata(&req)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if non_empty {
            let shared_runtime_root = app
                .state::<AppState>()
                .context
                .config
                .cache_path()
                .join("comfyui-runtime");
            let uv_bin = resolve_uv_binary(&shared_runtime_root, app)?;
            let uv_python_install_dir = shared_runtime_root
                .join(".python")
                .to_string_lossy()
                .to_string();
            run_uv_pip_strict(
                &uv_bin,
                &py_exe.to_string_lossy(),
                &[
                    "install",
                    "-r",
                    &req.to_string_lossy(),
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(install_root),
                &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
            )?;
        }
    }

    let installer = node_dir.join("install.py");
    if installer.exists() {
        let non_empty = std::fs::metadata(&installer)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if non_empty {
            run_command(
                &py_exe.to_string_lossy(),
                &[&installer.to_string_lossy()],
                Some(install_root),
            )?;
        }
    }

    let shared_runtime_root = app
        .state::<AppState>()
        .context
        .config
        .cache_path()
        .join("comfyui-runtime");
    let uv_bin = resolve_uv_binary(&shared_runtime_root, app)?;
    let uv_python_install_dir = shared_runtime_root
        .join(".python")
        .to_string_lossy()
        .to_string();
    reassert_requests_dependency_stack(
        &uv_bin,
        &py_exe.to_string_lossy(),
        install_root,
        &uv_python_install_dir,
    )?;

    Ok(())
}

/// The app's built-in custom nodes (Windows). See [`CustomNodeSpec`]'s doc
/// comment for why this table exists; referenced by `flag_key` from the
/// fresh-install flow (`run_comfyui_install`), the addon-state check
/// (`get_comfyui_addon_state`), and the enable/disable toggle
/// (`apply_comfyui_component_toggle`) instead of each hand-copying the repo
/// URL/folder name.
///
/// ComfyUI-Manager's `install_folder_name` is lowercase (`comfyui-manager`)
/// to match this platform's actual fresh-install behavior, unlike Linux's
/// `ComfyUI-Manager`; NTFS is case-insensitive so this has never caused a
/// functional bug, but the two casings were previously hand-typed
/// inconsistently across the install/toggle/addon-state call sites (the
/// toggle-enable path in particular used to create the capitalized form).
/// `known_folder_names` covers both so detection/removal never misses an
/// install made under either casing by an older app version.
pub(crate) const CUSTOM_NODES: &[CustomNodeSpec] = &[
    CustomNodeSpec {
        flag_key: "node_comfyui_manager",
        display_name: "comfyui-manager",
        repo_url: "https://github.com/Comfy-Org/ComfyUI-Manager",
        install_folder_name: "comfyui-manager",
        known_folder_names: &["comfyui-manager", "ComfyUI-Manager"],
    },
    CustomNodeSpec {
        flag_key: "node_comfyui_easy_use",
        display_name: "ComfyUI-Easy-Use",
        repo_url: "https://github.com/yolain/ComfyUI-Easy-Use",
        install_folder_name: "ComfyUI-Easy-Use",
        known_folder_names: &["ComfyUI-Easy-Use"],
    },
    CustomNodeSpec {
        flag_key: "node_rgthree_comfy",
        display_name: "rgthree-comfy",
        repo_url: "https://github.com/rgthree/rgthree-comfy",
        install_folder_name: "rgthree-comfy",
        known_folder_names: &["rgthree-comfy"],
    },
    CustomNodeSpec {
        flag_key: "node_comfyui_gguf",
        display_name: "ComfyUI-GGUF",
        repo_url: "https://github.com/city96/ComfyUI-GGUF",
        install_folder_name: "ComfyUI-GGUF",
        known_folder_names: &["ComfyUI-GGUF"],
    },
    CustomNodeSpec {
        flag_key: "node_comfyui_kjnodes",
        display_name: "comfyui-kjnodes",
        repo_url: "https://github.com/kijai/ComfyUI-KJNodes",
        install_folder_name: "comfyui-kjnodes",
        known_folder_names: &["comfyui-kjnodes", "ComfyUI-KJNodes"],
    },
    CustomNodeSpec {
        flag_key: "node_comfyui_crystools",
        display_name: "comfyui-crystools",
        repo_url: "https://github.com/crystian/comfyui-crystools.git",
        install_folder_name: "comfyui-crystools",
        known_folder_names: &["comfyui-crystools", "ComfyUI-Crystools"],
    },
];

pub(crate) fn custom_node_spec(flag_key: &str) -> Option<&'static CustomNodeSpec> {
    CUSTOM_NODES.iter().find(|spec| spec.flag_key == flag_key)
}

fn reassert_requests_dependency_stack(
    uv_bin: &str,
    py_path: &str,
    root: &Path,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    let py_exe = PathBuf::from(py_path);
    uv_pip_uninstall_best_effort(uv_bin, &py_exe, root, uv_python_install_dir, &["chardet"])?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--upgrade",
            "--force-reinstall",
            "requests==2.32.5",
            "urllib3==2.5.0",
            "charset_normalizer==3.4.5",
            "idna==3.10",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )
}
