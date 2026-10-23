#!/usr/bin/env bash
# Builds ComfyPilot's Linux packages on this machine, each in a clean podman
# container of its distro (the same script the release workflow runs):
#
#   scripts/build-linux-local.sh [deb|rpm|arch|appimage|all]   (default: all)
#
# Packages land in dist/. Rust toolchains, crates and build output are kept
# in a podman volume per distro, so later builds are quick. The catalog
# settings (ARCTIC_SUPABASE_*) come from the environment or .env.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGETS=("${@:-all}")
[[ "${TARGETS[0]}" == all ]] && TARGETS=(deb rpm arch appimage)

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
    appimage) echo "docker.io/library/ubuntu:22.04" ;;
    *) echo "unknown target: $1" >&2; exit 1 ;;
  esac
}

for target in "${TARGETS[@]}"; do
  image="$(image_for "$target")"
  echo "== $target in $image"
  # The AppImage has its own script, and keeps the Qt it downloads.
  script=(bash /src/packaging/build-linux.sh "$target" --install-deps)
  extra=()
  if [[ "$target" == appimage ]]; then
    script=(bash /src/packaging/appimage/build.sh --install-deps)
    extra=(-v comfypilot-qt683:/opt/qt)
  fi
  podman run --rm \
    -v "$ROOT:/src:Z" \
    -v "comfypilot-build-$target:/cache" \
    "${extra[@]}" \
    -e CARGO_HOME=/cache/cargo \
    -e RUSTUP_HOME=/cache/rustup \
    -e CARGO_TARGET_DIR=/cache/target \
    -e ARCTIC_SUPABASE_URL="${ARCTIC_SUPABASE_URL:-}" \
    -e ARCTIC_SUPABASE_PUBLISHABLE_KEY="${ARCTIC_SUPABASE_PUBLISHABLE_KEY:-}" \
    "$image" "${script[@]}"
done
ls -l "$ROOT/dist"
