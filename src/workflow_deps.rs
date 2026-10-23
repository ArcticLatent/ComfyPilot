//! What a ComfyUI workflow needs: the model files and custom node packs it
//! references, read from the workflow JSON, and checks against an install.
//!
//! Current ComfyUI saves this information in the workflow itself: loader
//! nodes list their models (`properties.models`: file name, model folder and
//! download URL) and every node records the pack it comes from
//! (`properties.cnr_id`, a Comfy Registry id, `comfy-core` for built-in
//! nodes; or `properties.aux_id`, a GitHub `owner/repo`). Older workflows
//! have neither, so model file names are also picked out of node widget
//! values, and nodes without pack information are reported as unresolved.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// File extensions treated as model files in widget values.
const MODEL_EXTENSIONS: &[&str] = &[
    ".safetensors",
    ".ckpt",
    ".pt",
    ".pth",
    ".bin",
    ".gguf",
    ".sft",
    ".onnx",
];

/// Nodes the ComfyUI frontend provides itself; they belong to no pack.
const FRONTEND_NODE_TYPES: &[&str] = &[
    "Note",
    "MarkdownNote",
    "Reroute",
    "PrimitiveNode",
    "PrimitiveString",
    "PrimitiveInt",
    "PrimitiveFloat",
    "PrimitiveBoolean",
];

/// Model folder for widget-only model references of common core loaders
/// (older workflows without `properties.models`).
fn loader_directory(node_type: &str) -> Option<&'static str> {
    Some(match node_type {
        "CheckpointLoaderSimple" | "CheckpointLoader" | "ImageOnlyCheckpointLoader" => {
            "checkpoints"
        }
        "UNETLoader" | "UnetLoaderGGUF" | "UnetLoaderGGUFAdvanced" => "diffusion_models",
        "VAELoader" => "vae",
        "CLIPLoader"
        | "DualCLIPLoader"
        | "TripleCLIPLoader"
        | "QuadrupleCLIPLoader"
        | "CLIPLoaderGGUF"
        | "DualCLIPLoaderGGUF" => "text_encoders",
        "LoraLoader" | "LoraLoaderModelOnly" => "loras",
        "ControlNetLoader" | "DiffControlNetLoader" => "controlnet",
        "CLIPVisionLoader" => "clip_vision",
        "UpscaleModelLoader" => "upscale_models",
        "StyleModelLoader" => "style_models",
        _ => return None,
    })
}

/// A model file a workflow loads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkflowModel {
    /// File name as the workflow names it (may include a sub-folder).
    pub name: String,
    /// ComfyUI model folder (`diffusion_models`, `vae`, ...), when known.
    pub directory: Option<String>,
    /// Download URL recorded in the workflow, if any.
    pub url: Option<String>,
    pub node_type: String,
}

/// Where a custom node pack comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSource {
    /// Comfy Registry id (`cnr_id`).
    Registry,
    /// GitHub `owner/repo` (`aux_id`).
    GitHub,
}

/// A custom node pack a workflow uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkflowNodePack {
    pub id: String,
    pub source: PackSource,
    /// Version or commit the workflow was saved with, if recorded.
    pub version: Option<String>,
    pub node_types: Vec<String>,
}

/// Everything a workflow needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WorkflowRequirements {
    pub models: Vec<WorkflowModel>,
    pub packs: Vec<WorkflowNodePack>,
    /// Node types with no pack information (older workflows, or nodes the
    /// saving ComfyUI couldn't attribute).
    pub unresolved_node_types: Vec<String>,
}

