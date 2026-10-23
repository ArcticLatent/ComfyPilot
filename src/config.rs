use crate::app::{APP_ID, LEGACY_APP_ID};
use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

const SETTINGS_FILE: &str = "settings.json";
// Fixed "account" names under this app's keychain service entry -- there's
// only ever one token per service per install, no per-user accounts within it.
const CIVITAI_TOKEN_KEYCHAIN_ACCOUNT: &str = "civitai-token";
const HF_TOKEN_KEYCHAIN_ACCOUNT: &str = "huggingface-token";

/// Best-effort OS-keychain access for the API tokens. Every function here
/// returns `None`/swallows its error rather than propagating one: a missing
/// or unavailable keychain (no Secret Service daemon running, a
/// minimal/headless Linux setup, a locked keychain, etc.) is a completely
/// normal, expected condition for a chunk of this app's userbase, not a
/// fault -- callers fall back to the plaintext token settings fields when
/// these return nothing, so a keychain failure never blocks reading or
/// saving a token, only which storage it ends up in.
mod keychain {
    use crate::app::{APP_ID, LEGACY_APP_ID};

    fn entry(account: &str) -> Option<keyring::Entry> {
        entry_for(APP_ID, account)
    }

    fn entry_for(service: &str, account: &str) -> Option<keyring::Entry> {
        match keyring::Entry::new(service, account) {
            Ok(entry) => Some(entry),
            Err(err) => {
                log::debug!("Keychain entry {account} unavailable: {err}");
                None
            }
        }
    }

    pub fn load(account: &str) -> Option<String> {
        load_from(entry(account)?, account)
    }

    fn load_from(entry: keyring::Entry, account: &str) -> Option<String> {
        match entry.get_password() {
            Ok(token) => Some(token),
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                log::debug!("Failed to read {account} from OS keychain: {err}");
                None
            }
        }
    }

    /// Returns `true` if the token is now stored in the keychain (so the
    /// caller can omit it from the plaintext settings file).
    pub fn store(account: &str, token: &str) -> bool {
        let Some(entry) = entry(account) else {
            return false;
        };
        match entry.set_password(token) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("Failed to store {account} in OS keychain: {err}");
                false
            }
        }
    }

    /// Best-effort delete; a missing/unavailable keychain is not an error
    /// here either, there's simply nothing to clean up.
    pub fn clear(account: &str) {
        let Some(entry) = entry(account) else { return };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => {
                log::debug!("Failed to clear {account} from OS keychain: {err}");
            }
        }
    }

    /// Loads a token at startup: the keychain wins over the settings file
    /// (which may be stale/pre-migration); a token only in the file is
    /// migrated into the keychain so future saves stop touching the file.
    /// A token saved before the rename (under the old app ID) is copied to
    /// the new entry; the old one stays for an old version still installed.
    pub fn load_or_migrate(account: &str, field: &mut Option<String>) {
        let legacy = || {
            let token = load_from(entry_for(LEGACY_APP_ID, account)?, account)?;
            store(account, &token);
            log::info!("Copied the saved {account} from {LEGACY_APP_ID}.");
            Some(token)
        };
        match load(account).or_else(legacy) {
            Some(token) => *field = Some(token),
            None => {
                if let Some(token) = field.as_deref() {
                    store(account, token);
                }
            }
        }
    }

    /// Keeps a token out of the file being written whenever the keychain
    /// takes it, and clears the keychain entry when the token was removed.
    pub fn redact_for_disk(account: &str, token: Option<&str>, on_disk: &mut Option<String>) {
        match token {
            Some(token) => {
                if store(account, token) {
                    *on_disk = None;
                }
            }
            None => clear(account),
        }
    }
}

#[derive(Debug)]
pub struct ConfigStore {
    root_dir: PathBuf,
    config_dir: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
    settings: RwLock<AppSettings>,
    /// Set when the settings file couldn't be read at startup.
    startup_notice: Option<String>,
}

