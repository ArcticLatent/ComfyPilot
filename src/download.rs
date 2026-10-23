use crate::{
    config::ConfigStore,
    model::{LoraDefinition, ModelArtifact, ResolvedModel, TargetCategory, WorkflowDefinition},
};
use anyhow::{anyhow, Context, Result};
use futures::{StreamExt, TryStreamExt};
use log::{info, warn};
use percent_encoding::percent_decode_str;
use reqwest::{header, Client, Url};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter, SeekFrom},
    process::Command,
    runtime::Runtime,
    sync::{Mutex, Semaphore},
    time::timeout,
};
use tokio_util::{io::StreamReader, sync::CancellationToken};

const MULTIPART_MIN_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const CHUNK_SIZE_BYTES: u64 = 64 * 1024 * 1024;
const CHUNK_CONCURRENCY: usize = 4;
const IO_BUFFER_INITIAL: usize = 128 * 1024;
/// How often sha256 verification reports progress.
const VERIFY_REPORT_INTERVAL: Duration = Duration::from_millis(250);
const IO_BUFFER_MIN: usize = 64 * 1024;
const IO_BUFFER_MAX: usize = 1024 * 1024;
const ADAPTIVE_STEP_BYTES: u64 = 5 * 1024 * 1024;
const ADAPTIVE_GROW_MBPS: f64 = 50.0;
const ADAPTIVE_SHRINK_MBPS: f64 = 5.0;
const STREAM_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_RESUME_ATTEMPTS: usize = 3;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static HF_CLI_AVAILABLE: OnceLock<bool> = OnceLock::new();
static HF_BIN_AVAILABLE: OnceLock<bool> = OnceLock::new();
static UVX_AVAILABLE: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct DownloadOutcome {
    pub artifact: ModelArtifact,
    pub destination: PathBuf,
    pub status: DownloadStatus,
}

#[derive(Clone, Debug)]
pub struct LoraDownloadOutcome {
    pub lora: LoraDefinition,
    pub destination: PathBuf,
    pub status: DownloadStatus,
}

#[derive(Clone, Debug)]
pub struct WorkflowDownloadOutcome {
    pub workflow: WorkflowDefinition,
    pub destination: PathBuf,
    pub status: DownloadStatus,
}

#[derive(Clone, Debug)]
pub struct CivitaiModelMetadata {
    pub file_name: String,
    pub download_url: Option<String>,
    pub preview: Option<CivitaiPreview>,
    pub preview_url: Option<String>,
    pub trained_words: Vec<String>,
    pub description: Option<String>,
    pub usage_strength: Option<f64>,
    pub creator_username: Option<String>,
    pub creator_link: Option<String>,
}

#[derive(Clone, Debug)]
pub enum CivitaiPreview {
    Image(Vec<u8>),
    Video { url: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadStatus {
    Downloaded,
    SkippedExisting,
}

#[derive(Clone, Debug)]
pub enum DownloadSignal {
    Started {
        artifact: String,
        index: usize,
        total: usize,
        size: Option<u64>,
    },
    Progress {
        artifact: String,
        index: usize,
        received: u64,
        size: Option<u64>,
    },
    /// The file is fully downloaded and its sha256 is being checked; for
    /// multi-part downloads that means re-reading the whole file.
    Verifying {
        artifact: String,
        index: usize,
        checked: u64,
        size: Option<u64>,
    },
    Finished {
        artifact: String,
        index: usize,
        size: Option<u64>,
        folder: Option<String>,
    },
    Failed {
        artifact: String,
        index: usize,
        error: String,
    },
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("unauthorized")]
    Unauthorized,
}

#[derive(Debug)]
pub struct DownloadManager {
    runtime: Arc<Runtime>,
    config: Arc<ConfigStore>,
    api_client: Client,
    download_clients: Vec<Client>,
    civitai_metadata_cache: Arc<Mutex<HashMap<u64, CivitaiModelMetadata>>>,
    civitai_metadata_order: Arc<Mutex<VecDeque<u64>>>,
}

impl DownloadManager {
    pub fn new(runtime: Arc<Runtime>, config: Arc<ConfigStore>) -> Self {
        let api_client = make_http_client();
        let download_clients = make_download_clients();

        Self {
            runtime,
            config,
            api_client,
            download_clients,
            civitai_metadata_cache: Arc::new(Mutex::new(HashMap::new())),
            civitai_metadata_order: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn download_variant(
        &self,
        comfy_root: PathBuf,
        resolved: ResolvedModel,
        progress: Sender<DownloadSignal>,
    ) -> tokio::task::JoinHandle<Result<Vec<DownloadOutcome>>> {
        self.download_variant_with_cancel(comfy_root, resolved, progress, None)
    }

    pub fn download_variant_with_cancel(
        &self,
        comfy_root: PathBuf,
        resolved: ResolvedModel,
        progress: Sender<DownloadSignal>,
        cancel: Option<CancellationToken>,
    ) -> tokio::task::JoinHandle<Result<Vec<DownloadOutcome>>> {
        let download_clients = self.download_clients.clone();
        let xet_enabled = self.config.settings().hf_xet_enabled;
        let hf_token = self.config.settings().hf_token;
        self.runtime.spawn(async move {
            let mut outcomes = Vec::new();
            let model_folder = resolved.master.id.clone();
            let artifacts = dedupe_artifacts(resolved.variant.artifacts);
            let total = artifacts.len();

            let mut stream = futures::stream::iter(artifacts.into_iter().enumerate().map(
                |(index, artifact)| {
                    let download_clients = download_clients.clone();
                    let comfy_root = comfy_root.clone();
                    let model_folder = model_folder.clone();
                    let progress = progress.clone();
                    let cancel = cancel.clone();
                    let hf_token = hf_token.clone();
                    async move {
                        if is_cancelled(cancel.as_ref()) {
                            return Err(anyhow!("download cancelled by user"));
                        }
                        let artifact_name = artifact.file_name().to_string();
                        let _ = progress.send(DownloadSignal::Started {
                            artifact: artifact_name.clone(),
                            index,
                            total,
                            size: artifact.size_bytes,
                        });

                        info!("Starting download: {}", artifact.file_name());
                        match download_artifact(
                            &download_clients,
                            &comfy_root,
                            &model_folder,
                            &artifact,
                            Some((progress.clone(), index, artifact_name.clone())),
                            xet_enabled,
                            hf_token.as_deref(),
                            cancel.as_ref(),
                        )
                        .await
                        {
                            Ok(outcome) => Ok(outcome),
                            Err(err) => {
                                let _ = progress.send(DownloadSignal::Failed {
                                    artifact: artifact_name,
                                    index,
                                    error: format!("{err:#}"),
                                });
                                Err(err)
                            }
                        }
                    }
                },
            ))
            .buffer_unordered(1);

            while let Some(result) = stream.next().await {
                match result {
                    Ok(outcome) => {
                        info!(
                            "{} -> {:?} ({:?})",
                            outcome.artifact.file_name(),
                            outcome.destination,
                            outcome.status
                        );
                        outcomes.push(outcome);
                    }
                    Err(err) => return Err(err),
                }
            }

            Ok(outcomes)
        })
    }

    pub fn download_lora(
        &self,
        comfy_root: PathBuf,
        lora: LoraDefinition,
        token: Option<String>,
        progress: Sender<DownloadSignal>,
    ) -> tokio::task::JoinHandle<Result<LoraDownloadOutcome>> {
        self.download_lora_with_cancel(comfy_root, lora, token, progress, None)
    }

    pub fn download_lora_with_cancel(
        &self,
        comfy_root: PathBuf,
        lora: LoraDefinition,
        token: Option<String>,
        progress: Sender<DownloadSignal>,
        cancel: Option<CancellationToken>,
    ) -> tokio::task::JoinHandle<Result<LoraDownloadOutcome>> {
        let download_clients = self.download_clients.clone();
        let api_client = self.api_client.clone();
        let xet_enabled = self.config.settings().hf_xet_enabled;
        let hf_token = self.config.settings().hf_token;
        self.runtime.spawn(async move {
            if is_cancelled(cancel.as_ref()) {
                return Err(anyhow!("download cancelled by user"));
            }
            let folder_name = lora
                .family
                .as_deref()
                .map(normalize_folder_name)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| sanitize_file_name(&lora.id));
            let loras_root = comfy_root.join(TargetCategory::from_slug("loras").comfyui_subdir());
            let lora_dir = loras_root.join(&folder_name);

            let base_url = lora.download_url.clone();
            let token_value = token.clone().and_then(|t| {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });

            let mut file_name = lora.derived_file_name();
            let mut url = base_url.clone();

            if base_url.contains("civitai.com") {
                match fetch_civitai_model_metadata(&api_client, &base_url, token_value.as_deref())
                    .await
                {
                    Ok(metadata) => {
                        file_name = metadata.file_name.clone();
                        if let Some(download_url) = metadata.download_url {
                            url = download_url;
                        }
                    }
                    Err(err) => {
                        warn!("Failed to fetch Civitai metadata for {}: {err}", base_url);
                    }
                }
            }

            file_name = sanitize_file_name(&file_name);

            let dest_path = lora_dir.join(&file_name);

            if fs::try_exists(&dest_path)
                .await
                .with_context(|| format!("failed to check {:?} existence", dest_path))?
            {
                let _ = progress.send(DownloadSignal::Started {
                    artifact: file_name.clone(),
                    index: 0,
                    total: 1,
                    size: Some(0),
                });
                let _ = progress.send(DownloadSignal::Finished {
                    artifact: file_name.clone(),
                    index: 0,
                    size: Some(0),
                    folder: dest_path
                        .parent()
                        .map(|p| p.to_string_lossy().to_string()),
                });
                return Ok(LoraDownloadOutcome {
                    lora,
                    destination: dest_path,
                    status: DownloadStatus::SkippedExisting,
                });
            }

            if url.trim().is_empty() {
                return Err(anyhow!("LoRA {} missing download URL", lora.id));
            }

            let mut auth_token: Option<String> = None;
            if url.contains("civitai.com") {
                if let Some(token_string) = token_value.clone() {
                    if !url.contains("token=") {
                        let separator = if url.contains('?') { '&' } else { '?' };
                        url = format!("{url}{separator}token={token_string}");
                    }
                    auth_token = Some(token_string);
                }
            }

            let _ = progress.send(DownloadSignal::Started {
                artifact: file_name.clone(),
                index: 0,
                total: 1,
                size: None,
            });

            match download_direct(
                &download_clients,
                &url,
                &lora_dir,
                &file_name,
                Some((progress.clone(), 0, file_name.clone())),
                auth_token.as_deref(),
                xet_enabled,
                hf_token.as_deref(),
                cancel.as_ref(),
            )
            .await
            {
                Ok(destination) => Ok(LoraDownloadOutcome {
                    lora,
                    destination,
                    status: DownloadStatus::Downloaded,
                }),
                Err(err) => {
                    // Check the error's concrete type before redacting: the
                    // redaction below rebuilds the error from its rendered
                    // text, which would otherwise erase the `DownloadError`
                    // type needed for this downcast.
                    let is_unauthorized = matches!(
                        err.downcast_ref::<DownloadError>(),
                        Some(DownloadError::Unauthorized)
                    );
                    // `url` may have the Civitai token appended as a `?token=`
                    // query param (required so the request survives Civitai's
                    // redirect to its CDN, which drops the Authorization
                    // header) -- any error surfaced from `download_direct`
                    // can carry that URL in its context chain, so redact the
                    // raw token before it reaches a progress event, log line,
                    // or downstream caller.
                    let err = redact_error(err, token_value.as_deref());

                    if is_unauthorized {
                        let message = if token_value.is_some() {
                            "Civitai rejected the token (401/403). Check that your API token is valid and active."
                        } else {
                            "This Civitai LoRA appears to require login. Add your Civitai API token and retry."
                        };
                        let _ = progress.send(DownloadSignal::Failed {
                            artifact: file_name.clone(),
                            index: 0,
                            error: message.to_string(),
                        });
                        if fs::try_exists(&lora_dir).await.unwrap_or(false) {
                            if let Ok(mut entries) = fs::read_dir(&lora_dir).await {
                                if matches!(entries.next_entry().await, Ok(None)) {
                                    let _ = fs::remove_dir(&lora_dir).await;
                                }
                            }
                        }
                        return Err(err);
                    }

                    let _ = progress.send(DownloadSignal::Failed {
                        artifact: file_name,
                        index: 0,
                        error: format!("{err:#}"),
                    });
                    Err(err)
                }
            }
        })
    }

    pub fn civitai_model_metadata(
        &self,
        download_url: String,
        token: Option<String>,
    ) -> tokio::task::JoinHandle<Result<CivitaiModelMetadata>> {
        let client = self.api_client.clone();
        let cache = Arc::clone(&self.civitai_metadata_cache);
        let order = Arc::clone(&self.civitai_metadata_order);
        self.runtime.spawn(async move {
            let model_version_id = extract_civitai_model_version_id(&download_url)
                .ok_or_else(|| anyhow!("unable to parse model version ID from {download_url}"))?;

            if let Some(cached) = {
                let cache_guard = cache.lock().await;
                cache_guard.get(&model_version_id).cloned()
            } {
                if cached.usage_strength.is_some() {
                    return Ok(cached);
                }
            }

            let metadata = fetch_civitai_model_metadata_internal(
                &client,
                model_version_id,
                &download_url,
                token.as_deref(),
            )
            .await?;

            {
                let mut cache_guard = cache.lock().await;
                let mut order_guard = order.lock().await;
                let mut cached_metadata = metadata.clone();
                if matches!(cached_metadata.preview, Some(CivitaiPreview::Image(_))) {
                    cached_metadata.preview = None;
                }
                cache_guard.insert(model_version_id, cached_metadata);
                order_guard.retain(|id| *id != model_version_id);
                order_guard.push_back(model_version_id);
                const MAX_CIVITAI_CACHE: usize = 200;
                while order_guard.len() > MAX_CIVITAI_CACHE {
                    if let Some(oldest) = order_guard.pop_front() {
                        cache_guard.remove(&oldest);
                    }
                }
            }

            Ok(metadata)
        })
    }

    pub fn civitai_preview_image(
        &self,
        image_url: String,
        token: Option<String>,
    ) -> tokio::task::JoinHandle<Result<Vec<u8>>> {
        let client = self.api_client.clone();
        self.runtime.spawn(async move {
            fetch_preview_image_bytes(&client, &image_url, token.as_deref())
                .await
                .ok_or_else(|| anyhow!("failed to download preview image"))
        })
    }

    pub fn download_workflow_with_cancel(
        &self,
        workflows_dir: PathBuf,
        workflow: WorkflowDefinition,
        progress: Sender<DownloadSignal>,
        cancel: Option<CancellationToken>,
    ) -> tokio::task::JoinHandle<Result<WorkflowDownloadOutcome>> {
        let download_clients = self.download_clients.clone();
        let hf_token = self.config.settings().hf_token;
        self.runtime.spawn(async move {
            if is_cancelled(cancel.as_ref()) {
                return Err(anyhow!("download cancelled by user"));
            }
            let url = workflow.workflow_json_url.trim().to_string();
            if url.is_empty() {
                return Err(anyhow!(
                    "Workflow {} is missing workflow_json_url",
                    workflow.id
                ));
            }

            let mut file_name = url
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string();
            if file_name.is_empty() {
                file_name = format!("{}.json", workflow.id);
            }
            if !file_name.to_ascii_lowercase().ends_with(".json") {
                file_name.push_str(".json");
            }
            file_name = sanitize_file_name(&file_name);
            let destination_path = workflows_dir.join(&file_name);

            if fs::try_exists(&destination_path)
                .await
                .with_context(|| format!("failed to check {:?} existence", destination_path))?
            {
                let _ = progress.send(DownloadSignal::Started {
                    artifact: file_name.clone(),
                    index: 0,
                    total: 1,
                    size: Some(0),
                });
                let _ = progress.send(DownloadSignal::Finished {
                    artifact: file_name.clone(),
                    index: 0,
                    size: Some(0),
                    folder: destination_path
                        .parent()
                        .map(|p| p.to_string_lossy().to_string()),
                });
                return Ok(WorkflowDownloadOutcome {
                    workflow,
                    destination: destination_path,
                    status: DownloadStatus::SkippedExisting,
                });
            }

            let _ = progress.send(DownloadSignal::Started {
                artifact: file_name.clone(),
                index: 0,
                total: 1,
                size: None,
            });

            let destination = download_direct(
                &download_clients,
                &url,
                &workflows_dir,
                &file_name,
                Some((progress.clone(), 0, file_name.clone())),
                None,
                false,
                hf_token.as_deref(),
                cancel.as_ref(),
            )
            .await?;

            Ok(WorkflowDownloadOutcome {
                workflow,
                destination,
                status: DownloadStatus::Downloaded,
            })
        })
    }

    /// Downloads one file from `url` into `dest_dir` as `file_name` (a model
    /// a workflow needs). Sends the Hugging Face token to Hugging Face URLs.
    pub fn download_file_with_cancel(
        &self,
        url: String,
        dest_dir: PathBuf,
        file_name: String,
        progress: Sender<DownloadSignal>,
        cancel: Option<CancellationToken>,
    ) -> tokio::task::JoinHandle<Result<PathBuf>> {
        let download_clients = self.download_clients.clone();
        let settings = self.config.settings();
        let (xet_enabled, hf_token) = (settings.hf_xet_enabled, settings.hf_token);
        // The Civitai token, for Civitai URLs only.
        let civitai_token = settings
            .civitai_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty() && is_civitai_url(&url));
        self.runtime.spawn(async move {
            if is_cancelled(cancel.as_ref()) {
                return Err(anyhow!("download cancelled by user"));
            }
            let file_name = sanitize_file_name(&file_name);
            fs::create_dir_all(&dest_dir)
                .await
                .with_context(|| format!("failed to create directory {:?}", dest_dir))?;
            let _ = progress.send(DownloadSignal::Started {
                artifact: file_name.clone(),
                index: 0,
                total: 1,
                size: None,
            });
            // As for LoRAs: the token also goes in the query, since Civitai's
            // redirect to its CDN drops the Authorization header.
            let url = match civitai_token.as_deref() {
                Some(token) => with_civitai_token_param(&url, token),
                None => url,
            };
            let result = download_direct(
                &download_clients,
                &url,
                &dest_dir,
                &file_name,
                Some((progress.clone(), 0, file_name.clone())),
                civitai_token.as_deref(),
                xet_enabled,
                hf_token.as_deref(),
                cancel.as_ref(),
            )
            .await;
            result.map_err(|err| {
                let unauthorized = matches!(
                    err.downcast_ref::<DownloadError>(),
                    Some(DownloadError::Unauthorized)
                );
                let err = redact_error(err, civitai_token.as_deref());
                match (unauthorized, civitai_token.is_some()) {
                    (true, true) => anyhow!(
                        "Civitai rejected the token (401/403). Check that your API token is valid and active."
                    ),
                    (true, false) => anyhow!(
                        "This Civitai file needs a login. Add your Civitai API token (\"Civitai token…\") and retry."
                    ),
                    _ => err,
                }
            })
        })
    }
}

/// `url` with Civitai's `token` query parameter (unless it has one).
fn with_civitai_token_param(url: &str, token: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) if !parsed.query_pairs().any(|(key, _)| key == "token") => {
            parsed.query_pairs_mut().append_pair("token", token);
            parsed.to_string()
        }
        _ => url.to_string(),
    }
}

