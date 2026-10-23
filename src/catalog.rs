use crate::{
    config::ConfigStore,
    model::{LoraDefinition, ModelCatalog, ModelVariant, ResolvedModel, WorkflowDefinition},
    vram::VramTier,
};
use anyhow::{anyhow, Context, Result};
use log::{info, warn};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::watch;

const CACHED_CATALOG_FILE: &str = "catalog.json";
const SUPABASE_CATALOG_TABLE: &str = "catalog_documents";
const SUPABASE_CATALOG_KEY: &str = "main";

/// Outcome of the latest catalog refresh, so the UI can say why the catalog
/// is missing instead of a generic "check your connection".
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogRefreshStatus {
    /// The startup refresh hasn't finished yet.
    Pending,
    Updated,
    /// This build has no catalog URL or read key.
    NotConfigured,
    /// The catalog server answered with an error status.
    Rejected {
        status: u16,
    },
    /// No answer from the catalog server (offline, DNS, proxy, timeout).
    Unreachable {
        detail: String,
    },
    /// The server answered, but not with a catalog this version can read.
    Invalid {
        detail: String,
    },
}

impl CatalogRefreshStatus {
    /// What to tell the user; empty when nothing went wrong.
    pub fn message(&self) -> String {
        match self {
            Self::Pending => "Loading the model catalog...".to_string(),
            Self::Updated => String::new(),
            Self::NotConfigured => {
                "This build has no catalog address configured. Please reinstall the app from the official release."
                    .to_string()
            }
            Self::Rejected { status: 401 | 403 } => {
                "The catalog server rejected this app's access key. Update the app; if you set ARCTIC_SUPABASE_* environment variables, remove them."
                    .to_string()
            }
            Self::Rejected { status } => format!(
                "The catalog server is having problems (HTTP {status}). Try Refresh again in a few minutes."
            ),
            Self::Unreachable { .. } => {
                "Can't reach the catalog server. Check your internet connection, VPN, proxy or firewall, then press Refresh."
                    .to_string()
            }
            Self::Invalid { .. } => {
                "The catalog server sent data this app version can't read. Please update the app."
                    .to_string()
            }
        }
    }
}

#[derive(Debug)]
pub struct CatalogService {
    catalog: RwLock<ModelCatalog>,
    config: Arc<ConfigStore>,
    refresh_status: watch::Sender<CatalogRefreshStatus>,
}

impl CatalogService {
    pub fn new(config: Arc<ConfigStore>) -> Result<Self> {
        let catalog = load_cached_catalog(&config).unwrap_or_default();
        info!(
            "Catalog initialised with {} models ({} LoRAs, {} workflows).",
            catalog.models.len(),
            catalog.loras.len(),
            catalog.workflows.len()
        );
        Ok(Self {
            catalog: RwLock::new(catalog),
            config,
            refresh_status: watch::Sender::new(CatalogRefreshStatus::Pending),
        })
    }

    pub fn refresh_status(&self) -> CatalogRefreshStatus {
        self.refresh_status.borrow().clone()
    }

    /// Waits until the first refresh has finished (or `timeout` passes) and
    /// returns its status. Lets a fresh install show the downloaded catalog
    /// instead of an empty one while the startup refresh is still running.
    pub async fn wait_for_refresh(&self, timeout: Duration) -> CatalogRefreshStatus {
        let mut receiver = self.refresh_status.subscribe();
        let _ = tokio::time::timeout(
            timeout,
            receiver.wait_for(|status| *status != CatalogRefreshStatus::Pending),
        )
        .await;
        self.refresh_status()
    }

