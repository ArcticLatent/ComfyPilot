.pragma library

// Catalog model rules, ported unchanged from the Tauri UI's catalog.js:
// VRAM/RAM tiers, variant order and labels, which artifacts a RAM tier gets,
// text-encoder defaults, and the "always" (additional files) groups.
// Pure functions: the page passes in the catalog, tiers and choices.

var DOT_SEP = " • ";
var ALWAYS_ONLY_VARIANT_ID = "__always_only__";

var vramTierLabels = {
    tier_s: "32+ GB VRAM",
    tier_a: "16/24 GB VRAM",
    tier_b: "12 GB VRAM",
    tier_c: "8 GB VRAM"
};
var tierStrength = { tier_c: 0, tier_b: 1, tier_a: 2, tier_s: 3 };
var vramOptions = [
    { id: "vram_8", label: "8 GB VRAM", tier: "tier_c" },
    { id: "vram_12", label: "12 GB VRAM", tier: "tier_b" },
    { id: "vram_16", label: "16 GB VRAM", tier: "tier_a" },
    { id: "vram_24", label: "24 GB VRAM", tier: "tier_a" },
    { id: "vram_32", label: "32 GB VRAM", tier: "tier_s" },
    { id: "vram_48", label: "48 GB VRAM", tier: "tier_s" },
    { id: "vram_80", label: "80 GB VRAM", tier: "tier_s" },
    { id: "vram_96", label: "96 GB VRAM", tier: "tier_s" }
];
var ramOptions = [
    { id: "ram_8", label: "8 GB RAM", gb: 8 },
    { id: "ram_16", label: "16 GB RAM", gb: 16 },
    { id: "ram_32", label: "32 GB RAM", gb: 32 },
    { id: "ram_64", label: "64 GB RAM", gb: 64 },
    { id: "ram_96", label: "96 GB RAM", gb: 96 },
    { id: "ram_128", label: "128 GB RAM", gb: 128 }
];

function formatFileSize(bytes) {
    var value = Number(bytes || 0);
    if (!isFinite(value) || value <= 0) return "";
    var units = ["B", "KB", "MB", "GB", "TB"];
    var i = 0;
    while (value >= 1024 && i < units.length - 1) {
        value /= 1024;
        i += 1;
    }
    return (value >= 10 || i === 0 ? Math.round(value) : value.toFixed(1)) + " " + units[i];
}

// ---- families --------------------------------------------------------------

function prettyFamilyId(family) {
    return String(family || "").trim()
        .replace(/[_-]+/g, " ")
        .replace(/\bflux\s*(\d+)\b/i, "FLUX $1")
        .replace(/\bwan\b/i, "WAN")
        .replace(/\bltx\s*(\d)(\d)\b/i, "LTX $1.$2")
        .replace(/\s+/g, " ");
}

function commonDisplayPrefix(names) {
    if (!names.length) return "";
    var prefix = names[0] || "";
    for (var n = 1; n < names.length; n++) {
        var name = names[n];
        var i = 0;
        while (i < prefix.length && i < name.length && prefix.charAt(i).toLowerCase() === name.charAt(i).toLowerCase())
            i += 1;
        prefix = prefix.slice(0, i);
        if (!prefix) break;
    }
    return prefix.replace(/[\s._/-]+$/g, "").replace(/\s+/g, " ").trim();
}

function modelFamilyLabel(family, models) {
    var familyId = String(family || "").trim();
    var names = (models || [])
        .filter(function (m) { return String(m && m.family || "").trim() === familyId; })
        .map(function (m) { return String(m && m.display_name || "").trim(); })
        .filter(Boolean);
    if (names.length === 1) return names[0] || familyId;
    var common = commonDisplayPrefix(names);
    var pretty = prettyFamilyId(familyId);
    // "FLUX 2 DEV" and "FLUX.2 KLEIN" only share "FLUX"; the id's "FLUX 2"
    // says more, so use it when it extends the shared prefix.
    if (pretty.length > common.length && pretty.toLowerCase().indexOf(common.toLowerCase()) === 0) return pretty;
    if (common.length >= 4) return common;
    return pretty || familyId;
}