/// The HTTP URL a catalog artifact downloads from.
pub fn artifact_download_url(artifact: &ModelArtifact) -> Result<String> {
    match &artifact.direct_url {
        Some(direct) => Ok(ensure_hf_download_url(direct)),
        None => build_download_url(&artifact.repo, &artifact.path),
    }
}

fn make_http_client() -> Client {
    Client::builder()
        .user_agent(format!(
            "ComfyPilot/{} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_NAME")
        ))
        .tcp_nodelay(true)
        .http2_adaptive_window(true)
        .pool_max_idle_per_host(4)
        .build()
        .expect("failed to construct reqwest client")
}

fn make_download_clients() -> Vec<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        ),
    );
    headers.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.5"),
    );

    let mut clients = Vec::new();
    for _ in 0..CHUNK_CONCURRENCY {
        let client = Client::builder()
            .default_headers(headers.clone())
            .http1_only()
            .tcp_keepalive(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to construct download HTTP client");
        clients.push(client);
    }
    clients
}

#[allow(clippy::too_many_arguments)]
async fn download_artifact(
    clients: &[Client],
    comfy_root: &Path,
    model_folder: &str,
    artifact: &ModelArtifact,
    progress: Option<(Sender<DownloadSignal>, usize, String)>,
    xet_enabled: bool,
    hf_token: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<DownloadOutcome> {
    if is_cancelled(cancel) {
        return Err(anyhow!("download cancelled by user"));
    }
    let dest_dir =
        catalog_model_destination(comfy_root, artifact.target_category.slug(), model_folder)?;
    fs::create_dir_all(&dest_dir)
        .await
        .with_context(|| format!("failed to create directory {:?}", dest_dir))?;

    // Catalog data is first-party, but sanitize anyway so this name is safe
    // to join onto `dest_dir` no matter where it originated.
    let initial_file_name = sanitize_file_name(artifact.file_name());
    let mut dest_path = dest_dir.join(&initial_file_name);

    if fs::try_exists(&dest_path)
        .await
        .with_context(|| format!("failed to check {:?} existence", dest_path))?
    {
        if let Some((sender, index, artifact_name)) = progress.as_ref() {
            let _ = sender.send(DownloadSignal::Finished {
                artifact: artifact_name.clone(),
                index: *index,
                size: Some(0),
                folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
            });
        }
        return Ok(DownloadOutcome {
            artifact: artifact.clone(),
            destination: dest_path,
            status: DownloadStatus::SkippedExisting,
        });
    }

    let url = if let Some(direct) = &artifact.direct_url {
        ensure_hf_download_url(direct)
    } else {
        build_download_url(&artifact.repo, &artifact.path)?
    };
    log::info!("Requesting {}", url);
    let auth_token = request_token(&url, None, hf_token);

    let mut xet_size_hint = artifact.size_bytes;
    if xet_size_hint.is_none() {
        let client = clients
            .first()
            .ok_or_else(|| anyhow!("missing HTTP client for downloads"))?;
        if let Ok(Some(metadata)) =
            fetch_head_metadata(client, &url, auth_token, &initial_file_name).await
        {
            xet_size_hint = metadata.content_length;
        }
    }

    if let Some(parsed) = parse_hf_resolve_url(&url) {
        let cli_available = hf_cli_available();
        log::info!(
            "HF download path decision: xet_enabled={}, hf_cli_available={}, repo_file={}",
            xet_enabled,
            cli_available,
            parsed.file_path
        );
        if xet_enabled && cli_available {
            match download_via_hf_cli(
                &parsed,
                &dest_dir,
                progress.clone(),
                xet_size_hint,
                hf_token,
                cancel,
            )
            .await
            {
                Ok(dest_path) => {
                    if let Some((sender, index, artifact_name)) = progress {
                        let _ = sender.send(DownloadSignal::Finished {
                            artifact: artifact_name,
                            index,
                            size: artifact.size_bytes,
                            folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
                        });
                    }
                    return Ok(DownloadOutcome {
                        artifact: artifact.clone(),
                        destination: dest_path,
                        status: DownloadStatus::Downloaded,
                    });
                }
                Err(err) => {
                    return Err(err.context(format!("hf CLI/Xet download failed for {url}")))
                }
            }
        }
    }

    let mut content_length = artifact.size_bytes;
    let mut accept_ranges = false;
    let mut final_file_name = initial_file_name.clone();

    let client = clients
        .first()
        .ok_or_else(|| anyhow!("missing HTTP client for downloads"))?;

    if let Ok(Some(metadata)) =
        fetch_head_metadata(client, &url, auth_token, &initial_file_name).await
    {
        if let Some(name) = metadata.file_name {
            final_file_name = name;
        }
        content_length = metadata.content_length;
        accept_ranges = metadata.accept_ranges;
    }

    if final_file_name != initial_file_name {
        dest_path = dest_dir.join(&final_file_name);
        if fs::try_exists(&dest_path)
            .await
            .with_context(|| format!("failed to check {:?} existence", dest_path))?
        {
            if let Some((sender, index, artifact_name)) = progress.as_ref() {
                let _ = sender.send(DownloadSignal::Finished {
                    artifact: artifact_name.clone(),
                    index: *index,
                    size: Some(0),
                    folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
                });
            }
            return Ok(DownloadOutcome {
                artifact: artifact.clone(),
                destination: dest_path,
                status: DownloadStatus::SkippedExisting,
            });
        }
    }

    let mut part_total = content_length;
    if part_total.is_none() || !accept_ranges {
        if let Ok(Some(total)) = probe_range_support(client, &url, auth_token).await {
            accept_ranges = true;
            part_total = Some(total);
        }
    }

    // A previous sequential stream is a contiguous prefix and can be resumed
    // cheaply. Do not switch that file to the multipart path, which pre-sizes
    // its temporary file and would otherwise erase the saved prefix.
    let saved_tmp_path =
        part_total.and_then(|total| find_resumable_tmp_path(&dest_dir, &final_file_name, total));

    if accept_ranges && saved_tmp_path.is_none() {
        if let Some(total_size) = part_total {
            if total_size >= MULTIPART_MIN_BYTES {
                let dest_path = download_ranged_to_file(
                    clients,
                    &url,
                    &dest_dir,
                    &final_file_name,
                    total_size,
                    progress.clone(),
                    auth_token,
                    artifact.sha256.as_deref(),
                    cancel,
                )
                .await?;

                if let Some((sender, index, artifact_name)) = progress {
                    let _ = sender.send(DownloadSignal::Finished {
                        artifact: artifact_name.clone(),
                        index,
                        size: Some(total_size),
                        folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
                    });
                }

                return Ok(DownloadOutcome {
                    artifact: artifact.clone(),
                    destination: dest_path,
                    status: DownloadStatus::Downloaded,
                });
            }
        }
    }

    let response = with_bearer(client.get(url.clone()), auth_token)
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;
    let status = response.status();
    if is_hf_url(&url) && matches!(status.as_u16(), 401 | 403) {
        return Err(hf_access_error(&url, status, auth_token.is_some()));
    }
    let mut response = response
        .error_for_status()
        .with_context(|| format!("unexpected status downloading {url}"))?;

    if content_length.is_none() {
        content_length = response.content_length();
    }

    if final_file_name == initial_file_name {
        final_file_name = filename_from_headers(response.headers(), &initial_file_name);
    }
    if final_file_name != initial_file_name {
        dest_path = dest_dir.join(&final_file_name);
        if fs::try_exists(&dest_path)
            .await
            .with_context(|| format!("failed to check {:?} existence", dest_path))?
        {
            if let Some((sender, index, artifact_name)) = progress.as_ref() {
                let _ = sender.send(DownloadSignal::Finished {
                    artifact: artifact_name.clone(),
                    index: *index,
                    size: Some(0),
                    folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
                });
            }
            return Ok(DownloadOutcome {
                artifact: artifact.clone(),
                destination: dest_path,
                status: DownloadStatus::SkippedExisting,
            });
        }
    }

    let tmp_path = saved_tmp_path.unwrap_or_else(|| unique_tmp_path(&dest_dir, &final_file_name));
    let mut received = fs::metadata(&tmp_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let expected_size = content_length.or(artifact.size_bytes);
    if expected_size.is_some_and(|total| received >= total) {
        // A complete-looking partial file still needs the normal checksum and
        // finalization path, so restart unless it is strictly smaller.
        received = 0;
    }

    if received > 0 {
        match with_bearer(client.get(url.clone()), auth_token)
            .header(header::RANGE, format!("bytes={received}-"))
            .send()
            .await
        {
            Ok(resumed) if resumed.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                info!("Resuming {:?} at byte {}", tmp_path, received);
                response = resumed;
            }
            Ok(resumed) => {
                warn!(
                    "Server rejected saved download resume at byte {} with status {}; restarting",
                    received,
                    resumed.status()
                );
                received = 0;
            }
            Err(err) => {
                warn!("Saved download resume request failed ({err}); restarting");
                received = 0;
            }
        }
    }

    let mut hasher = artifact.sha256.as_ref().map(|_| Sha256::new());
    if received > 0 {
        if let Some(hasher) = hasher.as_mut() {
            let mut existing = fs::File::open(&tmp_path)
                .await
                .with_context(|| format!("failed to read partial file {:?}", tmp_path))?;
            let mut hash_buffer = vec![0u8; IO_BUFFER_MAX];
            loop {
                let n = existing
                    .read(&mut hash_buffer)
                    .await
                    .with_context(|| format!("failed hashing partial file {:?}", tmp_path))?;
                if n == 0 {
                    break;
                }
                hasher.update(&hash_buffer[..n]);
            }
        }
    }

    let file = if received > 0 {
        fs::OpenOptions::new()
            .append(true)
            .open(&tmp_path)
            .await
            .with_context(|| format!("failed to reopen temporary file {:?}", tmp_path))?
    } else {
        fs::File::create(&tmp_path)
            .await
            .with_context(|| format!("failed to create temporary file {:?}", tmp_path))?
    };
    let mut file = BufWriter::new(file);

    log::info!(
        "Streaming into temporary file {:?} (destination {:?})",
        tmp_path,
        dest_path
    );

    let stream = response
        .bytes_stream()
        .map_err(std::io::Error::other)
        .boxed();
    let mut reader = StreamReader::new(stream);
    let mut buffer = vec![0u8; IO_BUFFER_INITIAL];
    let mut bytes_since = 0u64;
    let mut last_adjust = Instant::now();
    let mut last_progress = Instant::now();
    let mut resume_attempts = 0usize;

    if received > 0 {
        if let Some((sender, index, artifact_name)) = progress.as_ref() {
            let _ = sender.send(DownloadSignal::Progress {
                artifact: artifact_name.clone(),
                index: *index,
                received,
                size: expected_size,
            });
        }
    }

    loop {
        if is_cancelled(cancel) {
            fs::remove_file(&tmp_path).await.ok();
            return Err(anyhow!("download cancelled by user"));
        }
        let read_result = timeout(
            std::time::Duration::from_millis(500),
            reader.read(&mut buffer),
        )
        .await;

        let retry_reason = match read_result {
            Ok(Ok(0)) => {
                let expected = content_length.or(artifact.size_bytes);
                if expected.is_some_and(|total| received < total) {
                    Some(format!(
                        "stream ended early at {received} of {} bytes",
                        expected.unwrap_or_default()
                    ))
                } else {
                    break;
                }
            }
            Ok(Ok(n)) => {
                file.write_all(&buffer[..n])
                    .await
                    .with_context(|| format!("failed writing to {:?}", tmp_path))?;
                received += n as u64;
                last_progress = Instant::now();
                if let Some(hasher) = hasher.as_mut() {
                    hasher.update(&buffer[..n]);
                }
                bytes_since += n as u64;
                adapt_buffer_size(&mut buffer, &mut bytes_since, &mut last_adjust);
                if let Some((sender, index, artifact_name)) = progress.as_ref() {
                    let _ = sender.send(DownloadSignal::Progress {
                        artifact: artifact_name.clone(),
                        index: *index,
                        received,
                        size: content_length.or(artifact.size_bytes),
                    });
                }
                continue;
            }
            Ok(Err(err)) => Some(err.to_string()),
            Err(_) if last_progress.elapsed() < STREAM_STALL_TIMEOUT => continue,
            Err(_) => Some(format!(
                "no data received for {} seconds",
                STREAM_STALL_TIMEOUT.as_secs()
            )),
        };

        if resume_attempts >= STREAM_RESUME_ATTEMPTS {
            return Err(anyhow!(
                "failed streaming {url} after {resume_attempts} resume attempts: {}",
                retry_reason.unwrap_or_else(|| "unknown stream error".to_string())
            ));
        }

        resume_attempts += 1;
        let reason = retry_reason.unwrap_or_else(|| "unknown stream error".to_string());
        warn!(
            "Download stream interrupted at {} bytes (attempt {}/{}): {}",
            received, resume_attempts, STREAM_RESUME_ATTEMPTS, reason
        );
        file.flush()
            .await
            .with_context(|| format!("failed flushing {:?} before resume", tmp_path))?;
        tokio::time::sleep(Duration::from_secs(1 << (resume_attempts - 1))).await;

        let resumed = with_bearer(client.get(url.clone()), auth_token)
            .header(header::RANGE, format!("bytes={received}-"))
            .send()
            .await
            .with_context(|| format!("failed resuming {url} at byte {received}"))?;
        if resumed.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(anyhow!(
                "failed streaming {url}: {reason}; server rejected resume at byte {received} with status {}",
                resumed.status()
            ));
        }
        reader = StreamReader::new(
            resumed
                .bytes_stream()
                .map_err(std::io::Error::other)
                .boxed(),
        );
        last_progress = Instant::now();
    }

    file.flush()
        .await
        .with_context(|| format!("failed flushing {:?}", tmp_path))?;
    drop(file);

    if let Some(expected) = artifact.sha256.as_ref() {
        if let Some(hasher) = hasher {
            let digest = hasher.finalize();
            let actual = format!("{:x}", digest);
            if &actual != expected {
                fs::remove_file(&tmp_path).await.ok();
                return Err(anyhow!(
                    "checksum mismatch for {} (expected {}, got {})",
                    final_file_name,
                    expected,
                    actual
                ));
            }
        }
    }

    if fs::try_exists(&dest_path).await.unwrap_or(false) {
        fs::remove_file(&tmp_path).await.ok();
        return Ok(DownloadOutcome {
            artifact: artifact.clone(),
            destination: dest_path,
            status: DownloadStatus::SkippedExisting,
        });
    }

    if let Err(err) = fs::rename(&tmp_path, &dest_path).await {
        if fs::try_exists(&dest_path).await.unwrap_or(false) {
            fs::remove_file(&tmp_path).await.ok();
            return Ok(DownloadOutcome {
                artifact: artifact.clone(),
                destination: dest_path,
                status: DownloadStatus::SkippedExisting,
            });
        }
        return Err(err)
            .with_context(|| format!("failed to move {:?} to {:?}", tmp_path, dest_path));
    }

    log::info!("Finished download: {:?}", dest_path);

    if let Some((sender, index, artifact_name)) = progress {
        let _ = sender.send(DownloadSignal::Finished {
            artifact: artifact_name.clone(),
            index,
            size: content_length.or(artifact.size_bytes),
            folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
        });
    }

    Ok(DownloadOutcome {
        artifact: artifact.clone(),
        destination: dest_path,
        status: DownloadStatus::Downloaded,
    })
}

