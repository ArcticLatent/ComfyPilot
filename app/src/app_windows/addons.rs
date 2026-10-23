//! Per-addon installers (InsightFace, Trellis2, Nunchaku) for the Windows
//! backend -- the bespoke, GPU/toolchain-branching installers, as opposed to
//! `app_windows/custom_nodes.rs`'s generic git-clone-plus-`pip install`
//! primitive.
//!
//! Windows counterpart to `app_linux/addons.rs`, but not a mirror of it:
//! Windows' InsightFace path is substantially larger and structurally
//! different from Linux's. Linux installs InsightFace from a single
//! precompiled wheel (`install_linux_wheel_for_profile`) and is done. Windows
//! has no such wheel, so it pip-installs from source with an MSVC Build
//! Tools fallback (`install_insightface_variant`,
//! `looks_like_missing_msvc_tools`) and then retries against a numpy/opencv
//! ABI-mismatch loop (`ensure_insightface_runtime_compat`) that Linux simply
//! doesn't need. Trellis2 similarly pulls from a different upstream repo
//! with different prebuilt wheels than Linux's. None of this is unified here
//! -- it's real, load-bearing platform divergence, physically relocated but
//! not refactored into parity.

use crate::shared::{emit_install_event, remove_custom_node_dirs};
// General command-running/download/toolchain utilities defined in the
// parent `app_windows` module itself (not `shared.rs`), used well beyond
// these addon installers.
use super::{
    download_http_file, download_nunchaku_versions_json, install_visual_cpp_build_tools,
    looks_like_missing_msvc_tools, python_module_import_error, python_module_importable,
    run_command_env, run_uv_pip_strict, uv_pip_uninstall_best_effort,
};
use crate::host::AppHandle;
use std::path::Path;

pub(crate) fn install_insightface(
    app: &AppHandle,
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    uv_pip_uninstall_best_effort(
        uv_bin,
        Path::new(py_path),
        root,
        uv_python_install_dir,
        &["insightface", "filterpywhl", "facexlib"],
    )?;
    match install_insightface_variant(root, uv_bin, py_path, uv_python_install_dir) {
        Ok(()) => {}
        Err(err) if looks_like_missing_msvc_tools(&err) => {
            emit_install_event(
                app,
                "warn",
                "InsightFace requires Microsoft Visual C++ Build Tools. Installing them automatically...",
            );
            install_visual_cpp_build_tools(app)?;
            emit_install_event(
                app,
                "step",
                "Retrying InsightFace installation after Build Tools install...",
            );
            install_insightface_variant(root, uv_bin, py_path, uv_python_install_dir)?;
        }
        Err(err) => return Err(err),
    }
    ensure_insightface_runtime_compat(root, uv_bin, py_path, uv_python_install_dir)
}

fn install_insightface_variant(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--force-reinstall",
            "numpy==1.26.4",
            "opencv-python==4.11.0.86",
            "opencv-python-headless==4.11.0.86",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--force-reinstall",
            "insightface==0.7.3",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    // InsightFace imports pull in several runtime deps in a plain venv.
    // Install these explicitly, then re-pin numpy below for ABI stability.
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--upgrade",
            "scikit-image",
            "scikit-learn",
            "easydict",
            "prettytable",
            "albumentations",
            "cython",
            "matplotlib",
            "facexlib",
            "filterpywhl",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--force-reinstall",
            "numpy==1.26.4",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    if !python_module_importable(root, "cv2") {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &[
                "install",
                "--upgrade",
                "opencv-python==4.11.0.86",
                "opencv-python-headless==4.11.0.86",
                "--no-cache-dir",
                "--timeout=1000",
                "--retries",
                "10",
            ],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }
    Ok(())
}