/// Copies the `config` and `state` folders of the data folder used before the
/// rename (`legacy_root`) into `root`, once: when `root` has no settings file
/// yet and `legacy_root` has one. The legacy folder is left as it is (its
/// cache may hold the Python runtime of installed ComfyUIs). Returns whether
/// anything was copied.
pub fn migrate_legacy_data(legacy_root: &Path, root: &Path) -> std::io::Result<bool> {
    if root.join("config").join(SETTINGS_FILE).exists()
        || !legacy_root.join("config").join(SETTINGS_FILE).is_file()
    {
        return Ok(false);
    }
    for sub in ["config", "state"] {
        copy_dir(&legacy_root.join(sub), &root.join(sub))?;
    }
    Ok(true)
}

/// Copies `from` into `to` recursively (files that are already there are
/// kept).
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() && !target.exists() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// The cache directory `ConfigStore::new` uses, for code without access to
/// the store.
pub fn default_cache_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|base| base.data_local_dir().join(APP_ID).join("cache"))
}

impl ConfigStore {
    pub fn new() -> Result<Self> {
        let base = BaseDirs::new()
            .ok_or_else(|| anyhow!("unable to resolve base directories for {APP_ID}"))?;
        let root_dir = base.data_local_dir().join(APP_ID);
        match migrate_legacy_data(&base.data_local_dir().join(LEGACY_APP_ID), &root_dir) {
            Ok(true) => log::info!("Copied settings and state from {LEGACY_APP_ID}."),
            Ok(false) => {}
            Err(err) => log::warn!("Couldn't copy settings from {LEGACY_APP_ID}: {err}"),
        }
        let config_dir = root_dir.join("config");
        let state_dir = root_dir.join("state");
        let cache_dir = root_dir.join("cache");

        fs::create_dir_all(&config_dir)
            .with_context(|| format!("failed to create config directory {config_dir:?}"))?;

        fs::create_dir_all(&state_dir)
            .with_context(|| format!("failed to create state directory {state_dir:?}"))?;

        fs::create_dir_all(&cache_dir)
            .with_context(|| format!("failed to create cache directory {cache_dir:?}"))?;

        let settings_path = config_dir.join(SETTINGS_FILE);
        let (mut settings, startup_notice) = load_settings_or_recover(&settings_path);
        // Mature content is off at every start, whatever was saved.
        settings.civitai_show_mature = false;

        // The OS keychain is the preferred store for the API tokens; if
        // it already has one, it wins over whatever's in the plaintext
        // file (which may just be stale/pre-migration). Otherwise, if the
        // file has a plaintext token from before this existed (or from a
        // session where the keychain was unavailable), try to migrate it
        // into the keychain now so future saves stop touching the file --
        // the token fields stay populated in memory either way,
        // this only changes where the *next* write persists it.
        keychain::load_or_migrate(CIVITAI_TOKEN_KEYCHAIN_ACCOUNT, &mut settings.civitai_token);
        keychain::load_or_migrate(HF_TOKEN_KEYCHAIN_ACCOUNT, &mut settings.hf_token);

        Ok(Self {
            root_dir,
            config_dir,
            state_dir,
            cache_dir,
            settings: RwLock::new(settings),
            startup_notice,
        })
    }

    /// A message for the user when the settings file couldn't be read at
    /// startup and defaults were used instead.
    pub fn startup_notice(&self) -> Option<String> {
        self.startup_notice.clone()
    }