#[allow(clippy::too_many_arguments)]
async fn download_direct(
    clients: &[Client],
    url: &str,
    dest_dir: &Path,
    file_name: &str,
    progress: Option<(Sender<DownloadSignal>, usize, String)>,
    auth_token: Option<&str>,
    xet_enabled: bool,
    hf_token: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<PathBuf> {
    if is_cancelled(cancel) {
        return Err(anyhow!("download cancelled by user"));
    }
    let url = ensure_hf_download_url(url);
    let auth_token = request_token(&url, auth_token, hf_token);

    let mut xet_size_hint = None;
    if let Some(client) = clients.first() {
        if let Ok(Some(metadata)) = fetch_head_metadata(client, &url, auth_token, file_name).await {
            xet_size_hint = metadata.content_length;
        }
    }

    if let Some(parsed) = parse_hf_resolve_url(&url) {
        let cli_available = hf_cli_available();
        log::info!(
            "HF direct path decision: xet_enabled={}, hf_cli_available={}, repo_file={}",
            xet_enabled,
            cli_available,
            parsed.file_path
        );
        if xet_enabled && cli_available {
            match download_via_hf_cli(
                &parsed,
                dest_dir,
                progress.clone(),
                xet_size_hint,
                hf_token,
                cancel,
            )
            .await
            {
                Ok(path) => {
                    if let Some((sender, index, artifact_name)) = progress {
                        let _ = sender.send(DownloadSignal::Finished {
                            artifact: artifact_name,
                            index,
                            size: None,
                            folder: path.parent().map(|p| p.to_string_lossy().to_string()),
                        });
                    }
                    return Ok(path);
                }
                Err(err) => {
                    return Err(err.context(format!("hf CLI/Xet download failed for {url}")))
                }
            }
        }
    }
    let mut content_length = None;
    let mut accept_ranges = false;
    let mut final_file_name = file_name.to_string();

    let client = clients
        .first()
        .ok_or_else(|| anyhow!("missing HTTP client for downloads"))?;

    if let Ok(Some(metadata)) =
        fetch_head_metadata(client, &url, auth_token, &final_file_name).await
    {
        if let Some(name) = metadata.file_name {
            final_file_name = name;
        }
        content_length = metadata.content_length;
        accept_ranges = metadata.accept_ranges;
    }

    let mut part_total = content_length;
    if part_total.is_none() || !accept_ranges {
        if let Ok(Some(total)) = probe_range_support(client, &url, auth_token).await {
            accept_ranges = true;
            part_total = Some(total);
        }
    }

    let dest_path = dest_dir.join(&final_file_name);

    fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("failed to create directory {:?}", dest_dir))?;

    if fs::try_exists(&dest_path)
        .await
        .with_context(|| format!("failed to check {:?} existence", dest_path))?
    {
        if let Some((sender, index, artifact_name)) = progress {
            let _ = sender.send(DownloadSignal::Finished {
                artifact: artifact_name.clone(),
                index,
                size: Some(0),
                folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
            });
        }
        return Ok(dest_path);
    }

    if accept_ranges {
        if let Some(total_size) = part_total {
            if total_size >= MULTIPART_MIN_BYTES {
                let dest_path = download_ranged_to_file(
                    clients,
                    &url,
                    dest_dir,
                    &final_file_name,
                    total_size,
                    progress.clone(),
                    auth_token,
                    None,
                    cancel,
                )
                .await?;

                if let Some((sender, index, artifact_name)) = progress {
                    let _ = sender.send(DownloadSignal::Finished {
                        artifact: artifact_name.clone(),
                        index,
                        size: Some(total_size),
                        folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
                    });
                }

                return Ok(dest_path);
            }
        }
    }

    let mut request = client.get(&url);
    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;

    if response.status().is_client_error() || response.status().is_server_error() {
        let status = response.status();
        if url.contains("civitai.com") && matches!(status.as_u16(), 401 | 403) {
            return Err(DownloadError::Unauthorized.into());
        }
        if is_hf_url(&url) && matches!(status.as_u16(), 401 | 403) {
            return Err(hf_access_error(&url, status, auth_token.is_some()));
        }
        return Err(anyhow!("download failed for {url} (status {status})"));
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase());

    if content_length.is_none() {
        content_length = response.content_length();
    }

    if final_file_name == file_name {
        final_file_name = filename_from_headers(response.headers(), file_name);
    }
    let dest_path = dest_dir.join(&final_file_name);
    if fs::try_exists(&dest_path).await.unwrap_or(false) {
        if let Some((sender, index, artifact_name)) = progress {
            let _ = sender.send(DownloadSignal::Finished {
                artifact: artifact_name.clone(),
                index,
                size: Some(0),
                folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
            });
        }
        return Ok(dest_path);
    }

    if content_length.is_none() {
        content_length = response.content_length();
    }
    let tmp_path = unique_tmp_path(dest_dir, &final_file_name);
    let file = fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("failed to create temporary file {:?}", tmp_path))?;
    let mut file = BufWriter::new(file);

    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut reader = StreamReader::new(stream);
    let mut received: u64 = 0;
    let mut buffer = vec![0u8; IO_BUFFER_INITIAL];
    let mut sniff = Vec::with_capacity(2048);
    let mut bytes_since = 0u64;
    let mut last_adjust = Instant::now();

    loop {
        if is_cancelled(cancel) {
            fs::remove_file(&tmp_path).await.ok();
            return Err(anyhow!("download cancelled by user"));
        }
        let n = reader
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed streaming {url}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .await
            .with_context(|| format!("failed writing to {:?}", tmp_path))?;
        if sniff.len() < 2048 {
            let remaining = 2048usize.saturating_sub(sniff.len());
            let take = std::cmp::min(remaining, n);
            sniff.extend_from_slice(&buffer[..take]);
        }
        received += n as u64;
        bytes_since += n as u64;
        adapt_buffer_size(&mut buffer, &mut bytes_since, &mut last_adjust);
        if let Some((sender, index, artifact_name)) = progress.as_ref() {
            let _ = sender.send(DownloadSignal::Progress {
                artifact: artifact_name.clone(),
                index: *index,
                received,
                size: content_length,
            });
        }
    }

    file.flush()
        .await
        .with_context(|| format!("failed flushing {:?}", tmp_path))?;
    drop(file);

    if looks_like_non_binary_payload(content_type.as_deref(), &sniff, received, &final_file_name) {
        fs::remove_file(&tmp_path).await.ok();
        if url.contains("civitai.com") {
            if auth_token.is_some() {
                return Err(anyhow!(
                    "Civitai returned a non-file response. The token may be invalid/expired, or this model requires additional access."
                ));
            }
            return Err(anyhow!(
                "Civitai returned an access/challenge page. This LoRA likely requires login; add your Civitai API token and retry."
            ));
        }
        return Err(anyhow!(
            "Server returned a non-file response instead of model weights."
        ));
    }

    if fs::try_exists(&dest_path).await.unwrap_or(false) {
        fs::remove_file(&tmp_path).await.ok();
        return Ok(dest_path);
    }

    if let Err(err) = fs::rename(&tmp_path, &dest_path).await {
        if fs::try_exists(&dest_path).await.unwrap_or(false) {
            fs::remove_file(&tmp_path).await.ok();
            return Ok(dest_path);
        }
        return Err(err)
            .with_context(|| format!("failed to move {:?} to {:?}", tmp_path, dest_path));
    }

    if let Some((sender, index, artifact_name)) = progress {
        let _ = sender.send(DownloadSignal::Finished {
            artifact: artifact_name.clone(),
            index,
            size: content_length,
            folder: dest_path.parent().map(|p| p.to_string_lossy().to_string()),
        });
    }

    Ok(dest_path)
}

