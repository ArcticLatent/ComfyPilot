# Cross-platform development

> On the `qt-ui` branch the Tauri shell is gone: `src-tauri/` is now `app/`, the web
> frontend is replaced by QML, and commands go through `app/src/commands.rs`. The Linux/
> Windows split and extraction rules below still apply to the backend; the Tauri-specific
> parts (frontend, `tauri.conf.json`, `cargo tauri`, xwin cross-check) are out of date.
> See [qt-migration.md](qt-migration.md).

This repository is the single source of truth for Linux and Windows. The old
`ArcticDownloader-win` checkout is an import source only. The unified repository has
passed its interactive Windows smoke test, so the old repository can be archived after
these consolidation changes are committed and pushed.

## Current layout

| Concern | Linux | Windows | Shared |
| --- | --- | --- | --- |
| Tauri backend | `app/src/app_linux.rs` | `app/src/app_windows.rs` | `main.rs`, `contracts.rs`, `platform.rs`, and `shared.rs` |
| Frontend | `app/dist/` | `app/dist/` | capabilities control platform-specific options |
| Core services | — | — | `src/` |
| Dependencies | target-specific Cargo sections | target-specific Cargo sections | workspace dependency versions and one root `Cargo.lock` |

This boundary deliberately preserves the behavior of both existing applications. It
also lets common code be extracted gradually without blocking day-to-day releases.

The shared frontend is split by responsibility. `main.js` is only the composition
root; `features/bootstrap.js`, `catalog.js`, `comfyui.js`, `gpu-torch.js`,
`event-handlers.js`, and `download-progress.js` own application behavior, while
`lib/app-context.js` owns the shared state and DOM references. Feature factories use
explicit dependency injection and import only `lib/`, so the module graph has no
feature-to-feature cycles.

`jsconfig.json` enables the full TypeScript strict family for the checked JavaScript,
including `noImplicitAny` and unchecked-index validation. `global.d.ts` defines the
narrow Tauri IPC surface, feature-factory contracts, catalog/download domain objects,
and shared application state, including nullable lifecycle values. `npm run
check:frontend` also verifies that every required `byId(...)` reference in
`lib/app-context.js` exists exactly once in `index.html`.

## Daily development

```bash
scripts/dev.sh                                   # build and run, catalog settings from .env
cargo test --locked --lib                        # core crate
cargo test --locked --manifest-path app/Cargo.toml
cargo clippy --locked --manifest-path app/Cargo.toml --all-targets -- -D warnings
```

The root manifest is also the Cargo workspace root. Shared dependency versions live in
`[workspace.dependencies]`; each crate keeps its own feature selection. Run Cargo from
the repository root, including when using `--manifest-path app/Cargo.toml`, so
both crates consistently use the root `Cargo.lock` and workspace `target/` directory.

The Windows code is compiled, linted and tested on GitHub's `windows-latest` runner
(`check-windows.yml`) on every push and pull request; `scripts/windows-vm/` runs a local
Windows VM for interactive checks.

## Where changes go

- Put cross-platform catalog, settings, download, update, RAM, and VRAM logic in `src/`.
- Put OS process handling and installer commands in the matching `app_*.rs` module.
- Keep a command name and payload identical on both platforms when the UI calls it.
- Put UI changes in `app/qml/`; both targets load the same QML.

## Releases

Packages (.deb, .rpm, Arch, Windows installer) are built by `.github/workflows/release.yml`
and released with `scripts/release.sh`; see [releasing.md](releasing.md).

## Update manifest signing

`src/updater.rs` checks a downloaded update package's SHA-256 against the hash in the
manifest -- but until the manifest itself is authenticated, that hash only proves the
download matches *the manifest*, not that the manifest came from a real release rather
than a compromised release publisher, CI runner, or GitHub account. Both
`update.json` (Windows, and the legacy Linux fallback) and `linux-release.json` now carry
an Ed25519 `signature` field over their content, checked in `src/update_signing.rs`
against a public key embedded in the binary. The app refuses to trust a manifest that is
missing a signature or fails verification -- it fails closed, the same way a checksum
mismatch does.

- `src/update_signing.rs` holds the canonical byte-encoding both sides sign/verify
  against (`update_manifest_signing_payload`, `linux_release_manifest_signing_payload`)
  and the embedded public key. This is the only piece of the signing scheme that ships
  in the app, and it only ever verifies.
- `tools/manifest-signer` is a release-time-only CLI (workspace member, never shipped)
  that does the signing. It depends on the root `arctic-downloader` crate for the same
  payload-encoding functions, so the signer and the verifier can never disagree about
  what bytes were actually signed.
  - `manifest-signer keygen` -- generates a new keypair, printed once. Run this only to
    create the very first key or to rotate it.
  - `manifest-signer sign --format <update|linux-release> --manifest <path>` -- reads the
    private key from `ARCTIC_UPDATE_SIGNING_KEY` (base64) and rewrites the manifest in
    place with a `signature` field. Called from `packaging/release-manifests.sh` (the release workflow's publish
    job) right after each manifest is generated.
  - `manifest-signer verify --format <...> --manifest <path> [--pubkey <base64>]` --
    checks a manifest the same way the app does, against the embedded key unless
    `--pubkey` overrides it. Called from `packaging/release-manifests.sh` right after signing, so a
    broken signing key fails the release
    instead of shipping an update the app will silently refuse.
  - `manifest-signer merge-linux-release --base <path> --replacement <path> --output
    <path> [--pubkey <base64>]` -- verifies an existing signed Linux manifest and
    replaces only the package kinds present in the replacement. Not used by
    the current release workflow, which builds every package in one run. The optional
    public key is reserved for isolated pipeline tests with a throwaway keypair.

