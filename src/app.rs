use crate::{
    catalog::CatalogService,
    config::ConfigStore,
    download::DownloadManager,
    ram::{RamProfile, RamTier},
    updater::Updater,
};
use anyhow::{anyhow, Result};
use log::{info, warn};
use std::{
    io::{self, BufRead, BufReader, Read},
    sync::Arc,
};
use tokio::runtime::{Builder, Runtime};

/// ComfyPilot's app ID: its data folder, keychain entries, desktop entry and
/// Flatpak ID.
pub const APP_ID: &str = "io.github.ArcticLatent.ComfyPilot";
/// The ID before the rename from Arctic ComfyUI Helper. Its settings, state
/// and saved tokens are copied over once (`config::migrate_legacy_data`); its
/// folder stays, since installed ComfyUIs may use the Python runtime in it.
pub const LEGACY_APP_ID: &str = "io.github.ArcticHelper";

#[derive(Clone)]
pub struct AppContext {
    pub runtime: Arc<Runtime>,
    pub config: Arc<ConfigStore>,
    pub catalog: Arc<CatalogService>,
    pub downloads: Arc<DownloadManager>,
    pub updater: Arc<Updater>,
    pub ram_profile: Option<RamProfile>,
    pub display_version: String,
}

impl AppContext {
    pub fn ram_tier(&self) -> Option<RamTier> {
        self.ram_profile.map(|profile| profile.tier)
    }

    pub fn total_ram_gb(&self) -> Option<f64> {
        self.ram_profile.map(|profile| profile.total_gb)
    }
}

pub fn build_context() -> Result<AppContext> {
    let runtime = Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow!("failed to create Tokio runtime: {err}"))?,
    );

    let config = Arc::new(ConfigStore::new()?);
    let catalog = Arc::new(CatalogService::new(config.clone())?);

    // Refresh the catalog from the remote source in the background instead
    // of blocking startup on it. `CatalogService::new()` above already
    // loaded the last-cached catalog synchronously (a fast local file
    // read), so the UI has something to show immediately; a slow or
    // unavailable network connection previously added up to several
    // seconds of dead time to every launch for no benefit over that cached
    // fallback. `refresh_from_remote` updates `CatalogService`'s shared
    // state in place once it completes, and the existing "Refresh Catalog"
    // button already gives the user a way to pull in a background-
    // completed refresh immediately if they want it mid-session.
    {
        let catalog = catalog.clone();
        runtime.spawn(async move {
            match catalog.refresh_from_remote().await {
                Ok(true) => info!("Catalog refreshed from remote in the background."),
                Ok(false) => {}
                Err(err) => warn!("Unable to refresh catalog from remote source: {err:#}"),
            }
        });
    }

    let display_version = resolve_display_version(&config);
    let downloads = Arc::new(DownloadManager::new(runtime.clone(), config.clone()));
    let updater = Arc::new(Updater::new(
        runtime.clone(),
        config.clone(),
        display_version.clone(),
    )?);
    Ok(AppContext {
        runtime,
        config,
        catalog,
        downloads,
        updater,
        ram_profile: None,
        display_version,
    })
}

/// Drains a process output stream without requiring the child application's
/// console encoding to be UTF-8. Keeping the reader alive is important: if a
/// decoding error closes a Windows pipe, Python flushes can fail with
/// `OSError: [Errno 22] Invalid argument`.
pub fn drain_lossy_lines<R, F>(reader: R, mut on_line: F) -> io::Result<()>
where
    R: Read,
    F: FnMut(String),
{
    let mut buffered = BufReader::new(reader);
    let mut bytes = Vec::new();

    loop {
        bytes.clear();
        match buffered.read_until(b'\n', &mut bytes) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                    bytes.pop();
                }
                if !bytes.is_empty() {
                    on_line(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

fn resolve_display_version(_config: &ConfigStore) -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::drain_lossy_lines;
    use std::io::Cursor;

    #[test]
    fn keeps_draining_after_non_utf8_console_bytes() {
        let input = Cursor::new(b"first\r\ninvalid: \xff\xfe\nlast\n".to_vec());
        let mut lines = Vec::new();

        drain_lossy_lines(input, |line| lines.push(line)).unwrap();

        assert_eq!(lines, ["first", "invalid: ��", "last"]);
    }
}