fn looks_like_non_binary_payload(
    content_type: Option<&str>,
    first_bytes: &[u8],
    total_bytes: u64,
    file_name: &str,
) -> bool {
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let expects_binary = matches!(
        ext.as_deref(),
        Some("safetensors" | "ckpt" | "pt" | "pth" | "bin")
    );
    if !expects_binary {
        return false;
    }

    if let Some(ct) = content_type {
        if ct.contains("text/html")
            || ct.contains("application/json")
            || ct.contains("text/plain")
            || ct.contains("application/xml")
        {
            return true;
        }
    }

    if total_bytes > 512 * 1024 {
        return false;
    }

    let prefix = String::from_utf8_lossy(first_bytes).to_ascii_lowercase();
    let trimmed = prefix.trim_start();
    trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<?xml")
        || trimmed.starts_with("{\"error\"")
        || trimmed.starts_with("{\"detail\"")
        || trimmed.contains("cloudflare")
        || trimmed.contains("access denied")
}

struct HeadMetadata {
    content_length: Option<u64>,
    file_name: Option<String>,
    accept_ranges: bool,
}

fn with_bearer(request: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(token) => request.header("Authorization", format!("Bearer {token}")),
        None => request,
    }
}

async fn fetch_head_metadata(
    client: &Client,
    url: &str,
    auth_token: Option<&str>,
    fallback_name: &str,
) -> Result<Option<HeadMetadata>> {
    let mut request = client.head(url);
    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = match timeout(std::time::Duration::from_secs(8), request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return Ok(None),
        Err(_) => {
            warn!("HEAD metadata probe timed out for {}", url);
            return Ok(None);
        }
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    let headers = response.headers();
    let file_name = Some(filename_from_headers(headers, fallback_name));
    let accept_ranges = headers
        .get(header::ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("bytes"))
        .unwrap_or(false);

    let content_length = response.content_length().filter(|value| *value > 0);

    Ok(Some(HeadMetadata {
        content_length,
        file_name,
        accept_ranges,
    }))
}

async fn probe_range_support(
    client: &Client,
    url: &str,
    auth_token: Option<&str>,
) -> Result<Option<u64>> {
    let mut request = client.get(url).header(header::RANGE, "bytes=0-0");
    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = match timeout(std::time::Duration::from_secs(8), request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return Ok(None),
        Err(_) => {
            warn!("Range probe timed out for {}", url);
            return Ok(None);
        }
    };

    if response.status().as_u16() != 206 {
        return Ok(None);
    }

    let total = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range_total);

    if let Some(total) = total {
        Ok(Some(total))
    } else {
        Ok(None)
    }
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    let range = value.trim();
    let total = range.split('/').nth(1)?;
    total.parse().ok()
}

fn is_cancelled(cancel: Option<&CancellationToken>) -> bool {
    cancel.map(|token| token.is_cancelled()).unwrap_or(false)
}

/// Rebuilds `err` with every occurrence of `secret` scrubbed from its
/// rendered (`{:#}`, i.e. full context-chain) text. Used to keep a Civitai
/// API token -- embedded in a download URL because the Authorization header
/// doesn't survive Civitai's redirect to its CDN -- out of any error string
/// that reaches a progress event, log line, or UI toast. Rebuilding is
/// necessary because `anyhow::Error`'s context chain has no in-place
/// string-mutation API; callers that need to `downcast_ref` a specific error
/// type must do so on the original error *before* calling this, since the
/// rebuilt error is a plain string-backed `anyhow::Error`.
fn redact_error(err: anyhow::Error, secret: Option<&str>) -> anyhow::Error {
    match secret {
        Some(secret) if !secret.is_empty() => {
            let rendered = format!("{err:#}").replace(secret, "***");
            anyhow!(rendered)
        }
        _ => err,
    }
}

#[cfg(test)]
mod redact_error_tests {
    use super::redact_error;
    use anyhow::anyhow;

    #[test]
    fn strips_the_secret_from_every_layer_of_context() {
        let err = anyhow!("request failed for https://civitai.com/api/download/models/1?token=sekret123")
            .context("download failed for https://civitai.com/api/download/models/1?token=sekret123 (status 403)");

        let redacted = redact_error(err, Some("sekret123"));

        let rendered = format!("{redacted:#}");
        assert!(
            !rendered.contains("sekret123"),
            "token leaked into: {rendered}"
        );
    }

