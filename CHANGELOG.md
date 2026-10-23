# Changelog

Release notes for ComfyPilot, newest first. `scripts/release.sh` releases the
version at the top: each section starts with `## <version>`.

## 1.0.0

The first release of ComfyPilot, the successor to Arctic ComfyUI Helper. The app was
rebuilt as a native Qt Quick application; settings, install locations and saved tokens
carry over from Arctic ComfyUI Helper automatically.

### ComfyUI

- Installs ComfyUI with the PyTorch stack that fits your GPU: PyTorch 2.14 with CUDA 13
  on NVIDIA RTX 20-series and newer (driver 580+), current ROCm builds on AMD and XPU
  builds on Intel Arc, on Linux and Windows.
- ComfyUI-Manager now uses ComfyUI's built-in manager. Older installs with a cloned
  Manager are migrated.
- Updates no longer reinstall PyTorch: the stack is pinned per install, so a ComfyUI
  update can't swap it out.
- Several installs side by side, each with its own status, version and stack shown in
  the sidebar.
- Launch options follow ComfyUI's current memory model (dynamic VRAM, fast disk), and
  only options your ComfyUI version supports are offered.
- A configurable port, and a readiness check that confirms ComfyUI is really running
  there.
- ComfyUI's output in a console with colours and filters for errors, warnings and
  info.
- Built-in custom nodes are pinned to tested versions, and a snapshot is taken before
  every ComfyUI update.

### Models, LoRAs and workflows

- Hugging Face token support for gated models, alongside the Civitai token, with
  step-by-step instructions for getting both.
- Browse LoRAs and workflows on Civitai, with video previews in the detail view.
- Check a workflow against your install: see which models and custom nodes it needs
  and download or install what's missing.

### App

- Light and dark themes.
- System tray with a running indicator and Start, Stop and Restart ComfyUI.
- Updates install themselves: the Windows installer, or the .deb, .rpm or Arch package
  on Linux.
- Windows: a per-user installer, no administrator rights needed.
