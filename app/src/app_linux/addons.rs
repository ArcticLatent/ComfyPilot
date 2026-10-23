//! Per-addon installers (SageAttention, FlashAttention, InsightFace,
//! Nunchaku, Trellis2) for the Linux backend -- the bespoke, wheel/GPU-
//! branching installers, as opposed to `app_linux/custom_nodes.rs`'s
//! generic git-clone-plus-`pip install` primitive.
//!
//! Windows counterpart: `app_windows/addons.rs`. Not a mirror of it --
//! Linux installs InsightFace (and the attention backends) from a single
//! precompiled wheel keyed by torch profile and Hopper/SM90-ness
//! (`linux_wheel_url`, `install_linux_wheel_for_profile`); Windows has no
//! such wheel for InsightFace and instead pip-installs from source with an
//! MSVC Build Tools fallback and a numpy/opencv ABI-mismatch retry loop.
//! Trellis2 pulls from an entirely different upstream repo per platform
//! with different prebuilt wheels. None of this is unified here -- it's
//! real, load-bearing platform divergence, physically relocated but not
//! refactored into parity.

use crate::shared::remove_custom_node_dirs;
// General command-running/python-introspection utilities defined in the
// parent `app_linux` module itself (not `shared.rs`), used well beyond
// these addon installers.
use super::{
    clone_or_update_repo, discover_uv_binary, download_http_file, enforce_torch_profile_linux,
    is_nvidia_hopper_sm90, pip_has_package, pip_uninstall_best_effort, profile_from_torch_env,
    python_module_importable, run_command_capture, run_uv_pip_strict,
};
use std::path::Path;

fn linux_wheel_url(profile: &str, wheel_kind: &str, hopper_sm90: bool) -> Option<&'static str> {
    match (profile, wheel_kind, hopper_sm90) {
        // These builds include Hopper (sm_90a) wherever the add-on supports
        // it, so one folder serves every GPU; built and tested with
        // scripts/accelerator-wheels/.
        ("torch2141_cu130", "flash", _) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch2141-py312/flash_attn-2.8.3.post2-cp312-cp312-linux_x86_64.whl"),
        ("torch2141_cu130", "insightface", _) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch2141-py312/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch2141_cu130", "nunchaku", _) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch2141-py312/nunchaku-1.3.0.dev20261023%2Bcu13.0torch2.14-cp312-cp312-linux_x86_64.whl"),
        ("torch2141_cu130", "sage", _) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch2141-py312/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch2141_cu130", "sage3", _) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch2141-py312/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "flash", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312-sm90/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "insightface", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312-sm90/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "nunchaku", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312-sm90/nunchaku-1.3.0.dev20260215%2Bcu12.8torch2.7-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "sage", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312-sm90/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "sage3", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312-sm90/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "flash", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312-sm90/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "insightface", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312-sm90/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "nunchaku", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312-sm90/nunchaku-1.3.0.dev20260215%2Bcu12.8torch2.8-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "sage", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312-sm90/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "sage3", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312-sm90/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "flash", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312-sm90/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "insightface", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312-sm90/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "nunchaku", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312-sm90/nunchaku-1.3.0.dev20260215%2Bcu13.0torch2.9-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "sage", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312-sm90/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "sage3", true) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312-sm90/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "flash", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "insightface", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "nunchaku", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312/nunchaku-1.3.0.dev20260215%2Bcu12.8torch2.7-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "sage", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch271_cu128", "sage3", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch271-py312/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "flash", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "insightface", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "nunchaku", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312/nunchaku-1.3.0.dev20260215%2Bcu12.8torch2.8-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "sage", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch280_cu128", "sage3", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu128-torch280-py312/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "flash", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312/flash_attn-2.8.3-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "insightface", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312/insightface-0.7.3-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "nunchaku", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312/nunchaku-1.3.0.dev20260215%2Bcu13.0torch2.9-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "sage", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312/sageattention-2.2.0-cp312-cp312-linux_x86_64.whl"),
        ("torch291_cu130", "sage3", false) => Some("https://huggingface.co/arcticlatent/accelerator/resolve/main/cu130-torch291-py312/sageattn3-1.0.0-cp312-cp312-linux_x86_64.whl"),
        _ => None,
    }
}

