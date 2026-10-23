use crate::config::ConfigStore;
#[cfg(not(target_os = "windows"))]
use crate::update_signing::linux_release_manifest_signing_payload;
use crate::update_signing::{update_manifest_signing_payload, verify_with_embedded_key};
use anyhow::{bail, Context, Result};
use log::info;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "windows"))]
use std::{ffi::OsStr, io::IsTerminal};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{fs, io::AsyncWriteExt, process::Command, runtime::Runtime};

const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://github.com/ArcticLatent/ComfyPilot/releases/latest/download/update.json";
const DEFAULT_LINUX_RELEASE_MANIFEST_URL: &str =
    "https://github.com/ArcticLatent/ComfyPilot/releases/latest/download/linux-release.json";
const UPDATE_CACHE_DIR: &str = "updates";
#[cfg(target_os = "windows")]
const FALLBACK_PACKAGE_NAME: &str = "ComfyPilot-Setup.exe";
#[cfg(not(target_os = "windows"))]
const FALLBACK_PACKAGE_NAME: &str = "comfypilot-update.bin";

#[derive(Clone, Debug)]
pub struct AvailableUpdate {
    pub version: Version,
    pub download_url: String,
    pub sha256: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UpdateApplied {
    pub version: Version,
    pub package_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    version: String,
    download_url: String,
    sha256: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Deserialize)]
struct LinuxReleaseManifest {
    version: String,
    tag: String,
    repository: String,
    assets: Vec<LinuxReleaseAsset>,
    #[serde(default)]
    signature: Option<String>,
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Deserialize)]
struct LinuxReleaseAsset {
    name: String,
    sha256: String,
    download_url: String,
}

#[derive(Clone)]
pub struct Updater {
    runtime: Arc<Runtime>,
    config: Arc<ConfigStore>,
    client: Client,
    manifest_url: String,
    cache_dir: PathBuf,
    current_version: Version,
}

impl Updater {
    pub fn new(
        runtime: Arc<Runtime>,
        config: Arc<ConfigStore>,
        current_version_str: String,
    ) -> Result<Self> {
        let manifest_url = resolve_manifest_url();
        let cache_dir = config.cache_path();
        let current_version = parse_version(&current_version_str)
            .unwrap_or_else(|| Version::parse(env!("CARGO_PKG_VERSION")).expect("valid semver"));
        let client = Client::builder()
            .user_agent(format!(
                "ComfyPilot/{} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_NAME")
            ))
            .build()
            .context("failed to construct HTTP client for updater")?;