/// [{ value, label }] for each family, sorted by label.
function families(models) {
    var ids = [];
    (models || []).forEach(function (m) {
        if (ids.indexOf(m.family) < 0) ids.push(m.family);
    });
    return ids.map(function (id) { return { value: id, label: modelFamilyLabel(id, models) }; })
        .sort(function (a, b) { return a.label.localeCompare(b.label); });
}

/// The models matching the family ("" for all) and the search text.
function filterModels(models, family, search) {
    var text = String(search || "").trim().toLowerCase();
    return (models || []).filter(function (model) {
        if (family && model.family !== family) return false;
        if (!text) return true;
        var variantText = (model.variants || []).map(function (v) {
            return [v.id, v.model_size, v.quantization, v.note].filter(Boolean).join(" ");
        }).join(" ");
        return (model.display_name + " " + model.family + " " + model.id + " " + variantText).toLowerCase().indexOf(text) >= 0;
    });
}

function modelHasAlwaysArtifacts(model) {
    return Array.isArray(model && model.always) && model.always.some(function (g) {
        return Array.isArray(g && g.artifacts) && g.artifacts.length;
    });
}

// ---- tiers -----------------------------------------------------------------

function vramTierForMb(mb) {
    var value = Number(mb);
    if (!isFinite(value) || value <= 0) return "";
    if (value >= 32000) return "tier_s";
    if (value >= 16000) return "tier_a";
    if (value >= 12000) return "tier_b";
    return "tier_c";
}

function resolvedModelRamThresholds(model) {
    var cfg = (model && model.ram_tier_thresholds) || {};
    var num = function (v, d) { return isFinite(Number(v)) && v !== null && v !== undefined ? Number(v) : d; };
    return { tier_a: num(cfg.tier_a_min_gb, 64), tier_b: num(cfg.tier_b_min_gb, 32), tier_c: num(cfg.tier_c_min_gb, 0) };
}

function ramTierForGb(gb, thresholds) {
    var value = Number(gb);
    if (!isFinite(value)) return "";
    var mins = thresholds || { tier_a: 64, tier_b: 32, tier_c: 0 };
    if (value >= Number(mins.tier_a || 64)) return "tier_a";
    if (value >= Number(mins.tier_b || 32)) return "tier_b";
    return "tier_c";
}

function hasCustomRamThresholds(model) {
    var cfg = (model && model.ram_tier_thresholds) || {};
    return [cfg.tier_a_min_gb, cfg.tier_b_min_gb, cfg.tier_c_min_gb].some(function (v) {
        return v !== null && v !== undefined && isFinite(Number(v));
    });
}

/// The RAM thresholds the given models share, if they all set the same ones.
function sharedCustomRamThresholds(models) {
    var entries = (models || []).filter(hasCustomRamThresholds).map(resolvedModelRamThresholds);
    if (!entries.length) return null;
    var key = function (t) { return [t.tier_a, t.tier_b, t.tier_c].join("::"); };
    var first = key(entries[0]);
    return entries.every(function (e) { return key(e) === first; }) ? entries[0] : null;
}

function formatRamGb(value) {
    var num = Number(value || 0);
    if (!isFinite(num)) return "0";
    var rounded = Math.round(num * 10) / 10;
    return Math.abs(rounded - Math.round(rounded)) < 0.05 ? String(Math.round(rounded)) : rounded.toFixed(1);
}

function customRamOptionLabel(tierId, thresholds) {
    var mins = thresholds || { tier_a: 64, tier_b: 32, tier_c: 0 };
    if (tierId === "tier_a") return "Tier A (" + formatRamGb(mins.tier_a) + " GB+)";
    if (tierId === "tier_b") return "Tier B (" + formatRamGb(mins.tier_b) + "-" + formatRamGb(mins.tier_a) + " GB)";
    return "Tier C (<" + formatRamGb(mins.tier_b) + " GB)";
}

// ---- variants --------------------------------------------------------------