    pub fn catalog_snapshot(&self) -> ModelCatalog {
        // Recover from poisoning rather than panicking -- see
        // ConfigStore::settings()'s comment for why that's safe here too.
        self.catalog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn variants_for_tier(&self, model_id: &str, tier: VramTier) -> Vec<ModelVariant> {
        let catalog = self.catalog_snapshot();
        catalog
            .models
            .into_iter()
            .find(|m| m.id == model_id)
            .map(|master| master.variants_for_tier(tier))
            .unwrap_or_default()
    }

    pub fn resolve_variant(&self, model_id: &str, variant_id: &str) -> Option<ResolvedModel> {
        let catalog = self.catalog_snapshot();
        let master = catalog.models.into_iter().find(|m| m.id == model_id)?;
        let variant = master
            .variants
            .iter()
            .find(|variant| variant.id == variant_id)?
            .clone();
        Some(ResolvedModel { master, variant })
    }

    pub fn resolve_download_selection(
        &self,
        model_id: &str,
        variant_id: &str,
        fallback_tier: Option<VramTier>,
    ) -> Option<ResolvedModel> {
        let catalog = self.catalog_snapshot();
        let master = catalog.models.into_iter().find(|m| m.id == model_id)?;

        if let Some(variant) = master.find_variant(variant_id).cloned() {
            return Some(ResolvedModel { master, variant });
        }

        let trimmed_variant_id = variant_id.trim();
        if trimmed_variant_id.is_empty() || trimmed_variant_id == "__always_only__" {
            let tier = fallback_tier.unwrap_or(VramTier::TierC);
            if let Some(variant) = master.synthetic_always_only_variant(tier) {
                return Some(ResolvedModel { master, variant });
            }
        }

        None
    }

    pub fn loras(&self) -> Vec<LoraDefinition> {
        self.catalog_snapshot().loras
    }

    pub fn lora_families(&self) -> Vec<String> {
        self.catalog_snapshot().lora_families()
    }

    pub fn find_lora(&self, id: &str) -> Option<LoraDefinition> {
        self.catalog_snapshot().find_lora(id)
    }

    pub fn workflows(&self) -> Vec<WorkflowDefinition> {
        self.catalog_snapshot().workflows
    }

    pub fn workflow_families(&self) -> Vec<String> {
        self.catalog_snapshot().workflow_families()
    }

    pub fn find_workflow(&self, id: &str) -> Option<WorkflowDefinition> {
        self.catalog_snapshot().find_workflow(id)
    }

    /// Refreshes from Supabase. `Ok(true)` when the catalog was updated,
    /// `Ok(false)` when the server or configuration refused, `Err` when the
    /// server couldn't be reached or sent something unreadable. The outcome is
    /// also kept for [`Self::refresh_status`].
    pub async fn refresh_from_remote(&self) -> Result<bool> {
        let status = self.fetch_and_apply().await;
        self.refresh_status.send_replace(status.clone());
        match status {
            CatalogRefreshStatus::Updated => Ok(true),
            CatalogRefreshStatus::Pending
            | CatalogRefreshStatus::NotConfigured
            | CatalogRefreshStatus::Rejected { .. } => Ok(false),
            CatalogRefreshStatus::Unreachable { detail }
            | CatalogRefreshStatus::Invalid { detail } => Err(anyhow!(detail)),
        }
    }

    async fn fetch_and_apply(&self) -> CatalogRefreshStatus {
        let sources = SupabaseCatalogSource::candidates();
        if sources.is_empty() {
            warn!(
                "Supabase catalog is not configured; set ARCTIC_SUPABASE_URL and ARCTIC_SUPABASE_ANON_KEY or ARCTIC_SUPABASE_PUBLISHABLE_KEY."
            );
            return CatalogRefreshStatus::NotConfigured;
        }

        let client = match Client::builder()
            .user_agent(format!(
                "ComfyPilot/{} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_NAME")
            ))
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                return CatalogRefreshStatus::Unreachable {
                    detail: format!(
                        "failed to build HTTP client for Supabase catalog refresh: {err}"
                    ),
                }
            }
        };

        info!("Refreshing catalog from Supabase table {SUPABASE_CATALOG_TABLE}.{SUPABASE_CATALOG_KEY}");
        let mut bytes = None;
        for (index, source) in sources.iter().enumerate() {
            let has_fallback = index + 1 < sources.len();
            let url = match source.rest_url() {
                Ok(url) => url,
                Err(err) if has_fallback => {
                    warn!("{err:#}; retrying with the catalog settings embedded in this build.");
                    continue;
                }
                Err(_) => return CatalogRefreshStatus::NotConfigured,
            };
            // `apikey` alone selects the anon role for both legacy anon and
            // publishable keys; an extra `Authorization` header only adds another
            // way for the gateway to reject the request.
            let response = match client
                .get(url.clone())
                .header("apikey", &source.read_key)
                .send()
                .await
                .with_context(|| format!("failed to fetch Supabase catalog from {url}"))
            {
                Ok(response) => response,
                Err(err) if has_fallback => {
                    warn!(
                        "{err:#} using {}; retrying with the catalog settings embedded in this build.",
                        source.describe()
                    );
                    continue;
                }
                Err(err) => {
                    warn!("{err:#}");
                    return CatalogRefreshStatus::Unreachable {
                        detail: format!("{err:#}"),
                    };
                }
            };

            let status = response.status();
            if status == StatusCode::OK {
                match response.bytes().await {
                    Ok(body) => {
                        bytes = Some((url, body));
                        break;
                    }
                    Err(err) => {
                        return CatalogRefreshStatus::Unreachable {
                            detail: format!("failed to read Supabase catalog body: {err}"),
                        }
                    }
                }
            }

            let body = response.text().await.unwrap_or_default();
            warn!(
                "Supabase catalog refresh skipped: server returned {} ({:?}) using {}: {}",
                status.as_u16(),
                status,
                source.describe(),
                body.trim()
            );
            let auth_rejected = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN);
            if auth_rejected && has_fallback {
                warn!(
                    "Runtime ARCTIC_SUPABASE_* override was rejected; retrying with the catalog settings embedded in this build. Unset the override to silence this warning."
                );
                continue;
            }
            return CatalogRefreshStatus::Rejected {
                status: status.as_u16(),
            };
        }
        let Some((url, bytes)) = bytes else {
            return CatalogRefreshStatus::Unreachable {
                detail: "no catalog source answered".to_string(),
            };
        };

        let catalog = match serde_json::from_slice::<Vec<SupabaseCatalogDocument>>(&bytes)
            .with_context(|| format!("failed to parse Supabase catalog response from {url}"))
            .and_then(|mut rows| {
                rows.pop()
                    .map(|row| row.catalog)
                    .context("Supabase catalog_documents.main row was not found")
            }) {
            Ok(catalog) => catalog,
            Err(err) => {
                warn!("{err:#}");
                return CatalogRefreshStatus::Invalid {
                    detail: format!("{err:#}"),
                };
            }
        };

        // Keep going if the cache can't be written: the catalog still works
        // for this session.
        if let Err(err) = self.persist_catalog(&catalog) {
            warn!("{err:#}");
        }

        {
            let mut guard = self
                .catalog
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = catalog;
        }

        info!("Catalog updated from Supabase.");
        CatalogRefreshStatus::Updated
    }

    fn persist_catalog(&self, catalog: &ModelCatalog) -> Result<()> {
        let path = self.cached_catalog_path();
        let data = serde_json::to_vec_pretty(catalog)?;
        fs::write(&path, data)
            .with_context(|| format!("failed to write cached catalog to {path:?}"))?;
        Ok(())
    }

    fn cached_catalog_path(&self) -> PathBuf {
        self.config.cache_path().join(CACHED_CATALOG_FILE)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SupabaseCatalogSource {
    url: String,
    read_key: String,
    from_runtime_env: bool,
}

impl SupabaseCatalogSource {
    /// Effective settings first (runtime env overriding the build), then the
    /// build-embedded settings as a fallback when the overrides differ, so a
    /// stale exported key cannot permanently break catalog refresh.
    fn candidates() -> Vec<Self> {
        Self::candidates_from(|name| std::env::var(name).ok(), build_env)
    }

    fn candidates_from(
        runtime_env: impl Fn(&str) -> Option<String>,
        build_env: impl Fn(&str) -> Option<String>,
    ) -> Vec<Self> {
        let runtime = |name: &str| runtime_env(name).and_then(non_empty);
        let build = |name: &str| build_env(name).and_then(non_empty);
        let resolve = |name: &str| runtime(name).or_else(|| build(name));
        let read_key = |lookup: &dyn Fn(&str) -> Option<String>| {
            lookup("ARCTIC_SUPABASE_ANON_KEY").or_else(|| lookup("ARCTIC_SUPABASE_PUBLISHABLE_KEY"))
        };

        let mut sources = Vec::new();
        if let (Some(url), Some(key)) = (resolve("ARCTIC_SUPABASE_URL"), read_key(&resolve)) {
            let embedded = (build("ARCTIC_SUPABASE_URL"), read_key(&build));
            let from_runtime_env = embedded != (Some(url.clone()), Some(key.clone()));
            sources.push(Self {
                url,
                read_key: key,
                from_runtime_env,
            });
        }
        if sources
            .first()
            .is_some_and(|source| source.from_runtime_env)
        {
            if let (Some(url), Some(key)) = (build("ARCTIC_SUPABASE_URL"), read_key(&build)) {
                sources.push(Self {
                    url,
                    read_key: key,
                    from_runtime_env: false,
                });
            }
        }
        sources
    }

    fn describe(&self) -> String {
        let origin = if self.from_runtime_env {
            "runtime ARCTIC_SUPABASE_* environment override"
        } else {
            "settings embedded in this build"
        };
        let key_kind = if self.read_key.starts_with("sb_publishable_") {
            "publishable key"
        } else if self.read_key.starts_with("eyJ") {
            "legacy JWT anon key"
        } else {
            "unrecognised key format"
        };
        format!("{origin} ({}, {key_kind})", self.url)
    }

    fn rest_url(&self) -> Result<Url> {
        let mut url = Url::parse(self.url.trim())
            .with_context(|| format!("invalid ARCTIC_SUPABASE_URL: {}", self.url))?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("ARCTIC_SUPABASE_URL cannot be a base URL"))?
            .extend(["rest", "v1", SUPABASE_CATALOG_TABLE]);
        url.query_pairs_mut()
            .append_pair("select", "catalog")
            .append_pair("catalog_key", &format!("eq.{SUPABASE_CATALOG_KEY}"))
            .append_pair("limit", "1");
        Ok(url)
    }
}

