//! ComfyUI-Manager install/removal and single custom-node installs, the
//! same on both platforms. Platform pieces (uv, `install_custom_node`, the
//! built-in node table) come through `crate::platform`.

use super::{
    builtin_manager_installed, builtin_manager_requirements, custom_node_installed,
    disable_legacy_manager_clones, emit_install_event, remove_custom_node_dirs, AppState,
};
use crate::host::AppHandle;
use crate::platform::{install_custom_node, resolve_uv_binary, run_uv_pip_strict, CUSTOM_NODES};
use std::path::Path;

pub(crate) fn install_named_custom_node(
    app: &AppHandle,
    root: &Path,
    py_exe: &Path,
    repo_url: &str,
    folder_name: &str,
) -> Result<(), String> {
    let custom_nodes = root.join("custom_nodes");
    std::fs::create_dir_all(&custom_nodes).map_err(|err| err.to_string())?;
    install_custom_node(app, root, &custom_nodes, py_exe, repo_url, folder_name)
}

/// The shared uv binary and its Python install folder.
fn shared_uv(app: &AppHandle) -> Result<(String, String), String> {
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
    Ok((uv_bin, uv_python_install_dir))
}

/// Installs ComfyUI-Manager for the ComfyUI checkout at `comfy_dir`: the
/// built-in package pinned by ComfyUI (started with `--enable-manager`),
/// moving any legacy clone aside, or the legacy git clone for checkouts that
/// predate the built-in manager. Returns what was done.
pub(crate) fn install_comfyui_manager(
    app: &AppHandle,
    comfy_dir: &Path,
    py_exe: &Path,
) -> Result<String, String> {
    let custom_nodes = comfy_dir.join("custom_nodes");
    std::fs::create_dir_all(&custom_nodes).map_err(|err| err.to_string())?;
    let Some(requirements) = builtin_manager_requirements(comfy_dir) else {
        let spec = &CUSTOM_NODES[0];
        install_custom_node(
            app,
            comfy_dir,
            &custom_nodes,
            py_exe,
            spec.repo_url,
            spec.install_folder_name,
        )?;
        return Ok(
            "Installed ComfyUI-Manager as a custom node (this ComfyUI version has no built-in manager)."
                .to_string(),
        );
    };
    emit_install_event(
        app,
        "step",
        "Installing ComfyUI-Manager (built into ComfyUI)...",
    );
    let (uv_bin, uv_python_install_dir) = shared_uv(app)?;
    run_uv_pip_strict(
        &uv_bin,
        &py_exe.to_string_lossy(),
        &[
            "install",
            "-r",
            &requirements.to_string_lossy(),
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(comfy_dir),
        &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
    )?;
    let renamed = disable_legacy_manager_clones(&custom_nodes)?;
    let mut detail =
        "Installed the built-in ComfyUI-Manager; ComfyUI starts with --enable-manager.".to_string();
    if !renamed.is_empty() {
        detail.push_str(&format!(
            " Disabled the old custom-node copy ({}).",
            renamed.join(", ")
        ));
    }
    Ok(detail)
}

/// Removes ComfyUI-Manager from `root`: the built-in package and any legacy
/// clone. Folders a previous install disabled (`*.disabled`) are kept.
pub(crate) fn uninstall_comfyui_manager(
    app: &AppHandle,
    root: &Path,
    py_exe: &Path,
) -> Result<(), String> {
    if builtin_manager_installed(py_exe) {
        let (uv_bin, uv_python_install_dir) = shared_uv(app)?;
        run_uv_pip_strict(
            &uv_bin,
            &py_exe.to_string_lossy(),
            &["uninstall", "comfyui-manager"],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
        )?;
    }
    remove_custom_node_dirs(root, CUSTOM_NODES[0].known_folder_names);
    Ok(())
}

/// Whether `root` has ComfyUI-Manager, built-in or as a legacy clone.
pub(crate) fn comfyui_manager_installed(root: &Path, py_exe: Option<&Path>) -> bool {
    py_exe.is_some_and(builtin_manager_installed) || custom_node_installed(root, &CUSTOM_NODES[0])
}