function variantVramHint(variant, selectedTier) {
    var variantTier = String(variant && variant.tier || "").trim();
    var target = String(selectedTier || "").trim();
    var label = vramTierLabels[variantTier] || variantTier.toUpperCase();
    if (!target) return label;
    if (variantTier === target) return label + " detected fit";
    var vs = tierStrength[variantTier], ts = tierStrength[target];
    if (vs === undefined || ts === undefined) return label;
    return vs > ts ? label + " may need more VRAM" : label;
}

function modelVariantLabel(variant, selectedTier) {
    return [variant.model_size, variant.quantization, variant.note, variantVramHint(variant, selectedTier)]
        .filter(Boolean).join(DOT_SEP);
}

function variantSortRank(variant, selectedTier) {
    var variantTier = String(variant && variant.tier || "").trim();
    var target = String(selectedTier || "").trim();
    if (!target) return 0;
    var vs = tierStrength[variantTier], ts = tierStrength[target];
    if (vs === undefined || ts === undefined) return 50;
    if (variantTier === target) return 0;
    if (vs < ts) return 10 + (ts - vs);
    return 20 + (vs - ts);
}

/// The model's variants, the best fit for the VRAM tier first.
function variantsForModel(model, selectedTier) {
    var variants = (model && model.variants || []).slice();
    if (!selectedTier) return variants;
    return variants.map(function (variant, index) { return { variant: variant, index: index }; })
        .sort(function (a, b) {
            return (variantSortRank(a.variant, selectedTier) - variantSortRank(b.variant, selectedTier)) || (a.index - b.index);
        })
        .map(function (e) { return e.variant; });
}

function recommendedVariantId(model, selectedTier) {
    var variants = variantsForModel(model, selectedTier);
    if (variants.length) return variants[0].id;
    return modelHasAlwaysArtifacts(model) ? ALWAYS_ONLY_VARIANT_ID : "";
}

// ---- artifacts -------------------------------------------------------------

function artifactSupportedOnRam(artifact, availableTier) {
    var bucketTier = String(artifact && artifact.ram_bucket || "").trim();
    if (bucketTier) return String(availableTier || "").trim() === bucketTier;
    var requiredTier = String(artifact && artifact.min_ram_tier || "").trim();
    if (!requiredTier) return true;
    var currentTier = String(availableTier || "").trim();
    if (!currentTier) return false;
    var cs = tierStrength[currentTier], rs = tierStrength[requiredTier];
    if (cs === undefined || rs === undefined) return false;
    return cs >= rs;
}

function artifactSelectableInQueue(artifact, availableTier) {
    if (String(artifact && artifact.ram_bucket || "").trim()) return true;
    return artifactSupportedOnRam(artifact, availableTier);
}

function artifactFileName(artifact) {
    var fromPath = String(artifact && artifact.path || "").trim().split("/").filter(Boolean).pop();
    if (fromPath) return fromPath;
    var direct = String(artifact && artifact.direct_url || "").trim();
    if (!direct) return String(artifact && artifact.repo || "").trim() || "artifact";
    var noQuery = direct.split("?")[0] || direct;
    return noQuery.split("/").filter(Boolean).pop() || direct;
}

function artifactDisplayBaseName(artifact) {
    return artifactFileName(artifact).replace(/\.(safetensors|gguf|ckpt|pt|pth|bin|onnx|json|ya?ml|zip)$/i, "");
}

function artifactSearchText(artifact) {
    return [artifactFileName(artifact), String(artifact && artifact.path || "").trim(),
        String(artifact && artifact.direct_url || "").trim(), String(artifact && artifact.target_category || "").trim()]
        .join(" ").toLowerCase();
}

function isTextEncoderArtifact(artifact) {
    return /\b(text[_\s-]*encoders?|clip)\b/.test(artifactSearchText(artifact));
}

function isTextEncoderProjectionArtifact(artifact) {
    return /(?:^|[_\s\-/])(m?m?proj|projection)(?:$|[_\s\-.])/i.test(artifactSearchText(artifact));
}