#[derive(Debug, Deserialize)]
struct SupabaseCatalogDocument {
    catalog: ModelCatalog,
}

fn build_env(name: &str) -> Option<String> {
    match name {
        "ARCTIC_SUPABASE_URL" => option_env!("ARCTIC_SUPABASE_URL").map(str::to_string),
        "ARCTIC_SUPABASE_ANON_KEY" => option_env!("ARCTIC_SUPABASE_ANON_KEY").map(str::to_string),
        "ARCTIC_SUPABASE_PUBLISHABLE_KEY" => {
            option_env!("ARCTIC_SUPABASE_PUBLISHABLE_KEY").map(str::to_string)
        }
        _ => None,
    }
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn load_catalog_from_path(path: &PathBuf) -> Option<ModelCatalog> {
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<ModelCatalog>(&contents) {
            Ok(parsed) => {
                info!("Loaded cached catalog from {:?}", path);
                Some(parsed)
            }
            Err(err) => {
                warn!("Failed to parse cached catalog at {:?}: {err}", path);
                None
            }
        },
        Err(err) => {
            warn!("Failed to read cached catalog at {:?}: {err}", path);
            None
        }
    }
}

fn load_cached_catalog(config: &ConfigStore) -> Option<ModelCatalog> {
    let path = cached_catalog_path(config);
    if path.exists() {
        load_catalog_from_path(&path)
    } else {
        None
    }
}