Rotating the key: run `keygen`, update the `ARCTIC_UPDATE_SIGNING_KEY` secret with the
new private half, and ship a build with `UPDATE_MANIFEST_PUBLIC_KEY_B64` updated to the
new public half *before* the next release is signed with the new key -- otherwise
already-installed clients running the old public key can't verify it. `ARCTIC_UPDATE_PUBLIC_KEY`
is a runtime environment-variable override of the embedded key, for testing the whole
pipeline end to end against a throwaway keypair; it carries the same "requires control of
the machine's environment" caveat as the existing `ARCTIC_UPDATE_MANIFEST_URL` override.

## Consolidation status

The cross-platform boundary is complete for the 0.2.6 release. Shared contracts and
behavior live in `contracts.rs`, `shared.rs`, and `platform.rs`; Linux and Windows keep
only behavior whose operating-system semantics genuinely differ. Further movement is a
readability choice, not required platform parity work.

1. Extract matching Tauri commands and DTOs into small shared modules. **Complete**:
   `app/src/contracts.rs` owns the common application snapshots, update/preflight
   responses, install request, and attention-change request payloads. This prevents the
   frontend wire format from drifting between platform builds. `app/src/shared.rs`
   holds pure, side-effect-free helpers verified identical between `app_linux.rs`
   and `app_windows.rs` before moving — install-folder/state bookkeeping
   (`InstallState`, `write_install_state`, `find_in_progress_install`,
   `choose_install_folder`, `detect_existing_comfyui_root`,
   `is_recoverable_preclone_dir`, `clear_directory_contents`), custom-node path
   helpers, version/launch-arg/YAML string parsing, runtime event emission
   (`emit_comfyui_runtime_event` and friends, plus the `ComfyRuntimeEvent`/
   `DownloadProgressEvent`/etc. payload structs), the AMD/Intel GPU-detail caches,
   the model-artifact selection-key filtering used by batch downloads, and — as
   of the most recent passes — **`AppState` itself**, plus the settings/catalog/
   runtime-status commands and process-liveness checks built on it
   (`get_settings`, `get_catalog`, `save_civitai_token`, `cancel_comfyui_install`,
   `get_comfyui_runtime_status`, `comfyui_process_running`,
   `comfyui_external_running`, `comfyui_runtime_running`, `wait_for_comfyui_start`,
   `set_comfyui_custom_launch_args`, `set_comfyui_gpu_selection`,
   `set_comfyui_show_runtime_logs`). The three `set_comfyui_*` settings commands
   followed once `AppState` was shared — same shape as the others: pure
   `state.context.config.update_settings(...)` calls with no divergent callees
   (the sibling `set_comfyui_extra_model_config`/`get_comfyui_extra_model_config`/
   `get_effective_download_destination` were checked too and excluded, since they
   call `resolve_root_path`, confirmed `DIFFERENT`).
   `AppState` turned out to be a plain data struct with no `impl` block and no
   platform-specific fields — the type-identity barrier that had blocked sharing
   any `State<'_, AppState>`-taking function was artificial, not a real platform
   difference, once actually checked. `kill_managed_comfyui_child` — the "kill the
   child process this app itself spawned" half of `stop_comfyui_root_impl` — was
   pulled out the same way: byte-identical on both sides, even though the rest of
   that function (finding and killing an orphaned/externally-running listener)
   is genuinely platform-specific. See step 3 for where the rest of process
   start/stop orchestration landed.
   Cross-module `#[tauri::command]`s (e.g. `get_settings`) are
   registered in `generate_handler![...]` via their full path
   (`crate::shared::get_settings`) — the `#[tauri::command]` macro's generated
   dispatch item isn't brought into scope by a plain `use` of the function name,
   only by the qualified path or a matching `use` of the hidden macro item; the
   qualified path is the pattern used consistently here. Three rules keep the
   overall extraction safe:
   - Functions that call into something with (or likely to grow) platform-specific
     behavior — e.g. `normalize_path`, `command_available`, `python_for_root` — are
     deliberately left in place rather than extracted, since a shared function
     silently depending on one platform's semantics is worse than the duplication
     it removes.
   - **Before extracting a function, check for `#[cfg(target_os = ...)]`-gated
     duplicate definitions of the same name in the file you're pulling from.** A
     first pass at this nearly moved `kill_python_processes_for_root` as
     "identical" because a naive text scan matched Windows'
     `#[cfg(not(target_os = "windows"))]` no-op stub instead of the real
     `#[cfg(target_os = "windows")]` PowerShell implementation sitting right
     above it — grep for the function name and confirm exactly one definition
     exists on each side before trusting a diff. (That investigation did surface
     a real gap: Linux's `kill_python_processes_for_root` was a no-op. It now
     targets only the selected installation's exact virtual-environment interpreter,
     sends `TERM`, and escalates to `KILL` if necessary before reinstall/uninstall.)
   - **Before extracting a struct, read its full current field list on both sides
     by eye — don't trust a script's diff.** A second pass nearly unified
     `NvidiaGpuDetails`, but Windows' version is missing the `compute_capability`
     field that Linux's has (used for Hopper/SM90 detection) — a real, long-standing
     platform difference. An automated text-diff of the two struct bodies reported
     them as identical (a tooling bug, not a judgment call), and only a direct
     side-by-side read caught it before the struct — and the accessor function
     built on it — got merged. `AmdGpuDetails`/`IntelGpuDetails` *are* genuinely
     identical and did get extracted; `NvidiaGpuDetails`/`gpu_details_cache` stay
     platform-local, with a comment on the Linux struct explaining why.