/// Reads a workflow in ComfyUI's UI format (`nodes`, with subgraphs under
/// `definitions.subgraphs`).
pub fn parse_workflow_requirements(workflow: &Value) -> Result<WorkflowRequirements, String> {
    let top_nodes = workflow
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Not a ComfyUI workflow: no \"nodes\" list. Workflows saved in API format can't be checked; save the workflow from ComfyUI's menu instead."
                .to_string()
        })?;
    let subgraphs: Vec<&Value> = workflow
        .pointer("/definitions/subgraphs")
        .and_then(Value::as_array)
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    // A subgraph instance's node type is the subgraph's id, not a node class.
    let subgraph_ids: HashSet<&str> = subgraphs
        .iter()
        .filter_map(|subgraph| subgraph.get("id").and_then(Value::as_str))
        .collect();
    let nodes = top_nodes.iter().chain(
        subgraphs
            .iter()
            .filter_map(|subgraph| subgraph.get("nodes").and_then(Value::as_array))
            .flatten(),
    );

    let mut models: BTreeMap<String, WorkflowModel> = BTreeMap::new();
    let mut packs: BTreeMap<(PackSource, String), WorkflowNodePack> = BTreeMap::new();
    let mut unresolved: BTreeSet<String> = BTreeSet::new();

    for node in nodes {
        let Some(node_type) = node.get("type").and_then(Value::as_str) else {
            continue;
        };
        if subgraph_ids.contains(node_type) {
            continue;
        }
        let properties = node.get("properties");
        let property = |key: &str| {
            properties
                .and_then(|props| props.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };

        // Pack attribution.
        let pack = match (property("cnr_id"), property("aux_id")) {
            (Some("comfy-core"), _) => None,
            (Some(id), _) => Some((PackSource::Registry, id.to_string())),
            (None, Some(repo)) => Some((PackSource::GitHub, repo.to_string())),
            (None, None) => {
                if !FRONTEND_NODE_TYPES.contains(&node_type) {
                    unresolved.insert(node_type.to_string());
                }
                None
            }
        };
        if let Some(key) = pack {
            let entry = packs
                .entry(key.clone())
                .or_insert_with(|| WorkflowNodePack {
                    id: key.1.clone(),
                    source: key.0,
                    version: property("ver").map(str::to_string),
                    node_types: Vec::new(),
                });
            if !entry
                .node_types
                .iter()
                .any(|existing| existing == node_type)
            {
                entry.node_types.push(node_type.to_string());
            }
        }

        // Models with metadata.
        let listed = properties
            .and_then(|props| props.get("models"))
            .and_then(Value::as_array);
        for model in listed.into_iter().flatten() {
            let Some(name) = model.get("name").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let text = |key: &str| {
                model
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            let entry = models
                .entry(name.to_string())
                .or_insert_with(|| WorkflowModel {
                    name: name.to_string(),
                    directory: None,
                    url: None,
                    node_type: node_type.to_string(),
                });
            entry.directory = entry.directory.take().or_else(|| text("directory"));
            entry.url = entry.url.take().or_else(|| text("url"));
        }

        // Model files only named in widget values.
        let widgets = node.get("widgets_values").and_then(Value::as_array);
        for value in widgets.into_iter().flatten().filter_map(Value::as_str) {
            let value = value.trim();
            let lower = value.to_ascii_lowercase();
            if !MODEL_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) || value.contains("://") {
                continue;
            }
            models
                .entry(value.to_string())
                .or_insert_with(|| WorkflowModel {
                    name: value.to_string(),
                    directory: loader_directory(node_type).map(str::to_string),
                    url: None,
                    node_type: node_type.to_string(),
                });
        }
    }

    Ok(WorkflowRequirements {
        models: models.into_values().collect(),
        packs: packs.into_values().collect(),
        unresolved_node_types: unresolved.into_iter().collect(),
    })
}