    pub fn settings(&self) -> AppSettings {
        // Recover from poisoning rather than panicking: a panic elsewhere
        // while holding this lock doesn't leave `AppSettings` (simple owned
        // data) torn, so recovering keeps settings readable afterward
        // instead of every future read/write panicking again for the rest
        // of the process's life.
        self.settings
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn update_settings<F>(&self, mutate: F) -> Result<AppSettings>
    where
        F: FnOnce(&mut AppSettings),
    {
        let mut guard = self
            .settings
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mutate(&mut guard);
        let snapshot = guard.clone();
        self.persist_locked(&snapshot)?;
        Ok(snapshot)
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.clone()
    }

    pub fn state_path(&self) -> Option<PathBuf> {
        Some(self.state_dir.clone())
    }

    pub fn cache_path(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    pub fn root_path(&self) -> PathBuf {
        self.root_dir.clone()
    }

    fn persist_locked(&self, settings: &AppSettings) -> Result<()> {
        let path = self.config_path().join(SETTINGS_FILE);

        // Keep the API tokens out of the plaintext config file whenever
        // the OS keychain will actually take them; fall back to writing them
        // in the file (today's behavior, unchanged) if the keychain is
        // unavailable. `settings` itself (the in-memory copy other code
        // reads via `ConfigStore::settings()`) is never touched here --
        // only the on-disk serialization is redacted.
        let mut on_disk = settings.clone();
        keychain::redact_for_disk(
            CIVITAI_TOKEN_KEYCHAIN_ACCOUNT,
            settings.civitai_token.as_deref(),
            &mut on_disk.civitai_token,
        );
        keychain::redact_for_disk(
            HF_TOKEN_KEYCHAIN_ACCOUNT,
            settings.hf_token.as_deref(),
            &mut on_disk.hf_token,
        );

        let data = serde_json::to_vec_pretty(&on_disk)?;
        write_file_atomically(&path, &data)
            .with_context(|| format!("failed to write settings to {path:?}"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppSettings {
    pub comfyui_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_install_base: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_last_install_dir: Option<PathBuf>,
    pub prefer_quantized: bool,
    pub concurrent_downloads: usize,
    pub bandwidth_cap_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub civitai_token: Option<String>,
    /// Hugging Face access token, sent only to huggingface.co (gated and
    /// private model repos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_installed_version: Option<String>,
    #[serde(default = "default_true")]
    pub comfyui_pinned_memory_enabled: bool,
    #[serde(default)]
    pub comfyui_listen_enabled: bool,
    #[serde(default)]
    pub comfyui_lowvram_enabled: bool,
    #[serde(default)]
    pub comfyui_bf16_unet_enabled: bool,
    #[serde(default)]
    pub comfyui_async_offload_enabled: bool,
    #[serde(default)]
    pub comfyui_disable_smart_memory_enabled: bool,
    #[serde(default)]
    pub comfyui_disable_dynamic_vram_enabled: bool,
    #[serde(default)]
    pub comfyui_fast_disk_enabled: bool,
    /// Port ComfyUI listens on; a `--port` in the custom launch args wins.
    #[serde(default = "default_comfyui_port")]
    pub comfyui_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_attention_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_torch_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_gpu_selection: Option<String>,
    #[serde(default)]
    pub hf_xet_enabled: bool,
    /// Show LoRAs, workflows and previews Civitai rates above PG in the
    /// Civitai browsers. For this run only: every start begins with it off.
    #[serde(default)]
    pub civitai_show_mature: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_models_root: Option<PathBuf>,
    #[serde(default)]
    pub shared_models_use_default: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comfyui_custom_launch_args: String,
    #[serde(default = "default_true")]
    pub comfyui_show_runtime_logs: bool,
}

impl AppSettings {
    pub fn comfyui_root_valid(&self) -> Option<&Path> {
        self.comfyui_root
            .as_deref()
            .filter(|path| path.join("models").is_dir())
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            comfyui_root: None,
            comfyui_install_base: None,
            comfyui_last_install_dir: None,
            prefer_quantized: true,
            concurrent_downloads: 2,
            bandwidth_cap_mbps: None,
            civitai_token: None,
            hf_token: None,
            last_installed_version: None,
            comfyui_pinned_memory_enabled: true,
            comfyui_listen_enabled: false,
            comfyui_lowvram_enabled: false,
            comfyui_bf16_unet_enabled: false,
            comfyui_async_offload_enabled: false,
            comfyui_disable_smart_memory_enabled: false,
            comfyui_disable_dynamic_vram_enabled: false,
            comfyui_fast_disk_enabled: false,
            comfyui_port: DEFAULT_COMFYUI_PORT,
            comfyui_attention_backend: None,
            comfyui_torch_profile: None,
            comfyui_gpu_selection: None,
            hf_xet_enabled: false,
            civitai_show_mature: false,
            shared_models_root: None,
            shared_models_use_default: false,
            comfyui_custom_launch_args: String::new(),
            comfyui_show_runtime_logs: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Reads the settings file. An unreadable or damaged file (e.g. cut short
/// by a crash or a full disk) is moved aside and defaults are used, so the
/// app still starts; the returned notice tells the user.
fn load_settings_or_recover(path: &Path) -> (AppSettings, Option<String>) {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (AppSettings::default(), None)
        }
        Err(err) => {
            log::warn!("Failed to read settings file {path:?}: {err}");
            return (
                AppSettings::default(),
                Some(format!(
                    "The settings file {} could not be read ({err}), so the app started with default settings.",
                    path.display()
                )),
            );
        }
    };
    match serde_json::from_slice(&data) {
        Ok(settings) => (settings, None),
        Err(err) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = path.with_file_name(format!("{SETTINGS_FILE}.corrupt-{stamp}"));
            log::warn!("Settings file {path:?} is damaged ({err}); moving it to {backup:?}");
            let kept = match fs::rename(path, &backup) {
                Ok(()) => format!(" The damaged file was kept as {}.", backup.display()),
                Err(_) => String::new(),
            };
            (
                AppSettings::default(),
                Some(format!(
                    "Your settings file was damaged and could not be read ({err}), so the app started with default settings.{kept}"
                )),
            )
        }
    }
}

/// Writes `data` to a temporary file next to `path` and renames it over
/// `path`, so a crash or full disk can't leave a half-written file behind.
fn write_file_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

/// ComfyUI's own default port.
pub const DEFAULT_COMFYUI_PORT: u16 = 8188;

fn default_comfyui_port() -> u16 {
    DEFAULT_COMFYUI_PORT
}

#[cfg(test)]
mod settings_file_tests {
    use super::*;

    #[test]
    fn legacy_settings_and_state_are_copied_once() {
        let base = temp_dir("legacy");
        let (legacy, root) = (base.join("old"), base.join("new"));
        fs::create_dir_all(legacy.join("config")).unwrap();
        fs::create_dir_all(legacy.join("state/sub")).unwrap();
        fs::create_dir_all(legacy.join("cache")).unwrap();
        fs::write(
            legacy.join("config").join(SETTINGS_FILE),
            "{\"comfyui_root\":\"/x\"}",
        )
        .unwrap();
        fs::write(legacy.join("state/sub/a.json"), "1").unwrap();
        fs::write(legacy.join("cache/big.bin"), "x").unwrap();
        // The log folder already exists in the new root (logging starts first).
        fs::create_dir_all(root.join("logs")).unwrap();

        assert!(migrate_legacy_data(&legacy, &root).unwrap());
        assert!(root.join("config").join(SETTINGS_FILE).is_file());
        assert_eq!(
            fs::read_to_string(root.join("state/sub/a.json")).unwrap(),
            "1"
        );
        assert!(!root.join("cache").exists());
        assert!(legacy.join("config").join(SETTINGS_FILE).is_file());

        // Not again once the new settings exist, and not without old ones.
        fs::write(root.join("config").join(SETTINGS_FILE), "{}").unwrap();
        assert!(!migrate_legacy_data(&legacy, &root).unwrap());
        assert!(!migrate_legacy_data(&base.join("missing"), &base.join("other")).unwrap());
        let _ = fs::remove_dir_all(base);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("arctic-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn damaged_settings_are_moved_aside_and_defaults_used() {
        let dir = temp_dir("damaged");
        let path = dir.join(SETTINGS_FILE);
        fs::write(&path, b"{\"prefer_quantized\": true, \"concurrent_downl").unwrap();
        let (settings, notice) = load_settings_or_recover(&path);
        assert_eq!(
            settings.concurrent_downloads,
            AppSettings::default().concurrent_downloads
        );
        assert!(notice.unwrap().contains("damaged"));
        assert!(!path.exists());
        let backups: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0]
            .file_name()
            .to_string_lossy()
            .starts_with("settings.json.corrupt-"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_settings_use_defaults_quietly_and_saves_are_atomic() {
        let dir = temp_dir("atomic");
        let path = dir.join(SETTINGS_FILE);
        let (_, notice) = load_settings_or_recover(&path);
        assert!(notice.is_none());
        write_file_atomically(&path, b"{}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