        Ok(Self {
            runtime,
            config,
            client,
            manifest_url,
            cache_dir,
            current_version,
        })
    }

    pub fn check_for_update(&self) -> tokio::task::JoinHandle<Result<Option<AvailableUpdate>>> {
        let client = self.client.clone();
        let manifest_url = self.manifest_url.clone();
        let current_version = self.current_version.clone();

        self.runtime.spawn(async move {
            let manifest = fetch_manifest(&client, &manifest_url).await?;
            let target_version = Version::parse(manifest.version.trim())
                .context("update manifest contained invalid semver version")?;

            if target_version <= current_version {
                info!(
                    "No update available (current {}, manifest {}).",
                    current_version, target_version
                );
                return Ok(None);
            }

            let download_url = manifest.download_url.trim();
            if download_url.is_empty() {
                bail!("update manifest is missing download_url");
            }

            let sha256 = manifest.sha256.trim();
            if sha256.is_empty() {
                bail!("update manifest is missing sha256");
            }

            Ok(Some(AvailableUpdate {
                version: target_version,
                download_url: download_url.to_string(),
                sha256: sha256.to_ascii_lowercase(),
                notes: manifest.notes,
            }))
        })
    }

    pub fn download_and_install(
        &self,
        update: AvailableUpdate,
    ) -> tokio::task::JoinHandle<Result<UpdateApplied>> {
        let client = self.client.clone();
        let cache_dir = self.cache_dir.clone();
        let config = self.config.clone();

        self.runtime.spawn(async move {
            let updates_dir = cache_dir.join(UPDATE_CACHE_DIR);
            fs::create_dir_all(&updates_dir)
                .await
                .context("failed to prepare update cache directory")?;

            let file_name = installer_file_name(&update.download_url)
                .unwrap_or_else(|| FALLBACK_PACKAGE_NAME.to_string());
            let package_path = updates_dir.join(file_name);
            if fs::try_exists(&package_path).await.unwrap_or(false) {
                let _ = fs::remove_file(&package_path).await;
            }

            info!(
                "Downloading update {} from {}",
                update.version, update.download_url
            );
            let mut response = client
                .get(&update.download_url)
                .send()
                .await
                .context("failed to request update bundle")?
                .error_for_status()
                .context("failed to download update bundle")?;

            let mut file = fs::File::create(&package_path)
                .await
                .context("failed to create update package file")?;
            let mut hasher = Sha256::new();

            while let Some(chunk) = response
                .chunk()
                .await
                .context("failed to read update bundle chunk")?
            {
                hasher.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .context("failed to write update package to disk")?;
            }
            file.flush()
                .await
                .context("failed to flush update package to disk")?;

            let digest = format!("{:x}", hasher.finalize());
            if digest != update.sha256 {
                let _ = fs::remove_file(&package_path).await;
                bail!(
                    "downloaded update checksum mismatch (expected {}, got {})",
                    update.sha256,
                    digest
                );
            }

            info!(
                "Applying standalone update {} from {:?}",
                update.version, package_path
            );
            run_install_command(&package_path).await?;
            if let Err(err) = store_installed_version(update.version.clone(), &config) {
                log::warn!(
                    "Update was applied but its installed version could not be saved: {err:#}"
                );
            }

            Ok(UpdateApplied {
                version: update.version,
                package_path,
            })
        })
    }
}