pub(crate) fn install_linux_wheel_for_profile(
    root: &Path,
    py_path: &str,
    profile: &str,
    wheel_kind: &str,
    hopper_sm90: bool,
    force_reinstall: bool,
) -> Result<(), String> {
    let wheel = linux_wheel_url(profile, wheel_kind, hopper_sm90).ok_or_else(|| {
        format!("No Linux wheel mapping for profile '{profile}' and wheel '{wheel_kind}'.")
    })?;
    let uv_bin = discover_uv_binary().ok_or_else(|| {
        "uv runtime not found. Install uv first or run Install ComfyUI to auto-bootstrap."
            .to_string()
    })?;
    let mut args: Vec<&str> = vec!["install", "--upgrade"];
    if force_reinstall {
        args.push("--reinstall");
    }
    // These are precompiled stack-pinned wheels; let selected torch profile stay authoritative.
    args.push("--no-deps");
    args.push(wheel);
    run_uv_pip_strict(&uv_bin, py_path, &args, Some(root), &[])
}

pub(crate) fn install_sageattention_linux(
    root: &Path,
    py_path: &str,
    profile: &str,
    hopper_sm90: bool,
) -> Result<(), String> {
    install_linux_wheel_for_profile(root, py_path, profile, "sage", hopper_sm90, true)
}

pub(crate) fn install_flashattention_linux(
    root: &Path,
    py_path: &str,
    profile: &str,
    hopper_sm90: bool,
) -> Result<(), String> {
    install_linux_wheel_for_profile(root, py_path, profile, "flash", hopper_sm90, true)
}

fn prewarm_matplotlib_cache(root: &Path, py_path: &str) {
    let mpl_cache = root.join(".venv").join("var").join("matplotlib");
    let _ = std::fs::create_dir_all(&mpl_cache);
    let code = format!(
        "import os, logging; \
os.environ.setdefault('MPLBACKEND', 'Agg'); \
os.environ['MPLCONFIGDIR'] = r'''{}'''; \
logging.getLogger('matplotlib.font_manager').setLevel(logging.ERROR); \
import matplotlib; matplotlib.use('Agg', force=True); \
from matplotlib import font_manager as fm; \
fm._load_fontmanager(try_read_cache=False)",
        mpl_cache.to_string_lossy()
    );
    let _ = run_command_capture(py_path, &["-c", &code], Some(root));
}

pub(crate) fn install_nunchaku_node_requirements(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
    nunchaku_node: &Path,
) -> Result<(), String> {
    let req = nunchaku_node.join("requirements.txt");
    if req.exists() {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &["install", "-r", &req.to_string_lossy()],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }
    // ComfyUI-nunchaku imports these directly for multiple nodes (Flux/IPAdapter/PuLID).
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &["install", "--upgrade", "accelerate", "diffusers"],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    if !python_module_importable(root, "accelerate") {
        return Err("Nunchaku install incomplete: missing 'accelerate' module.".to_string());
    }
    if !python_module_importable(root, "diffusers") {
        return Err("Nunchaku install incomplete: missing 'diffusers' module.".to_string());
    }
    prewarm_matplotlib_cache(root, py_path);
    Ok(())
}

fn insightface_present(root: &Path) -> bool {
    pip_has_package(root, "insightface") || python_module_importable(root, "insightface")
}

fn remove_insightface_site_packages_artifacts(root: &Path) -> Result<(), String> {
    let markers = vec![
        super::normalize_pkg_token("insightface"),
        super::normalize_pkg_token("facexlib"),
        super::normalize_pkg_token("filterpywhl"),
    ];
    super::remove_site_packages_artifacts_with_markers(root, &markers)
}

