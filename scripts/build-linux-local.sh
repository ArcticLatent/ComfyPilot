#!/usr/bin/env bash
# Builds ComfyPilot's Linux packages on this machine, each in a clean podman
# container of its distro (the same script the release workflow runs):
#
#   scripts/build-linux-local.sh [deb|rpm|arch|all]   (default: all)
#
# Packages land in dist/. Rust toolchains, crates and build output are kept
# in a podman volume per distro, so later builds are quick. The catalog
# settings (ARCTIC_SUPABASE_*) come from the environment or .env.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGETS=("${@:-all}")
[[ "${TARGETS[0]}" == all ]] && TARGETS=(deb rpm arch)

if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$ROOT/.env"
  set +a
fi

image_for() {
  case "$1" in
    deb) echo "docker.io/library/debian:trixie" ;;
    rpm) echo "registry.fedoraproject.org/fedora:43" ;;
    arch) echo "docker.io/library/archlinux:latest" ;;
    *) echo "unknown target: $1" >&2; exit 1 ;;
  esac
}

for target in "${TARGETS[@]}"; do
  image="$(image_for "$target")"
  echo "== $target in $image"
  podman run --rm \
    -v "$ROOT:/src:Z" \
    -v "comfypilot-build-$target:/cache" \
    -e CARGO_HOME=/cache/cargo \
    -e RUSTUP_HOME=/cache/rustup \
    -e CARGO_TARGET_DIR=/cache/target \
    -e ARCTIC_SUPABASE_URL="${ARCTIC_SUPABASE_URL:-}" \
    -e ARCTIC_SUPABASE_PUBLISHABLE_KEY="${ARCTIC_SUPABASE_PUBLISHABLE_KEY:-}" \
    "$image" bash /src/packaging/build-linux.sh "$target" --install-deps
done
ls -l "$ROOT/dist"