fn resolve_manifest_url() -> String {
    if let Ok(url) = std::env::var("ARCTIC_UPDATE_MANIFEST_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Some(url) = option_env!("ARCTIC_UPDATE_MANIFEST_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if cfg!(target_os = "linux") {
        DEFAULT_LINUX_RELEASE_MANIFEST_URL.to_string()
    } else {
        DEFAULT_UPDATE_MANIFEST_URL.to_string()
    }
}

fn parse_version(raw: &str) -> Option<Version> {
    let trimmed = raw.trim();
    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(normalized).ok()
}

fn store_installed_version(version: Version, config: &ConfigStore) -> Result<()> {
    config
        .update_settings(|settings| settings.last_installed_version = Some(version.to_string()))
        .context("failed to persist the installed application version")?;
    Ok(())
}

async fn fetch_manifest(client: &Client, url: &str) -> Result<UpdateManifest> {
    let response = client
        .get(url)
        .send()
        .await
        .context("failed to fetch update manifest")?
        .error_for_status()
        .context("update manifest request returned error status")?;
    #[cfg(target_os = "windows")]
    {
        let manifest = response
            .json::<UpdateManifest>()
            .await
            .context("failed to parse update manifest JSON")?;
        verify_update_manifest(&manifest)?;
        Ok(manifest)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let bytes = response
            .bytes()
            .await
            .context("failed to read update manifest bytes")?;

        if let Ok(legacy) = serde_json::from_slice::<UpdateManifest>(&bytes) {
            verify_update_manifest(&legacy)?;
            return Ok(legacy);
        }

        let linux = serde_json::from_slice::<LinuxReleaseManifest>(&bytes)
            .context("failed to parse update manifest JSON (legacy and linux-release formats)")?;
        verify_linux_release_manifest(&linux)?;
        let asset = select_linux_release_asset(&linux)
            .context("no compatible Linux package artifact found in linux-release manifest")?;
        let download_url = asset.download_url.clone();
        let sha256 = asset.sha256.to_ascii_lowercase();
        let asset_name = asset.name.clone();
        Ok(UpdateManifest {
            version: linux.version,
            download_url,
            sha256,
            notes: Some(format!("Selected Linux package asset: {asset_name}")),
            // The per-asset manifest synthesized here isn't independently
            // signed -- its data was already authenticated as part of the
            // whole `linux-release.json` manifest in
            // verify_linux_release_manifest above, and this value never
            // gets re-serialized/re-checked.
            signature: None,
        })
    }
}

/// Refuses to trust `manifest` unless it carries a signature that verifies
/// against the embedded (or `ARCTIC_UPDATE_PUBLIC_KEY`-overridden) release
/// public key. Without this, `sha256` in the manifest only proves the
/// downloaded bytes match *this manifest* -- not that this manifest itself
/// came from a real release rather than a compromised token/CI run/account.
fn verify_update_manifest(manifest: &UpdateManifest) -> Result<()> {
    let signature = manifest
        .signature
        .as_deref()
        .context("update manifest has no signature -- refusing to trust an unsigned update")?;
    let payload = update_manifest_signing_payload(
        &manifest.version,
        &manifest.download_url,
        &manifest.sha256,
    );
    verify_with_embedded_key(&payload, signature)
        .context("update manifest signature verification failed -- refusing to trust it")
}

#[cfg(not(target_os = "windows"))]
fn verify_linux_release_manifest(manifest: &LinuxReleaseManifest) -> Result<()> {
    let signature = manifest.signature.as_deref().context(
        "linux-release manifest has no signature -- refusing to trust an unsigned update",
    )?;
    let asset_tuples: Vec<(String, String, String)> = manifest
        .assets
        .iter()
        .map(|asset| {
            (
                asset.name.clone(),
                asset.sha256.clone(),
                asset.download_url.clone(),
            )
        })
        .collect();
    let payload = linux_release_manifest_signing_payload(
        &manifest.version,
        &manifest.tag,
        &manifest.repository,
        &asset_tuples,
    );
    verify_with_embedded_key(&payload, signature)
        .context("linux-release manifest signature verification failed -- refusing to trust it")
}

fn installer_file_name(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.path_segments()?.next_back().map(str::to_string))
        .filter(|name| !name.trim().is_empty())
}

/// Windows: runs the downloaded ComfyPilot installer (Inno Setup, per-user)
/// silently and returns; the app then exits (`app-exit`). The installer
/// closes what's left of the app, installs over it, and starts it again (its
/// `[Run]` entry also runs in silent mode).
#[cfg(target_os = "windows")]
async fn run_install_command(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if ext.as_deref() != Some("exe") {
        bail!(
            "unsupported update package type for Windows updater: {:?} (expected the .exe installer)",
            path
        );
    }
    let log_path = path.with_extension("log");
    Command::new(path)
        .arg("/VERYSILENT")
        .arg("/SUPPRESSMSGBOXES")
        .arg("/NORESTART")
        .arg("/CLOSEAPPLICATIONS")
        .arg(format!("/LOG={}", log_path.display()))
        .spawn()
        .with_context(|| format!("failed to start the installer {:?}", path))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
async fn run_install_command(path: &Path) -> Result<()> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let file_arg = path.to_string_lossy().to_string();

    if file_name.ends_with(".appimage") {
        let Some(target) = running_appimage() else {
            bail!(
                "Downloaded an AppImage update, but this copy isn't running as an AppImage: {}",
                path.display()
            );
        };
        return replace_appimage(&path, &target)
            .await
            .with_context(|| format!("failed to replace {}", target.display()));
    }
    if file_name.ends_with(".deb") {
        if let Err(err) = run_privileged_install("apt", &["install", "-y", &file_arg]).await {
            bail!(
                "Update downloaded, but automatic install failed.\n\
                 Run manually:\n\
                 sudo apt install -y \"{}\"\n\
                 Details: {err}",
                file_arg
            );
        }
        return Ok(());
    }
    if file_name.ends_with(".src.rpm") {
        bail!(
            "Refusing to auto-install source RPM update package: {}",
            path.display()
        );
    }
    if file_name.ends_with(".rpm") {
        if let Err(err) = run_privileged_install("dnf", &["install", "-y", &file_arg]).await {
            bail!(
                "Update downloaded, but automatic install failed.\n\
                 Run manually:\n\
                 sudo dnf install -y \"{}\"\n\
                 Details: {err}",
                file_arg
            );
        }
        return Ok(());
    }
    if file_name.contains(".pkg.tar") {
        if let Err(err) = run_privileged_install("pacman", &["-U", "--noconfirm", &file_arg]).await
        {
            bail!(
                "Update downloaded, but automatic install failed.\n\
                 Run manually:\n\
                 sudo pacman -U --noconfirm \"{}\"\n\
                 Details: {err}",
                file_arg
            );
        }
        return Ok(());
    }

    bail!(
        "Unsupported Linux update package format: {}",
        path.display()
    )
}

