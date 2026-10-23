Name:           comfypilot
Version:        0.2.9
Release:        1%{?dist}
Summary:        Install and run ComfyUI, and download models, LoRAs and workflows
%global debug_package %{nil}
%global _debugsource_packages 0
# The C++ side (cxx-qt) is linked by rustc; with LTO objects ld drops the
# Qt libraries it only needs at link time.
%global _lto_cflags %{nil}

License:        Apache-2.0
URL:            https://github.com/ArcticLatent/ComfyPilot
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc-c++
BuildRequires:  qt6-qtbase-devel
BuildRequires:  qt6-qtdeclarative-devel
BuildRequires:  pkgconfig(dbus-1)

# QML modules (Quick, Controls, Dialogs, Qt.labs.platform tray), SVG icons,
# WebP previews, video previews (Qt Multimedia, FFmpeg backend), Wayland; the
# libraries themselves are found automatically.
Requires:       qt6-qtdeclarative
Requires:       qt6-qtsvg
Requires:       qt6-qtimageformats
Requires:       qt6-qtmultimedia
Requires:       qt6-qtwayland
Requires:       xdg-desktop-portal

# The app was called Arctic ComfyUI Helper before.
Obsoletes:      arctic-comfyui-helper < 0.3
Provides:       arctic-comfyui-helper = %{version}-%{release}

%description
Native desktop app for installing and running ComfyUI, picking the right
PyTorch stack for your GPU, downloading models, browsing LoRAs and workflows
on Civitai, and checking what a workflow needs.

%prep
%autosetup -n %{name}-%{version}

%build
# A shared target folder (packaging/build-linux.sh) speeds up repeated builds.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-%{_topdir}/cargo-target}"
# Allow reuse of incremental state for local packaging speed.
export CARGO_INCREMENTAL=1
# Prefer fast linker when available; fall back to bfd to avoid missing mold/lld issues.
if command -v mold >/dev/null 2>&1; then
  case " ${RUSTFLAGS:-} " in
    *" -C link-arg=-fuse-ld=mold "*) ;;
    *) export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=mold" ;;
  esac
elif command -v ld.bfd >/dev/null 2>&1; then
  case " ${RUSTFLAGS:-} " in
    *" -C link-arg=-fuse-ld=bfd "*) ;;
    *) export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=bfd" ;;
  esac
fi

export QMAKE=qmake6
cargo build --release --jobs "%{?_smp_build_ncpus}" --manifest-path app/Cargo.toml

%global app_id io.github.ArcticLatent.ComfyPilot

%install
install -Dpm0755 "${CARGO_TARGET_DIR:-%{_topdir}/cargo-target}/release/ComfyPilot" \
  %{buildroot}%{_bindir}/comfypilot
install -Dpm0644 packaging/linux/%{app_id}.desktop \
  %{buildroot}%{_datadir}/applications/%{app_id}.desktop
install -Dpm0644 packaging/linux/%{app_id}.metainfo.xml \
  %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
install -Dpm0644 app/icons/icon.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg

%files
%license LICENSE
%doc README.md
%{_bindir}/comfypilot
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg

%changelog
* Fri Oct 23 2026 Arctic Latent <contact@arcticlatent.com> - 0.2.9-1
- Renamed to ComfyPilot; Qt Quick user interface.

* Sat Feb 14 2026 Arctic Latent <contact@arcticlatent.com> - 0.1.0-1
- Initial Fedora package