pub(crate) fn ensure_insightface_runtime_compat(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    for _ in 0..5 {
        let Some(err) = python_module_import_error(root, "insightface.app") else {
            cleanup_tilde_site_packages(root);
            return Ok(());
        };

        let expected_96_got_88 = err.contains("numpy.dtype size changed")
            && err.contains("Expected 96")
            && err.contains("got 88");
        let expected_88_got_96 = err.contains("numpy.dtype size changed")
            && err.contains("Expected 88")
            && err.contains("got 96");
        let missing_cv2 = err.contains("No module named 'cv2'");
        let missing_skimage = err.contains("No module named 'skimage'");

        if expected_96_got_88 {
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &[
                    "install",
                    "--force-reinstall",
                    "numpy==1.26.4",
                    "opencv-python==4.11.0.86",
                    "opencv-python-headless==4.11.0.86",
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &[
                    "install",
                    "--force-reinstall",
                    "insightface==0.7.3",
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
            cleanup_tilde_site_packages(root);
        } else if expected_88_got_96 {
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &[
                    "install",
                    "--force-reinstall",
                    "numpy==1.26.4",
                    "opencv-python==4.11.0.86",
                    "opencv-python-headless==4.11.0.86",
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
            cleanup_tilde_site_packages(root);
        } else if missing_cv2 {
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &[
                    "install",
                    "--upgrade",
                    "opencv-python==4.11.0.86",
                    "opencv-python-headless==4.11.0.86",
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
            cleanup_tilde_site_packages(root);
        } else if missing_skimage {
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &[
                    "install",
                    "--upgrade",
                    "scikit-image",
                    "--no-cache-dir",
                    "--timeout=1000",
                    "--retries",
                    "10",
                ],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
            cleanup_tilde_site_packages(root);
        } else {
            return Err(format!("InsightFace install incomplete: {err}"));
        }
    }
    if let Some(err2) = python_module_import_error(root, "insightface.app") {
        return Err(format!("InsightFace install incomplete: {err2}"));
    }
    cleanup_tilde_site_packages(root);
    Ok(())
}

fn cleanup_tilde_site_packages(root: &Path) {
    let site_packages = root.join(".venv").join("Lib").join("site-packages");
    let Ok(entries) = std::fs::read_dir(&site_packages) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with('~') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) fn finalize_nunchaku_install(
    app: &AppHandle,
    root: &Path,
    _uv_bin: &str,
    _py_path: &str,
    _uv_python_install_dir: &str,
    nunchaku_node: &Path,
) -> Result<(), String> {
    // Match linux flow: fetch versions JSON and cleanup stale temp site-packages artifacts.
    let nunchaku_versions_path = nunchaku_node.join("nunchaku_versions.json");
    let _ = download_nunchaku_versions_json(app, &nunchaku_versions_path);

    cleanup_tilde_site_packages(root);

    Ok(())
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
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &["install", "--upgrade", "accelerate", "diffusers"],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    Ok(())
}

pub(crate) fn uninstall_insightface(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    uv_pip_uninstall_best_effort(
        uv_bin,
        Path::new(py_path),
        root,
        uv_python_install_dir,
        &["insightface", "filterpywhl", "facexlib"],
    )?;
    Ok(())
}

pub(crate) fn install_trellis2(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    let model_folder = root
        .join("models")
        .join("facebook")
        .join("dinov3-vitl16-pretrain-lvd1689m");
    std::fs::create_dir_all(&model_folder).map_err(|err| err.to_string())?;
    let model_file = model_folder.join("model.safetensors");
    if let Ok(meta) = std::fs::metadata(&model_file) {
        if meta.len() < 1_212_559_800 {
            let _ = std::fs::remove_file(&model_file);
        }
    }
    download_http_file(
        "https://huggingface.co/PIA-SPACE-LAB/dinov3-vitl-pretrain-lvd1689m/resolve/main/model.safetensors",
        &model_file,
    )?;
    download_http_file(
        "https://huggingface.co/PIA-SPACE-LAB/dinov3-vitl-pretrain-lvd1689m/resolve/main/config.json",
        &model_folder.join("config.json"),
    )?;
    download_http_file(
        "https://huggingface.co/PIA-SPACE-LAB/dinov3-vitl-pretrain-lvd1689m/resolve/main/preprocessor_config.json",
        &model_folder.join("preprocessor_config.json"),
    )?;

    let venv_dir = root.join(".venv");
    let site_packages = venv_dir.join("Lib").join("site-packages");
    for stale in [
        "o_voxel",
        "o_voxel-0.0.1.dist-info",
        "cumesh",
        "cumesh-0.0.1.dist-info",
        "nvdiffrast",
        "nvdiffrast-0.4.0.dist-info",
        "nvdiffrec_render",
        "nvdiffrec_render-0.0.0.dist-info",
        "flex_gemm",
        "flex_gemm-0.0.1.dist-info",
    ] {
        let path = site_packages.join(stale);
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    let addon_root = root.join("custom_nodes");
    std::fs::create_dir_all(&addon_root).map_err(|err| err.to_string())?;
    let trellis_node = addon_root.join("ComfyUI-Trellis2");
    if trellis_node.exists() {
        let _ = std::fs::remove_dir_all(&trellis_node);
    }
    run_command_env(
        "git",
        &[
            "clone",
            "https://github.com/visualbruno/ComfyUI-Trellis2",
            &trellis_node.to_string_lossy(),
        ],
        Some(root),
        &[("GIT_LFS_SKIP_SMUDGE", "1")],
    )?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "-r",
            &trellis_node.join("requirements.txt").to_string_lossy(),
            "--no-deps",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--upgrade",
            "open3d",
            "--no-cache-dir",
            "--timeout=1000",
            "--retries",
            "10",
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    let wheel_root = trellis_node.join("wheels").join("Windows").join("Torch280");
    for wheel in [
        "cumesh-0.0.1-cp312-cp312-win_amd64.whl",
        "nvdiffrast-0.4.0-cp312-cp312-win_amd64.whl",
        "nvdiffrec_render-0.0.0-cp312-cp312-win_amd64.whl",
        "flex_gemm-0.0.1-cp312-cp312-win_amd64.whl",
        "o_voxel-0.0.1-cp312-cp312-win_amd64.whl",
    ] {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &["install", &wheel_root.join(wheel).to_string_lossy()],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }
    download_http_file(
        "https://raw.githubusercontent.com/visualbruno/CuMesh/main/cumesh/remeshing.py",
        &site_packages.join("cumesh").join("remeshing.py"),
    )?;
    Ok(())
}

pub(crate) fn uninstall_trellis2(
    root: &Path,
    uv_bin: &str,
    py_path: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    remove_custom_node_dirs(root, &["ComfyUI-Trellis2"]);
    uv_pip_uninstall_best_effort(
        uv_bin,
        Path::new(py_path),
        root,
        uv_python_install_dir,
        &[
            "o_voxel",
            "cumesh",
            "nvdiffrast",
            "nvdiffrec_render",
            "flex_gemm",
            "open3d",
        ],
    )?;
    Ok(())
}
