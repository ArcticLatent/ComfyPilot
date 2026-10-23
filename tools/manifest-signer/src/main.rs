//! Release-time signing tool for Arctic ComfyUI Helper update manifests.
//!
//! This binary is never shipped -- it only runs from the release scripts
//! (`scripts/build-release.ps1`, `scripts/build-release-linux.sh`) and CI
//! (`.github/workflows/release-windows.yml`). It's the only place the
//! private signing key is ever read; the app itself (`src/updater.rs`) only
//! ever verifies, using the public key embedded in
//! `src/update_signing.rs::UPDATE_MANIFEST_PUBLIC_KEY_B64`.
//!
//! Usage:
//!   manifest-signer keygen
//!       Generates a new Ed25519 keypair and prints both halves as base64.
//!       Run once; store the private half as the ARCTIC_UPDATE_SIGNING_KEY
//!       secret and paste the public half into update_signing.rs.
//!
//!   manifest-signer sign --format <update|linux-release> --manifest <path>
//!       Reads ARCTIC_UPDATE_SIGNING_KEY (base64 private key) from the
//!       environment, signs the manifest at <path>, and rewrites it in
//!       place with a `signature` field added.
//!
//!   manifest-signer verify --format <update|linux-release> --manifest <path> [--pubkey <base64>]
//!       Verifies the `signature` field already in the manifest. Uses the
//!       embedded default public key unless --pubkey overrides it. Exits
//!       non-zero on any failure -- intended for the verify-release*
//!       scripts to catch a broken signing key before a release ships.
//!
//!   manifest-signer merge-linux-release --base <path> --replacement <path> --output <path> [--pubkey <base64>]
//!       Verifies an existing signed Linux manifest, replaces package kinds
//!       present in the replacement manifest, and writes an unsigned merged
//!       manifest ready for the normal `sign` command. Used by `--arch-only`
//!       releases so rebuilding only Arch does not remove Debian/RPM/Nix
//!       assets from the public update manifest. The public-key override is
//!       intended for isolated release-pipeline tests.

use anyhow::{anyhow, bail, Context, Result};
use arctic_downloader::update_signing::{
    linux_release_manifest_signing_payload, update_manifest_signing_payload, verify_signature,
    UPDATE_MANIFEST_PUBLIC_KEY_B64,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

const SIGNING_KEY_ENV: &str = "ARCTIC_UPDATE_SIGNING_KEY";

#[derive(Debug, Serialize, Deserialize)]
struct UpdateManifestFile {
    version: String,
    download_url: String,
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LinuxAssetFile {
    name: String,
    sha256: String,
    download_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LinuxReleaseManifestFile {
    version: String,
    tag: String,
    repository: String,
    assets: Vec<LinuxAssetFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Update,
    LinuxRelease,
}

fn parse_format(value: &str) -> Result<Format> {
    match value {
        "update" => Ok(Format::Update),
        "linux-release" => Ok(Format::LinuxRelease),
        other => bail!("unknown --format '{other}' (expected 'update' or 'linux-release')"),
    }
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args
        .next()
        .context("missing command (keygen | sign | verify | merge-linux-release)")?;

    match command.as_str() {
        "keygen" => keygen(),
        "sign" => {
            let (format, manifest) = parse_manifest_args(args)?;
            sign(format, &manifest)
        }
        "verify" => {
            let (format, manifest, pubkey) = parse_verify_args(args)?;
            verify(format, &manifest, pubkey.as_deref())
        }
        "merge-linux-release" => {
            let (base, replacement, output, pubkey) = parse_merge_args(args)?;
            merge_linux_release_files(&base, &replacement, &output, pubkey.as_deref())
        }
        other => bail!(
            "unknown command '{other}' (expected 'keygen', 'sign', 'verify', or 'merge-linux-release')"
        ),
    }
}

fn parse_merge_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(PathBuf, PathBuf, PathBuf, Option<String>)> {
    let mut base = None;
    let mut replacement = None;
    let mut output = None;
    let mut pubkey = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => {
                base = Some(PathBuf::from(
                    args.next().context("--base requires a value")?,
                ))
            }
            "--replacement" => {
                replacement = Some(PathBuf::from(
                    args.next().context("--replacement requires a value")?,
                ))
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().context("--output requires a value")?,
                ))
            }
            "--pubkey" => {
                pubkey = Some(args.next().context("--pubkey requires a value")?);
            }
            other => bail!("unrecognized argument '{other}'"),
        }
    }
    Ok((
        base.context("--base is required")?,
        replacement.context("--replacement is required")?,
        output.context("--output is required")?,
        pubkey,
    ))
}

