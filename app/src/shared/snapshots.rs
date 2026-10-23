//! ComfyUI install snapshots in ComfyUI-Manager's format, saved before the
//! app changes an install (ComfyUI updates), so a broken update can be
//! rolled back from ComfyUI-Manager's Snapshot Manager.
//!
//! The format matches what ComfyUI-Manager writes and restores: ComfyUI's
//! commit, each git custom node's remote and commit, registry nodes'
//! versions, single-file nodes, and the installed Python packages (`pip
//! freeze`, as `{"name==version": ""}`).

use crate::platform::run_command_capture;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One custom node folder's state.
#[derive(Debug, PartialEq, Eq)]
enum NodeState {
    Git {
        url: String,
        hash: String,
        disabled: bool,
    },
    Registry {
        id: String,
        version: String,
    },
    File {
        filename: String,
        disabled: bool,
    },
}

/// Saves a snapshot of the install at `root` into ComfyUI-Manager's
/// snapshot folder and returns its path. `uv_bin` lists the Python packages
/// (`uv pip freeze`); without it `python -m pip freeze` is used.
pub fn save_comfyui_snapshot(
    root: &Path,
    py_exe: &Path,
    uv_bin: Option<&str>,
    label: &str,
) -> Result<PathBuf, String> {
    let comfyui_hash = git_head(root);
    let nodes = custom_node_states(&root.join("custom_nodes"));
    let py = py_exe.to_string_lossy().to_string();
    let freeze = match uv_bin {
        Some(uv) => run_command_capture(uv, &["pip", "freeze", "--python", &py], Some(root)),
        None => run_command_capture(&py, &["-m", "pip", "freeze"], Some(root)),
    }
    .map(|(stdout, _)| stdout)
    .map_err(|err| format!("Listing Python packages for the snapshot failed: {err}"))?;
    let snapshot = build_snapshot(comfyui_hash, &nodes, &freeze);

    let dir = snapshot_dir(root);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("Cannot create {}: {err}", dir.display()))?;
    let path = dir.join(format!(
        "{}_{label}.json",
        timestamp_for_path(SystemTime::now())
    ));
    let text = serde_json::to_string_pretty(&snapshot).map_err(|err| err.to_string())?;
    std::fs::write(&path, text).map_err(|err| format!("Cannot write {}: {err}", path.display()))?;
    Ok(path)
}

/// ComfyUI-Manager's snapshot folder for `root`: the built-in manager's,
/// unless only the legacy custom-node manager has been used there.
fn snapshot_dir(root: &Path) -> PathBuf {
    let user = root.join("user");
    let legacy = user.join("default").join("ComfyUI-Manager");
    if !user.join("__manager").exists() && legacy.exists() {
        legacy.join("snapshots")
    } else {
        user.join("__manager").join("snapshots")
    }
}

fn git_head(dir: &Path) -> Option<String> {
    if !dir.join(".git").exists() {
        return None;
    }
    let (stdout, _) = run_command_capture("git", &["rev-parse", "HEAD"], Some(dir)).ok()?;
    let hash = stdout.trim().to_string();
    (hash.len() >= 7).then_some(hash)
}

fn git_origin(dir: &Path) -> Option<String> {
    let (stdout, _) =
        run_command_capture("git", &["config", "--get", "remote.origin.url"], Some(dir)).ok()?;
    let url = stdout.trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// The state of every custom node in `custom_nodes_dir`, including those
/// in ComfyUI-Manager's `.disabled` folder.
fn custom_node_states(custom_nodes_dir: &Path) -> Vec<NodeState> {
    let mut entries: Vec<(PathBuf, bool)> = Vec::new();
    for (dir, disabled) in [
        (custom_nodes_dir.to_path_buf(), false),
        (custom_nodes_dir.join(".disabled"), true),
    ] {
        if let Ok(read) = std::fs::read_dir(&dir) {
            entries.extend(read.flatten().map(|entry| (entry.path(), disabled)));
        }
    }
    let mut states = Vec::new();
    for (path, in_disabled_dir) in entries {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == ".disabled" || name == "__pycache__" {
            continue;
        }
        let disabled = in_disabled_dir || name.ends_with(".disabled");
        if path.is_dir() {
            if let (Some(hash), Some(url)) = (git_head(&path), git_origin(&path)) {
                states.push(NodeState::Git {
                    url,
                    hash,
                    disabled,
                });
            } else if !disabled && path.join(".tracking").is_file() {
                // Installed by ComfyUI-Manager from the Comfy Registry.
                if let Some((id, version)) = pyproject_id_version(&path) {
                    states.push(NodeState::Registry { id, version });
                }
            }
        } else if name.ends_with(".py") || name.ends_with(".py.disabled") {
            states.push(NodeState::File {
                filename: name.clone(),
                disabled: name.ends_with(".disabled"),
            });
        }
    }
    states
}

/// `[project]` name and version from a pack's pyproject.toml.
fn pyproject_id_version(dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join("pyproject.toml")).ok()?;
    let (mut in_project, mut name, mut version) = (false, None, None);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_project = line == "[project]";
            continue;
        }
        if !in_project {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        match key.trim() {
            "name" => name = Some(value),
            "version" => version = Some(value),
            _ => {}
        }
    }
    Some((name?, version?))
}