pub(crate) fn install_insightface(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    let profile = profile_from_torch_env(root)?;
    install_linux_wheel_for_profile(
        root,
        py_path,
        &profile,
        "insightface",
        is_nvidia_hopper_sm90(),
        true,
    )?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &["install", "--upgrade", "onnx", "onnxruntime"],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    // The wheel goes in with --no-deps, so add what `insightface.app` imports.
    // OpenCV only when none is importable: custom nodes often bring
    // opencv-python, and a second OpenCV distribution conflicts with it.
    let mut deps = vec![
        "install",
        "requests",
        "tqdm",
        "matplotlib",
        "scikit-image",
        "albumentations",
    ];
    if !python_module_importable(root, "cv2") {
        deps.push("opencv-python-headless");
    }
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &deps,
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    if !python_module_importable(root, "onnx") {
        return Err("InsightFace install incomplete: missing 'onnx' module.".to_string());
    }
    if !insightface_present(root) {
        return Err("InsightFace install incomplete: package/module not detected.".to_string());
    }
    if !python_module_importable(root, "insightface.app") {
        return Err(
            "InsightFace installed but `insightface.app` fails to import; check the install log."
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn uninstall_insightface(
    root: &Path,
    _uv_bin: &str,
    py_path: &str,
    _uv_python_install_dir: &str,
) -> Result<(), String> {
    pip_uninstall_best_effort(root, py_path, &["insightface", "filterpywhl", "facexlib"]);
    remove_insightface_site_packages_artifacts(root)?;
    if insightface_present(root)
        || pip_has_package(root, "facexlib")
        || pip_has_package(root, "filterpywhl")
    {
        return Err(
            "Failed to fully remove InsightFace dependencies. Stop ComfyUI and retry.".to_string(),
        );
    }
    Ok(())
}

pub(crate) fn install_trellis2(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    // Trellis2 stack is pinned to torch280_cu128 in this app.
    enforce_torch_profile_linux(
        uv_bin,
        py_path,
        root,
        "torch280_cu128",
        uv_python_install_dir,
    )?;

    let custom_nodes_dir = root.join("custom_nodes");
    std::fs::create_dir_all(&custom_nodes_dir).map_err(|err| err.to_string())?;

    let trellis_dir = custom_nodes_dir.join("ComfyUI-TRELLIS2");
    clone_or_update_repo(
        root,
        &trellis_dir,
        "https://github.com/ArcticLatent/ComfyUI-TRELLIS2",
    )?;
    let trellis_req = trellis_dir.join("requirements.txt");
    if trellis_req.exists() {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &["install", "-r", &trellis_req.to_string_lossy(), "--no-deps"],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &["install", "--upgrade", "open3d"],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }

    let geometry_dir = custom_nodes_dir.join("ComfyUI-GeometryPack");
    clone_or_update_repo(
        root,
        &geometry_dir,
        "https://github.com/PozzettiAndrea/ComfyUI-GeometryPack",
    )?;
    let geometry_req = geometry_dir.join("requirements.txt");
    if geometry_req.exists() {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &[
                "install",
                "-r",
                &geometry_req.to_string_lossy(),
                "--no-deps",
            ],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &["install", "--upgrade", "tomli"],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;

    let ultrashape_dir = custom_nodes_dir.join("ComfyUI-UltraShape1");
    clone_or_update_repo(
        root,
        &ultrashape_dir,
        "https://github.com/jtydhr88/ComfyUI-UltraShape1",
    )?;
    let ultrashape_req = ultrashape_dir.join("requirements.txt");
    if ultrashape_req.exists() {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &[
                "install",
                "-r",
                &ultrashape_req.to_string_lossy(),
                "--no-deps",
            ],
            Some(&ultrashape_dir),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &["install", "-U", "accelerate"],
            Some(&ultrashape_dir),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }

    let ultrashape_models_dir = root.join("models").join("UltraShape");
    std::fs::create_dir_all(&ultrashape_models_dir).map_err(|err| err.to_string())?;
    let ultrashape_model_file = ultrashape_models_dir.join("ultrashape_v1.pt");
    if !ultrashape_model_file.exists() {
        download_http_file(
            "https://huggingface.co/infinith/UltraShape/resolve/main/ultrashape_v1.pt",
            &ultrashape_model_file,
        )?;
    }

    // Re-assert stack after Trellis requirements/custom nodes.
    enforce_torch_profile_linux(
        uv_bin,
        py_path,
        root,
        "torch280_cu128",
        uv_python_install_dir,
    )?;

    Ok(())
}

pub(crate) fn uninstall_trellis2(
    root: &Path,
    _uv_bin: &str,
    py_path: &str,
    _uv_python_install_dir: &str,
) -> Result<(), String> {
    remove_custom_node_dirs(
        root,
        &[
            "ComfyUI-TRELLIS2",
            "ComfyUI-GeometryPack",
            "ComfyUI-UltraShape1",
        ],
    );
    pip_uninstall_best_effort(root, py_path, &["accelerate", "open3d"]);
    Ok(())
}