2. Move GPU detection into `gpu/linux.rs` and `gpu/windows.rs` behind one interface.
   **Investigated and re-scoped.** Splitting GPU detection into dedicated per-platform
   files turned out not to be the right move once the actual code was read closely:
   the detection logic (`query_{nvidia,amd,intel}_gpu_details_blocking` and the
   `detect_{nvidia,amd,intel}_gpu_details` caching wrappers around them) differs by
   platform in more than just the system call. The caching/retry semantics are
   genuinely different — Linux tracks a separate "probe complete" flag and gives up
   permanently after one failed probe; Windows has no such flag and resets its
   "probe started" flag on failure so a later call retries indefinitely. Relocating
   ~150-300 lines of already-divergent-in-substance code per platform into new files
   wouldn't reduce duplication, just move it, and two files that only coincidentally
   share a directory aren't "one interface" in any real sense.
   What *is* identical, and now lives in `shared.rs` as adapter-callers over
   `detect_{nvidia,amd,intel}_gpu_details` (added to the existing adapter re-export
   list from step 3, rather than a new module): `detect_nvidia_gpu`,
   `detect_amd_gpu_name`, `detect_intel_gpu_name` — thin wrappers that add nothing
   but a fake-GPU testing hook and a tuple/field reshape. This is the "one interface"
   the roadmap item was after; `shared.rs` already *is* that interface, so a
   dedicated `gpu/` module would have been the same thing with extra ceremony.
   `gpu_detection_pending` and `is_nvidia_hopper_sm90` were checked too and are
   Linux-only with no Windows counterpart at all — not candidates, just unique code.

   **Separately, readability-only**: the old ~6,800/~6,100-line platform files have been
   reduced to roughly 3,600/2,500 lines and are backed by focused sibling modules. This
   roadmap item originally asked "should GPU detection be shared
   between platforms" (no) rather than "should Linux's own GPU detection live in its own
   file within `app_linux/`" (yes, and it doesn't conflict with the answer above). The
   Linux-only GPU-probing/caching block above -- `NvidiaGpuDetails`,
   `detect_{nvidia,amd,intel}_gpu_details`, the `query_*_gpu_details_blocking` functions,
   `gpu_detection_pending`, `is_nvidia_hopper_sm90`, and the two
   `fake_*_allow_*_setup_enabled` test hooks -- moved into `app_linux/gpu_detection.rs`
   as the first slice of this split, using Rust's 2018+ file-plus-sibling-directory
   module convention (`app_linux.rs` stays as the module file; `app_linux/gpu_detection.rs`
   is a submodule of it, no `mod.rs` rename needed). `app_linux.rs` re-exports
   everything the rest of the file and `platform.rs` reference
   (`pub(crate) use gpu_detection::{...};`), so no call site outside the two files
   changed. One thing this surfaced: `driver_version`/`compute_capability` on
   `NvidiaGpuDetails` were module-private, which was invisible as long as the struct and
   its only other users (`comfy_install_recommendation_for` and its tests) shared one
   file; splitting made that boundary real, so those two fields became `pub(crate)`
   alongside `name`/`vram_mb` rather than newly inaccessible. The pre-existing
   `#[cfg(test)] mod gpu_detection_tests` was itself a grab-bag, not purely about GPU
   detection -- only one of its five tests actually tested detection code
   (`find_nvidia_gpu_name_in_lspci`); that one moved with the code it tests, and the
   remaining four (install-recommendation, torch-profile, HF-Xet-preflight tests that
   merely take GPU details as input) stayed in `app_linux.rs` under the more accurate
   name `install_recommendation_tests`.

   **Second slice**: tray icon/menu/status handling (`TrayMenuItems`, `setup_tray`,
   `update_tray_comfy_status`, `stopped_tray_icon`/`started_tray_icon`,
   `tray_enabled_for_platform`) moved into `app_linux/tray.rs` the same way. Two things
   worth noting for the next slice:
   - `include_bytes!("../icons/...")` paths had to become `../../icons/...` --
     one more directory deep from the new file. Easy to get wrong silently (a
     missing icon degrades to a fallback rather than a compile error in some of
     these call sites), so this is worth specifically checking, not just trusting
     `cargo check` on any future `include_bytes!`/`include_str!` move.
   - Both feature configurations need checking, not just the default one:
     `cargo check`/`clippy --no-default-features` (the `desktop-tray` feature off)
     caught unused imports and a `clippy::needless_return` that
     `--all-targets` alone (default features) didn't, because the
     `#[cfg(not(feature = "desktop-tray"))]` no-op variants of
     `update_tray_comfy_status`/`setup_tray` only compile in that configuration.
     Neither CI workflow currently runs `--no-default-features` -- this was
     caught by hand, not by the existing checks. **Closed for Linux**:
     `check-linux.yml` now runs check/clippy/test with `--no-default-features`
     as well. **Not added for Windows**, and this isn't a gap to close the same
     way: trying the identical flag through `cargo xwin check` immediately
     failed with `unresolved import tauri::tray` -- `app_windows.rs`'s tray
     code has no `#[cfg(feature = "desktop-tray")]` gating at all, unlike
     Linux's. That's a real, pre-existing platform asymmetry (Windows always
     ships tray support; only Linux/Flatpak builds without it), not something
     introduced by this split and not something to paper over by adding a CI
     check that would just fail. If Windows is ever meant to support running
     without tray support, that needs the same `#[cfg(not(feature =
     "desktop-tray"))]` treatment Linux already has -- real work, not a CI
     wiring change -- and is out of scope here.

   **Windows now has both slices too**, done as paired extractions rather than
   finishing Linux end-to-end first: `app_windows/gpu_detection.rs` and
   `app_windows/tray.rs`, mirroring `app_linux/`'s. Real differences worth
   recording, not glossed over:
   - Windows' `NvidiaGpuDetails` has no `compute_capability` field (already
     known -- see the struct-diffing note earlier in this section -- and
     confirmed again here rather than "fixed").
     *Update (2026-10-22):* Windows now queries it too, for CUDA 13 (cu130)
     eligibility (`shared::nvidia_supports_cu130`), so the fields match; the
     probes behind them still differ, so the structs stay per platform.
   - Windows has no `gpu_detection_pending`/`is_nvidia_hopper_sm90`/
     `fake_*_allow_*_setup_enabled` functions at all; `AppSnapshot`'s
     `gpu_detection_pending` field is computed inline
     (`nvidia.is_none() && amd.is_none() && intel.is_none()`) rather than
     from dedicated probe-flag statics.
   - `windows_rocm_supported_gpu`/`windows_xpu_supported_gpu` stayed in
     `app_windows.rs`, not `gpu_detection.rs` -- pure GPU-name string
     matching for install-profile selection, not detection/probing, the same
     distinction that kept Linux's `comfy_install_recommendation_for` out of
     `app_linux/gpu_detection.rs`.
   - Windows' tray code has no `stopped_tray_icon` (it falls back to
     `app.default_window_icon()` directly) and no `tray_enabled_for_platform`
     (tray is unconditional -- see the `--no-default-features` note above).
   - `NvidiaGpuDetails` isn't re-exported from `app_windows.rs` the way it is
     from `app_linux.rs`: nothing outside `gpu_detection.rs` names the type
     directly on Windows (only Linux's tests construct it as a literal), so
     re-exporting it would just be an unused-import warning.
   - Verification differs by necessity: Windows changes were checked with
     `cargo xwin check`/`clippy` (cross-compiled from Linux, per "Check the
     Windows build from Linux" above) since there's no way to run
     `cargo test` for the Windows target from this environment -- the actual
     Windows test suite only runs in `check-windows.yml` on a real
     `windows-latest` runner.

   **Third slice, deliberately narrow**: "install/custom-node management" turned out to
   span most of both files once actually measured (~5,200 of `app_linux.rs`'s 6,558
   lines; ~5,000 of `app_windows.rs`'s 5,831), scattered non-contiguously through each
   file rather than sitting in one place -- this is what the step-3 note below already
   found for install specifically ("genuine divergence accumulated over time, ... needs
   real design work"). Moving all of it in one pass was rejected as too large a single
   change to verify safely. What *did* move, to `app_*/custom_nodes.rs`: the built-in
   `CUSTOM_NODES` table, `custom_node_spec`, `install_custom_node` (the generic
   git-clone-plus-`pip install`-plus-`install.py` sequence), and
   `install_named_custom_node`. This is the one piece of "install" that's genuinely
   self-contained -- called from both the fresh-install path and the individual
   enable/disable toggle, but doesn't itself reach back into either orchestrator. The
   bespoke per-addon installers (InsightFace, Trellis2, Nunchaku -- each with its own
   wheel-selection/GPU-branching logic) and the two orchestrators
   (`run_comfyui_install[_linux]`, `apply_comfyui_component_toggle`) stay put for now.
   One real, pre-existing behavioral difference this surfaced (already flagged in a
   comment at the Windows definition, not something this pass introduced): Windows'
   `install_custom_node` re-pins the `requests`/`urllib3`/`charset_normalizer`/`idna`
   dependency stack (`reassert_requests_dependency_stack`) after every custom-node
   install; Linux's does not.

   **Fourth slice**: the per-addon installers themselves, to `app_*/addons.rs`. Linux:
   `linux_wheel_url` (the huge profile/wheel-kind/Hopper match table), `install_linux_wheel_for_profile`,
   `install_sageattention_linux`, `install_flashattention_linux`, `install_nunchaku_node_requirements`,
   `install_insightface`, `uninstall_insightface`, `install_trellis2`, `uninstall_trellis2`, plus three
   helpers (`prewarm_matplotlib_cache`, `insightface_present`,
   `remove_insightface_site_packages_artifacts`) that turned out, once checked, to be called
   from nowhere outside this set -- moved in as module-private rather than left `pub(crate)`
   in the parent for a single caller. Windows: the equivalent
   `install_insightface`/`install_insightface_variant`/`ensure_insightface_runtime_compat`/
   `cleanup_tilde_site_packages`/`finalize_nunchaku_install`/`install_nunchaku_node_requirements`/
   `uninstall_insightface`/`install_trellis2`/`uninstall_trellis2`.
   **This is where "not a mirror" stopped being a caveat and became the headline finding**:
   Windows' InsightFace path is roughly 3x the code of Linux's for the same feature. Linux
   installs InsightFace from one precompiled wheel keyed by torch profile and Hopper-ness and
   is done (`install_linux_wheel_for_profile`, shared with the attention-backend wheels).
   Windows has no such wheel, so it pip-installs from source with an MSVC Build Tools
   install-and-retry fallback (`install_insightface_variant`, `looks_like_missing_msvc_tools`,
   left in the parent -- general toolchain concern, not addon-specific) and then a
   numpy/opencv ABI-mismatch retry loop (`ensure_insightface_runtime_compat`) that Linux has
   no equivalent of at all. Trellis2 pulls from a different upstream repo per platform
   (`ArcticLatent/ComfyUI-TRELLIS2` + two more repos on Linux; `visualbruno/ComfyUI-Trellis2`
   alone on Windows) with different prebuilt wheels. None of this was unified -- moving code
   into a same-named file on each platform is not the same claim as the two files agreeing on
   what the file contains, and this pair doesn't.
   One privacy mechanic worth noting since it wasn't obvious going in: a plain (unmarked,
   private) `fn` defined in `app_linux.rs`/`app_windows.rs` is already visible to
   `app_linux::addons`/`app_windows::addons` without adding `pub(crate)` -- Rust's privacy
   rule is "visible to the defining module and all its descendants," and a `mod addons;`
   declared inside `app_linux.rs` makes `addons` a descendant, not a sibling, of `app_linux`.
   Every general-purpose helper this slice depends on (`discover_uv_binary`,
   `run_uv_pip_strict`, `profile_from_torch_env`, `clone_or_update_repo`, `pip_has_package`,
   etc. on Linux; `run_uv_pip_strict`, `uv_pip_uninstall_best_effort`, `download_http_file`,
   etc. on Windows) stayed at whatever visibility it already had and just worked via
   `use super::{...}` -- confirmed empirically (both platforms compiled clean on the first
   attempt after writing the new files), not just reasoned about after the fact.

   **Fifth slice**: install-location management and state-reporting, to
   `app_*/install_state.rs` -- `set_comfyui_install_base`, `list_comfyui_installations`,
   `get_comfyui_addon_state` (plus their private `ComfyInstallationEntry`/`ComfyAddonState`
   response structs, each used by nothing else). Distinct from `addons.rs`
   (installing/uninstalling a specific addon) and from the orchestrators that remain in
   `app_*.rs`: these three only read or record state, they don't mutate an installation.
   Grep distance between these functions in the original file was misleading before
   actually reading it -- `list_comfyui_installations` looked ~750 lines away from its
   neighbors by line-number subtraction, but that gap was almost entirely unrelated
   interleaved code (`resolve_root_path`, Python-venv discovery, GitHub release checks,
   process-killing helpers -- runtime-plumbing candidates for a future slice), not the
   function itself, which is ~70 lines. Don't trust `grep -n` distance as a size estimate
   in these files; read the actual boundaries.
   **New mechanic hit for the first time this slice, not seen in the first four**: all
   three are `#[tauri::command]`s referenced by bare name in `generate_handler!` inside
   `run()`. `docs` already recorded, for the `shared.rs` extractions, that the
   `#[tauri::command]` macro's generated dispatch item isn't brought into scope by a plain
   `use` of the function name -- only the qualified path resolves it. That rule turned out
   to apply identically to a parent-to-submodule move like this one, not just cross-module:
   `generate_handler!` entries became `install_state::set_comfyui_install_base` (etc.)
   rather than a bare name relying on a `pub(crate) use` re-export. Since nothing else in
   either file calls these three by name (each command's own definition was its only
   occurrence before the move), no re-export was added at all -- confirmed compiling clean
   on the first attempt on both platforms once the qualified paths were in place.
   Real divergence preserved, not unified: Windows normalizes paths with
   `strip_windows_verbatim_prefix` (stripping the `\\?\` prefix `std::fs::canonicalize`
   adds on Windows) everywhere Linux uses `normalize_canonical_path`, and
   `get_comfyui_addon_state` checks `pip_has_package` in addition to
   `python_module_importable` for sage/flash presence on Windows, where Linux's checks
   that only for nunchaku.

   **Sixth slice**: ComfyUI runtime start/stop plumbing itself, to `app_*/runtime.rs` --
   `resolve_root_path`, `start_comfyui_root_impl`, `spawn_comfyui_start_monitor`,
   `spawn_comfyui_runtime_log_stream`, Python-interpreter discovery
   (`python_for_root`/`python_exe_candidates_for_root`/`python_exe_works`/
   `resolve_start_python_exe`/`python_exe_for_root`), `git_latest_release_tag` (+ Linux's
   GitHub-API fallback chain: `GithubTagEntry`/`comfyui_origin_github_repo`/
   `parse_github_repo_from_url`/`github_latest_release_tag`), process-killing
   (`kill_python_processes_for_root`, plus Linux-only `comfyui_listener_running`/
   `host_comfyui_running_for_needle`/`signal_host_pids`/`kill_host_comfyui_for_root` --
   Windows kills by port/PowerShell process match instead, no equivalent helper set), and
   `restart_comfyui_after_mutation`. `pip_has_package` stayed in the parent on both
   platforms despite sitting in the middle of this code in the original file: it's already
   consumed by `install_state.rs`/`addons.rs` via `use super::pip_has_package;`, and
   nothing in this slice calls it, so moving it would only have added churn to two
   already-verified files for no benefit -- not every function physically inside a
   slice's line range belongs in the slice.
   **The cfg-gated-duplicate trap the roadmap already warned about (see step 1's "before
   extracting a function" rule) showed up for real here**, not just as a documented risk:
   Windows' `kill_python_processes_for_root` has two definitions --
   `#[cfg(target_os = "windows")]` (a real PowerShell `Get-CimInstance Win32_Process`
   implementation matching by executable path or command-line substring) and
   `#[cfg(not(target_os = "windows"))]` (an `Ok(false)` no-op, presumably a defensive stub
   for this Windows-only file somehow being compiled elsewhere). `grep -n "^fn
   kill_python_processes_for_root"` surfaces both immediately if you search for the exact
   function name rather than trusting a single earlier match -- both moved together with
   their original `#[cfg(...)]` attributes intact, rather than either the trap (silently
   keeping only the no-op) or overcorrecting (merging them into one).
   Real divergence preserved, not unified: Windows' `resolve_start_python_exe` self-heals
   by bootstrapping a uv-managed Python runtime when no working interpreter is found;
   Linux's just errors. Windows' `restart_comfyui_after_mutation` doesn't wait for ComfyUI
   to finish starting before updating tray status; Linux's does. Windows has no GitHub-API
   fallback for `git_latest_release_tag` at all -- only `git ls-remote`.
   All six `#[tauri::command]`-registration/`platform.rs`-cross-reference/visibility
   mechanics from the last two slices repeated cleanly here with no new surprises: both
   platforms compiled with only unused-import warnings (no errors) on the first attempt
   after writing each new file, fixed by trimming now-redundant entries from each parent's
   top-level `use` blocks.

   **Seventh and final slice**: the install/toggle orchestrators themselves, to
   `app_*/install.rs` -- `run_comfyui_install`/`run_comfyui_install_linux`,
   `start_comfyui_install`, `apply_comfyui_component_toggle`. This was the piece flagged
   repeatedly, in this doc and out loud while working through the rest, as needing
   "investigate first, maybe don't extract" rather than an assumed-easy mechanical move --
   it's exactly the code step 3 below already found to be "genuine divergence accumulated
   over time, not surface duplication" when it was checked for a *shared* extraction. That
   finding was about whether the two platforms' logic could become *one* implementation
   (no); it was never a finding about whether each platform's *own* version could still
   move into its own file, which is the only thing every slice in this list has ever done.
   Once actually read end to end on both sides (not skimmed), the same-platform move turned
   out to carry the same mechanical risk as every other slice and no more -- large, but not
   entangled with anything outside itself.
   **Scale, to calibrate against the other six slices**: `run_comfyui_install` alone is
   ~510 lines on Linux and ~650 on Windows -- bigger than any single function extracted
   so far, and this doc's own earlier slice already got the "grep distance is not the same
   as function size" lesson backwards once (assumed small, was large); here it's the
   opposite miscalibration risk (assumed unmanageably large, turned out to be one cleanly
   bounded function per platform, not scattered).
   Divergence reconfirmed by direct reading, not by trusting the standing claim: Windows'
   `run_comfyui_install` validates SageAttention3 against RTX 50-series GPUs, branches on
   ROCm/XPU/CUDA torch stacks, migrates a legacy nested `ComfyUI/ComfyUI` layout, and
   writes its own `install.log` -- none of which Linux's version does at all. Windows'
   `start_comfyui_install` uses `spawn_blocking` (with a comment explaining exactly why:
   the install function is long-running and fully synchronous, so `spawn` would starve
   other async work on the runtime's worker threads); Linux's uses plain `spawn` and never
   needed that reasoning written down because the difference was never noticed as a
   difference until this pass read both side by side. Windows never sets
   `comfyui_torch_profile` after a fresh install; Linux always does. Windows'
   `selected_attention_backend` returns `Option<&str>`; Linux's returns `&str` (both
   already on record from step 3, reconfirmed here). Windows' `apply_comfyui_component_toggle`
   additionally validates CUDA-only addons against ROCm/XPU profiles and re-checks the
   Trellis2 torch-profile requirement at toggle time; Linux's toggle path has neither check.
   None of this was touched -- every difference above is preserved verbatim in the new files.
   Same `#[tauri::command]`-qualified-path mechanic as the last two slices, applied to two
   more commands per platform; no new mechanics needed. Both platforms compiled with only
   unused-import warnings (no errors) on the first attempt after writing each new file.

   **Eighth slice, a targeted fix plus one more extraction, done after the "seven-piece
   shape" above was declared complete**: two things surfaced once the top-level files were
   actually re-read rather than assumed finished.
   First, an oversight in the sixth slice: `start_comfyui_root_impl` had moved to
   `runtime.rs` but its counterpart `stop_comfyui_root_impl` had not, left behind in each
   top-level file with a stale re-export comment implying otherwise. Fixed by moving both
   platforms' `stop_comfyui_root_impl` (and Windows' single-caller
   `kill_listener_process_on_port` helper, `#[cfg(target_os = "windows")]`) into
   `runtime.rs` alongside their `start_*` counterpart, updating each parent's
   `pub(crate) use runtime::{...}` list.
   Second, the torch/Python environment utility group, to `app_*/torch_env.rs` --
   uv/pip plumbing, attention-backend package install/cleanup, torch-profile
   detection/enforcement, CUDA runtime library-path discovery (Linux) or local-uv
   bootstrap/download (Windows), and the launch-arg/env assembly consumed when starting
   ComfyUI. This is core shared infrastructure depended on by every other slice above, not
   a new orchestrator -- moving it was mechanical (confirm callers, mark `pub(crate)`,
   re-export, verify) even though the group itself is large (~30 functions, ~700 lines on
   Linux).
   **First real asymmetry in *shape* between the two platforms' slices, not just
   content**: every prior slice's Linux and Windows halves lived in one contiguous block
   each. This one didn't on Windows -- `ensure_uv_python_installed` through
   `comfyui_launch_args` was one contiguous ~700-line block as expected, but
   `python_module_importable`, `python_module_import_error`, and `nunchaku_backend_present`
   lived ~450 lines further down, separated by several unrelated `#[tauri::command]`s
   (`set_comfyui_root`, `check_updates_now`, `download_lora_asset`, `open_folder`, etc.)
   and by `pip_has_package` (which, like Linux's `pip_has_package`, deliberately stayed in
   the parent rather than moving -- nothing in this module calls it, and it's already
   consumed elsewhere via `use super::pip_has_package;`). All three scattered functions
   moved into the same `torch_env.rs` as the contiguous block regardless, since that's
   where they belong logically and Linux's equivalents live there too; physical distance
   in the original file was not a reason to split them into a second module.
   **`python_module_import_error` looked like dead code on first grep** (zero call sites
   in `app_windows.rs` itself) **until the submodule search widened to `app_windows/*.rs`**,
   which found it consumed by `addons.rs` via `use super::{...}` -- a reminder that
   "unused" has to be checked crate-wide, not just within the file being read, especially
   once a directory-module split like this one is already in place and half the callers
   live one level down.
   Two visibility fixes needed once compiled: `apply_torch_allocator_env_compat` (a
   `shared.rs` function) was being re-exported into `runtime.rs` transitively through the
   parent's own `use crate::shared::{...}` block rather than imported directly -- moving
   the torch/Python group made that block's copy unused, which would have broken
   `runtime.rs`'s `use super::apply_torch_allocator_env_compat;` if left as a dangling
   re-export; fixed by having `runtime.rs` import it straight from `crate::shared` instead,
   on both platforms. And `ComfyInstallRequest` (defined in each top-level file, previously
   plain `struct`) needed to become `pub(crate) struct` on both platforms, since
   `selected_attention_backend` -- itself needing `pub(crate)` for cross-module callers --
   takes it by reference and Rust's `private_interfaces` lint flags a public-enough function
   exposing a less-visible type in its signature.
   Divergence reconfirmed by direct reading, not touched: Windows' `torch_profile_from_versions`
   takes four version strings (torch/cuda/hip/xpu) where Linux's takes two (cuda/hip/xpu
   already folded into one field by the caller); Windows' `selected_attention_backend`
   returns `Option<&'static str>` where Linux's returns `&'static str` (both already on
   record from the third and seventh slices, reconfirmed here); Windows has no equivalent
   of Linux's `force_cleanup_attention_backends`/`remove_site_packages_artifacts_with_markers`
   site-packages sweep at all; and Windows carries an entire ROCm/XPU non-uv install path
   (`install_windows_rocm_torch_stack`, `install_windows_xpu_torch_stack`,
   `windows_rocm_sdk_*_packages`) that Linux has no counterpart for -- Linux's ROCm/XPU
   support is just a different `torch_profile_to_packages_linux` index URL, still installed
   through the normal uv path.

   **This closes out the roadmap items originally listed as remaining candidates.** Both
   `app_linux.rs` and `app_windows.rs` have gone from one undifferentiated file each to a
   directory module with the same eight-module core: `gpu_detection.rs`, `tray.rs`,
   `custom_nodes.rs`, `addons.rs`, `install_state.rs`, `runtime.rs`, `install.rs`,
   `torch_env.rs`. Linux additionally isolates GTK/icon integration in `desktop.rs`;
   Windows isolates Job Object lifetime handling in `process_guard.rs`. The top-level
   files retain `run()`/`generate_handler!` wiring, small commands, and cohesive workflows
   that remain platform-specific (Linux distro/guided GPU setup and Windows host tooling).
   Those are intentionally not forced behind a shared abstraction.
3. Move ComfyUI install/runtime operations into platform adapters behind shared commands.
   **Complete at the useful adapter boundary.** Install was investigated and found to
   have genuine platform semantics that should remain separate (see below). The runtime
   orchestration functions
   (`start_comfyui_root`, `start_comfyui_root_background`, `stop_comfyui_root`,
   `stop_comfyui_for_mutation`, `resolve_comfyui_instance_name`) are byte-identical on
   both platforms and now live in `shared.rs`. They call down into leaf functions that
   are genuinely platform-specific (`resolve_root_path`, `start_comfyui_root_impl`,
   `stop_comfyui_root_impl`, `update_tray_comfy_status`, `spawn_comfyui_start_monitor`),
   which stay defined in `app_linux.rs`/`app_windows.rs` exactly as they were — only
   widened from private to `pub(crate)` — and are exposed through `platform.rs` via a
   compile-time Linux/Windows re-export pair. Shared orchestration imports only that
   boundary rather than selecting an application module itself.
   This is the "platform adapter" the roadmap item names: since only one platform is
   ever compiled into a given binary, a `#[cfg]`-gated re-export is the right-sized
   substitute for a runtime trait object here — no dynamic dispatch needed, and it's
   naturally immune to the `#[cfg]`-gated-duplicate trap from step 1 (a name is
   imported, never copied, so it resolves correctly no matter how many `#[cfg]`
   variants of it exist on the platform side — `update_tray_comfy_status` has two).
   Two things confirmed genuinely different and left alone during this pass:
   - `spawn_comfyui_start_monitor` remains platform-local, but both implementations now
     open `http://127.0.0.1:8188` after startup. This closes the Windows parity gap found
     during the VM smoke test.
   - Install (`run_comfyui_install`/`run_comfyui_install_linux`, the async
     `start_comfyui_install` command, and all `install_*` add-on/custom-node helpers)
     was checked and does **not** fit the adapter pattern this round: even the outer
     command wrapper has real behavioral differences (Linux always persists
     `comfyui_torch_profile`; Windows doesn't set it in the same place, and
     `selected_attention_backend` has a different return type — `&str` on Linux vs.
     `Option<&str>` on Windows). This is genuine divergence accumulated over time, not
     surface duplication. It is intentionally left split rather than hidden behind a
     misleading common implementation.
   - **Follow-up done**: `get_comfyui_extra_model_config`, `get_effective_download_destination`,
     `set_comfyui_extra_model_config`, and the pure `normalize_optional_path` wrapper
     (previously blocked only by `normalize_path` having no adapter) now live in
     `shared.rs` too, using the same `resolve_root_path`-adapter treatment plus three
     new adapters: `normalize_path`, `comfy_extra_model_config`, `effective_download_root`,
     `write_extra_model_paths_yaml`. Each divergence was real but small (e.g.
     `effective_download_root` differs only by one `log::info!` call Linux has and
     Windows doesn't; `write_extra_model_paths_yaml` differs in path-normalization call
     and uses a `Vec::join` vs. a raw `format!` string for the same YAML output) —
     confirmed by direct read, including the `ComfyExtraModelConfig`/
     `ComfyExtraModelConfigResponse`/`EffectiveDownloadDestinationResponse` struct
     bodies *and* their derive attributes line-by-line (a derive mismatch would compile
     fine right up until some caller needed the missing trait, so it doesn't show up
     as a clean pass/fail the way a missing field does).
   - **Catalog/download commands, done as a follow-up sweep** (not one of the original
     5 roadmap items, but the same pattern): `refresh_catalog`, `get_lora_metadata`,
     `download_model_assets`, `download_model_assets_batch`, `download_workflow_asset`,
     `get_comfyui_update_status` (+`_blocking`), `get_comfyui_resume_state`,
     `model_artifacts_for_download_request`, `spawn_progress_emitter`,
     `git_commit_for_ref`, `git_current_branch` now live in `shared.rs`, using two new
     adapters (`run_command_capture`, `git_latest_release_tag`) plus the existing ones.
     **A real process mistake surfaced here, not just a near-miss**: an early
     diff script used `fn NAME(` as its match pattern and silently skipped every
     `async fn` declaration (matching nothing on either side, so empty diffed against
     empty read as "identical"). Every one of these functions is `async fn`. This
     produced a false "all verified identical" claim that got stated as fact before
     being caught — the fix wasn't a better safety net, it was re-running the exact
     same check with the pattern actually matching `async fn` too, which immediately
     turned up real, substantive differences in half the batch (`check_updates_now`
     and `auto_update_startup` both have a Flatpak/package-manager early-return Linux
     has and Windows doesn't; `download_lora_asset`, `apply_comfyui_component_toggle`,
     `update_selected_comfyui`, and `start_comfyui_install` all have real platform-specific
     logic beyond styling). Excluded, not moved. The lesson isn't "add more checks" —
     it's that a verification script's silence (no diff output) must be confirmed to
     mean "compared and found equal," never assumed, especially before it's repeated
     back as a finding.
4. Add a `get_platform_capabilities` command and merge the two frontends into one `dist/`.
   **Complete and smoke-tested in the Windows VM.** Both targets expose typed
   platform/GPU/Torch capability data, and `tauri.windows.conf.json` points to the shared
   `dist/`. The temporary rollback frontend in `dist-windows/` has been removed.
5. Run one interactive Windows smoke test, then archive `ArcticDownloader-win` read-only.
   **Smoke test complete.** Installation, model downloads, ComfyUI startup, Manager,
   platform-aware GPU/Torch selection, and the shared frontend were exercised in the
   Windows VM. After the unified branch is published, archiving the old checkout is the
   remaining manual cleanup action.

Each extraction should preserve command names and include a Linux test plus a Windows
compile check. This keeps releases available throughout the migration.