function isClipLTextEncoderArtifact(artifact) {
    if (!isTextEncoderArtifact(artifact) || isTextEncoderProjectionArtifact(artifact)) return false;
    return /(?:^|[_\s\-/.])clip[_\s\-.]?l(?:$|[_\s\-.])/i.test(artifactSearchText(artifact));
}

function isQuantizedTextEncoderArtifact(artifact) {
    if (!isTextEncoderArtifact(artifact) || isTextEncoderProjectionArtifact(artifact)) return false;
    var text = artifactSearchText(artifact);
    return /\bgguf\b/.test(text) || /\bqat\b/.test(text)
        || /(?:^|[_\-.])q\d(?:[_\-.]|$)/i.test(text)
        || /(?:^|[_\-.])q\d_[a-z](?:[_\-.]|$)/i.test(text)
        || /(?:^|[_\-.])fp[2-8](?:[_\-.]|$)/i.test(text)
        || /(?:^|[_\-.])int\d+(?:[_\-.]|$)/i.test(text);
}

function quantizedTextEncoderSortRank(artifact) {
    var text = artifactSearchText(artifact);
    var fp = text.match(/(?:^|[_\-.])fp([2-8])(?:[_\-.]|$)/i);
    if (fp) return 10 + (8 - Number(fp[1]));
    var int = text.match(/(?:^|[_\-.])int(\d+)(?:[_\-.]|$)/i);
    if (int) return 20 + (8 - Math.min(Number(int[1]), 8));
    var q = text.match(/(?:^|[_\-.])q(\d)(?:[_\-.]|$)|(?:^|[_\-.])q(\d)_[a-z](?:[_\-.]|$)/i);
    if (q) return 30 + (8 - Number(q[1] || q[2]));
    if (/\bqat\b/.test(text)) return 40;
    if (/\bgguf\b/.test(text)) return 50;
    return 99;
}

function quantizedTextEncoderLabel(artifact) {
    var text = artifactSearchText(artifact);
    var fp = text.match(/(?:^|[_\-.])fp([2-8])(?:[_\-.]|$)/i);
    if (fp) return "FP" + fp[1];
    var int = text.match(/(?:^|[_\-.])int(\d+)(?:[_\-.]|$)/i);
    if (int) return "INT" + int[1];
    var q = text.match(/(?:^|[_\-.])q(\d)(?:[_\-.]|$)|(?:^|[_\-.])q(\d)_[a-z](?:[_\-.]|$)/i);
    if (q) return "Q" + (q[1] || q[2]);
    if (/\bqat\b/.test(text)) return "QAT";
    if (/\bgguf\b/.test(text)) return "GGUF";
    return "quantized";
}

function isFullPrecisionTextEncoderArtifact(artifact) {
    if (!isTextEncoderArtifact(artifact) || isTextEncoderProjectionArtifact(artifact)) return false;
    return !isQuantizedTextEncoderArtifact(artifact);
}

function artifactSizeBytes(artifact) {
    var size = Number(artifact && artifact.size_bytes);
    return isFinite(size) && size > 0 ? size : 0;
}

function artifactRuntimeRamBytes(artifact) {
    var nested = Number(artifact && artifact.memory_estimate && artifact.memory_estimate.runtime_ram_bytes);
    if (isFinite(nested) && nested > 0) return nested;
    var flat = Number(artifact && artifact.runtime_ram_bytes);
    return isFinite(flat) && flat > 0 ? flat : 0;
}

function artifactChoiceKey(artifact) {
    return [String(artifact && artifact.target_category || "").trim(), String(artifact && artifact.repo || "").trim(),
        String(artifact && artifact.path || "").trim(), String(artifact && artifact.direct_url || "").trim(),
        artifactFileName(artifact)].join("::");
}

function artifactChoiceStateKey(modelId, variantId, artifact) {
    return [modelId, variantId, artifactChoiceKey(artifact)].join("::");
}

function artifactDefaultSupportedOnRam(artifact, ramTier) {
    var bucketTier = String(artifact && artifact.ram_bucket || "").trim();
    if (bucketTier) {
        var currentTier = String(ramTier || "").trim();
        return currentTier ? bucketTier === currentTier : true;
    }
    return artifactSupportedOnRam(artifact, ramTier);
}

