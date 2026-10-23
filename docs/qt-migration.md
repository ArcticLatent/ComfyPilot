# Migration from Tauri to Qt Quick (Qt 6)

Branch: `qt-ui`. `main` and `app-improvements` keep the working Tauri app until this
branch reaches parity.

## Decisions (2026-10-23)

- **Stack:** Rust + QML through [cxx-qt](https://github.com/KDAB/cxx-qt) 0.10. The core
  crate (`src/`) and the platform backend (installs, torch, add-ons, runtime, downloads)
  stay in Rust; only the interface is rewritten, in QML.
- **Look:** the design of Harlo (the author's other Rust + Qt app, `~/Projects/piqt`):
  Qt Quick Controls **Basic** style under Harlo's own components (Theme roles with its
  `harlo-dark` palette, Icon, ControlSurface, FlatButton, PanelHeader, fields, combo,
  chips, badges, status dots), its layout (left rail | sidebar | page; the side panels moved to a bottom
  dock because the user wants no vertical panels on the right), Geist + JetBrains Mono fonts, and a widgets-capable `QApplication` from a
  small C++ helper (`app/cpp/arctic_qt.h`). Material was tried first and dropped.
- **Windows distribution:** a per-user installer (no admin rights). The updater downloads
  the new installer and runs it silently. The single self-replacing `.exe` goes away.
- **Licensing:** Qt is used under the LGPLv3 and linked dynamically; packages ship Qt's
  license texts.

## Architecture

```
src/                core crate (unchanged): catalog, config, downloads, updater, Civitai
app/                the application (was src-tauri/)
  src/host.rs       stand-in for the bits of Tauri the backend used: AppHandle, State,
                    Emitter/Manager, async_runtime. Events go to the UI over a channel.
  src/commands.rs   the command table: name -> Rust function, JSON arguments in, JSON out.
                    Same command names and camelCase argument names as the Tauri frontend.
  src/bridge.rs     cxx-qt QObject `Backend` (QML singleton):
                      invoke(command, argsJson) -> request id
                      signal replied(id, ok, json)   signal event(name, json)
  src/qt_app.rs     QApplication, bundled fonts, window icon, Qt messages to the log
                    file (C++ in cpp/arctic_qt.h)
  src/main.rs       backend init, Basic style, QML engine.
  qml/              Main.qml, pages, Harlo-derived components, Api.qml (promises),
                    Theme.qml; fonts/ (OFL) and icons/REMIXICON-LICENSE
  app_linux*/ app_windows*/ shared*/ platform.rs contracts.rs   backend, as before
```

Commands run off the UI thread: async commands on the Tokio runtime, the rest on its
blocking pool. Results and events are queued back to the Qt thread with cxx-qt's
`qt_thread().queue`. UI-only actions the backend used to do through Tauri (show/hide the
window, tray status, notifications, quit) become events the QML side handles.

## Phases

1. **Skeleton (done 2026-10-23, Linux):** Tauri, the web frontend, npm tooling and the
   xwin cross-check removed; `src-tauri/` renamed `app/`; host shim, generated command
   tables (66 Linux / 62 Windows commands, same names and arguments as before), `Backend`
   bridge, `Api`/`Theme` singletons; a Material window with header (system info, update
   check, diagnostics, logs), the four tabs as placeholders showing live data, and the
   status log. Tray backend moved to `shared/tray.rs` (events + commands for a QML tray).
   CI installs Qt 6.8 LTS. Not yet built on Windows (needs Qt in the VM).
   Restyled the same day to Harlo's design: sidebar with the four sections and the
   ComfyUI installs, system card, left rail (diagnostics, logs, updates); Activity /
   Downloads / ComfyUI output live in a bottom dock (`DockBar` tabs, resizable panel
   above it; the right rail was dropped: no vertical panels on the right). Window layout
   kept in QtCore Settings.
2. **ComfyUI page (built 2026-10-23, not yet tested by hand):** `ComfyPage.qml` holds the
   state and the old rules (exclusive attention backends, Nunchaku→InsightFace, Sage3 on
   RTX 50 only, Trellis2 on torch280_cu128, NVIDIA-only add-ons on other stacks, launch
   flags the install's ComfyUI lacks). `ComfyManageView`: start/stop, open in browser,
   update check/update, version and torch tiles, add-ons, custom nodes, launch options,
   shared models folder. `ComfyInstallView`: install folder (or pick an existing install),
   shared models, GPU and torch (recommendation), ROCm/XPU guided setup, resume/fresh,
   options, preflight, install/cancel. The Tauri "Install New / Manage Existing" dropdown
   became the sidebar: an install row manages it, "+ Install" opens the install view.
   Install and runtime messages go to the Activity panel.
3. **Models page (built 2026-10-23, downloads not yet tested by hand):** `ModelsPage.qml`
   with the old selection rules moved verbatim into `ModelLogic.js` (`.pragma library`,
   shipped as a resource: VRAM/RAM tiers, variant order and labels, per-RAM-tier files,
   text-encoder defaults, additional-file groups). Family chips + search, list grouped
   by family with a variant picker, detected VRAM/RAM with overrides, a download queue
   along the bottom of the page (cards side by side, collapsible) with each model's files
   and tickable extras and the total size, Hugging Face token and
   Xet in a dialog, catalog refresh. Downloads go through the `Downloads` singleton
   (download-progress events) and show in the Downloads panel (per-file and overall
   progress, cancel, finished list with open-folder) — shared by LoRAs and Workflows.
4. **LoRAs tab (built 2026-10-23, download not yet tested by hand):** `CivitaiBrowser.qml`
   with `kind: "loras"` (was `LorasPage.qml`), the Civitai browser on the existing
   `lora_browser.rs` commands. Search, sort, period
   and base model (kept in `Settings`), a grid of 3:4 preview cards that loads more as
   you scroll, Installed badges, the Mature content switch and the Civitai token dialog.
   A card opens an in-page detail view: gallery with thumbnails (videos open in the
   browser), version and file pickers, size and save path, click-to-copy trigger words,
   Download (through `Downloads`, kind `lora`), tags and description. Mature content is
   only the switch (filtered in the backend); no blur. The startup "top LoRAs" fetch in
   Main.qml is gone. Previews come through a disk cache (`<cache>/network`, 300 MB,
   `cpp/arctic_qt.h`), which uses the cached copy for Civitai hosts whatever its age:
   the URLs are content-addressed, but the CDN serves them already past their max-age.
   Log hits with `QT_LOGGING_RULES=arctic.netcache.debug=true`.
5. **Workflows tab (built 2026-10-23):** `WorkflowsPage.qml`. Catalog workflows as
   compact cards (preview, family, "Get on Patreon" or Download, Tutorial); the
   dependency check with the install's saved workflows on the left (or any file through
   Qt's FileDialog; `pick_workflow_file` (rfd) is no longer used) and the report on the
   right: models and packs with missing ones first, Download (confirmed, through
   `Downloads`) and Install (confirmed; ComfyUI stopped and restarted), nodes without a
   pack, check again after a download ends. Workflow text is shown as plain text.
   `PlaceholderPage.qml` is gone.
   A second view, **Civitai** (the default; Civitai | Library switch in both headers), is
   `CivitaiBrowser` with `kind: "workflows"`: Civitai's Workflows type, only models whose
   latest version has a `.json` file (about 60%; the rest ship `.zip`), details listing
   only `.json` files, downloads into `user/default/workflows` (`workflow_browser.rs`;
   kept only if it parses as a ComfyUI workflow) and "Check what it needs" hands the saved
   file to the Library check. Many workflow downloads need a Civitai login; the API gives
   no sign of which, so a 401 says to add the token. `download_file_with_cancel` now
   sends the Civitai token (header + `token` query, as for LoRAs) to civitai.com only.
6. **Shell (built 2026-10-23, tray not yet tried on a real desktop):**
   - Tray: `AppTray.qml` (Qt Labs Platform `SystemTrayIcon`, which uses QSystemTrayIcon
     with the QApplication): click shows the window; menu Show / ComfyUI state / Start /
     Restart (`tray_restart_comfyui`: stop, wait up to 15 s for the port, start) / Stop /
     Quit. The icon's C turns green (`assets/icon-running.svg`) while ComfyUI runs.
     On Wayland a menu without a parent window can't be shown as a popup ("Failed to
     create grabbing popup"); the desktop draws the tray menu over D-Bus anyway, so
     `WaylandOrphanMenuGuard` (cpp/arctic_qt.h) keeps such a menu off screen and closes
     it. Seen intermittently at startup; reproduced by popping up a parentless QMenu.
   - `--hidden` starts in the tray only (for autostart); the window shows after 2 s if
     there is no tray. Shown when the backend wants one (`get_tray_status`, `tray-status`;
     `ARCTIC_ENABLE_TRAY=0`) and the desktop has a tray.
   - Close: with a tray the window hides (a one-time "still running in the tray"
     message), otherwise the app quits. `setQuitOnLastWindowClosed(false)`, so QML
     quits explicitly, and `main.rs` exits if Main.qml fails to load (else an invisible
     process would stay).
   - Notifications: `notify` events and finished/failed downloads, through the tray,
     only while the window isn't in front.
   - Single instance: `claim_single_instance` (cpp/arctic_qt.h) with QLocalServer at
     `$XDG_RUNTIME_DIR[/app/$FLATPAK_ID]/io.github.ArcticHelper-<user>.sock` (a
     per-user name in the temp folder if that path is too long; a pipe name on
     Windows). A second launch tells the first to show its window and exits.
   - Updates: a quiet check 5 s after startup; the rail button then installs (confirm
     with version and notes, the update screen, `auto_update_startup`, `app-exit`), or
     opens the release page when updates are managed externally.
   - Startup screen: `StartupOverlay.qml`, only if loading takes over 300 ms, gone when
     settings and installs are in (4 s at most); also covers the window while updating.
   - A damaged settings file: the reset notice opens as a dialog (and goes to Activity).
7. **Packaging and CI (done 2026-10-23):** .deb (Debian 13), .rpm (Fedora 43) and Arch
   packages built in distro containers, a per-user Inno Setup installer for Windows
   (`windeployqt`), the updater running that installer, and one release workflow plus
   `scripts/release.sh`; see [releasing.md](releasing.md). Flatpak, COPR, AUR and Nix
   were dropped.
8. **Cleanup and merge:** docs, README, remove leftovers, merge into `main`.

## Video previews (2026-10-23)

The Civitai detail view plays a video preview inline (`MediaPlayer` + `VideoOutput` from
QtMultimedia, FFmpeg backend): muted and looping, play/pause, sound, open in the
browser; only while the detail view shows it (leaving it stops playback). Grid cards
show stills. Packages now need Qt Multimedia and its FFmpeg plugin (Arch
`qt6-multimedia qt6-multimedia-ffmpeg`, Fedora `qt6-qtmultimedia`, Debian
`qml6-module-qtmultimedia`, whose `libqt6multimedia6` carries the FFmpeg plugin);
`windeployqt` bundles the FFmpeg plugin and DLLs (`packaging/windows/build.ps1` checks).

## Rebrand to ComfyPilot (2026-10-23)

Visible branding only so far: window title, sidebar and startup logo
(`app/qml/assets/logo.svg`, via `LogoMark.qml`), the mark as window/tray/app icon
(`app/qml/assets/icon.svg`, `app/icons/*` regenerated from it), tray texts,
notifications, update dialog, diagnostics, the desktop entry's and metainfo's name, the
Qt application name (so Qt settings and caches now live under "ComfyPilot"). The app ID
`io.github.ArcticHelper`, data folders, binary and package names, keychain entries and the
GitHub release repository/update feed were unchanged at first.

Then (decided 2026-10-23): app ID `io.github.ArcticLatent.ComfyPilot` (`APP_ID`; data
folder, keychain service, desktop entry, single-instance socket), crate `comfypilot`,
binary `ComfyPilot`, log `comfypilot.log`, user agents `ComfyPilot/<version>`. On first
start `config::migrate_legacy_data` copies `config/` and `state/` from the
`io.github.ArcticHelper` data folder (never its cache: installed ComfyUIs may use the
Python runtime there; the old folder is left alone), and saved tokens are copied from the
old keychain service. Packages are `comfypilot` (replacing `arctic-comfyui-helper`),
releases and the update feed are on `ArcticLatent/ComfyPilot`. Still to do: a last
Arctic release pointing existing installs to it. File names inside users' ComfyUI installs
(`arctic-torch-overrides.txt`, `arctic-*` git branches) stay, so existing installs keep
working.

## Notes

- QML warnings go to the systemd journal when stderr isn't a terminal; set
  `QT_FORCE_STDERR_LOGGING=1` (scripts/dev.sh does).
- Download progress: the downloader reports every chunk (thousands a second);
  `spawn_progress_emitter` thins it to one event per file per 150 ms (`ProgressCoalescer`,
  never backwards). Before that, the UI thread's queue grew without limit (gigabytes of
  RAM) and the bars jumped. `Downloads` keeps rows in ListModels so delegates persist.
  Checked with a real 6 GB download: 160–190 MB RSS, cancel immediate.
- Civitai's CDN: on an image, `transcode=true` returns the full-size original (MBs);
  on a video it's what turns the MP4 into a still. `visible_media` uses `anim=false`
  for images and `anim=false,transcode=true` for videos.
- QML's Promise has no `finally`; end chains that have a `.catch` with
  `.then(Api.after(fn))`.
- `-qwindowgeometry 1300x2700` makes a tall window to capture a whole page.
- `--screenshot=<png>` with `QT_QPA_PLATFORM=offscreen` renders the UI without a window;
  `--section=<comfyui|models|loras|workflows>`, `--view=<manage|install>` and
  `--panel=<activity|downloads|comfyui-log|none>`; `--queue=<model id>,…` queues models;
  `--lora=<n>` / `--civitai-workflow=<n>` open the nth LoRA / Civitai workflow card;
  `--workflows-view=<library|civitai>` picks the Workflows view; `--check=<workflow.json>` checks a workflow; `--screenshot-delay=<ms>` waits for images
  (default 6000). Offscreen uses the software renderer, which skips shader effects;
  add `QT_QUICK_BACKEND=rhi QSG_RHI_BACKEND=opengl` to render them. Use a scratch `XDG_CONFIG_HOME` so it doesn't change your window state.
- Qt/QML messages are also copied into `arctic-helper.log`.
- Regenerating the command tables: they were generated once from the Tauri
  `generate_handler!` lists; now they are edited by hand (one arm per command, per platform).
- Fixed on this branch, also needed on `main`: the RAM size was read as KiB but sysinfo
  0.30 returns bytes, so every machine counted as 64 GB+ for model tiers (`src/ram.rs`).

## Reference

The last Tauri frontend is in git (`app-improvements`, `src-tauri/dist/`). Port behavior
from it tab by tab; the command names and payloads are unchanged.