    #[test]
    fn leaves_the_error_untouched_when_there_is_no_secret() {
        let err = anyhow!("some unrelated failure");
        let redacted = redact_error(err, None);
        assert_eq!(format!("{redacted:#}"), "some unrelated failure");
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_ranged_to_file(
    clients: &[Client],
    url: &str,
    dest_dir: &Path,
    final_file_name: &str,
    total_size: u64,
    progress: Option<(Sender<DownloadSignal>, usize, String)>,
    auth_token: Option<&str>,
    expected_sha: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<PathBuf> {
    if is_cancelled(cancel) {
        return Err(anyhow!("download cancelled by user"));
    }
    fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("failed to create directory {:?}", dest_dir))?;

    let dest_path = dest_dir.join(final_file_name);
    if fs::try_exists(&dest_path).await.unwrap_or(false) {
        return Ok(dest_path);
    }

    let tmp_path = unique_tmp_path(dest_dir, final_file_name);
    let file = fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("failed to create temporary file {:?}", tmp_path))?;
    file.set_len(total_size)
        .await
        .with_context(|| format!("failed to size {:?}", tmp_path))?;
    drop(file);

    let chunk_size = CHUNK_SIZE_BYTES;
    let mut ranges = Vec::new();
    let mut offset = 0u64;
    while offset < total_size {
        let end = std::cmp::min(total_size - 1, offset + chunk_size - 1);
        ranges.push((offset, end));
        offset = end + 1;
    }

    let semaphore = Arc::new(Semaphore::new(CHUNK_CONCURRENCY));
    let received = Arc::new(AtomicU64::new(0));
    let multipart_abort = CancellationToken::new();
    let completed_chunks = Arc::new(
        (0..ranges.len())
            .map(|_| AtomicBool::new(false))
            .collect::<Vec<_>>(),
    );
    let artifact_name = progress.as_ref().map(|(_, _, name)| name.clone());

    let client_count = clients.len();
    if client_count == 0 {
        return Err(anyhow!("missing HTTP client for ranged download"));
    }

    let tasks = futures::stream::iter(ranges.iter().copied().enumerate()).map(|(idx, (start, end))| {
        let tmp_path = tmp_path.clone();
        let semaphore = Arc::clone(&semaphore);
        let client = clients[idx % client_count].clone();
        let url = url.to_string();
        let progress = progress.clone();
        let auth_token = auth_token.map(|token| token.to_string());
        let received = Arc::clone(&received);
        let completed_chunks = Arc::clone(&completed_chunks);
        let multipart_abort = multipart_abort.clone();
        let artifact_name = artifact_name.clone();
        let cancel = cancel.cloned();
        async move {
            if multipart_abort.is_cancelled() {
                return Err(anyhow!("multipart download stopped after another range failed"));
            }
            if is_cancelled(cancel.as_ref()) {
                return Err(anyhow!("download cancelled by user"));
            }
            let _permit = semaphore.acquire().await?;
            let mut request = client
                .get(&url)
                .header(header::RANGE, format!("bytes={start}-{end}"));
            if let Some(token) = auth_token.as_deref() {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            let response = request
                .send()
                .await
                .with_context(|| format!("request failed for {url}"))?
                .error_for_status()
                .with_context(|| format!("unexpected status downloading {url}"))?;

            if response.status().as_u16() != 206 {
                return Err(anyhow!("server did not honor range request for {url}"));
            }

            let mut file = fs::OpenOptions::new()
                .write(true)
                .open(&tmp_path)
                .await
                .with_context(|| format!("failed to open {:?}", tmp_path))?;
            file.seek(SeekFrom::Start(start))
                .await
                .with_context(|| format!("failed to seek in {:?}", tmp_path))?;
            let stream = response.bytes_stream().map_err(std::io::Error::other);
            let mut reader = StreamReader::new(stream);
            let mut buffer = vec![0u8; IO_BUFFER_INITIAL];
            let mut bytes_since = 0u64;
            let mut last_adjust = Instant::now();
            let mut last_progress = Instant::now();
            let expected_chunk_size = end - start + 1;
            let mut chunk_received = 0u64;

            loop {
                if is_cancelled(cancel.as_ref()) {
                    return Err(anyhow!("download cancelled by user"));
                }
                let n = match timeout(
                    std::time::Duration::from_millis(500),
                    reader.read(&mut buffer),
                )
                .await
                {
                    Ok(Ok(n)) => n,
                    Ok(Err(err)) => {
                        return Err(err).with_context(|| format!("failed streaming {url}"));
                    }
                    Err(_) if last_progress.elapsed() < STREAM_STALL_TIMEOUT => continue,
                    Err(_) => {
                        return Err(anyhow!(
                            "no data received for {} seconds while downloading bytes {start}-{end} from {url}",
                            STREAM_STALL_TIMEOUT.as_secs()
                        ));
                    }
                };
                if n == 0 {
                    break;
                }
                if chunk_received + n as u64 > expected_chunk_size {
                    return Err(anyhow!(
                        "server sent more than the requested range {start}-{end} for {url}"
                    ));
                }
                file.write_all(&buffer[..n])
                    .await
                    .with_context(|| format!("failed writing to {:?}", tmp_path))?;
                chunk_received += n as u64;
                last_progress = Instant::now();
                let new_total = received.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
                bytes_since += n as u64;
                adapt_buffer_size(&mut buffer, &mut bytes_since, &mut last_adjust);
                if let (Some((sender, index, _)), Some(name)) =
                    (progress.as_ref(), artifact_name.as_ref())
                {
                    let _ = sender.send(DownloadSignal::Progress {
                        artifact: name.clone(),
                        index: *index,
                        received: new_total,
                        size: Some(total_size),
                    });
                }
            }

            if chunk_received != expected_chunk_size {
                return Err(anyhow!(
                    "range {start}-{end} ended early at {chunk_received} of {expected_chunk_size} bytes for {url}"
                ));
            }
            file.flush()
                .await
                .with_context(|| format!("failed flushing range {start}-{end} to {:?}", tmp_path))?;
            completed_chunks[idx].store(true, Ordering::Release);

            Ok::<_, anyhow::Error>(())
        }
    });

    let mut tasks = tasks.buffer_unordered(CHUNK_CONCURRENCY);
    let mut download_error = None;
    while let Some(result) = tasks.next().await {
        if let Err(err) = result {
            if download_error.is_none() {
                download_error = Some(err);
                multipart_abort.cancel();
            }
        }
    }

    if let Some(err) = download_error {
        let contiguous_chunks = completed_chunks
            .iter()
            .take_while(|complete| complete.load(Ordering::Acquire))
            .count();
        let resumable_bytes = std::cmp::min(total_size, contiguous_chunks as u64 * chunk_size);
        if resumable_bytes > 0 && resumable_bytes < total_size {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(&tmp_path)
                .await
                .with_context(|| {
                    format!("failed to preserve resumable prefix in {:?}", tmp_path)
                })?;
            file.set_len(resumable_bytes).await.with_context(|| {
                format!("failed to truncate resumable prefix in {:?}", tmp_path)
            })?;
            warn!(
                "Multipart download failed; preserved {} contiguous bytes in {:?} for resume",
                resumable_bytes, tmp_path
            );
        } else {
            fs::remove_file(&tmp_path).await.ok();
        }
        return Err(err);
    }

    if let Some(expected) = expected_sha {
        let mut file = fs::File::open(&tmp_path)
            .await
            .with_context(|| format!("failed to read {:?}", tmp_path))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; IO_BUFFER_MAX];
        let mut checked = 0u64;
        let mut last_report: Option<Instant> = None;
        loop {
            let n = file
                .read(&mut buffer)
                .await
                .with_context(|| format!("failed to read {:?}", tmp_path))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            checked += n as u64;
            // A 40 GB file takes minutes to re-read; report progress so the
            // transfer doesn't look stuck at 100%.
            if last_report.is_none_or(|at| at.elapsed() >= VERIFY_REPORT_INTERVAL) {
                last_report = Some(Instant::now());
                if let (Some((sender, index, _)), Some(name)) =
                    (progress.as_ref(), artifact_name.as_ref())
                {
                    let _ = sender.send(DownloadSignal::Verifying {
                        artifact: name.clone(),
                        index: *index,
                        checked,
                        size: Some(total_size),
                    });
                }
            }
        }
        let digest = hasher.finalize();
        let actual = format!("{:x}", digest);
        if actual != expected {
            fs::remove_file(&tmp_path).await.ok();
            return Err(anyhow!(
                "checksum mismatch for {} (expected {}, got {})",
                final_file_name,
                expected,
                actual
            ));
        }
    }

    if fs::try_exists(&dest_path).await.unwrap_or(false) {
        fs::remove_file(&tmp_path).await.ok();
        return Ok(dest_path);
    }

    if let Err(err) = fs::rename(&tmp_path, &dest_path).await {
        if fs::try_exists(&dest_path).await.unwrap_or(false) {
            fs::remove_file(&tmp_path).await.ok();
            return Ok(dest_path);
        }
        return Err(err)
            .with_context(|| format!("failed to move {:?} to {:?}", tmp_path, dest_path));
    }

    Ok(dest_path)
}

fn filename_from_headers(headers: &header::HeaderMap, fallback: &str) -> String {
    // The filename in `Content-Disposition` is attacker-influenceable (it comes
    // straight from the remote server/redirect target), so it must never be
    // trusted to build a filesystem path on its own. `sanitize_file_name`
    // strips path separators and drive-letter colons, which prevents both
    // `../` traversal and absolute-path overwrites (`PathBuf::join` would
    // otherwise happily replace the whole destination with an absolute path).
    headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_disposition)
        .map(|name| sanitize_file_name(&name))
        .unwrap_or_else(|| fallback.to_string())
}

fn parse_content_disposition(value: &str) -> Option<String> {
    for part in value.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix("filename*=") {
            let rest = rest.trim_matches('"');
            let encoded = rest.split("''").last().unwrap_or(rest);
            if let Ok(decoded) = percent_decode_str(encoded).decode_utf8() {
                return Some(decoded.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("filename=") {
            let name = rest.trim_matches('"');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn extract_civitai_model_version_id(url: &str) -> Option<u64> {
    let lower = url.to_ascii_lowercase();

    if let Some(pos) = lower.find("modelversionid=") {
        let remainder = &url[pos + "modelversionid=".len()..];
        let id_str = remainder.split(['&', '#', '/']).next().unwrap_or_default();
        if let Ok(id) = id_str.parse() {
            return Some(id);
        }
    }

    if let Some(pos) = lower.find("/model-versions/") {
        let remainder = &url[pos + "/model-versions/".len()..];
        let id_str = remainder.split(['?', '/', '&']).next().unwrap_or_default();
        if let Ok(id) = id_str.parse() {
            return Some(id);
        }
    }

    if let Some(pos) = lower.find("/models/") {
        let remainder = &url[pos + "/models/".len()..];
        let id_str = remainder.split(['?', '/', '&']).next().unwrap_or_default();
        if let Ok(id) = id_str.parse() {
            return Some(id);
        }
    }

    None
}

/// A file name safe to write into a folder: decoded, without path parts.
pub fn sanitize_file_name(name: &str) -> String {
    let sanitized = percent_decode_str(name)
        .decode_utf8_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect::<String>();
    if sanitized.trim_matches('_').is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

/// Builds a model destination from remotely supplied catalog fields without
/// allowing either field to escape `<download-root>/models`.
///
/// Target categories may be nested (`ultralytics/bbox` is used by the live
/// catalog), while model IDs must remain a single directory name. Both `/`
/// and `\` are treated as separators on every host so a catalog accepted on
/// Linux cannot become unsafe when the same data is consumed on Windows.
fn catalog_model_destination(
    comfy_root: &Path,
    target_category: &str,
    model_id: &str,
) -> Result<PathBuf> {
    let category = validated_catalog_relative_path(target_category, "target category", true)?;
    let model = validated_catalog_relative_path(model_id, "model ID", false)?;
    let models_root = comfy_root.join("models");
    let destination = models_root.join(category).join(model);

    // The component validation above is the security boundary. Keep this
    // additional invariant check so later refactors cannot accidentally
    // reintroduce a rooted or parent component into the constructed path.
    if !destination.starts_with(&models_root) {
        return Err(anyhow!(
            "catalog model destination escaped the configured models directory"
        ));
    }

    Ok(destination)
}

fn validated_catalog_relative_path(
    value: &str,
    field: &str,
    allow_nested: bool,
) -> Result<PathBuf> {
    if value.is_empty() || value.trim() != value {
        return Err(anyhow!("catalog {field} must not be empty or padded"));
    }
    if value.starts_with(['/', '\\']) {
        return Err(anyhow!("catalog {field} must be a relative path"));
    }
    if value.chars().any(|ch| ch.is_control() || ch == ':') {
        return Err(anyhow!("catalog {field} contains an unsafe path character"));
    }

    let components: Vec<&str> = value.split(['/', '\\']).collect();
    if !allow_nested && components.len() != 1 {
        return Err(anyhow!("catalog {field} must be a single path component"));
    }
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(anyhow!("catalog {field} contains an unsafe path component"));
    }

    Ok(components.iter().collect())
}

#[cfg(test)]
mod filename_safety_tests {
    use super::{
        catalog_model_destination, filename_from_headers, find_resumable_tmp_path, is_civitai_url,
        parse_content_disposition, request_token, sanitize_file_name, with_civitai_token_param,
    };
    use reqwest::header::{HeaderMap, HeaderValue, CONTENT_DISPOSITION};
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn civitai_token_param_is_added_once() {
        assert_eq!(
            with_civitai_token_param("https://civitai.com/api/download/models/1", "abc"),
            "https://civitai.com/api/download/models/1?token=abc"
        );
        assert_eq!(
            with_civitai_token_param(
                "https://civitai.com/api/download/models/1?type=Config",
                "abc"
            ),
            "https://civitai.com/api/download/models/1?type=Config&token=abc"
        );
        assert_eq!(
            with_civitai_token_param("https://civitai.com/x?token=old", "abc"),
            "https://civitai.com/x?token=old"
        );
    }

    #[test]
    fn civitai_urls_are_only_civitai_hosts() {
        assert!(is_civitai_url("https://civitai.com/api/download/models/1"));
        assert!(is_civitai_url("https://www.civitai.com/x"));
        for url in [
            "http://civitai.com/api/download/models/1",
            "https://civitai.com.evil.example/x",
            "https://evilcivitai.com/x",
            "https://example.com/?u=https://civitai.com/x",
        ] {
            assert!(!is_civitai_url(url), "{url}");
        }
    }

    #[test]
    fn hf_token_is_only_sent_to_hugging_face() {
        let hf = "https://huggingface.co/org/model/resolve/main/model.safetensors?download=1";
        assert_eq!(request_token(hf, None, Some("hf_abc")), Some("hf_abc"));
        assert_eq!(
            request_token(
                "https://hf.co/org/model/resolve/main/x",
                None,
                Some("hf_abc")
            ),
            Some("hf_abc")
        );
        // Other hosts, including look-alikes, never get it.
        for url in [
            "https://civitai.com/api/download/models/1",
            "https://cdn-lfs.huggingface.co/repos/x",
            "https://huggingface.co.evil.example/x",
            "https://example.com/?u=https://huggingface.co/x",
        ] {
            assert_eq!(request_token(url, None, Some("hf_abc")), None, "{url}");
        }
        // The caller's own token wins; an empty HF token counts as none.
        assert_eq!(
            request_token(hf, Some("civitai"), Some("hf_abc")),
            Some("civitai")
        );
        assert_eq!(request_token(hf, None, Some("")), None);
        assert_eq!(request_token(hf, None, None), None);
    }

    #[test]
    fn sanitize_file_name_strips_traversal_and_absolute_path_markers() {
        // No path separators or drive-letter colons should survive, so the
        // result is always safe to `Path::join` onto a destination dir.
        assert_eq!(sanitize_file_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_file_name("/etc/passwd"), "_etc_passwd");
        assert_eq!(
            sanitize_file_name(r"C:\Windows\System32\evil.dll"),
            "C__Windows_System32_evil.dll"
        );
        assert_eq!(sanitize_file_name(r"..\..\evil.exe"), ".._.._evil.exe");
    }

    #[test]
    fn sanitize_file_name_falls_back_when_nothing_but_separators() {
        assert_eq!(sanitize_file_name("///"), "download");
    }

    #[test]
    fn catalog_destination_allows_legitimate_nested_categories() {
        assert_eq!(
            catalog_model_destination(
                Path::new("/home/user/ComfyUI"),
                "ultralytics/bbox",
                "detector-model"
            )
            .unwrap(),
            Path::new("/home/user/ComfyUI/models/ultralytics/bbox/detector-model")
        );
    }

    #[test]
    fn catalog_destination_rejects_parent_and_absolute_paths_on_every_platform() {
        let root = Path::new("/home/user/ComfyUI");
        for category in [
            "../outside",
            "ultralytics/../../outside",
            r"ultralytics\..\outside",
            "/tmp/models",
            r"C:\Users\Public",
            r"\\server\share",
        ] {
            assert!(
                catalog_model_destination(root, category, "model").is_err(),
                "unsafe category was accepted: {category}"
            );
        }

        for model_id in ["../outside", r"..\outside", "/tmp", r"C:\Temp", "a/b"] {
            assert!(
                catalog_model_destination(root, "checkpoints", model_id).is_err(),
                "unsafe model ID was accepted: {model_id}"
            );
        }
    }

    #[test]
    fn parse_content_disposition_reads_plain_filename() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="model.safetensors""#),
            Some("model.safetensors".to_string())
        );
    }

    #[test]
    fn parse_content_disposition_reads_rfc5987_filename() {
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''model.safetensors"),
            Some("model.safetensors".to_string())
        );
    }

    #[test]
    fn filename_from_headers_sanitizes_a_traversal_attempt() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static(r#"attachment; filename="../../../etc/passwd""#),
        );

        let resolved = filename_from_headers(&headers, "fallback.bin");

        // The join must stay a direct child of dest_dir: no separators means
        // no way to climb out of it or land on an absolute path.
        let dest_dir = Path::new("/home/user/ComfyUI/models/checkpoints");
        let dest_path = dest_dir.join(&resolved);
        assert_eq!(dest_path.parent(), Some(dest_dir));
        assert!(!resolved.contains('/') && !resolved.contains('\\'));
    }

    #[test]
    fn filename_from_headers_sanitizes_an_absolute_path_attempt() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static(r#"attachment; filename="/etc/passwd""#),
        );

        let resolved = filename_from_headers(&headers, "fallback.bin");
        let dest_dir = Path::new("/home/user/ComfyUI/models/checkpoints");
        let dest_path = dest_dir.join(&resolved);

        // Before the fix, PathBuf::join with an absolute component would
        // discard dest_dir entirely and resolve straight to "/etc/passwd".
        assert_eq!(dest_path.parent(), Some(dest_dir));
    }

    #[test]
    fn filename_from_headers_falls_back_without_content_disposition() {
        let headers = HeaderMap::new();
        assert_eq!(
            filename_from_headers(&headers, "fallback.bin"),
            "fallback.bin"
        );
    }

    #[test]
    fn resumable_tmp_path_prefers_the_largest_contiguous_partial() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "arctic-helper-resume-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temporary test directory should be created");

        let smaller = dir.join("model.gguf.part.0");
        let larger = dir.join("model.gguf.part.1");
        fs::write(&smaller, vec![0_u8; 32]).expect("smaller partial should be written");
        fs::write(&larger, vec![0_u8; 64]).expect("larger partial should be written");
        fs::write(dir.join("another-model.gguf.part.0"), vec![0_u8; 96])
            .expect("unrelated partial should be written");

        assert_eq!(
            find_resumable_tmp_path(&dir, "model.gguf", 128),
            Some(larger)
        );

        fs::remove_dir_all(&dir).expect("temporary test directory should be removed");
    }
}

