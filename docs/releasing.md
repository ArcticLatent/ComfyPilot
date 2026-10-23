# Releasing

One command releases ComfyPilot: the .deb, .rpm and Arch packages, the Windows installer,
the signed update manifests and the GitHub release with its notes.

## Making a release

1. Add a section at the top of [CHANGELOG.md](../CHANGELOG.md):

   ```markdown
   ## 1.0.1

   - Fixed …
   ```

   The version comes from that heading; the section's text becomes the release notes and
   the notes shown in the app's update dialog.

2. Run, from an up-to-date `main`:

   ```bash
   scripts/release.sh --dry-run   # optional: shows the version and notes
   scripts/release.sh
   ```

   It checks that `main` matches GitHub and only CHANGELOG.md has uncommitted changes, sets
   the version in `Cargo.toml`, `app/Cargo.toml` and `Cargo.lock`, commits "Release
   1.0.1", tags `v1.0.1`, pushes both, then follows the Release workflow and prints the
   release's address. Building takes about 15 minutes.

Needs `git`, `cargo` and the GitHub CLI (`gh auth login`).

## What the workflow does

`.github/workflows/release.yml` runs on a `v*` tag:

| Job | Runs on | Builds |
| --- | --- | --- |
| Linux (deb) | `debian:trixie` container | `comfypilot_<v>-1_amd64.deb` |
| Linux (rpm) | `fedora:43` container | `comfypilot-<v>-1.fc43.x86_64.rpm` |
| Linux (arch) | `archlinux` container | `comfypilot-<v>-1-x86_64.pkg.tar.zst` |
| Windows | `windows-latest`, Qt 6.8.3 | `ComfyPilot-Setup-<v>.exe` (Inno Setup) |
| Publish | `ubuntu-latest` | `update.json`, `linux-release.json`, `SHA256SUMS`, the release |

The publish job signs `update.json` (Windows) and `linux-release.json` (Linux) with the
update-signing key and verifies them against the public key built into the app, so a
wrong key stops the release instead of shipping updates nobody can install. Installed
apps read both from `releases/latest/download/`.

"Run workflow" on the Actions page builds every package without publishing (artifacts
only), which is useful for testing packaging changes.

### Repository secrets

| Secret | Used for |
| --- | --- |
| `ARCTIC_SUPABASE_URL` | The catalog's address, built into the app |
| `ARCTIC_SUPABASE_PUBLISHABLE_KEY` | The catalog's public read key, built into the app |
| `ARCTIC_UPDATE_SIGNING_KEY` | Private key that signs the update manifests ([details](cross-platform-development.md#update-manifest-signing)) |

## Building packages locally

```bash
scripts/build-linux-local.sh            # all three, or: deb | rpm | arch
```

Builds each package in a clean podman container of its distro with the same script CI runs
(`packaging/build-linux.sh`), reading the catalog settings from `.env`. Packages land in
`dist/`; toolchains and build output are cached in podman volumes
(`comfypilot-build-<target>`).

On Windows (Rust, MSVC and Qt 6.8 installed):

```powershell
pwsh packaging/windows/build.ps1
```

## Files

| Path | What |
| --- | --- |
| `CHANGELOG.md` | Release notes, newest first |
| `scripts/release.sh` | The release command |
| `scripts/changelog.sh` | Reads the version and notes from CHANGELOG.md |
| `packaging/build-linux.sh` | Builds one Linux package inside its distro |
| `packaging/debian/`, `packaging/fedora/`, `packaging/arch/` | Package definitions |
| `packaging/linux/` | Desktop entry and AppStream metadata |
| `packaging/windows/` | Windows build script and Inno Setup installer |
| `packaging/release-manifests.sh` | Writes and signs the update manifests |
| `tools/manifest-signer/` | The signing tool |