/// The AppImage file this process runs from (`$APPIMAGE`, set by the
/// AppImage runtime), if it is one.
#[cfg(not(target_os = "windows"))]
pub fn running_appimage() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
    path.is_file().then_some(path)
}

/// Puts the downloaded AppImage in place of the running one: a copy next to
/// it, made executable, then renamed over it (the running copy keeps its
/// open file, so this is safe while it runs).
#[cfg(not(target_os = "windows"))]
async fn replace_appimage(downloaded: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let file_name = target
        .file_name()
        .and_then(OsStr::to_str)
        .context("the AppImage has no file name")?;
    let staged = target.with_file_name(format!(".{file_name}.update"));
    fs::copy(downloaded, &staged).await.with_context(|| {
        format!(
            "can't write next to {} (is its folder writable?)",
            target.display()
        )
    })?;
    fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).await?;
    if let Err(err) = fs::rename(&staged, target).await {
        let _ = fs::remove_file(&staged).await;
        return Err(err.into());
    }
    let _ = fs::remove_file(downloaded).await;
    info!("Replaced {} with the update", target.display());
    Ok(())
}

/// Starts the app again once this process has exited (after an update
/// installed over it): a detached shell waits for this PID, then runs the
/// AppImage or the installed binary. Waiting matters: a second instance
/// started while this one runs would hand over to it and exit.
#[cfg(not(target_os = "windows"))]
pub fn relaunch_after_exit() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let program = match running_appimage() {
        Some(appimage) => appimage,
        None => {
            // A package upgrade replaced the binary: /proc/self/exe then
            // reads "<path> (deleted)"; the path itself has the new one.
            let exe = std::env::current_exe().context("can't tell which program this is")?;
            let exe = exe.to_string_lossy();
            PathBuf::from(exe.strip_suffix(" (deleted)").unwrap_or(&exe).to_string())
        }
    };
    std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.3; done; exec "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(&program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to schedule a restart of {}", program.display()))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