fn parse_manifest_args(mut args: impl Iterator<Item = String>) -> Result<(Format, PathBuf)> {
    let mut format = None;
    let mut manifest = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => {
                let value = args.next().context("--format requires a value")?;
                format = Some(parse_format(&value)?);
            }
            "--manifest" => {
                let value = args.next().context("--manifest requires a value")?;
                manifest = Some(PathBuf::from(value));
            }
            other => bail!("unrecognized argument '{other}'"),
        }
    }
    Ok((
        format.context("--format is required")?,
        manifest.context("--manifest is required")?,
    ))
}

fn parse_verify_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(Format, PathBuf, Option<String>)> {
    let mut format = None;
    let mut manifest = None;
    let mut pubkey = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => {
                let value = args.next().context("--format requires a value")?;
                format = Some(parse_format(&value)?);
            }
            "--manifest" => {
                let value = args.next().context("--manifest requires a value")?;
                manifest = Some(PathBuf::from(value));
            }
            "--pubkey" => {
                pubkey = Some(args.next().context("--pubkey requires a value")?);
            }
            other => bail!("unrecognized argument '{other}'"),
        }
    }
    Ok((
        format.context("--format is required")?,
        manifest.context("--manifest is required")?,
        pubkey,
    ))
}

fn keygen() -> Result<()> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed)
        .map_err(|err| anyhow!("failed to read OS randomness for key generation: {err}"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    println!("Generated a new Ed25519 update-signing keypair.");
    println!();
    println!("Private key (store ONLY as the {SIGNING_KEY_ENV} secret, never commit it):");
    println!("  {}", STANDARD.encode(signing_key.to_bytes()));
    println!();
    println!("Public key (paste into src/update_signing.rs::UPDATE_MANIFEST_PUBLIC_KEY_B64):");
    println!("  {}", STANDARD.encode(verifying_key.to_bytes()));
    Ok(())
}

fn load_signing_key() -> Result<SigningKey> {
    let raw = env::var(SIGNING_KEY_ENV)
        .with_context(|| format!("{SIGNING_KEY_ENV} is not set (run `manifest-signer keygen` once, store the private key as this secret)"))?;
    let bytes = STANDARD
        .decode(raw.trim())
        .context("ARCTIC_UPDATE_SIGNING_KEY is not valid base64")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("ARCTIC_UPDATE_SIGNING_KEY must decode to exactly 32 bytes"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn sign(format: Format, manifest_path: &PathBuf) -> Result<()> {
    let signing_key = load_signing_key()?;
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read manifest at {manifest_path:?}"))?;

    match format {
        Format::Update => {
            let mut manifest: UpdateManifestFile =
                serde_json::from_str(&raw).with_context(|| {
                    format!("failed to parse {manifest_path:?} as an update manifest")
                })?;
            let payload = update_manifest_signing_payload(
                &manifest.version,
                &manifest.download_url,
                &manifest.sha256,
            );
            let signature = signing_key.sign(&payload);
            manifest.signature = Some(STANDARD.encode(signature.to_bytes()));
            write_pretty(manifest_path, &manifest)?;
        }
        Format::LinuxRelease => {
            let mut manifest: LinuxReleaseManifestFile =
                serde_json::from_str(&raw).with_context(|| {
                    format!("failed to parse {manifest_path:?} as a linux-release manifest")
                })?;
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
            let signature = signing_key.sign(&payload);
            manifest.signature = Some(STANDARD.encode(signature.to_bytes()));
            write_pretty(manifest_path, &manifest)?;
        }
    }

    println!("Signed {manifest_path:?}");
    Ok(())
}

fn write_pretty<T: Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    let mut json = serde_json::to_string_pretty(value).context("failed to serialize manifest")?;
    json.push('\n');
    fs::write(path, json).with_context(|| format!("failed to write manifest to {path:?}"))
}

fn linux_asset_kind(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if name.contains(".pkg.tar") {
        Some("arch")
    } else if name.ends_with(".deb") {
        Some("deb")
    } else if name.ends_with(".src.rpm") {
        Some("src-rpm")
    } else if name.ends_with(".rpm") {
        Some("rpm")
    } else if name.ends_with(".flatpak") {
        Some("flatpak")
    } else if name.ends_with(".tar.gz") && name.contains("nix") {
        Some("nix")
    } else {
        None
    }
}

fn verify_linux_release_file(
    manifest: &LinuxReleaseManifestFile,
    pubkey_override: Option<&str>,
) -> Result<()> {
    let signature = manifest
        .signature
        .as_deref()
        .context("base Linux manifest has no signature")?;
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
    verify_signature(
        &payload,
        signature,
        pubkey_override.unwrap_or(UPDATE_MANIFEST_PUBLIC_KEY_B64),
    )
    .context("existing Linux release manifest has an invalid signature")
}

fn merge_linux_release_manifests(
    mut base: LinuxReleaseManifestFile,
    replacement: LinuxReleaseManifestFile,
) -> Result<LinuxReleaseManifestFile> {
    if base.version != replacement.version
        || base.tag != replacement.tag
        || base.repository != replacement.repository
    {
        bail!("base and replacement Linux manifests do not describe the same release");
    }

    let replacement_kinds: Vec<&'static str> = replacement
        .assets
        .iter()
        .filter_map(|asset| linux_asset_kind(&asset.name))
        .collect();
    base.assets.retain(|asset| {
        let package_kind_is_preserved = match linux_asset_kind(&asset.name) {
            Some(kind) => !replacement_kinds.contains(&kind),
            None => true,
        };
        !replacement.assets.iter().any(|new| new.name == asset.name) && package_kind_is_preserved
    });
    base.assets.extend(replacement.assets);
    base.assets.sort_by(|a, b| a.name.cmp(&b.name));
    base.signature = None;
    Ok(base)
}

fn merge_linux_release_files(
    base_path: &PathBuf,
    replacement_path: &PathBuf,
    output_path: &PathBuf,
    pubkey_override: Option<&str>,
) -> Result<()> {
    let base: LinuxReleaseManifestFile = serde_json::from_str(
        &fs::read_to_string(base_path)
            .with_context(|| format!("failed to read base manifest at {base_path:?}"))?,
    )
    .with_context(|| format!("failed to parse base manifest at {base_path:?}"))?;
    verify_linux_release_file(&base, pubkey_override)?;

    let replacement: LinuxReleaseManifestFile =
        serde_json::from_str(&fs::read_to_string(replacement_path).with_context(|| {
            format!("failed to read replacement manifest at {replacement_path:?}")
        })?)
        .with_context(|| format!("failed to parse replacement manifest at {replacement_path:?}"))?;

    let merged = merge_linux_release_manifests(base, replacement)?;
    write_pretty(output_path, &merged)?;
    println!("Merged Linux release manifest: {output_path:?}");
    Ok(())
}

fn verify(format: Format, manifest_path: &PathBuf, pubkey_override: Option<&str>) -> Result<()> {
    let public_key = pubkey_override.unwrap_or(UPDATE_MANIFEST_PUBLIC_KEY_B64);
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read manifest at {manifest_path:?}"))?;

    match format {
        Format::Update => {
            let manifest: UpdateManifestFile = serde_json::from_str(&raw).with_context(|| {
                format!("failed to parse {manifest_path:?} as an update manifest")
            })?;
            let signature = manifest
                .signature
                .as_deref()
                .context("manifest has no `signature` field -- was it signed?")?;
            let payload = update_manifest_signing_payload(
                &manifest.version,
                &manifest.download_url,
                &manifest.sha256,
            );
            verify_signature(&payload, signature, public_key)?;
        }
        Format::LinuxRelease => {
            let manifest: LinuxReleaseManifestFile =
                serde_json::from_str(&raw).with_context(|| {
                    format!("failed to parse {manifest_path:?} as a linux-release manifest")
                })?;
            let signature = manifest
                .signature
                .as_deref()
                .context("manifest has no `signature` field -- was it signed?")?;
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
            verify_signature(&payload, signature, public_key)?;
        }
    }

    println!("Signature OK: {manifest_path:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> LinuxAssetFile {
        LinuxAssetFile {
            name: name.to_string(),
            sha256: format!("sha-{name}"),
            download_url: format!("https://example.invalid/{name}"),
        }
    }

    fn manifest(assets: &[&str]) -> LinuxReleaseManifestFile {
        LinuxReleaseManifestFile {
            version: "1.2.3".to_string(),
            tag: "v1.2.3".to_string(),
            repository: "ArcticLatent/Arctic-Helper".to_string(),
            assets: assets.iter().map(|name| asset(name)).collect(),
            signature: Some("old-signature".to_string()),
        }
    }

    #[test]
    fn merge_replaces_one_package_kind_and_preserves_the_rest() {
        let base = manifest(&[
            "helper-old-x86_64.pkg.tar.zst",
            "helper-amd64.deb",
            "helper-x86_64.rpm",
        ]);
        let replacement = manifest(&["helper-new-x86_64.pkg.tar.zst"]);

        let merged = merge_linux_release_manifests(base, replacement).unwrap();
        let names: Vec<&str> = merged
            .assets
            .iter()
            .map(|asset| asset.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "helper-amd64.deb",
                "helper-new-x86_64.pkg.tar.zst",
                "helper-x86_64.rpm",
            ]
        );
        assert!(merged.signature.is_none());
    }

    #[test]
    fn merge_rejects_different_releases() {
        let base = manifest(&["helper-amd64.deb"]);
        let mut replacement = manifest(&["helper-x86_64.pkg.tar.zst"]);
        replacement.version = "1.2.4".to_string();

        assert!(merge_linux_release_manifests(base, replacement).is_err());
    }
}
