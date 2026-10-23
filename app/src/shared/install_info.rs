//! A line about a ComfyUI install for the sidebar: its ComfyUI version and
//! PyTorch build, read from files (no Python is started), so listing the
//! installs stays instant.

use std::path::Path;

/// "v0.42.5 · PyTorch 2.9.1 · CUDA 13.0", or as much of it as is known.
pub(crate) fn install_summary(root: &Path) -> Option<String> {
    let parts: Vec<String> = [
        comfyui_version(root).map(|version| format!("v{version}")),
        torch_stack(root),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// `__version__` from ComfyUI's `comfyui_version.py`.
fn comfyui_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("comfyui_version.py")).ok()?;
    text.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("__version__")?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// "PyTorch 2.9.1 · CUDA 13.0" from the installed torch's version.
fn torch_stack(root: &Path) -> Option<String> {
    let version = torch_dist_version(root)?;
    let (base, local) = version.split_once('+').unwrap_or((&version, ""));
    Some(match accelerator_label(local) {
        Some(accelerator) => format!("PyTorch {base} · {accelerator}"),
        None => format!("PyTorch {base}"),
    })
}

/// The installed torch version, from its `torch-<version>.dist-info` folder
/// in the venv's site-packages (`lib/pythonX.Y/site-packages`, or
/// `Lib/site-packages` on Windows).
fn torch_dist_version(root: &Path) -> Option<String> {
    for venv in [".venv", "venv"] {
        let venv = root.join(venv);
        let mut dirs = vec![venv.join("Lib").join("site-packages")];
        if let Ok(entries) = std::fs::read_dir(venv.join("lib")) {
            dirs.extend(entries.flatten().map(|e| e.path().join("site-packages")));
        }
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // `torch-2.9.1+cu130.dist-info`, not torchvision's or torchaudio's.
                let version = name
                    .strip_prefix("torch-")
                    .and_then(|rest| rest.strip_suffix(".dist-info"))
                    .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()));
                if let Some(version) = version {
                    return Some(version.to_string());
                }
            }
        }
    }
    None
}

/// "cu130" → "CUDA 13.0", "rocm7.2" → "ROCm 7.2", "xpu" → "Intel XPU",
/// "cpu" → "CPU"; anything else as it is.
fn accelerator_label(local: &str) -> Option<String> {
    let local = local.to_ascii_lowercase();
    if let Some(digits) = local.strip_prefix("cu") {
        if digits.len() >= 3 && digits.chars().all(|c| c.is_ascii_digit()) {
            let (major, minor) = digits.split_at(digits.len() - 1);
            return Some(format!("CUDA {major}.{minor}"));
        }
    }
    if let Some(version) = local.strip_prefix("rocm") {
        return Some(format!("ROCm {version}"));
    }
    match local.as_str() {
        "" => None,
        "xpu" => Some("Intel XPU".to_string()),
        "cpu" => Some("CPU".to_string()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("comfypilot-info-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn summary_reads_version_and_torch_build() {
        let root = temp_root("linux");
        fs::write(
            root.join("comfyui_version.py"),
            "# generated\n__version__ = \"0.42.5\"\n",
        )
        .unwrap();
        let site = root.join(".venv/lib/python3.12/site-packages");
        fs::create_dir_all(site.join("torchvision-0.24.1+cu130.dist-info")).unwrap();
        fs::create_dir_all(site.join("torch-2.9.1+cu130.dist-info")).unwrap();
        assert_eq!(
            install_summary(&root).as_deref(),
            Some("v0.42.5 · PyTorch 2.9.1 · CUDA 13.0")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn summary_handles_windows_layout_and_missing_parts() {
        let root = temp_root("windows");
        fs::create_dir_all(root.join("venv/Lib/site-packages/torch-2.14.0+rocm7.1.dist-info"))
            .unwrap();
        assert_eq!(
            install_summary(&root).as_deref(),
            Some("PyTorch 2.14.0 · ROCm 7.1")
        );
        let empty = temp_root("empty");
        assert_eq!(install_summary(&empty), None);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(empty);
    }

    #[test]
    fn accelerator_labels() {
        assert_eq!(accelerator_label("cu128").as_deref(), Some("CUDA 12.8"));
        assert_eq!(accelerator_label("xpu").as_deref(), Some("Intel XPU"));
        assert_eq!(accelerator_label("cpu").as_deref(), Some("CPU"));
        assert_eq!(accelerator_label(""), None);
    }
}
