//! Ed25519 signature verification for update manifests.
//!
//! `Updater::download_and_install` already checks the downloaded package's
//! SHA-256 against the hash in the manifest -- but until this module, that
//! hash came from the *same* manifest the checksum is meant to protect, over
//! the same channel (GitHub Releases). Anyone able to edit a release (a
//! compromised release publisher, a compromised GitHub account, a
//! malicious release publisher) could publish any binary with a matching hash
//! and it would pass. Requiring a signature over the manifest, checked against
//! a public key baked into the binary, means an attacker also needs the private
//! signing key (currently supplied only to the signing step as the
//! `ARCTIC_UPDATE_SIGNING_KEY` repository secret) to ship a trusted update.
//!
//! The signing half lives in `tools/manifest-signer`, deliberately kept out
//! of the shipped app so the private-key-handling code never ships to users.
//! Both sides call the `*_signing_payload` functions below so the bytes being
//! signed and the bytes being verified can never drift from each other.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Public half of the release-signing keypair. The private half is held as the
/// `ARCTIC_UPDATE_SIGNING_KEY` GitHub Actions secret, scoped to the signing
/// step, and is never committed. Rotating the key means regenerating both, updating the secret,
/// and shipping a build with the new constant here before old builds lose
/// the ability to verify new releases.
pub const UPDATE_MANIFEST_PUBLIC_KEY_B64: &str = "SA4+y0mbn8up0DyAbGQQvohK+tFVqjr6rrIN+nOPk1E=";

const UPDATE_MANIFEST_DOMAIN: &[u8] = b"arctic-update-manifest\x00v1\x00";
const LINUX_RELEASE_MANIFEST_DOMAIN: &[u8] = b"arctic-linux-release-manifest\x00v1\x00";

/// Canonical bytes signed/verified for the single-asset `update.json` format
/// (Windows, and the legacy Linux fallback format).
///
/// A domain-separated, NUL-delimited encoding: NUL bytes can't appear in any
/// of these fields (URLs and hex digests), so there's no ambiguity between,
/// say, `version="1"` + `download_url="2foo"` and `version="12"` +
/// `download_url="foo"`. The domain prefix keeps a signature over this
/// format from ever being replayed as a valid signature over the
/// differently-shaped Linux release-list format, even if the byte strings
/// happened to collide.
pub fn update_manifest_signing_payload(version: &str, download_url: &str, sha256: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        UPDATE_MANIFEST_DOMAIN.len() + version.len() + download_url.len() + sha256.len() + 3,
    );
    buf.extend_from_slice(UPDATE_MANIFEST_DOMAIN);
    buf.extend_from_slice(version.as_bytes());
    buf.push(0);
    buf.extend_from_slice(download_url.as_bytes());
    buf.push(0);
    buf.extend_from_slice(sha256.as_bytes());
    buf.push(0);
    buf
}

/// Canonical bytes signed/verified for the multi-asset `linux-release.json`
/// format. Assets are sorted by name first so the signature doesn't depend
/// on filesystem/glob enumeration order between the machine that signed the
/// manifest and any future re-verification of it.
pub fn linux_release_manifest_signing_payload(
    version: &str,
    tag: &str,
    repository: &str,
    assets: &[(String, String, String)],
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(LINUX_RELEASE_MANIFEST_DOMAIN);
    for field in [version, tag, repository] {
        buf.extend_from_slice(field.as_bytes());
        buf.push(0);
    }

    let mut sorted: Vec<&(String, String, String)> = assets.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, sha256, download_url) in sorted {
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(sha256.as_bytes());
        buf.push(0);
        buf.extend_from_slice(download_url.as_bytes());
        buf.push(0);
    }
    buf
}

fn parse_public_key(key_b64: &str) -> Result<VerifyingKey> {
    let bytes = STANDARD
        .decode(key_b64.trim())
        .context("update manifest public key is not valid base64")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("update manifest public key must be exactly 32 bytes"))?;
    VerifyingKey::from_bytes(&bytes)
        .context("update manifest public key is not a valid Ed25519 key")
}

fn parse_signature(signature_b64: &str) -> Result<Signature> {
    let bytes = STANDARD
        .decode(signature_b64.trim())
        .context("update manifest signature is not valid base64")?;
    let bytes: [u8; 64] = bytes
        .try_into()
        .map_err(|_| anyhow!("update manifest signature must be exactly 64 bytes"))?;
    Ok(Signature::from_bytes(&bytes))
}