fn cached_catalog_path(config: &ConfigStore) -> PathBuf {
    config.cache_path().join(CACHED_CATALOG_FILE)
}

#[cfg(test)]
mod tests {
    use super::SupabaseCatalogSource;
    use std::collections::HashMap;

    const URL: &str = "https://project.supabase.co";
    const KEY: &str = "sb_publishable_embedded";

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    fn embedded() -> impl Fn(&str) -> Option<String> {
        lookup(&[
            ("ARCTIC_SUPABASE_URL", URL),
            ("ARCTIC_SUPABASE_PUBLISHABLE_KEY", KEY),
        ])
    }

    #[test]
    fn embedded_settings_are_used_alone_without_overrides() {
        let sources = SupabaseCatalogSource::candidates_from(lookup(&[]), embedded());
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].read_key, KEY);
        assert!(!sources[0].from_runtime_env);
    }

    #[test]
    fn stale_runtime_key_falls_back_to_embedded_settings() {
        let sources = SupabaseCatalogSource::candidates_from(
            lookup(&[("ARCTIC_SUPABASE_ANON_KEY", "eyJstale")]),
            embedded(),
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].read_key, "eyJstale");
        assert!(sources[0].from_runtime_env);
        assert_eq!(sources[1].read_key, KEY);
        assert!(!sources[1].from_runtime_env);
    }

    #[test]
    fn empty_runtime_values_and_whitespace_do_not_mask_embedded_settings() {
        let sources = SupabaseCatalogSource::candidates_from(
            lookup(&[
                ("ARCTIC_SUPABASE_ANON_KEY", ""),
                ("ARCTIC_SUPABASE_URL", " https://project.supabase.co\r\n"),
            ]),
            embedded(),
        );
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, URL);
        assert_eq!(sources[0].read_key, KEY);
        assert!(!sources[0].from_runtime_env);
    }
}