/// Node types built into the ComfyUI checkout at `comfy_dir`, read from its
/// Python sources: `NODE_CLASS_MAPPINGS` keys and `node_id="..."` schemas.
/// Empty when the sources can't be read.
pub fn core_node_types(comfy_dir: &Path) -> HashSet<String> {
    fn python_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth > 0 {
                    python_files(&path, depth - 1, files);
                }
            } else if path.extension().is_some_and(|ext| ext == "py") {
                files.push(path);
            }
        }
    }
    let mut files = vec![comfy_dir.join("nodes.py")];
    for dir in ["comfy_extras", "comfy_api_nodes"] {
        python_files(&comfy_dir.join(dir), 3, &mut files);
    }
    let mut types = HashSet::new();
    for file in files {
        if let Ok(source) = std::fs::read_to_string(&file) {
            collect_core_node_types(&source, &mut types);
        }
    }
    types
}

/// Adds the node types declared in one Python source file.
fn collect_core_node_types(source: &str, types: &mut HashSet<String>) {
    // Text up to the next matching quote after `start` (which must be a quote).
    fn quoted(rest: &str) -> Option<&str> {
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = rest[1..].find(quote)?;
        Some(&rest[1..1 + end])
    }
    for (index, _) in source.match_indices("node_id=") {
        if let Some(name) = quoted(source[index + "node_id=".len()..].trim_start()) {
            types.insert(name.to_string());
        }
    }
    for (index, _) in source.match_indices("NODE_CLASS_MAPPINGS") {
        let rest = &source[index..];
        let Some(open) = rest
            .find('{')
            .filter(|open| rest[..*open].trim_end().ends_with('='))
        else {
            continue;
        };
        let body = &rest[open + 1..];
        let body = &body[..body.find('}').unwrap_or(body.len())];
        for line in body.lines() {
            if let Some(name) = quoted(line.trim_start()) {
                if line.trim_start()[name.len() + 2..]
                    .trim_start()
                    .starts_with(':')
                {
                    types.insert(name.to_string());
                }
            }
        }
    }
}

/// The file name part of a model name (`sub/dir/x.safetensors` -> `x.safetensors`),
/// accepting either separator since workflows saved on Windows use `\`.
pub fn model_file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Lower-cased file names of every file under the given model folders
/// (searched a few levels deep; models are often sorted into sub-folders).
pub fn index_model_files(model_roots: &[PathBuf]) -> HashSet<String> {
    fn walk(dir: &Path, depth: usize, found: &mut HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, found);
                }
            } else {
                found.insert(entry.file_name().to_string_lossy().to_ascii_lowercase());
            }
        }
    }
    let mut found = HashSet::new();
    for root in model_roots {
        walk(root, 5, &mut found);
    }
    found
}

/// An installed custom node folder and what identifies it.
#[derive(Clone, Debug, Default)]
pub struct InstalledPack {
    pub folder: String,
    /// `[project] name` from its pyproject.toml (the Comfy Registry id for
    /// registry packs).
    pub project_name: Option<String>,
    /// `origin` remote URL of its git checkout.
    pub repo_url: Option<String>,
}

/// The custom node packs in `custom_nodes_dir` (disabled `*.disabled`
/// folders excluded).
pub fn installed_packs(custom_nodes_dir: &Path) -> Vec<InstalledPack> {
    let Ok(entries) = std::fs::read_dir(custom_nodes_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let folder = entry.file_name().to_string_lossy().to_string();
            if folder.ends_with(".disabled") || folder.starts_with('.') || folder == "__pycache__" {
                return None;
            }
            let path = entry.path();
            Some(InstalledPack {
                project_name: std::fs::read_to_string(path.join("pyproject.toml"))
                    .ok()
                    .and_then(|text| toml_project_name(&text)),
                repo_url: std::fs::read_to_string(path.join(".git").join("config"))
                    .ok()
                    .and_then(|text| git_origin_url(&text)),
                folder,
            })
        })
        .collect()
}

/// `name` from the `[project]` table of a pyproject.toml.
fn toml_project_name(text: &str) -> Option<String> {
    let mut in_project = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_project = line == "[project]";
            continue;
        }
        if in_project {
            if let Some(value) = line.strip_prefix("name").map(str::trim_start) {
                if let Some(value) = value.strip_prefix('=') {
                    let name = value.trim().trim_matches(|c| c == '"' || c == '\'');
                    return (!name.is_empty()).then(|| name.to_string());
                }
            }
        }
    }
    None
}