fn normalize_folder_name(name: &str) -> String {
    let mut normalized = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else if !normalized.ends_with('_') {
            normalized.push('_');
        }
    }
    normalized.trim_matches('_').to_string()
}

fn unique_tmp_path(dest_dir: &Path, final_file_name: &str) -> PathBuf {
    let suffix = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dest_dir.join(format!("{final_file_name}.part.{suffix}"))
}

fn find_resumable_tmp_path(
    dest_dir: &Path,
    final_file_name: &str,
    total_size: u64,
) -> Option<PathBuf> {
    let prefix = format!("{final_file_name}.part.");
    std::fs::read_dir(dest_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(&prefix) {
                return None;
            }
            let len = entry.metadata().ok()?.len();
            (len > 0 && len < total_size).then_some((len, path))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, path)| path)
}

fn adapt_buffer_size(buffer: &mut Vec<u8>, bytes_since: &mut u64, last_adjust: &mut Instant) {
    if *bytes_since < ADAPTIVE_STEP_BYTES {
        return;
    }

    let elapsed = last_adjust.elapsed().as_secs_f64().max(0.001);
    let mbps = *bytes_since as f64 / 1024.0 / 1024.0 / elapsed;
    if mbps > ADAPTIVE_GROW_MBPS && buffer.len() < IO_BUFFER_MAX {
        let next = (buffer.len() * 2).min(IO_BUFFER_MAX);
        buffer.resize(next, 0);
    } else if mbps < ADAPTIVE_SHRINK_MBPS && buffer.len() > IO_BUFFER_MIN {
        let next = (buffer.len() / 2).max(IO_BUFFER_MIN);
        buffer.resize(next, 0);
    }

    *bytes_since = 0;
    *last_adjust = Instant::now();
}

fn dedupe_artifacts(artifacts: Vec<ModelArtifact>) -> Vec<ModelArtifact> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for artifact in artifacts {
        let key = (
            artifact.target_category.slug().to_string(),
            artifact.repo.clone(),
            artifact.path.clone(),
            artifact.direct_url.clone(),
            artifact.file_name().to_string(),
        );
        if seen.insert(key) {
            deduped.push(artifact);
        }
    }
    deduped
}

/// Bytes a model download still has to write for `artifacts`, counting only
/// artifacts with a known size: files already in place need nothing, and a
/// resumable partial file counts as already downloaded.
pub fn remaining_download_bytes(
    comfy_root: &Path,
    model_folder: &str,
    artifacts: &[ModelArtifact],
) -> u64 {
    dedupe_artifacts(artifacts.to_vec())
        .iter()
        .filter_map(|artifact| {
            let size = artifact.size_bytes?;
            let dest_dir = catalog_model_destination(
                comfy_root,
                artifact.target_category.slug(),
                model_folder,
            )
            .ok()?;
            let file_name = sanitize_file_name(artifact.file_name());
            if dest_dir.join(&file_name).exists() {
                return Some(0);
            }
            let partial = find_resumable_tmp_path(&dest_dir, &file_name, size)
                .and_then(|path| std::fs::metadata(path).ok())
                .map_or(0, |meta| meta.len());
            Some(size.saturating_sub(partial))
        })
        .sum()
}

#[cfg(test)]
mod remaining_bytes_tests {
    use super::*;

    fn artifact(path: &str, size: Option<u64>) -> ModelArtifact {
        serde_json::from_value(serde_json::json!({
            "repo": "hf://org/repo", "path": path, "target_category": "vae", "size_bytes": size
        }))
        .unwrap()
    }

    #[test]
    fn present_and_partial_files_reduce_what_is_needed() {
        let root = std::env::temp_dir().join(format!("arctic-remaining-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = catalog_model_destination(&root, "vae", "m").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("done.safetensors"), b"x").unwrap();
        std::fs::write(dir.join("half.safetensors.part.3"), vec![0u8; 40]).unwrap();
        let artifacts = [
            artifact("done.safetensors", Some(1_000)),
            artifact("half.safetensors", Some(100)),
            artifact("new.safetensors", Some(500)),
            artifact("unknown.safetensors", None),
        ];
        assert_eq!(remaining_download_bytes(&root, "m", &artifacts), 60 + 500);
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Whether `url` is on civitai.com, the only host the Civitai token is sent
/// to by `download_file_with_cancel`.
fn is_civitai_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .filter(|parsed| parsed.scheme() == "https")
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "civitai.com" || host.ends_with(".civitai.com"))
}

/// Whether `url` is on Hugging Face, the only host the HF token is sent to.
/// (reqwest drops the header when a request is redirected to another host,
/// such as Hugging Face's CDN.)
fn is_hf_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "huggingface.co" || host == "hf.co")
}

/// The bearer token for a request to `url`: the caller's own (Civitai) token,
/// else the Hugging Face token for Hugging Face URLs.
fn request_token<'a>(
    url: &str,
    explicit: Option<&'a str>,
    hf_token: Option<&'a str>,
) -> Option<&'a str> {
    explicit.or_else(|| hf_token.filter(|token| !token.is_empty() && is_hf_url(url)))
}

/// The error for a Hugging Face 401/403: the repo is gated or private.
fn hf_access_error(url: &str, status: reqwest::StatusCode, sent_token: bool) -> anyhow::Error {
    if sent_token {
        anyhow!(
            "Hugging Face refused access to {url} (status {status}). Open the model page on huggingface.co, accept its license or request access, and check that your token can read it."
        )
    } else {
        anyhow!(
            "{url} is gated or private on Hugging Face (status {status}). Accept the model's license on huggingface.co, then save a Hugging Face token in the Models tab and retry."
        )
    }
}

fn ensure_hf_download_url(url: &str) -> String {
    if let Ok(mut parsed) = Url::parse(url) {
        if parsed.host_str() == Some("huggingface.co") && parsed.path().contains("/resolve/") {
            let has_download = parsed
                .query_pairs()
                .any(|(k, _)| k.eq_ignore_ascii_case("download"));
            if !has_download {
                parsed.query_pairs_mut().append_pair("download", "1");
                return parsed.to_string();
            }
        }
    }
    url.to_string()
}

#[derive(Debug, Clone)]
struct HfResolveUrl {
    repo_id: String,
    revision: String,
    file_path: String,
    file_name: String,
}

fn parse_hf_resolve_url(url: &str) -> Option<HfResolveUrl> {
    let parsed = Url::parse(url).ok()?;
    if parsed.host_str() != Some("huggingface.co") {
        return None;
    }
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let resolve_idx = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("resolve"))?;
    if resolve_idx < 2 || resolve_idx + 2 >= segments.len() {
        return None;
    }

    let repo_id = segments[..resolve_idx].join("/");
    let revision = segments.get(resolve_idx + 1)?.to_string();
    let file_path = segments[(resolve_idx + 2)..].join("/");
    let file_name = segments.last()?.to_string();
    if repo_id.is_empty() || revision.is_empty() || file_path.is_empty() || file_name.is_empty() {
        return None;
    }

    Some(HfResolveUrl {
        repo_id,
        revision,
        file_path,
        file_name,
    })
}

fn hf_cli_available() -> bool {
    *HF_CLI_AVAILABLE.get_or_init(|| hf_bin_available() || uvx_available())
}

fn hf_bin_available() -> bool {
    *HF_BIN_AVAILABLE.get_or_init(|| {
        std::process::Command::new("hf")
            .arg("--help")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    })
}

fn uvx_available() -> bool {
    *UVX_AVAILABLE.get_or_init(|| {
        std::process::Command::new("uvx")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    })
}

fn hf_command() -> Command {
    if hf_bin_available() {
        Command::new("hf")
    } else {
        let mut c = Command::new("uvx");
        c.arg("hf");
        c
    }
}