async fn run_privileged_install(program: &str, args: &[&str]) -> Result<()> {
    let mut attempts: Vec<String> = Vec::new();

    match run_install_command_direct(program, args).await {
        Ok(()) => {
            return Ok(());
        }
        Err(err) => attempts.push(format!("{program} {} => {err}", args.join(" "))),
    }

    let mut sudo_non_interactive = vec!["-n", program];
    sudo_non_interactive.extend_from_slice(args);
    match run_install_command_direct("sudo", &sudo_non_interactive).await {
        Ok(()) => {
            return Ok(());
        }
        Err(err) => attempts.push(format!("sudo {} => {err}", sudo_non_interactive.join(" "))),
    }

    let mut pkexec_args = vec![program];
    pkexec_args.extend_from_slice(args);
    match run_install_command_direct("pkexec", &pkexec_args).await {
        Ok(()) => {
            return Ok(());
        }
        Err(err) => attempts.push(format!("pkexec {} => {err}", pkexec_args.join(" "))),
    }

    if can_use_interactive_sudo() {
        let mut sudo_interactive = vec![program];
        sudo_interactive.extend_from_slice(args);
        match run_install_command_direct("sudo", &sudo_interactive).await {
            Ok(()) => {
                return Ok(());
            }
            Err(err) => attempts.push(format!("sudo {} => {err}", sudo_interactive.join(" "))),
        }
    } else {
        attempts.push("sudo interactive skipped (no terminal attached)".to_string());
    }

    bail!(
        "could not run installer with required privileges. \
         If running from desktop GUI, ensure a PolicyKit agent is active; otherwise run from a terminal with --nerdstats so sudo can prompt. \
         attempts: {}",
        attempts.join(" | ")
    );
}