/// The `origin` remote URL from a `.git/config`.
fn git_origin_url(text: &str) -> Option<String> {
    let mut in_origin = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
            continue;
        }
        if in_origin {
            if let Some(value) = line.strip_prefix("url") {
                let url = value.trim_start().strip_prefix('=')?.trim();
                return (!url.is_empty()).then(|| url.to_string());
            }
        }
    }
    None
}

/// `owner/repo` of a GitHub URL, lower-cased, without `.git`.
fn github_slug(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split_once("github.com")?
        .1
        .trim_start_matches([':', '/']);
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}").to_ascii_lowercase())
}

/// Whether `pack` is among the installed packs. `repository` is the pack's
/// GitHub URL when known (from the Comfy Registry, for registry packs).
pub fn pack_installed(
    pack: &WorkflowNodePack,
    repository: Option<&str>,
    installed: &[InstalledPack],
) -> bool {
    let id = pack.id.to_ascii_lowercase();
    let wanted_slug = match pack.source {
        PackSource::GitHub => github_slug(&format!("github.com/{}", pack.id)),
        PackSource::Registry => repository.and_then(github_slug),
    };
    let wanted_repo_name = wanted_slug
        .as_deref()
        .and_then(|slug| slug.split('/').nth(1))
        .map(str::to_string);
    installed.iter().any(|candidate| {
        let folder = candidate.folder.to_ascii_lowercase();
        folder == id
            || wanted_repo_name.as_deref() == Some(folder.as_str())
            || candidate
                .project_name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&pack.id))
            || (wanted_slug.is_some()
                && candidate.repo_url.as_deref().and_then(github_slug) == wanted_slug)
    })
}