async fn download_via_hf_cli(
    parsed: &HfResolveUrl,
    dest_dir: &Path,
    progress: Option<(Sender<DownloadSignal>, usize, String)>,
    expected_size: Option<u64>,
    hf_token: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<PathBuf> {
    fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("failed to create directory {:?}", dest_dir))?;
    let staging_root = dest_dir.join(".arctic-hf-staging");

    let flat_existing = dest_dir.join(&parsed.file_name);
    let nested_existing = dest_dir.join(&parsed.file_path);
    if fs::try_exists(&flat_existing).await.unwrap_or(false) {
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_existing);
    }
    if fs::try_exists(&nested_existing).await.unwrap_or(false) {
        fs::rename(&nested_existing, &flat_existing)
            .await
            .with_context(|| {
                format!(
                    "failed to move existing hf file from {} to {}",
                    nested_existing.display(),
                    flat_existing.display()
                )
            })?;
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_existing);
    }

    let stage_id = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stage_dir = staging_root.join(format!("job-{stage_id}"));
    if let Some(parent) = Path::new(&parsed.file_path).parent() {
        if parent != Path::new("") {
            fs::create_dir_all(stage_dir.join(parent))
                .await
                .with_context(|| {
                    format!(
                        "failed to prepare hf staging parent {:?}",
                        stage_dir.join(parent)
                    )
                })?;
        }
    } else {
        fs::create_dir_all(&stage_dir)
            .await
            .with_context(|| format!("failed to create hf staging dir {}", stage_dir.display()))?;
    }

    let mut cmd = hf_command();
    log::info!(
        "HF/Xet staging download: repo_file={}, local_dir={}",
        parsed.file_path,
        stage_dir.display()
    );
    cmd.arg("download")
        .arg(&parsed.repo_id)
        .arg(&parsed.file_path)
        .arg("--revision")
        .arg(&parsed.revision)
        .arg("--local-dir")
        .arg(&stage_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("HF_HUB_DISABLE_PROGRESS_BARS", "1")
        .env("HF_XET_HIGH_PERFORMANCE", "1");
    if let Some(token) = hf_token.filter(|token| !token.is_empty()) {
        cmd.env("HF_TOKEN", token);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| "failed to spawn 'hf download' command")?;

    let expected_size = if expected_size.is_some() {
        expected_size
    } else {
        hf_dry_run_size(parsed, hf_token).await
    };

    let mut last_reported = 0u64;
    if let Some((sender, index, artifact_name)) = progress.as_ref() {
        // Seed UI with known total size early so progress can be determinate.
        let _ = sender.send(DownloadSignal::Progress {
            artifact: artifact_name.clone(),
            index: *index,
            received: 0,
            size: expected_size,
        });
    }
    let status = loop {
        if let Some(token) = cancel {
            if token.is_cancelled() {
                let _ = child.kill().await;
                let _ = child.wait().await;
                cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
                return Err(anyhow!("download cancelled by user"));
            }
        }

        if let Some((sender, index, artifact_name)) = progress.as_ref() {
            let mut received = hf_downloaded_bytes(&stage_dir, parsed)
                .await
                .unwrap_or(last_reported);
            if let Some(size) = expected_size {
                received = received.min(size);
            }
            if received > last_reported {
                last_reported = received;
                let _ = sender.send(DownloadSignal::Progress {
                    artifact: artifact_name.clone(),
                    index: *index,
                    received,
                    size: expected_size,
                });
            }
        }

        match child
            .try_wait()
            .with_context(|| "failed waiting for 'hf download' command")?
        {
            Some(status) => break status,
            None => tokio::time::sleep(std::time::Duration::from_millis(150)).await,
        }
    };

    if !status.success() {
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Err(anyhow!("hf download exited with status {}", status));
    }

    let flat_path = dest_dir.join(&parsed.file_name);
    // `hf download` materializes nested repo paths under `--local-dir`.
    // Move the downloaded file back to the flat destination used by non-Xet mode.
    let nested_path = stage_dir.join(&parsed.file_path);
    if fs::try_exists(&nested_path).await.unwrap_or(false) {
        move_file_with_fallback(&nested_path, &flat_path).await?;
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_path);
    }

    // Defensive fallback: if a legacy/old binary wrote into dest_dir directly,
    // normalize it back to flat layout and clean up empty nested folders.
    let legacy_nested_path = dest_dir.join(&parsed.file_path);
    if fs::try_exists(&legacy_nested_path).await.unwrap_or(false) {
        move_file_with_fallback(&legacy_nested_path, &flat_path).await?;
        if let Some(parent) = legacy_nested_path.parent() {
            remove_empty_parents_until(parent, dest_dir).await;
        }
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_path);
    }

    let staged_flat_path = stage_dir.join(&parsed.file_name);
    if fs::try_exists(&staged_flat_path).await.unwrap_or(false) {
        move_file_with_fallback(&staged_flat_path, &flat_path).await?;
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_path);
    }

    if fs::try_exists(&flat_path).await.unwrap_or(false) {
        cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
        return Ok(flat_path);
    }

    cleanup_xet_local_sidecars(dest_dir, &staging_root).await;
    Err(anyhow!(
        "hf download completed but output file was not found at {} or {}",
        flat_path.display(),
        nested_path.display()
    ))
}

async fn move_file_with_fallback(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create destination parent {}", parent.display()))?;
    }
    match fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            fs::copy(src, dst).await.with_context(|| {
                format!(
                    "failed to copy hf-downloaded file from {} to {} after rename error: {}",
                    src.display(),
                    dst.display(),
                    rename_err
                )
            })?;
            fs::remove_file(src).await.ok();
            Ok(())
        }
    }
}

async fn remove_empty_parents_until(start: &Path, stop_at: &Path) {
    let mut current = start.to_path_buf();
    loop {
        if current == stop_at {
            break;
        }
        match fs::remove_dir(&current).await {
            Ok(()) => {
                if let Some(parent) = current.parent() {
                    current = parent.to_path_buf();
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

async fn cleanup_xet_local_sidecars(dest_dir: &Path, staging_root: &Path) {
    let _ = fs::remove_dir_all(staging_root).await;
    if let Some(parent) = staging_root.parent() {
        remove_empty_parents_until(parent, dest_dir).await;
    }

    // Drop legacy local Hugging Face cache under model folders to avoid duplicate payload usage.
    let legacy_download_cache = dest_dir.join(".cache").join("huggingface").join("download");
    if fs::try_exists(&legacy_download_cache)
        .await
        .unwrap_or(false)
    {
        let _ = fs::remove_dir_all(&legacy_download_cache).await;
        if let Some(parent) = legacy_download_cache.parent() {
            remove_empty_parents_until(parent, dest_dir).await;
        }
    }
}

async fn hf_dry_run_size(parsed: &HfResolveUrl, hf_token: Option<&str>) -> Option<u64> {
    let mut cmd = hf_command();
    if let Some(token) = hf_token.filter(|token| !token.is_empty()) {
        cmd.env("HF_TOKEN", token);
    }
    let output = cmd
        .arg("download")
        .arg(&parsed.repo_id)
        .arg(&parsed.file_path)
        .arg("--revision")
        .arg(&parsed.revision)
        .arg("--dry-run")
        .env("HF_XET_HIGH_PERFORMANCE", "1")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        // Example: [dry-run] Will download 1 files (out of 1) totalling 28.6G.
        if let Some(rest) = line.split("totalling ").nth(1) {
            let token = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches('.')
                .trim();
            if let Some(bytes) = parse_hf_size_token(token) {
                return Some(bytes);
            }
        }

        // Fallback: parse table lines like ".../filename.safetensors 28.6G"
        if line.contains(&parsed.file_name) {
            if let Some(last) = line.split_whitespace().last() {
                if let Some(bytes) = parse_hf_size_token(last.trim_end_matches('.').trim()) {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

fn parse_hf_size_token(token: &str) -> Option<u64> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unit_char = trimmed.chars().last()?;
    let (num_part, mul) = match unit_char {
        'K' | 'k' => (&trimmed[..trimmed.len() - 1], 1024_f64),
        'M' | 'm' => (&trimmed[..trimmed.len() - 1], 1024_f64 * 1024_f64),
        'G' | 'g' => (
            &trimmed[..trimmed.len() - 1],
            1024_f64 * 1024_f64 * 1024_f64,
        ),
        'T' | 't' => (
            &trimmed[..trimmed.len() - 1],
            1024_f64 * 1024_f64 * 1024_f64 * 1024_f64,
        ),
        _ => (trimmed, 1.0),
    };
    let value = num_part.parse::<f64>().ok()?;
    if value <= 0.0 {
        return None;
    }
    Some((value * mul) as u64)
}

async fn hf_downloaded_bytes(dest_dir: &Path, parsed: &HfResolveUrl) -> Option<u64> {
    let flat_path = dest_dir.join(&parsed.file_name);
    let nested_path = dest_dir.join(&parsed.file_path);

    // Prefer real destination file growth when present. Cache growth can be bursty.
    if let Ok(meta) = fs::metadata(&flat_path).await {
        return Some(meta.len());
    }
    if let Ok(meta) = fs::metadata(&nested_path).await {
        return Some(meta.len());
    }

    // During hf_xet downloads, bytes are typically written to a hashed
    // `*.incomplete` file under:
    // <local-dir>/.cache/huggingface/download/<repo_subdir>/
    // Track that folder directly for per-file progress.
    let mut rel_dir = PathBuf::new();
    if let Some(parent) = Path::new(&parsed.file_path).parent() {
        if parent != Path::new("") {
            rel_dir = parent.to_path_buf();
        }
    }
    let download_dir = dest_dir
        .join(".cache")
        .join("huggingface")
        .join("download")
        .join(rel_dir);

    let mut best = 0u64;
    let mut rd = fs::read_dir(&download_dir).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".incomplete") {
            continue;
        }
        if let Ok(meta) = entry.metadata().await {
            best = best.max(meta.len());
        }
        // Some hf builds may use temp suffixes; keep a defensive branch.
        if path.extension().and_then(|x| x.to_str()) == Some("tmp") {
            if let Ok(meta) = entry.metadata().await {
                best = best.max(meta.len());
            }
        }
    }

    if best > 0 {
        Some(best)
    } else {
        None
    }
}

fn build_download_url(repo: &str, path: &str) -> Result<String> {
    if let Some(rest) = repo.strip_prefix("hf://") {
        let mut parts = rest.split('@');
        let repo_path = parts
            .next()
            .ok_or_else(|| anyhow!("invalid Hugging Face repo string: {repo}"))?;
        let revision = parts.next().unwrap_or("main");
        Ok(format!(
            "https://huggingface.co/{repo_path}/resolve/{revision}/{path}?download=1"
        ))
    } else if let Some(blob_index) = repo.find("/blob/") {
        let (base, remainder) = repo.split_at(blob_index);
        let remainder = &remainder["/blob/".len()..];
        let mut segments = remainder.splitn(2, '/');
        let revision = segments
            .next()
            .ok_or_else(|| anyhow!("missing revision in {repo}"))?;
        let file_path = segments
            .next()
            .ok_or_else(|| anyhow!("missing file path in {repo}"))?;
        let repo_path = base.trim_start_matches("https://huggingface.co/");
        Ok(format!(
            "https://huggingface.co/{repo_path}/resolve/{revision}/{file_path}?download=1"
        ))
    } else if repo.starts_with("https://") {
        Ok(format!("{repo}/resolve/main/{path}?download=1"))
    } else {
        Err(anyhow!("unsupported repository scheme in {repo}"))
    }
}

async fn fetch_civitai_model_metadata(
    client: &Client,
    download_url: &str,
    token: Option<&str>,
) -> Result<CivitaiModelMetadata> {
    let model_version_id = extract_civitai_model_version_id(download_url)
        .ok_or_else(|| anyhow!("unable to parse model version ID from {download_url}"))?;
    fetch_civitai_model_metadata_internal(client, model_version_id, download_url, token).await
}

async fn fetch_civitai_model_metadata_internal(
    client: &Client,
    model_version_id: u64,
    download_url: &str,
    token: Option<&str>,
) -> Result<CivitaiModelMetadata> {
    let api_url = format!("https://civitai.com/api/v1/model-versions/{model_version_id}");

    let mut request = client.get(&api_url);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {api_url}"))?;

    if response.status().as_u16() == 401 {
        return Err(DownloadError::Unauthorized.into());
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("unexpected status downloading metadata from {api_url}"))?;

    let payload: CivitaiModelVersion = response
        .json()
        .await
        .with_context(|| format!("failed to parse metadata payload for {api_url}"))?;

    let model_description = payload
        .model
        .as_ref()
        .and_then(|model| model.description.clone());

    let CivitaiModelVersion {
        trained_words,
        images,
        files,
        download_url: api_download_url,
        model,
        model_id,
        description,
        meta,
        settings,
    } = payload;

    let selected_file = select_civitai_file(&files, download_url);
    let file_name = selected_file
        .and_then(|file| file.name.clone())
        .unwrap_or_else(|| fallback_file_name_from_url(download_url, model_version_id));
    let selected_download_url = selected_file
        .and_then(|file| file.download_url.clone())
        .or(api_download_url.clone());

    let (preview, preview_url) = resolve_preview(client, &images, token, model_version_id).await;

    let mut description = select_richest_description(description, model_description);
    let mut usage_strength = extract_usage_strength(settings.as_ref(), meta.as_ref(), &images);
    let mut creator_username = None;
    let mut creator_link = None;

    if let Some(model) = model {
        if let Some(creator) = model.creator {
            creator_username = creator.username;
            creator_link = creator.link;
        }
        if description.is_none() {
            description = normalize_description(model.description);
        }
    }

    let description_too_short = description
        .as_ref()
        .map(|text| description_word_count(text) < 400)
        .unwrap_or(true);

    if creator_username.is_none()
        || description.is_none()
        || usage_strength.is_none()
        || description_too_short
    {
        if let Some(model_id) = model_id {
            match fetch_civitai_model_details(client, model_id, model_version_id, token).await {
                Ok(details) => {
                    description =
                        select_richest_description(description, details.description.clone());
                    if creator_username.is_none() {
                        if let Some(creator) = details.creator {
                            creator_username = creator.username;
                            creator_link = creator.link;
                        }
                    }
                    if usage_strength.is_none() {
                        usage_strength = details.version_strength;
                    }
                }
                Err(err) => warn!("Failed to fetch creator info for model {model_id}: {err}"),
            }
        }
    }

    if usage_strength.is_none() {
        if let Some(strength) = fetch_strength_from_html(client, model_id, model_version_id).await {
            usage_strength = Some(strength);
        }
    }

    Ok(CivitaiModelMetadata {
        file_name,
        download_url: selected_download_url,
        preview,
        preview_url,
        trained_words,
        description,
        usage_strength,
        creator_username,
        creator_link,
    })
}

fn normalize_description(description: Option<String>) -> Option<String> {
    description
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn select_richest_description(
    version_description: Option<String>,
    model_description: Option<String>,
) -> Option<String> {
    let version_description = normalize_description(version_description);
    let model_description = normalize_description(model_description);

    match (version_description, model_description) {
        (Some(version), Some(model)) => {
            if description_word_count(&model) > description_word_count(&version) {
                Some(model)
            } else {
                Some(version)
            }
        }
        (Some(version), None) => Some(version),
        (None, Some(model)) => Some(model),
        (None, None) => None,
    }
}

fn description_word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn fallback_file_name_from_url(url: &str, model_version_id: u64) -> String {
    url.rsplit('/')
        .next()
        .and_then(|segment| segment.split('?').next())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| format!("model-{model_version_id}.safetensors"))
}

fn select_civitai_file<'a>(
    files: &'a [CivitaiFile],
    download_url: &str,
) -> Option<&'a CivitaiFile> {
    if let Ok(reference) = Url::parse(download_url) {
        if let Some(matched) = files.iter().find(|file| {
            file.download_url
                .as_deref()
                .and_then(|candidate| Url::parse(candidate).ok())
                .is_some_and(|candidate| urls_equivalent(&candidate, &reference))
        }) {
            return Some(matched);
        }
    }

    files
        .iter()
        .find(|file| file.r#type.as_deref() == Some("Model"))
        .or_else(|| files.first())
}

fn urls_equivalent(candidate: &Url, reference: &Url) -> bool {
    if candidate.path() != reference.path() {
        return false;
    }

    let mut left: Vec<(String, String)> = candidate
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut right: Vec<(String, String)> = reference
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    left.retain(|(key, _)| key != "token");
    right.retain(|(key, _)| key != "token");

    left.sort();
    right.sort();

    left == right
}

async fn resolve_preview(
    client: &Client,
    images: &[CivitaiImage],
    token: Option<&str>,
    model_version_id: u64,
) -> (Option<CivitaiPreview>, Option<String>) {
    let mut first_image: Option<&str> = None;
    let mut first_video: Option<&str> = None;

    for image in images {
        let Some(url) = image.url.as_deref() else {
            continue;
        };
        if url.is_empty() {
            continue;
        }

        if is_video_url(url) && first_video.is_none() {
            first_video = Some(url);
        } else if !is_video_url(url) && first_image.is_none() {
            first_image = Some(url);
        }
    }

    if let Some(image_url) = first_image {
        let preview_url = Some(image_url.to_string());
        let bytes = fetch_preview_image_bytes(client, image_url, token).await;
        let preview = bytes.map(CivitaiPreview::Image);
        if preview.is_none() {
            warn!("Failed to download image bytes for model version {model_version_id}");
        }
        return (preview, preview_url);
    }

    if let Some(video_url) = first_video {
        return (
            Some(CivitaiPreview::Video {
                url: video_url.to_string(),
            }),
            Some(video_url.to_string()),
        );
    }

    (None, None)
}

fn is_video_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".mov")
}

async fn fetch_preview_image_bytes(
    client: &Client,
    image_url: &str,
    token: Option<&str>,
) -> Option<Vec<u8>> {
    let mut image_request = client.get(image_url);
    if let Some(token) = token {
        image_request = image_request.header("Authorization", format!("Bearer {}", token));
    }

    match image_request.send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.bytes().await {
                    Ok(bytes) => Some(bytes.to_vec()),
                    Err(err) => {
                        warn!("Failed to download image bytes from {image_url}: {err}");
                        None
                    }
                }
            } else {
                warn!(
                    "Image request for {image_url} returned status {}",
                    response.status()
                );
                None
            }
        }
        Err(err) => {
            warn!("Failed to request image for {image_url}: {err}");
            None
        }
    }
}

