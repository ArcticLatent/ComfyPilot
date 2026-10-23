import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The ComfyUI section: manage the selected install, or install a new one.
// Holds the section's state and rules (ported from the Tauri UI's
// comfyui.js / gpu-torch.js); ComfyManageView and ComfyInstallView draw it.
Rectangle {
    id: page

    /// The window (settings, installs, snapshot, log, refresh helpers).
    property var app
    /// "manage" (the selected install) or "install" (a new one).
    property string view: "manage"
    // ---- state ---------------------------------------------------------------
    property var capabilities: null
    readonly property var torchProfiles: capabilities ? capabilities.torch_profiles || [] : []
    readonly property string platformName: capabilities && capabilities.platform === "windows" ? "Windows" : "Linux"
    readonly property string root: app && app.settings ? app.settings.comfyui_root || "" : ""
    readonly property string installBase: app && app.settings ? app.settings.comfyui_install_base || "" : ""
    readonly property var install: app ? app.selectedInstall() : null
    readonly property bool hasInstall: install !== null
    /// The selected install's add-ons, flags and torch stack (get_comfyui_addon_state).
    property var addonState: null
    property bool addonStateLoading: false
    /// What a new install will get.
    property var choices: defaultChoices()
    property string gpuSelection: "auto"
    property var recommendation: null
    property string torchProfile: ""
    /// The torch profile was picked by hand (the recommendation leaves it alone).
    property bool torchLocked: false
    /// The selected install's ComfyUI is running.
    property bool running: false
    /// The install whose ComfyUI is running ("" when none). When the backend
    /// can't tell which (a ComfyUI it didn't start, on Windows), the selected one.
    property string runningRoot: ""
    /// Another install's ComfyUI is running (it holds the port).
    readonly property bool otherRunning: runningRoot !== "" && !samePath(runningRoot, root)
    /// A ComfyUI is starting; `startingHere`: the selected install's.
    property bool starting: false
    property string startingRoot: ""
    readonly property bool startingHere: starting && samePath(startingRoot, root)
    property string runtimeUrl: ""
    property var updateStatus: null
    property bool updateChecking: false
    property bool updateBusy: false
    /// The option being applied ("" when none).
    property string busyKey: ""
    property bool installBusy: false
    property var preflight: null
    property bool preflightRunning: false
    property var resumeState: null
    property string sharedModelsPath: ""
    property bool sharedModelsDefault: false
    property var guidedStatus: null
    property bool guidedBusy: false
    // ---- option definitions --------------------------------------------------
    // kind: attention (one at a time), attentionFlag (one at a time),
    // component (installed/removed), setting (a launch setting; applies at
    // once in both views).
    readonly property var addonDefs: [{
        "key": "sage",
        "label": "SageAttention",
        "kind": "attention",
        "backend": "sage",
        "state": "sage_attention",
        "description": "Faster, lighter attention on NVIDIA GPUs, with the same results."
    }, {
        "key": "sage3",
        "label": "SageAttention3",
        "kind": "attention",
        "backend": "sage3",
        "state": "sage_attention3",
        "description": "SageAttention for RTX 50 GPUs."
    }, {
        "key": "flash",
        "label": "FlashAttention",
        "kind": "attention",
        "backend": "flash",
        "state": "flash_attention",
        "description": "Fused-kernel attention: faster and lighter on NVIDIA GPUs. Recommended with Trellis2."
    }, {
        "key": "nunchaku",
        "label": "Nunchaku",
        "kind": "attention",
        "backend": "nunchaku",
        "state": "nunchaku",
        "description": "A fast inference backend and node set for supported models, with less VRAM. Brings InsightFace."
    }, {
        "key": "insightface",
        "label": "InsightFace",
        "kind": "component",
        "component": "addon_insightface",
        "state": "insight_face",
        "description": "Face analysis, used by Nunchaku's PuLID (face consistency) nodes."
    }, {
        "key": "trellis2",
        "label": "Trellis2",
        "kind": "component",
        "component": "addon_trellis2",
        "state": "trellis2",
        "description": "Microsoft's TRELLIS-2 image-to-3D model and its nodes. Doesn't work with SageAttention."
    }, {
        "key": "manager",
        "label": "ComfyUI-Manager",
        "kind": "component",
        "component": "node_comfyui_manager",
        "state": "node_comfyui_manager",
        "description": "Install, update and remove custom nodes from inside ComfyUI. Current ComfyUI has it built in; ComfyUI starts with --enable-manager."
    }]
    readonly property var nodeDefs: [{
        "key": "easy_use",
        "label": "ComfyUI-Easy-Use",
        "kind": "component",
        "component": "node_comfyui_easy_use",
        "state": "node_comfyui_easy_use",
        "description": "Simplified, all-in-one nodes for common workflows."
    }, {
        "key": "rgthree",
        "label": "rgthree-comfy",
        "kind": "component",
        "component": "node_rgthree_comfy",
        "state": "node_rgthree_comfy",
        "description": "Quality-of-life nodes: context, power lora loader, groups."
    }, {
        "key": "gguf",
        "label": "ComfyUI-GGUF",
        "kind": "component",
        "component": "node_comfyui_gguf",
        "state": "node_comfyui_gguf",
        "description": "Load GGUF-quantized models."
    }, {
        "key": "kjnodes",
        "label": "comfyui-kjnodes",
        "kind": "component",
        "component": "node_comfyui_kjnodes",
        "state": "node_comfyui_kjnodes",
        "description": "Kijai's utility nodes, used by many video workflows."
    }, {
        "key": "crystools",
        "label": "comfyui-crystools",
        "kind": "component",
        "component": "node_comfyui_crystools",
        "state": "node_comfyui_crystools",
        "description": "Resource monitor and debugging nodes."
    }]
    readonly property var flagDefs: [{
        "key": "pinned",
        "label": "Pinned memory",
        "kind": "setting",
        "component": "addon_pinned_memory",
        "setting": "comfyui_pinned_memory_enabled",
        "invert": false,
        "defaultOn": true,
        "description": "Faster CPU-to-GPU transfers (often 10–50% faster sampling). Turn off if you see crashes or out-of-memory errors."
    }, {
        "key": "flag_sage",
        "label": "--use-sage-attention",
        "kind": "attentionFlag",
        "backend": "sage",
        "state": "launch_sage_attention",
        "mono": true,
        "description": "Start ComfyUI with SageAttention when it's installed."
    }, {
        "key": "flag_flash",
        "label": "--use-flash-attention",
        "kind": "attentionFlag",
        "backend": "flash",
        "state": "launch_flash_attention",
        "mono": true,
        "description": "Start ComfyUI with FlashAttention when it's installed."
    }, {
        "key": "listen",
        "label": "--listen",
        "kind": "setting",
        "component": "launch_listen",
        "setting": "comfyui_listen_enabled",
        "state": "listen_enabled",
        "flag": "--listen",
        "mono": true,
        "description": "Reachable from other devices on your network. Only on networks you trust."
    }, {
        "key": "lowvram",
        "label": "--lowvram",
        "kind": "setting",
        "component": "launch_lowvram",
        "setting": "comfyui_lowvram_enabled",
        "state": "lowvram_enabled",
        "flag": "--lowvram",
        "mono": true,
        "description": "Older low-VRAM mode. Ignored while dynamic VRAM is on (the default on NVIDIA)."
    }, {
        "key": "bf16",
        "label": "--bf16-unet",
        "kind": "setting",
        "component": "launch_bf16_unet",
        "setting": "comfyui_bf16_unet_enabled",
        "state": "bf16_unet_enabled",
        "flag": "--bf16-unet",
        "mono": true,
        "description": "BFloat16 UNet: less VRAM, possibly faster on supported GPUs."
    }, {
        "key": "async",
        "label": "--async-offload",
        "kind": "setting",
        "component": "launch_async_offload",
        "setting": "comfyui_async_offload_enabled",
        "state": "async_offload_enabled",
        "flag": "--async-offload",
        "mono": true,
        "description": "Asynchronous model offloading. Already the default on NVIDIA in current ComfyUI."
    }, {
        "key": "smart",
        "label": "--disable-smart-memory",
        "kind": "setting",
        "component": "launch_disable_smart_memory",
        "setting": "comfyui_disable_smart_memory_enabled",
        "state": "disable_smart_memory_enabled",
        "flag": "--disable-smart-memory",
        "mono": true,
        "description": "More predictable VRAM use; for troubleshooting."
    }, {
        "key": "dynamic",
        "label": "--disable-dynamic-vram",
        "kind": "setting",
        "component": "launch_disable_dynamic_vram",
        "setting": "comfyui_disable_dynamic_vram_enabled",
        "state": "disable_dynamic_vram_enabled",
        "flag": "--disable-dynamic-vram",
        "mono": true,
        "description": "Back to estimate-based model loading, if a workflow runs out of memory with dynamic VRAM."
    }, {
        "key": "fastdisk",
        "label": "--fast-disk",
        "kind": "setting",
        "component": "launch_fast_disk",
        "setting": "comfyui_fast_disk_enabled",
        "state": "fast_disk_enabled",
        "flag": "--fast-disk",
        "mono": true,
        "description": "With dynamic VRAM, offload weights to disk instead of RAM. Good with a fast NVMe drive."
    }]
    /// The torch stack the rules apply to: the install's, or the one chosen.
    readonly property string effectiveProfile: view === "manage" ? (addonState && addonState.torch_profile ? addonState.torch_profile : "") : torchProfile
    readonly property bool nonCuda: profileBackend(effectiveProfile) !== "" && profileBackend(effectiveProfile) !== "nvidia"
    readonly property bool sage3Eligible: recommendation ? String(recommendation.gpu_name || "").toLowerCase().includes("rtx 50") : false
    readonly property bool anyBusy: busyKey !== "" || installBusy || updateBusy
    // ---- GPU and torch -------------------------------------------------------
    readonly property var detectedGpus: {
        const snap = app ? app.snapshot : null;
        const out = [];
        if (!snap)
            return out;

        // "NVIDIA GeForce…" already says who made it; "Radeon RX…" doesn't.
        const named = (vendor, name) => {
            return name.toLowerCase().startsWith(vendor.toLowerCase()) ? name : vendor + " " + name;
        };
        if (snap.nvidia_gpu_name)
            out.push({
            "value": "nvidia",
            "label": named("NVIDIA", snap.nvidia_gpu_name) + (snap.nvidia_gpu_vram_mb ? " (" + Math.round(snap.nvidia_gpu_vram_mb / 1024) + " GB)" : "")
        });

        if (snap.amd_gpu_name)
            out.push({
            "value": "amd",
            "label": named("AMD", snap.amd_gpu_name)
        });

        if (snap.intel_gpu_name)
            out.push({
            "value": "intel",
            "label": named("Intel", snap.intel_gpu_name)
        });

        return out;
    }
    readonly property var gpuOptions: [{
        "value": "auto",
        "label": "Automatic (recommended)"
    }].concat(detectedGpus)
    readonly property string gpuVendor: gpuSelection !== "auto" ? gpuSelection : (detectedGpus.find((g) => {
        return g.value === "nvidia";
    }) || detectedGpus[0] || {
        "value": ""
    }).value
    /// The torch profiles for the chosen GPU's vendor (all when unknown).
    readonly property var profileOptions: torchProfiles.filter((p) => {
        return !gpuVendor || String(p.backend) === gpuVendor;
    })
    // ---- ROCm / Intel XPU guided setup (Linux) -------------------------------
    readonly property var guidedTarget: {
        const caps = capabilities || {
        };
        if (String(torchProfile).includes("_rocm") && caps.supports_rocm_guided_setup)
            return {
            "key": "rocm",
            "name": "ROCm",
            "check": "get_rocm_guided_status",
            "install": "install_rocm_guided",
            "detected": "amd_detected"
        };

        if (String(torchProfile).includes("_xpu") && caps.supports_xpu_guided_setup)
            return {
            "key": "xpu",
            "name": "Intel XPU",
            "check": "get_xpu_guided_status",
            "install": "install_xpu_guided",
            "detected": "intel_detected"
        };

        return null;
    }
    readonly property string versionText: {
        if (!updateStatus)
            return updateChecking ? "Checking..." : "Unknown";

        const installed = updateStatus.head_matches_latest_tag ? (updateStatus.latest_version || updateStatus.installed_version) : updateStatus.installed_version;
        return installed ? "v" + String(installed).replace(/^v/, "") : "Unknown";
    }
    // The install view checks the system by itself: when it opens and when
    // what would be installed changes.
    readonly property bool preflightBlocks: preflight !== null && !preflight.ok
    readonly property var preflightIssues: preflight ? (preflight.items || []).filter((i) => {
        return String(i.status).toLowerCase() !== "pass" && String(i.status).toLowerCase() !== "ok";
    }) : []
    readonly property var preflightFailures: preflightIssues.filter((i) => {
        return String(i.status).toLowerCase() === "fail";
    })

    function defaultChoices() {
        return {
            "sage": false,
            "sage3": false,
            "flash": false,
            "nunchaku": false,
            "insightface": false,
            "trellis2": false,
            "manager": true,
            "easy_use": false,
            "rgthree": false,
            "gguf": false,
            "kjnodes": false,
            "crystools": false,
            "flag_sage": false,
            "flag_flash": false
        };
    }

    // ---- helpers ---------------------------------------------------------------
    function log(message, level) {
        if (app)
            app.log(message, level);

    }

    function nameOf(path) {
        const parts = String(path || "").replace(/\\/g, "/").split("/").filter(Boolean);
        return parts.length ? parts[parts.length - 1] : "ComfyUI";
    }

    function parentOf(path) {
        const normalized = String(path || "").replace(/\\/g, "/").replace(/\/+$/, "");
        const index = normalized.lastIndexOf("/");
        return index > 0 ? normalized.slice(0, index) : normalized;
    }

    function profileLabel(value) {
        const found = torchProfiles.find((p) => {
            return p.value === value;
        });
        return found ? found.label : (value || "Unknown");
    }

    function profileBackend(value) {
        const found = torchProfiles.find((p) => {
            return p.value === value;
        });
        return found ? String(found.backend) : "";
    }

    function preferredProfile(vendor, requested) {
        const options = torchProfiles.filter((p) => {
            return String(p.backend) === vendor;
        });
        if (requested && options.some((p) => {
            return p.value === requested;
        }))
            return requested;

        if (vendor === "nvidia" && options.some((p) => {
            return p.value === "torch280_cu128";
        }))
            return "torch280_cu128";

        return options.length ? options[0].value : (requested || "");
    }

    function refreshRecommendation(attempt) {
        Api.call("get_comfyui_install_recommendation", {
            "gpuSelection": gpuSelection === "auto" ? null : gpuSelection
        }).then((reco) => {
            page.recommendation = reco;
            if (!page.torchLocked && torchProfiles.some((p) => {
                return p.value === reco.torch_profile;
            }))
                page.torchProfile = reco.torch_profile;

            if ((reco.detection_pending || !reco.gpu_name) && attempt < 8)
                recommendationRetry.restartFor(attempt + 1);

        }).catch((err) => {
            page.recommendation = null;
            if (!page.torchLocked)
                page.torchProfile = preferredProfile(gpuVendor || "nvidia", "");

            log("Torch recommendation failed: " + err, "warn");
        });
    }

    function setGpuSelection(value) {
        const previous = gpuSelection;
        gpuSelection = value;
        torchLocked = false;
        Api.call("set_comfyui_gpu_selection", {
            "gpuSelection": value
        }).then((settings) => {
            app.settings = settings;
            torchProfile = preferredProfile(gpuVendor, "");
            refreshRecommendation(0);
            refreshGuided(false);
        }).catch((err) => {
            gpuSelection = previous;
            log("GPU selection failed: " + err, "error");
        });
    }

    function setTorchProfile(value) {
        torchProfile = value;
        torchLocked = true;
        clearBlockedChoices();
        refreshGuided(false);
    }

    function refreshGuided(logResult) {
        const target = guidedTarget;
        if (!target) {
            guidedStatus = null;
            return ;
        }
        guidedBusy = true;
        Api.call(target.check).then((status) => {
            page.guidedStatus = Object.assign({
                "target": target.key
            }, status || {
            });
            if (logResult && status && status.detail)
                log(target.name + " check: " + status.detail);

        }).catch((err) => {
            page.guidedStatus = {
                "target": target.key,
                "supported": false,
                "detail": "Couldn't check " + target.name + ": " + err
            };
        }).then(Api.after(() => {
            return page.guidedBusy = false;
        }));
    }

    function runGuidedSetup() {
        const target = guidedTarget;
        if (!target)
            return ;

        guidedBusy = true;
        log("Starting guided " + target.name + " setup...");
        app.openPanel("activity");
        Api.call(target.install).then((status) => {
            if (status && status.detail)
                log(target.name + " setup: " + status.detail);

        }).catch((err) => {
            return log(target.name + " setup failed: " + err, "error");
        }).then(Api.after(() => {
            page.guidedBusy = false;
            refreshGuided(false);
        }));
    }

    // ---- option values and rules ---------------------------------------------
    function settingValue(def) {
        const settings = app ? app.settings : null;
        if (!settings)
            return def.defaultOn === true;

        const value = settings[def.setting];
        return value === undefined || value === null ? def.defaultOn === true : value === true;
    }

    function valueOf(def) {
        if (def.kind === "setting")
            return settingValue(def);

        if (view === "manage") {
            if (!addonState)
                return false;

            if (def.key === "flag_sage")
                return addonState.launch_sage_attention === true || addonState.launch_sage_attention3 === true;

            return addonState[def.state] === true;
        }
        if (def.key === "insightface" && choices.nunchaku)
            return true;

        return choices[def.key] === true;
    }

    function blockedReason(def) {
        if (def.key === "sage3" && !sage3Eligible)
            return "Needs an NVIDIA RTX 50 GPU.";

        if (def.key === "trellis2" && effectiveProfile !== "torch280_cu128")
            return "Needs the Torch 2.8.0 + cu128 stack.";

        if ((def.kind === "attention" || def.kind === "attentionFlag") && nonCuda)
            return "NVIDIA GPUs only.";

        if (def.key === "insightface" && valueOf(addonDefs[3]))
            return "Included with Nunchaku.";

        if (def.flag && view === "manage" && addonState && Array.isArray(addonState.supported_launch_flags) && !addonState.supported_launch_flags.includes(def.flag))
            return "Not in this ComfyUI version; update ComfyUI to use it.";

        return "";
    }

    /// Unchecks new-install choices the chosen torch stack rules out.
    function clearBlockedChoices() {
        const next = Object.assign({
        }, choices);
        for (const def of addonDefs.concat(flagDefs)) if (def.kind !== "setting" && blockedReason(def) !== "" && def.key !== "insightface") {
            next[def.key] = false;
        }
        choices = next;
    }

    // ---- toggling ------------------------------------------------------------
    function toggle(def, on) {
        if (def.kind === "setting")
            return applySetting(def, on);

        if (view !== "manage") {
            const next = Object.assign({
            }, choices);
            next[def.key] = on;
            if (on && def.kind === "attention") {
                for (const other of addonDefs) if (other.kind === "attention" && other.key !== def.key) {
                    next[other.key] = false;
                }
            }
            if (on && def.kind === "attentionFlag") {
                for (const other of flagDefs) if (other.kind === "attentionFlag" && other.key !== def.key) {
                    next[other.key] = false;
                }
            }
            choices = next;
            return ;
        }
        if (def.kind === "attention")
            return applyAttention(def, on);

        if (def.kind === "attentionFlag")
            return applyAttentionFlag(def, on);

        return applyComponent(def, on);
    }

    function afterChange() {
        busyKey = "";
        loadAddonState();
        app.refreshSettings();
    }

    function applyAttention(def, on) {
        if (!root)
            return ;

        const current = addonDefs.find((d) => {
            return d.kind === "attention" && d.key !== def.key && valueOf(d);
        });
        const message = on ? (current ? "Install " + def.label + "? It replaces " + current.label + "." : "Install " + def.label + "?") : "Remove " + def.label + "?";
        confirmDialog.ask(message, on ? "Install" : "Remove", !on).then((ok) => {
            if (!ok)
                return ;

            busyKey = def.key;
            log((on ? "Installing " : "Removing ") + def.label + "...");
            Api.call("apply_attention_backend_change", {
                "request": {
                    "comfyuiRoot": root,
                    "targetBackend": on ? def.backend : "none",
                    "torchProfile": effectiveProfile || null
                }
            }).then((result) => {
                if (result)
                    log(result, "ok");

            }).catch((err) => {
                return log("Attention backend change failed: " + err, "error");
            }).then(Api.after(afterChange));
        });
    }

    function applyAttentionFlag(def, on) {
        if (!root)
            return ;

        const backend = def.backend === "sage" && valueOf(addonDefs[1]) ? "sage3" : def.backend;
        busyKey = def.key;
        Api.call("set_comfyui_launch_attention_backend", {
            "request": {
                "comfyuiRoot": root,
                "targetBackend": on ? backend : "none"
            }
        }).then((result) => {
            if (result)
                log(result);

        }).catch((err) => {
            return log("Launch flag change failed: " + err, "error");
        }).then(Api.after(afterChange));
    }

    function applyComponent(def, on) {
        if (!root)
            return ;

        confirmDialog.ask((on ? "Install " : "Remove ") + def.label + "?", on ? "Install" : "Remove", !on).then((ok) => {
            if (!ok)
                return ;

            busyKey = def.key;
            log((on ? "Installing " : "Removing ") + def.label + "...");
            Api.call("apply_comfyui_component_toggle", {
                "request": {
                    "comfyuiRoot": root,
                    "component": def.component,
                    "enabled": on,
                    "torchProfile": effectiveProfile || null
                }
            }).then((result) => {
                if (result)
                    log(result, "ok");

            }).catch((err) => {
                return log(def.label + " change failed: " + err, "error");
            }).then(Api.after(afterChange));
        });
    }

    function applySetting(def, on) {
        busyKey = def.key;
        Api.call("apply_comfyui_component_toggle", {
            "request": {
                "comfyuiRoot": root || null,
                "component": def.component,
                "enabled": on,
                "torchProfile": effectiveProfile || null
            }
        }).then((result) => {
            if (result)
                log(result);

        }).catch((err) => {
            return log(def.label + " change failed: " + err, "error");
        }).then(Api.after(afterChange));
    }

    // ---- the selected install ------------------------------------------------
    function loadAddonState() {
        if (!root) {
            addonState = null;
            return ;
        }
        const requested = root;
        addonStateLoading = true;
        Api.call("get_comfyui_addon_state", {
            "comfyuiRoot": requested
        }).then((state) => {
            if (requested === page.root)
                page.addonState = state;

        }).catch(() => {
            if (requested === page.root)
                page.addonState = null;

        }).then(Api.after(() => {
            return page.addonStateLoading = false;
        }));
    }

    function loadSharedModels() {
        if (!root)
            return ;

        Api.call("get_comfyui_extra_model_config", {
            "comfyuiRoot": root
        }).then((cfg) => {
            page.sharedModelsPath = cfg && cfg.configured ? String(cfg.base_path || "") : "";
            page.sharedModelsDefault = cfg && cfg.configured ? cfg.use_as_default === true : false;
        }).catch((err) => {
            return log("Couldn't read the shared models folder: " + err, "warn");
        });
    }

    function saveSharedModels(path, useDefault) {
        sharedModelsPath = path;
        sharedModelsDefault = path !== "" && useDefault;
        if (view !== "manage" || !root)
            return ;

        Api.call("set_comfyui_extra_model_config", {
            "comfyuiRoot": root,
            "extraModelRoot": path || null,
            "useAsDefault": sharedModelsDefault
        }).then(() => {
            return log(path ? "Shared models folder set to " + path + "." : "Shared models folder cleared.");
        }).catch((err) => {
            return log("Saving the shared models folder failed: " + err, "error");
        });
    }

    function checkForUpdate() {
        if (!root)
            return ;

        updateChecking = true;
        Api.call("get_comfyui_update_status", {
            "comfyuiRoot": root
        }).then((status) => {
            page.updateStatus = status;
        }).catch((err) => {
            page.updateStatus = null;
            log("ComfyUI update check failed: " + err, "warn");
        }).then(Api.after(() => {
            return page.updateChecking = false;
        }));
    }

    function updateComfy() {
        if (!root || updateBusy)
            return ;

        updateBusy = true;
        log("Updating " + nameOf(root) + "...");
        app.openPanel("activity");
        Api.call("update_selected_comfyui", {
            "comfyuiRoot": root
        }).then((result) => {
            if (result)
                log(result, "ok");

        }).catch((err) => {
            return log("ComfyUI update failed: " + err, "error");
        }).then(Api.after(() => {
            page.updateBusy = false;
            checkForUpdate();
            loadAddonState();
            // The sidebar shows each install's ComfyUI version.
            app.refreshInstalls();
        }));
    }

    function samePath(a, b) {
        return String(a || "").replace(/\\/g, "/").replace(/\/+$/, "") === String(b || "").replace(/\\/g, "/").replace(/\/+$/, "");
    }

    function refreshRuntime() {
        Api.call("get_comfyui_runtime_status").then((status) => {
            const anyRunning = status.running === true;
            page.runningRoot = anyRunning ? status.running_root || page.root : "";
            page.running = anyRunning && page.samePath(page.runningRoot, page.root);
            page.runtimeUrl = status.url || "";
            if (anyRunning)
                page.starting = false;

        }).catch(() => {
            page.running = false;
            page.runningRoot = "";
        });
    }

    function startOrStop() {
        if (running) {
            starting = false;
            Api.call("stop_comfyui_root").then((stopped) => {
                return log(stopped ? "Stopping ComfyUI..." : "ComfyUI wasn't running.");
            }).catch((err) => {
                return log("Stop failed: " + err, "error");
            }).then(Api.after(refreshRuntime));
            return ;
        }
        if (!root)
            return ;

        starting = true;
        startingRoot = root;
        Api.call("start_comfyui_root", {
            "comfyuiRoot": root
        }).catch((err) => {
            page.starting = false;
            log("Start failed: " + err, "error");
        });
    }

    function openComfy() {
        if (runtimeUrl)
            Api.call("open_external_url", {
            "url": runtimeUrl
        }).catch((err) => {
            return log("Couldn't open ComfyUI: " + err, "error");
        });

    }

    function openFolder() {
        if (root)
            Api.call("open_folder", {
            "path": root
        }).catch((err) => {
            return log("Couldn't open the folder: " + err, "error");
        });

    }

    function loadSelected() {
        updateStatus = null;
        addonState = null;
        loadAddonState();
        loadSharedModels();
        checkForUpdate();
    }

    // ---- installing ----------------------------------------------------------
    /// Where installs live: picks a base folder, or an install folder itself.
    function chooseBase(path) {
        Api.call("inspect_comfyui_path", {
            "path": path
        }).then((inspection) => {
            const detected = inspection && inspection.detected_root ? inspection.detected_root : "";
            const pickedInstall = detected !== "" && detected.replace(/\\/g, "/") === String(inspection.selected).replace(/\\/g, "/");
            const base = pickedInstall ? parentOf(detected) : path;
            return Api.call("set_comfyui_install_base", {
                "comfyuiInstallBase": base
            }).then((settings) => {
                app.settings = settings;
                return app.refreshInstalls();
            }).then(() => {
                if (pickedInstall)
                    app.selectInstall(detected);
                else if (view === "manage" && app.installs.length && !page.hasInstall)
                    app.selectInstall(app.installs[app.installs.length - 1].root);
                refreshResume();
                log(app.installs.length ? "Found " + app.installs.length + " ComfyUI install" + (app.installs.length === 1 ? "" : "s") + " in " + base + "." : "No ComfyUI installs in " + base + " yet.");
            });
        }).catch((err) => {
            return log("Choosing the folder failed: " + err, "error");
        });
    }

    function installRequest() {
        return {
            "installRoot": installBase,
            "extraModelRoot": sharedModelsPath || null,
            "extraModelUseDefault": sharedModelsDefault && sharedModelsPath !== "",
            "torchProfile": torchProfile || null,
            "includeSageAttention": choices.sage,
            "includeSageAttention3": choices.sage3,
            "includeFlashAttention": choices.flash,
            "includeInsightFace": choices.insightface || choices.nunchaku,
            "includeNunchaku": choices.nunchaku,
            "includeTrellis2": choices.trellis2,
            "includePinnedMemory": settingValue(flagDefs[0]),
            "nodeComfyuiManager": choices.manager,
            "nodeComfyuiEasyUse": choices.easy_use,
            "nodeRgthreeComfy": choices.rgthree,
            "nodeComfyuiGguf": choices.gguf,
            "nodeComfyuiKjnodes": choices.kjnodes,
            "nodeComfyuiCrystools": choices.crystools
        };
    }

    function runPreflight() {
        preflightRunning = true;
        return Api.call("run_comfyui_preflight", {
            "request": installRequest()
        }).then((result) => {
            page.preflight = result;
            return result;
        }).catch((err) => {
            page.preflight = {
                "ok": false,
                "summary": "Preflight failed to run.",
                "items": [{
                    "status": "fail",
                    "title": "Preflight",
                    "detail": String(err)
                }]
            };
            return null;
        }).then(Api.after(() => {
            return page.preflightRunning = false;
        }));
    }

    function schedulePreflight() {
        if (view === "install" || !hasInstall)
            preflightTimer.restart();

    }

    function refreshResume() {
        Api.call("get_comfyui_resume_state", {
            "installBase": installBase || null
        }).then((result) => {
            return page.resumeState = result && result.found ? result : null;
        }).catch(() => {
            return page.resumeState = null;
        });
    }

    function startInstall(fresh) {
        if (installBusy) {
            Api.call("cancel_comfyui_install").then((cancelled) => {
                return log(cancelled ? "Cancelling the install..." : "No install is running.", "warn");
            });
            return ;
        }
        if (!installBase) {
            log("Choose where to install ComfyUI first.", "warn");
            return ;
        }
        app.openPanel("activity");
        const stopFirst = running ? Api.call("stop_comfyui_root").then(() => {
            return log("Stopped ComfyUI before installing.");
        }) : Promise.resolve();
        stopFirst.then(() => {
            return runPreflight();
        }).then((result) => {
            if (!result || !result.ok) {
                log("Preflight found problems. Fix them before installing.", "error");
                return ;
            }
            const request = installRequest();
            request.forceFresh = fresh === true;
            page.installBusy = true;
            log(fresh ? "Starting a fresh ComfyUI install..." : "Starting the ComfyUI install...");
            return Api.call("start_comfyui_install", {
                "request": request
            }).catch((err) => {
                page.installBusy = false;
                log("The install didn't start: " + err, "error");
            });
        });
    }

    /// Opens ComfyUI in the browser once it answers (after a restart).
    function openWhenReady() {
        readyPoll.tries = 0;
        readyPoll.start();
    }

    onViewChanged: schedulePreflight()
    onInstallBaseChanged: schedulePreflight()
    onTorchProfileChanged: schedulePreflight()
    onChoicesChanged: schedulePreflight()
    onSharedModelsPathChanged: schedulePreflight()
    color: Theme.canvas
    onRootChanged: {
        if (root)
            loadSelected();

        // Running or not is per install.
        running = runningRoot !== "" && samePath(runningRoot, root);
        refreshRuntime();
    }
    Component.onCompleted: {
        Api.call("get_platform_capabilities").then((caps) => {
            page.capabilities = caps;
            const settings = app.settings || {
            };
            page.gpuSelection = settings.comfyui_gpu_selection || "auto";
            const saved = String(settings.comfyui_torch_profile || "");
            if (saved && caps.torch_profiles.some((p) => {
                return p.value === saved;
            }) && page.gpuSelection === "auto") {
                page.torchProfile = saved;
                page.torchLocked = true;
            }
            page.refreshRecommendation(0);
            page.refreshGuided(false);
        });
        refreshRuntime();
        refreshResume();
        if (root)
            loadSelected();

    }

    Timer {
        id: preflightTimer

        interval: 500
        onTriggered: {
            if (!page.installBusy) {
                page.runPreflight();
            }
        }
    }

    Timer {
        id: recommendationRetry

        property int nextAttempt: 0

        function restartFor(attempt) {
            nextAttempt = attempt;
            restart();
        }

        interval: 600
        onTriggered: page.refreshRecommendation(nextAttempt)
    }

    // ---- backend events ------------------------------------------------------
    Connections {
        function onEvent(name, payload) {
            if (name === "comfyui-install-progress") {
                if (payload.phase === "failed" || payload.phase === "cancelled") {
                    page.installBusy = false;
                } else if (payload.phase === "finished") {
                    page.installBusy = false;
                    page.resumeState = null;
                    const installed = String(payload.folder || "").trim();
                    page.app.refreshInstalls().then(() => {
                        if (installed) {
                            page.app.selectInstall(installed);
                            page.view = "manage";
                        }
                    });
                }
            } else if (name === "comfyui-runtime") {
                // (A start from the tray starts the selected install.)
                if (payload.phase === "starting" && !page.starting) {
                    page.starting = true;
                    page.startingRoot = page.root;
                }
                else if (payload.phase === "started" || payload.phase === "stopped" || payload.phase.endsWith("failed"))
                    page.starting = false;
                if (payload.phase === "restarted_after_changes")
                    page.openWhenReady();

                page.refreshRuntime();
            } else if (name === "tray-status") {
                // Which install is running comes from the runtime status.
                page.refreshRuntime();
            }
        }

        target: Api
    }

    Timer {
        id: readyPoll

        property int tries: 0

        interval: 500
        repeat: true
        onTriggered: {
            tries += 1;
            Api.call("get_comfyui_runtime_status").then((status) => {
                if (status.running && readyPoll.running) {
                    readyPoll.stop();
                    page.runtimeUrl = status.url || "";
                    page.openComfy();
                }
            });
            if (tries > 90)
                stop();

        }
    }

    Timer {
        // While starting, ask often; otherwise now and then.
        interval: page.starting ? 1400 : 6500
        running: true
        repeat: true
        onTriggered: page.refreshRuntime()
    }

    ConfirmDialog {
        id: confirmDialog
    }

    // ---- the views -----------------------------------------------------------
    StackLayout {
        anchors.fill: parent
        currentIndex: page.view === "install" || !page.hasInstall ? 1 : 0

        ComfyManageView {
            ctl: page
        }

        ComfyInstallView {
            ctl: page
        }

    }

}