/// Verifies `signature_b64` (base64-encoded Ed25519 signature) over
/// `payload` using `public_key_b64` (base64-encoded 32-byte public key).
/// Both `updater.rs` (embedded default key) and `tools/manifest-signer`
/// (`verify` subcommand, for release-script self-checks) call this so the
/// two never disagree about what "valid" means.
pub fn verify_signature(payload: &[u8], signature_b64: &str, public_key_b64: &str) -> Result<()> {
    let verifying_key = parse_public_key(public_key_b64)?;
    let signature = parse_signature(signature_b64)?;
    verifying_key
        .verify(payload, &signature)
        .context("update manifest signature verification failed")
}

/// Same as [`verify_signature`], against the public key baked into this
/// binary (or `ARCTIC_UPDATE_PUBLIC_KEY` when set, for local testing against
/// a throwaway keypair -- an override here is no more powerful than the
/// existing `ARCTIC_UPDATE_MANIFEST_URL` override: both require control of
/// the machine's environment, at which point signature checking isn't the
/// weakest link).
pub fn verify_with_embedded_key(payload: &[u8], signature_b64: &str) -> Result<()> {
    let key = std::env::var("ARCTIC_UPDATE_PUBLIC_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UPDATE_MANIFEST_PUBLIC_KEY_B64.to_string());
    verify_signature(payload, signature_b64, &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    // A throwaway test keypair -- never the real release-signing key.
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn round_trips_update_manifest_signature() {
        let signing_key = test_signing_key();
        let payload = update_manifest_signing_payload(
            "1.2.3",
            "https://example.invalid/asset.exe",
            "deadbeef",
        );
        let signature = signing_key.sign(&payload);
        let signature_b64 = STANDARD.encode(signature.to_bytes());
        let public_key_b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());

        verify_signature(&payload, &signature_b64, &public_key_b64)
            .expect("signature should verify against the matching payload and key");
    }

    #[test]
    fn rejects_a_tampered_field() {
        let signing_key = test_signing_key();
        let payload = update_manifest_signing_payload("1.2.3", "https://example.invalid/a", "abc");
        let signature = signing_key.sign(&payload);
        let signature_b64 = STANDARD.encode(signature.to_bytes());
        let public_key_b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());

        // Same signature, but the sha256 an attacker would need to change to
        // point at a different (malicious) binary.
        let tampered = update_manifest_signing_payload("1.2.3", "https://example.invalid/a", "xyz");
        assert!(verify_signature(&tampered, &signature_b64, &public_key_b64).is_err());
    }

    #[test]
    fn rejects_wrong_public_key() {
        let signing_key = test_signing_key();
        let other_key = SigningKey::from_bytes(&[9u8; 32]);
        let payload = update_manifest_signing_payload("1.2.3", "https://example.invalid/a", "abc");
        let signature = signing_key.sign(&payload);
        let signature_b64 = STANDARD.encode(signature.to_bytes());
        let wrong_public_key_b64 = STANDARD.encode(other_key.verifying_key().to_bytes());

        assert!(verify_signature(&payload, &signature_b64, &wrong_public_key_b64).is_err());
    }

    #[test]
    fn field_boundaries_do_not_collide() {
        // Without NUL-delimiting, ("1", "2foo") and ("12", "foo") would sign
        // identical bytes under naive concatenation.
        let a = update_manifest_signing_payload("1", "2foo", "sha");
        let b = update_manifest_signing_payload("12", "foo", "sha");
        assert_ne!(a, b);
    }

    #[test]
    fn linux_manifest_payload_is_independent_of_asset_order() {
        let assets_a = vec![
            (
                "a.deb".to_string(),
                "sha-a".to_string(),
                "url-a".to_string(),
            ),
            (
                "b.rpm".to_string(),
                "sha-b".to_string(),
                "url-b".to_string(),
            ),
        ];
        let assets_b = vec![
            (
                "b.rpm".to_string(),
                "sha-b".to_string(),
                "url-b".to_string(),
            ),
            (
                "a.deb".to_string(),
                "sha-a".to_string(),
                "url-a".to_string(),
            ),
        ];

        let payload_a =
            linux_release_manifest_signing_payload("1.0.0", "v1.0.0", "Owner/Repo", &assets_a);
        let payload_b =
            linux_release_manifest_signing_payload("1.0.0", "v1.0.0", "Owner/Repo", &assets_b);
        assert_eq!(payload_a, payload_b);
    }

    #[test]
    fn linux_manifest_payload_is_not_a_valid_update_manifest_payload() {
        // Domain separation: the same underlying strings shouldn't produce
        // interchangeable signed bytes across the two manifest shapes.
        let update_payload = update_manifest_signing_payload("1.0.0", "v1.0.0", "Owner/Repo");
        let linux_payload =
            linux_release_manifest_signing_payload("1.0.0", "v1.0.0", "Owner/Repo", &[]);
        assert_ne!(update_payload, linux_payload);
    }
}