fn extract_usage_strength(
    settings: Option<&CivitaiModelSettings>,
    meta: Option<&CivitaiVersionMeta>,
    images: &[CivitaiImage],
) -> Option<f64> {
    if let Some(strength) = settings.and_then(|s| normalized_strength(s.strength)) {
        return Some(strength);
    }

    if let Some(strength) = meta.and_then(|m| normalized_strength(m.strength)) {
        return Some(strength);
    }

    for image in images {
        let Some(meta) = image.meta.as_ref() else {
            continue;
        };

        for resource in &meta.resources {
            if let Some(weight) = normalized_strength(resource.weight) {
                let is_lora = resource
                    .r#type
                    .as_deref()
                    .map(|t| t.eq_ignore_ascii_case("lora"))
                    .unwrap_or(true);
                if is_lora {
                    return Some(weight);
                }
            }
            if let Some(weight) = normalized_strength(resource.strength) {
                let is_lora = resource
                    .r#type
                    .as_deref()
                    .map(|t| t.eq_ignore_ascii_case("lora"))
                    .unwrap_or(true);
                if is_lora {
                    return Some(weight);
                }
            }
        }
    }

    None
}

fn normalized_strength(value: Option<f64>) -> Option<f64> {
    match value {
        Some(v) if v.is_finite() && v > 0.0 => Some(v),
        _ => None,
    }
}

async fn fetch_strength_from_html(
    client: &Client,
    model_id: Option<u64>,
    model_version_id: u64,
) -> Option<f64> {
    let mut urls = Vec::new();
    if let Some(id) = model_id {
        urls.push(format!(
            "https://civitai.com/models/{id}?modelVersionId={model_version_id}"
        ));
    }
    urls.push(format!(
        "https://civitai.com/model-versions/{model_version_id}"
    ));

    for url in urls {
        let response = match client.get(&url).send().await {
            Ok(response) => response,
            Err(err) => {
                warn!("Failed to fetch model page {url}: {err}");
                continue;
            }
        };

        if !response.status().is_success() {
            warn!(
                "Model page request for version {model_version_id} returned status {}",
                response.status()
            );
            continue;
        }

        let html = match response.text().await {
            Ok(body) => body,
            Err(err) => {
                warn!("Failed to read model page body {url}: {err}");
                continue;
            }
        };

        if let Some(strength) = parse_strength_from_html(&html, model_version_id) {
            return Some(strength);
        }
    }

    None
}

fn parse_strength_from_html(html: &str, model_version_id: u64) -> Option<f64> {
    let marker = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = html.find(marker)?;
    let after = &html[start + marker.len()..];
    let end = after.find("</script>")?;
    let json_str = &after[..end];

    let value: Value = serde_json::from_str(json_str).ok()?;
    find_strength_in_value(&value, model_version_id)
}

fn find_strength_in_value(value: &Value, model_version_id: u64) -> Option<f64> {
    let mut queue: VecDeque<&Value> = VecDeque::new();
    queue.push_back(value);
    let mut visited = 0usize;
    // Current Civitai __NEXT_DATA__ payloads routinely contain thousands of
    // JSON values. This traversal is iterative, so a realistic work budget is
    // sufficient without risking recursive stack growth.
    const MAX_VISITED: usize = 100_000;

    while let Some(current) = queue.pop_front() {
        visited += 1;
        if visited > MAX_VISITED {
            log::debug!("Stopping Civitai strength metadata scan after {MAX_VISITED} JSON values.");
            break;
        }

        if let Some(obj) = current.as_object() {
            if let Some(id) = obj.get("id").and_then(|v| v.as_u64()) {
                if id == model_version_id {
                    if let Some(s) = normalized_strength(
                        obj.get("settings")
                            .and_then(|s| s.get("strength"))
                            .and_then(|v| v.as_f64()),
                    ) {
                        return Some(s);
                    }
                    if let Some(s) = normalized_strength(
                        obj.get("meta")
                            .and_then(|m| m.get("strength"))
                            .and_then(|v| v.as_f64()),
                    ) {
                        return Some(s);
                    }
                }
            }

            for val in obj.values() {
                queue.push_back(val);
            }
        } else if let Some(array) = current.as_array() {
            for item in array {
                queue.push_back(item);
            }
        }
    }

    None
}

#[cfg(test)]
mod strength_scan_tests {
    use super::find_strength_in_value;
    use serde_json::{json, Value};

    #[test]
    fn finds_strength_beyond_the_old_500_value_limit() {
        let model_version_id = 987_654;
        let mut values: Vec<Value> = (0..750)
            .map(|index| json!({ "unrelated": index }))
            .collect();
        values.push(json!({
            "id": model_version_id,
            "settings": { "strength": 0.85 }
        }));

        assert_eq!(
            find_strength_in_value(&Value::Array(values), model_version_id),
            Some(0.85)
        );
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiModelVersion {
    #[serde(default)]
    trained_words: Vec<String>,
    #[serde(default)]
    images: Vec<CivitaiImage>,
    #[serde(default)]
    files: Vec<CivitaiFile>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    model: Option<CivitaiModel>,
    #[serde(default)]
    model_id: Option<u64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    meta: Option<CivitaiVersionMeta>,
    #[serde(default)]
    settings: Option<CivitaiModelSettings>,
}

#[derive(Debug, Deserialize)]
struct CivitaiImage {
    url: Option<String>,
    #[serde(default)]
    meta: Option<CivitaiImageMeta>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiFile {
    name: Option<String>,
    download_url: Option<String>,
    #[serde(rename = "type")]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiModel {
    #[serde(default)]
    creator: Option<CivitaiCreator>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiCreator {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiModelResponse {
    #[serde(default)]
    creator: Option<CivitaiCreator>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model_versions: Vec<CivitaiModelVersionSummary>,
}

#[derive(Debug)]
struct CivitaiModelDetails {
    creator: Option<CivitaiCreator>,
    description: Option<String>,
    version_strength: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiImageMeta {
    #[serde(default)]
    resources: Vec<CivitaiResource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiResource {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    weight: Option<f64>,
    #[serde(default)]
    strength: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiVersionMeta {
    #[serde(default)]
    strength: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiModelSettings {
    #[serde(default)]
    strength: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CivitaiModelVersionSummary {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    meta: Option<CivitaiVersionMeta>,
    #[serde(default)]
    settings: Option<CivitaiModelSettings>,
}

async fn fetch_civitai_model_details(
    client: &Client,
    model_id: u64,
    model_version_id: u64,
    token: Option<&str>,
) -> Result<CivitaiModelDetails> {
    let api_url = format!("https://civitai.com/api/v1/models/{model_id}");
    let mut request = client.get(&api_url);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {api_url}"))?;

    if response.status().as_u16() == 401 {
        return Err(DownloadError::Unauthorized.into());
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("unexpected status downloading metadata from {api_url}"))?;

    let payload: CivitaiModelResponse = response
        .json()
        .await
        .with_context(|| format!("failed to parse metadata payload for {api_url}"))?;

    let mut version_strength = payload
        .model_versions
        .iter()
        .find(|version| version.id == Some(model_version_id))
        .and_then(|version| {
            normalized_strength(version.settings.as_ref().and_then(|s| s.strength))
                .or_else(|| normalized_strength(version.meta.as_ref().and_then(|m| m.strength)))
        });

    if version_strength.is_none() {
        version_strength = payload.model_versions.iter().find_map(|version| {
            normalized_strength(version.settings.as_ref().and_then(|s| s.strength))
                .or_else(|| normalized_strength(version.meta.as_ref().and_then(|m| m.strength)))
        });
    }

    Ok(CivitaiModelDetails {
        creator: payload.creator,
        description: payload.description,
        version_strength,
    })
}
