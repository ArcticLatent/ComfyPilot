#!/usr/bin/env bash
# Builds ComfyPilot's package for one Linux family, meant to run as root in a
# clean container of that distro (the release workflow, or locally through
# scripts/build-linux-local.sh):
#
#   packaging/build-linux.sh deb|rpm|arch [--install-deps]
#
#   deb   Debian 13 / Ubuntu 25.04+ (.deb)
#   rpm   Fedora (.rpm)
#   arch  Arch Linux and derivatives (.pkg.tar.zst)
#
# The version comes from app/Cargo.toml. The checkout is copied to a scratch
# folder first (it may be read-only or owned by another user), and the
# package lands in dist/. --install-deps installs the distro's build
# dependencies; Rust comes from rustup (deb, rpm) or the distro (arch).
#
# Honoured environment: CARGO_HOME, RUSTUP_HOME and CARGO_TARGET_DIR (caches
# between builds), ARCTIC_SUPABASE_URL / ARCTIC_SUPABASE_PUBLISHABLE_KEY
# (the catalog settings built into the app; required unless
# ALLOW_NO_CATALOG=1).
set -euo pipefail

TARGET="${1:-}"
INSTALL_DEPS=0
[[ "${2:-}" == "--install-deps" ]] && INSTALL_DEPS=1

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/dist"
WORK="${WORK_DIR:-/tmp/comfypilot-build}"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/app/Cargo.toml" | head -1)"
# The app's Cargo.toml asks for this Rust at least.
MIN_RUST="1.89"

fail() {
  echo "build-linux: $*" >&2
  exit 1
}

[[ -n "$VERSION" ]] || fail "no version in app/Cargo.toml"
case "$TARGET" in
  deb | rpm | arch) ;;
  *) fail "usage: $0 deb|rpm|arch [--install-deps]" ;;
esac
if [[ -z "${ARCTIC_SUPABASE_URL:-}" || -z "${ARCTIC_SUPABASE_PUBLISHABLE_KEY:-}" ]] &&
  [[ "${ALLOW_NO_CATALOG:-}" != 1 ]]; then
  fail "ARCTIC_SUPABASE_URL and ARCTIC_SUPABASE_PUBLISHABLE_KEY must be set (or ALLOW_NO_CATALOG=1)"
fi

install_deps() {
  case "$TARGET" in
    deb)
      apt-get update
      DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential debhelper fakeroot pkg-config ca-certificates curl git binutils \
        qt6-base-dev qt6-base-dev-tools qt6-declarative-dev libdbus-1-dev
      ;;
    rpm)
      dnf install -y rpm-build gcc-c++ make git curl tar binutils \
        qt6-qtbase-devel qt6-qtdeclarative-devel dbus-devel
      ;;
    arch)
      pacman -Syu --noconfirm --needed base-devel git binutils pkgconf rust \
        qt6-base qt6-declarative qt6-svg qt6-imageformats qt6-multimedia \
        qt6-multimedia-ffmpeg qt6-wayland xdg-desktop-portal dbus
      ;;
  esac
}

# rustup's stable toolchain when there's no cargo or it's older than MIN_RUST.
ensure_rust() {
  local cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
  export PATH="$cargo_bin:$PATH"
  if command -v cargo >/dev/null 2>&1; then
    local have
    have="$(cargo --version | awk '{print $2}')"
    if [[ "$(printf '%s\n%s\n' "$MIN_RUST" "$have" | sort -V | head -1)" == "$MIN_RUST" ]]; then
      return
    fi
  fi
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
  command -v cargo >/dev/null 2>&1 || fail "cargo is missing after installing rustup"
}

# The checkout, without build output, in a scratch folder.
copy_source() {
  rm -rf "$WORK"
  mkdir -p "$WORK/src"
  tar -C "$ROOT" \
    --exclude=./.git --exclude=./target --exclude=./dist --exclude=./.env \
    --exclude=./packaging/arch/src --exclude=./packaging/arch/pkg \
    -cf - . | tar -C "$WORK/src" -xf -
}

build_deb() {
  cd "$WORK/src"
  cp -a packaging/debian/debian debian
  # One changelog entry for this version (the package's own history).
  cat >debian/changelog <<EOF
comfypilot (${VERSION}-1) unstable; urgency=medium

  * Release ${VERSION}.

 -- Arctic Latent <contact@arcticlatent.com>  $(date -R)
EOF
  # -d: Rust comes from rustup, not the cargo package in Build-Depends.
  dpkg-buildpackage -us -uc -b -d
  cp -f "../comfypilot_${VERSION}-1_amd64.deb" "$OUT/"
}

build_rpm() {
  local top="$WORK/rpmbuild"
  mkdir -p "$top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
  tar -C "$WORK/src" --transform "s,^\.,comfypilot-${VERSION}," \
    -czf "$top/SOURCES/comfypilot-${VERSION}.tar.gz" .
  sed "s/^Version:.*/Version:        ${VERSION}/" "$WORK/src/packaging/fedora/comfypilot.spec" \
    >"$top/SPECS/comfypilot.spec"
  # --nodeps: Rust comes from rustup, not the cargo/rust packages.
  rpmbuild --define "_topdir $top" --nodeps -bb "$top/SPECS/comfypilot.spec"
  find "$top/RPMS" -name "comfypilot-${VERSION}-*.x86_64.rpm" -exec cp -f {} "$OUT/" \;
}

build_arch() {
  # makepkg won't run as root.
  id builder >/dev/null 2>&1 || useradd -m builder
  sed -i "s/^pkgver=.*/pkgver=${VERSION}/" "$WORK/src/packaging/arch/PKGBUILD"
  chown -R builder: "$WORK"
  local dir
  for dir in "${CARGO_HOME:-}" "${CARGO_TARGET_DIR:-}"; do
    [[ -n "$dir" ]] || continue
    mkdir -p "$dir"
    chown -R builder: "$dir"
  done
  runuser -u builder -- env \
    CARGO_HOME="${CARGO_HOME:-/home/builder/.cargo}" \
    ARCTIC_SUPABASE_URL="${ARCTIC_SUPABASE_URL:-}" \
    ARCTIC_SUPABASE_PUBLISHABLE_KEY="${ARCTIC_SUPABASE_PUBLISHABLE_KEY:-}" \
    COMFYPILOT_CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-}" \
    bash -c "cd '$WORK/src/packaging/arch' && makepkg -f --noconfirm"
  cp -f "$WORK/src/packaging/arch/comfypilot-${VERSION}-1-x86_64.pkg.tar.zst" "$OUT/"
}

((INSTALL_DEPS)) && install_deps
[[ "$TARGET" == arch ]] || ensure_rust
mkdir -p "$OUT"
copy_source
echo "Building ComfyPilot ${VERSION} (${TARGET})..."
"build_${TARGET}"
echo "Built:"
find "$OUT" -maxdepth 1 -type f -name "*${VERSION}*" -printf '  %f (%s bytes)\n'