#[cfg(not(target_os = "windows"))]
fn can_use_interactive_sudo() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[cfg(not(target_os = "windows"))]
async fn run_install_command_direct(program: &str, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let output = cmd.output().await.with_context(|| {
        format!(
            "failed to run install command: {program} {}",
            args.join(" ")
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("exit status {}", output.status)
        };
        bail!(
            "install command failed: {} {} :: {}",
            program,
            args.join(" "),
            detail
        );
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn detect_linux_distro_family() -> String {
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut id = String::new();
    let mut id_like = String::new();
    for line in os_release.lines() {
        if let Some(value) = line.strip_prefix("ID=") {
            id = value.trim_matches('"').to_ascii_lowercase();
        } else if let Some(value) = line.strip_prefix("ID_LIKE=") {
            id_like = value.trim_matches('"').to_ascii_lowercase();
        }
    }
    let haystack = format!("{id} {id_like}");
    if haystack.contains("nixos") {
        "nixos".to_string()
    } else if haystack.contains("arch") {
        "arch".to_string()
    } else if haystack.contains("debian") || haystack.contains("ubuntu") {
        "debian".to_string()
    } else if haystack.contains("fedora")
        || haystack.contains("rhel")
        || haystack.contains("centos")
    {
        "fedora".to_string()
    } else {
        "unknown".to_string()
    }
}

#[cfg(not(target_os = "windows"))]
fn select_linux_release_asset(manifest: &LinuxReleaseManifest) -> Option<&LinuxReleaseAsset> {
    let distro = detect_linux_distro_family();
    let arch = std::env::consts::ARCH.to_ascii_lowercase();
    let package_kind = if running_appimage().is_some() {
        "appimage"
    } else if crate::env_flags::external_package_manager().as_deref() == Some("nix") {
        "nix"
    } else {
        distro.as_str()
    };

    select_linux_release_asset_for(manifest, package_kind, &arch)
}

#[cfg(not(target_os = "windows"))]
fn select_linux_release_asset_for<'a>(
    manifest: &'a LinuxReleaseManifest,
    distro: &str,
    arch: &str,
) -> Option<&'a LinuxReleaseAsset> {
    // The current Linux release pipeline only produces x86-64 packages.
    // Refusing other architectures is safer than offering an incompatible
    // artifact and failing later in the system package manager.
    if arch != "x86_64" {
        return None;
    }

    let candidates: Vec<&LinuxReleaseAsset> = manifest
        .assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_ascii_lowercase();
            !name.ends_with(".src.rpm") && (name.contains("x86_64") || name.contains("amd64"))
        })
        .collect();

    match distro {
        "appimage" => candidates
            .iter()
            .find(|asset| asset.name.to_ascii_lowercase().ends_with(".appimage"))
            .copied(),
        "arch" => candidates
            .iter()
            .find(|asset| asset.name.to_ascii_lowercase().contains(".pkg.tar"))
            .copied(),
        "debian" => candidates
            .iter()
            .find(|asset| asset.name.to_ascii_lowercase().ends_with(".deb"))
            .copied(),
        "fedora" => candidates
            .iter()
            .find(|asset| asset.name.to_ascii_lowercase().ends_with(".rpm"))
            .copied(),
        // Nix packages use this artifact only to discover and authenticate
        // newer versions. Installation remains delegated to the profile or
        // declarative configuration that owns the immutable store path.
        "nix" => candidates
            .iter()
            .find(|asset| {
                let name = asset.name.to_ascii_lowercase();
                name.starts_with("arctic-comfyui-helper-nix-") && name.ends_with(".tar.gz")
            })
            .copied(),
        "nixos" | "unknown" => None,
        _ => None,
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    fn linux_manifest() -> LinuxReleaseManifest {
        LinuxReleaseManifest {
            version: "1.2.3".to_string(),
            tag: "v1.2.3".to_string(),
            repository: "ArcticLatent/Arctic-Helper".to_string(),
            assets: [
                "arctic-helper-1.2.3-amd64.deb",
                "arctic-helper-1.2.3-x86_64.rpm",
                "arctic-helper-1.2.3-x86_64.src.rpm",
                "arctic-helper-1.2.3-x86_64.pkg.tar.zst",
                "ComfyPilot-1.2.3-x86_64.AppImage",
                "arctic-comfyui-helper-nix-x86_64.tar.gz",
            ]
            .into_iter()
            .map(|name| LinuxReleaseAsset {
                name: name.to_string(),
                sha256: "abc".to_string(),
                download_url: format!("https://example.invalid/{name}"),
            })
            .collect(),
            signature: None,
        }
    }

    #[test]
    fn selects_only_the_native_package_for_supported_distros() {
        let manifest = linux_manifest();
        let cases = [
            ("debian", ".deb"),
            ("fedora", ".rpm"),
            ("arch", ".pkg.tar.zst"),
        ];

        for (distro, suffix) in cases {
            let selected = select_linux_release_asset_for(&manifest, distro, "x86_64")
                .expect("supported distro should have a matching package");
            assert!(
                selected.name.ends_with(suffix),
                "selected {}",
                selected.name
            );
            assert!(!selected.name.ends_with(".src.rpm"));
        }
    }

    #[test]
    fn selects_the_appimage_when_running_as_one() {
        let manifest = linux_manifest();
        let selected = select_linux_release_asset_for(&manifest, "appimage", "x86_64")
            .expect("an AppImage install should find the AppImage");
        assert_eq!(selected.name, "ComfyPilot-1.2.3-x86_64.AppImage");
    }

    #[test]
    fn replacing_an_appimage_swaps_the_file_and_makes_it_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("comfypilot-appimage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("ComfyPilot.AppImage");
        let downloaded = dir.join("ComfyPilot-2.0.0-x86_64.AppImage");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&downloaded, "new").unwrap();

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(replace_appimage(&downloaded, &target))
            .unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        assert!(!downloaded.exists());
        assert!(!dir.join(".ComfyPilot.AppImage.update").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selects_binary_flake_for_nix_managed_installations() {
        let manifest = linux_manifest();
        let selected = select_linux_release_asset_for(&manifest, "nix", "x86_64")
            .expect("Nix installations should discover the binary flake");

        assert_eq!(selected.name, "arctic-comfyui-helper-nix-x86_64.tar.gz");
    }

    #[test]
    fn rejects_unsupported_architectures_and_package_managers() {
        let manifest = linux_manifest();

        assert!(select_linux_release_asset_for(&manifest, "debian", "aarch64").is_none());
        assert!(select_linux_release_asset_for(&manifest, "nixos", "x86_64").is_none());
        assert!(select_linux_release_asset_for(&manifest, "unknown", "x86_64").is_none());
    }
}