/// The snapshot document in ComfyUI-Manager's format.
fn build_snapshot(comfyui_hash: Option<String>, nodes: &[NodeState], pip_freeze: &str) -> Value {
    let mut git = Map::new();
    let mut registry = Map::new();
    let mut files = Vec::new();
    for node in nodes {
        match node {
            NodeState::Git {
                url,
                hash,
                disabled,
            } => {
                git.insert(url.clone(), json!({ "hash": hash, "disabled": disabled }));
            }
            NodeState::Registry { id, version } => {
                registry.insert(id.clone(), json!(version));
            }
            NodeState::File { filename, disabled } => {
                files.push(json!({ "filename": filename, "disabled": disabled }));
            }
        }
    }
    let mut pips = Map::new();
    for line in pip_freeze
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        match line.split_once(" @ ") {
            Some((name, url)) => pips.insert(name.to_string(), json!(url)),
            None => pips.insert(line.to_string(), json!("")),
        };
    }
    json!({
        "comfyui": comfyui_hash,
        "git_custom_nodes": git,
        "cnr_custom_nodes": registry,
        "file_custom_nodes": files,
        "pips": pips,
    })
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC), the form ComfyUI-Manager uses in snapshot
/// file names.
fn timestamp_for_path(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}_{:02}-{:02}-{:02}",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_match_comfyui_manager_file_names() {
        let at = |secs| timestamp_for_path(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01_00-00-00");
        assert_eq!(at(1_793_000_000), "2026-10-26_07-33-20");
        assert_eq!(at(951_782_400), "2000-02-29_00-00-00");
    }

    #[test]
    fn snapshot_has_comfyui_managers_shape() {
        let nodes = [
            NodeState::Git {
                url: "https://github.com/kijai/ComfyUI-KJNodes".into(),
                hash: "abc123".into(),
                disabled: false,
            },
            NodeState::Registry {
                id: "comfyui-gguf".into(),
                version: "1.2.0".into(),
            },
            NodeState::File {
                filename: "my_node.py".into(),
                disabled: true,
            },
        ];
        let freeze = "torch==2.14.1+cu130\nmy-pkg @ file:///tmp/my_pkg\n\n";
        let snapshot = build_snapshot(Some("deadbeef".into()), &nodes, freeze);
        assert_eq!(snapshot["comfyui"], "deadbeef");
        assert_eq!(
            snapshot["git_custom_nodes"]["https://github.com/kijai/ComfyUI-KJNodes"],
            json!({ "hash": "abc123", "disabled": false })
        );
        assert_eq!(snapshot["cnr_custom_nodes"]["comfyui-gguf"], "1.2.0");
        assert_eq!(snapshot["file_custom_nodes"][0]["filename"], "my_node.py");
        assert_eq!(snapshot["pips"]["torch==2.14.1+cu130"], "");
        assert_eq!(snapshot["pips"]["my-pkg"], "file:///tmp/my_pkg");
    }

    #[test]
    fn snapshots_go_to_the_managers_folder() {
        let root = std::env::temp_dir().join(format!("arctic-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("user/default/ComfyUI-Manager")).unwrap();
        assert_eq!(
            snapshot_dir(&root),
            root.join("user/default/ComfyUI-Manager/snapshots")
        );
        std::fs::create_dir_all(root.join("user/__manager")).unwrap();
        assert_eq!(snapshot_dir(&root), root.join("user/__manager/snapshots"));
        let _ = std::fs::remove_dir_all(root);
    }
}