function groupPreferredTierATextEncoderKey(group, ramTier) {
    if (String(ramTier || "").trim() !== "tier_a") return "";
    var preferred = null;
    (group && group.artifacts || []).forEach(function (artifact) {
        if (!artifactSelectableInQueue(artifact, ramTier)) return;
        if (!artifactDefaultSupportedOnRam(artifact, ramTier)) return;
        if (!isFullPrecisionTextEncoderArtifact(artifact)) return;
        if (!preferred || artifactSizeBytes(artifact) > artifactSizeBytes(preferred)) preferred = artifact;
    });
    return preferred ? artifactChoiceKey(preferred) : "";
}

function artifactDefaultChecked(artifact, ramTier, group) {
    var preferredKey = groupPreferredTierATextEncoderKey(group, ramTier);
    if (preferredKey && isTextEncoderArtifact(artifact) && !isTextEncoderProjectionArtifact(artifact)) {
        if (isClipLTextEncoderArtifact(artifact)) return artifactDefaultSupportedOnRam(artifact, ramTier);
        return artifactChoiceKey(artifact) === preferredKey;
    }
    return artifactDefaultSupportedOnRam(artifact, ramTier);
}

/// Whether an additional file is ticked: the user's choice, else the default.
function artifactChoiceChecked(choices, modelId, variantId, artifact, ramTier, group) {
    var key = artifactChoiceStateKey(modelId, variantId, artifact);
    if (choices && Object.prototype.hasOwnProperty.call(choices, key)) return choices[key] === true;
    return artifactDefaultChecked(artifact, ramTier, group);
}

/// A line about a RAM-bucketed file: its quality/memory trade-off.
function ramBucketLabel(tierId, artifact, thresholds) {
    if (isFullPrecisionTextEncoderArtifact(artifact)) return "Highest fidelity, largest memory use";
    if (isQuantizedTextEncoderArtifact(artifact)) {
        var label = quantizedTextEncoderLabel(artifact);
        if (/^FP8$/i.test(label)) return "High fidelity, lower memory than full precision";
        if (/^Q[56]$/i.test(label)) return "Good quality, lower memory than FP8 (" + label + ")";
        if (/^(Q4|FP4|INT4)$/i.test(label)) return "Balanced quality and memory use (" + label + ")";
        if (/^(Q[123]|FP[23]|INT[123])$/i.test(label)) return "Lowest memory use, most quality tradeoff (" + label + ")";
        return "Quantized, lower memory than full precision (" + label + ")";
    }
    if (!tierId) return "";
    return customRamOptionLabel(tierId, thresholds);
}

function targetCategoryLabel(category) {
    var normalized = String(category || "").trim().toLowerCase();
    var labels = {
        clip: "Text Encoders", text_encoders: "Text Encoders", loras: "LoRAs", upscale_models: "Upscale Models",
        vae: "VAE", clip_vision: "CLIP Vision", diffusion_models: "Diffusion Models", unet: "UNet",
        controlnet: "ControlNet", sams: "SAM Models", pulid: "PuLID", style_models: "Style Models",
        facerestore_models: "Face Restore Models"
    };
    if (labels[normalized]) return labels[normalized];
    return normalized.split("/").map(function (part) {
        return part.replace(/[_-]+/g, " ").replace(/\b\w/g, function (c) { return c.toUpperCase(); });
    }).filter(Boolean).join(" / ");
}

function queueArtifactGroupLabel(group) {
    var unique = [];
    (group && group.artifacts || []).forEach(function (a) {
        var label = targetCategoryLabel(a && a.target_category);
        if (label && unique.indexOf(label) < 0) unique.push(label);
    });
    if (unique.length) return unique.join(" / ");
    var id = String(group && group.id || "").trim().replace(/[_-]+/g, " ");
    if (!id) return "Always";
    return id.replace(/\b\w/g, function (c) { return c.toUpperCase(); });
}