/// A model folder name that is safe to join onto a models root.
pub fn safe_model_directory(directory: &str) -> Option<&str> {
    let directory = directory.trim();
    let valid = !directory.is_empty()
        && directory
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then_some(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arctic-wf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_models_packs_and_subgraphs() {
        let workflow = json!({
            "nodes": [
                {"type": "UNETLoader", "properties": {"cnr_id": "comfy-core", "models": [
                    {"name": "flux1-dev.safetensors", "directory": "diffusion_models",
                     "url": "https://huggingface.co/Comfy-Org/flux1-dev/resolve/main/flux1-dev.safetensors"}
                ]}, "widgets_values": ["flux1-dev.safetensors", "default"]},
                {"type": "VAELoader", "properties": {"cnr_id": "comfy-core"}, "widgets_values": ["ae.safetensors"]},
                {"type": "GetNode", "properties": {"cnr_id": "comfyui-kjnodes", "ver": "1.5.2"}},
                {"type": "SetNode", "properties": {"cnr_id": "comfyui-kjnodes", "ver": "1.5.2"}},
                {"type": "Note", "properties": {}},
                {"type": "MysteryNode", "properties": {}},
                {"type": "4f1c-subgraph", "properties": {}}
            ],
            "definitions": {"subgraphs": [{"id": "4f1c-subgraph", "nodes": [
                {"type": "UnetLoaderGGUF", "properties": {"aux_id": "city96/ComfyUI-GGUF", "ver": "abc123"},
                 "widgets_values": ["wan2.2-Q4_K_M.gguf"]}
            ]}]}
        });
        let needs = parse_workflow_requirements(&workflow).unwrap();
        let names: Vec<_> = needs
            .models
            .iter()
            .map(|m| (m.name.as_str(), m.directory.as_deref(), m.url.is_some()))
            .collect();
        assert_eq!(
            names,
            [
                ("ae.safetensors", Some("vae"), false),
                ("flux1-dev.safetensors", Some("diffusion_models"), true),
                ("wan2.2-Q4_K_M.gguf", Some("diffusion_models"), false),
            ]
        );
        assert_eq!(needs.packs.len(), 2);
        assert_eq!(needs.packs[0].id, "comfyui-kjnodes");
        assert_eq!(needs.packs[0].node_types, ["GetNode", "SetNode"]);
        assert_eq!(needs.packs[0].version.as_deref(), Some("1.5.2"));
        assert_eq!(needs.packs[1].source, PackSource::GitHub);
        assert_eq!(needs.unresolved_node_types, ["MysteryNode"]);
    }

    #[test]
    fn core_node_types_come_from_mappings_and_schemas() {
        let mut types = HashSet::new();
        collect_core_node_types(
            "NODE_CLASS_MAPPINGS = {\n    \"KSampler\": KSampler,\n    'VAELoader': VAELoader,\n}\n\
             x = \"NotANode\"\n\
             return io.Schema(\n    node_id=\"ComfySwitchNode\",\n    display_name=\"If/Else\")\n",
            &mut types,
        );
        let mut found: Vec<_> = types.into_iter().collect();
        found.sort();
        assert_eq!(found, ["ComfySwitchNode", "KSampler", "VAELoader"]);
    }

    #[test]
    fn api_format_workflows_are_rejected_with_a_hint() {
        let api = json!({"3": {"class_type": "KSampler", "inputs": {}}});
        assert!(parse_workflow_requirements(&api)
            .unwrap_err()
            .contains("API format"));
    }

    #[test]
    fn model_names_and_folders_are_handled_safely() {
        assert_eq!(model_file_name("wan\\sub\\x.safetensors"), "x.safetensors");
        assert_eq!(model_file_name("sub/x.safetensors"), "x.safetensors");
        assert_eq!(
            safe_model_directory("diffusion_models"),
            Some("diffusion_models")
        );
        assert_eq!(safe_model_directory("../etc"), None);
        assert_eq!(safe_model_directory("a/b"), None);

        let root = temp_dir("models");
        std::fs::create_dir_all(root.join("vae/flux")).unwrap();
        std::fs::write(root.join("vae/flux/AE.safetensors"), "").unwrap();
        let index = index_model_files(&[root.clone(), root.join("missing")]);
        assert!(index.contains("ae.safetensors"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_packs_match_by_folder_project_name_or_repo() {
        let nodes = temp_dir("packs");
        std::fs::create_dir_all(nodes.join("ComfyUI-KJNodes/.git")).unwrap();
        std::fs::write(
            nodes.join("ComfyUI-KJNodes/.git/config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/kijai/ComfyUI-KJNodes.git\n",
        )
        .unwrap();
        std::fs::create_dir_all(nodes.join("my-gguf")).unwrap();
        std::fs::write(
            nodes.join("my-gguf/pyproject.toml"),
            "[build-system]\nrequires = []\n[project]\nname = \"comfyui-gguf\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(nodes.join("ComfyUI-Manager.disabled")).unwrap();
        let installed = installed_packs(&nodes);
        assert_eq!(installed.len(), 2);

        let pack = |id: &str, source| WorkflowNodePack {
            id: id.into(),
            source,
            version: None,
            node_types: vec![],
        };
        // Registry id with the repository from the registry.
        assert!(pack_installed(
            &pack("comfyui-kjnodes", PackSource::Registry),
            Some("https://github.com/kijai/ComfyUI-KJNodes"),
            &installed
        ));
        // Registry id matching a pyproject name.
        assert!(pack_installed(
            &pack("comfyui-gguf", PackSource::Registry),
            None,
            &installed
        ));
        // GitHub slug matching the git remote.
        assert!(pack_installed(
            &pack("kijai/ComfyUI-KJNodes", PackSource::GitHub),
            None,
            &installed
        ));
        assert!(!pack_installed(
            &pack("comfyui-manager", PackSource::Registry),
            None,
            &installed
        ));
        let _ = std::fs::remove_dir_all(nodes);
    }
}
