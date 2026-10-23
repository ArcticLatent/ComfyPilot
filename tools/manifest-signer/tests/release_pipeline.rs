use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "arctic-manifest-release-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str], signing_key: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_manifest-signer"));
    command.args(args);
    if let Some(key) = signing_key {
        command.env("ARCTIC_UPDATE_SIGNING_KEY", key);
    }
    command.output().expect("run manifest-signer")
}

fn path_arg(path: &Path) -> &str {
    path.to_str().expect("temporary path should be UTF-8")
}

fn write_manifest(path: &Path, assets: Value) {
    let manifest = json!({
        "version": "9.8.7",
        "tag": "v9.8.7",
        "repository": "Example/Arctic-Helper",
        "assets": assets,
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).expect("serialize fixture"),
    )
    .expect("write manifest fixture");
}

#[test]
fn arch_only_release_replaces_arch_and_preserves_other_signed_assets() {
    let test_dir = TestDir::new();
    let base = test_dir.path("base.json");
    let replacement = test_dir.path("arch-replacement.json");
    let merged = test_dir.path("merged.json");

    write_manifest(
        &base,
        json!([
            {
                "name": "helper-9.8.7-1-x86_64.pkg.tar.zst",
                "sha256": "old-arch-sha",
                "download_url": "https://example.invalid/old-arch"
            },
            {
                "name": "helper_9.8.7_amd64.deb",
                "sha256": "deb-sha",
                "download_url": "https://example.invalid/deb"
            },
            {
                "name": "helper-9.8.7.x86_64.rpm",
                "sha256": "rpm-sha",
                "download_url": "https://example.invalid/rpm"
            },
            {
                "name": "arctic-comfyui-helper-nix-x86_64.tar.gz",
                "sha256": "nix-sha",
                "download_url": "https://example.invalid/nix"
            }
        ]),
    );
    write_manifest(
        &replacement,
        json!([{
            "name": "helper-9.8.7-2-x86_64.pkg.tar.zst",
            "sha256": "new-arch-sha",
            "download_url": "https://example.invalid/new-arch"
        }]),
    );

    let signing_key = SigningKey::from_bytes(&[42; 32]);
    let private_key = STANDARD.encode(signing_key.to_bytes());
    let public_key = STANDARD.encode(signing_key.verifying_key().to_bytes());

    let output = run(
        &[
            "sign",
            "--format",
            "linux-release",
            "--manifest",
            path_arg(&base),
        ],
        Some(&private_key),
    );
    assert!(output.status.success(), "sign base: {output:?}");

    let output = run(
        &[
            "merge-linux-release",
            "--base",
            path_arg(&base),
            "--replacement",
            path_arg(&replacement),
            "--output",
            path_arg(&merged),
            "--pubkey",
            &public_key,
        ],
        None,
    );
    assert!(output.status.success(), "merge manifests: {output:?}");

    let unsigned: Value = serde_json::from_slice(&fs::read(&merged).expect("read merged manifest"))
        .expect("parse merged manifest");
    assert!(unsigned.get("signature").is_none());
    let names: Vec<&str> = unsigned["assets"]
        .as_array()
        .expect("assets array")
        .iter()
        .map(|asset| asset["name"].as_str().expect("asset name"))
        .collect();
    assert_eq!(
        names,
        [
            "arctic-comfyui-helper-nix-x86_64.tar.gz",
            "helper-9.8.7-2-x86_64.pkg.tar.zst",
            "helper-9.8.7.x86_64.rpm",
            "helper_9.8.7_amd64.deb",
        ]
    );

    let output = run(
        &[
            "sign",
            "--format",
            "linux-release",
            "--manifest",
            path_arg(&merged),
        ],
        Some(&private_key),
    );
    assert!(output.status.success(), "sign merged manifest: {output:?}");

    let output = run(
        &[
            "verify",
            "--format",
            "linux-release",
            "--manifest",
            path_arg(&merged),
            "--pubkey",
            &public_key,
        ],
        None,
    );
    assert!(
        output.status.success(),
        "verify merged manifest: {output:?}"
    );

    let mut tampered: Value = serde_json::from_slice(&fs::read(&base).expect("read signed base"))
        .expect("parse signed base");
    tampered["assets"][0]["sha256"] = Value::String("tampered".to_string());
    fs::write(
        &base,
        serde_json::to_vec_pretty(&tampered).expect("serialize tampered base"),
    )
    .expect("write tampered base");

    let output = run(
        &[
            "merge-linux-release",
            "--base",
            path_arg(&base),
            "--replacement",
            path_arg(&replacement),
            "--output",
            path_arg(&merged),
            "--pubkey",
            &public_key,
        ],
        None,
    );
    assert!(!output.status.success(), "tampered base must be rejected");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid signature"),
        "unexpected error: {output:?}"
    );
}