function queueArtifactGroupRank(group) {
    var haystack = [String(group && group.id || "").toLowerCase(), String(group && group.label || "").toLowerCase()]
        .concat((group && group.artifacts || []).map(function (a) { return String(a && a.target_category || "").toLowerCase(); }))
        .join(" ");
    return /\b(text[_\s-]*encoders?|clip)\b/.test(haystack) ? 0 : 1;
}

function queueArtifactRank(artifact) {
    if (isFullPrecisionTextEncoderArtifact(artifact)) return 0;
    if (isTextEncoderProjectionArtifact(artifact)) return 1;
    if (isQuantizedTextEncoderArtifact(artifact)) return 2;
    return 3;
}

function sortQueueArtifacts(artifacts) {
    if (!artifacts.some(function (a) { return isTextEncoderArtifact(a) || isTextEncoderProjectionArtifact(a); }))
        return artifacts;
    return artifacts.map(function (artifact, index) { return { artifact: artifact, index: index }; })
        .sort(function (a, b) {
            var rank = queueArtifactRank(a.artifact) - queueArtifactRank(b.artifact);
            if (rank) return rank;
            if (isQuantizedTextEncoderArtifact(a.artifact) && isQuantizedTextEncoderArtifact(b.artifact)) {
                var quant = quantizedTextEncoderSortRank(a.artifact) - quantizedTextEncoderSortRank(b.artifact);
                if (quant) return quant;
            }
            var size = artifactSizeBytes(b.artifact) - artifactSizeBytes(a.artifact);
            if (size) return size;
            return a.index - b.index;
        })
        .map(function (e) { return e.artifact; });
}

/// The model's additional-file groups for the RAM tier: [{ label, artifacts }].
function alwaysArtifactGroupsForModel(model, ramTier) {
    return (model && model.always || []).map(function (group, index) {
        var seen = {};
        var artifacts = (group && group.artifacts || []).filter(function (artifact) {
            if (!artifactSelectableInQueue(artifact, ramTier)) return false;
            var key = artifactChoiceKey(artifact);
            if (seen[key]) return false;
            seen[key] = true;
            return true;
        });
        return { index: index, rank: queueArtifactGroupRank(group), label: queueArtifactGroupLabel(group), artifacts: sortQueueArtifacts(artifacts) };
    }).filter(function (g) { return g.artifacts.length > 0; })
        .sort(function (a, b) { return (a.rank - b.rank) || (a.index - b.index); });
}

/// The chosen variant's own files for the RAM tier.
function selectedVariantArtifacts(variant, ramTier) {
    var seen = {};
    return (variant && variant.artifacts || []).filter(function (artifact) {
        if (!artifactSupportedOnRam(artifact, ramTier)) return false;
        var key = artifactChoiceKey(artifact);
        if (seen[key]) return false;
        seen[key] = true;
        return true;
    });
}

/// The artifact keys to download for a queued model.
function artifactKeysForDownload(model, variant, modelId, variantId, ramTier, choices) {
    var keys = [];
    selectedVariantArtifacts(variant, ramTier).forEach(function (a) {
        var k = artifactChoiceKey(a);
        if (keys.indexOf(k) < 0) keys.push(k);
    });
    alwaysArtifactGroupsForModel(model, ramTier).forEach(function (group) {
        group.artifacts.forEach(function (a) {
            if (artifactChoiceChecked(choices, modelId, variantId, a, ramTier, group)) {
                var k = artifactChoiceKey(a);
                if (keys.indexOf(k) < 0) keys.push(k);
            }
        });
    });
    return keys;
}

/// Total bytes of the files a queued model would download.
function downloadBytes(model, variant, modelId, variantId, ramTier, choices) {
    var total = 0;
    selectedVariantArtifacts(variant, ramTier).forEach(function (a) { total += artifactSizeBytes(a); });
    alwaysArtifactGroupsForModel(model, ramTier).forEach(function (group) {
        group.artifacts.forEach(function (a) {
            if (artifactChoiceChecked(choices, modelId, variantId, a, ramTier, group)) total += artifactSizeBytes(a);
        });
    });
    return total;
}
