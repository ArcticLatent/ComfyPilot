import ArcticHelper
import "ModelLogic.js" as Logic
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The Models section: the Arctic catalog's models by family or search, a
// variant each (best fit for the VRAM first), and a download queue where
// the additional files (text encoders, VAE, LoRAs…) can be ticked.
Rectangle {
    id: page

    /// The window (settings, catalog, snapshot, log).
    property var app
    readonly property var catalog: app ? app.catalog : null
    readonly property var models: catalog ? catalog.models || [] : []
    readonly property string root: app && app.settings ? app.settings.comfyui_root || "" : ""
    property string family: ""
    property string search: ""
    /// "" for the detected tier, else a vramOptions / ramOptions id.
    property string vramOverride: ""
    property string ramOverride: ""
    /// modelId → variantId for the queued models.
    property var selected: ({
    })
    /// modelId → variantId picked by hand (queued or not).
    property var picked: ({
    })
    /// Ticked / unticked additional files (Logic.artifactChoiceStateKey → bool).
    property var choices: ({
    })
    property var destination: null
    /// The queue's file cards are shown.
    property bool queueOpen: true
    property bool refreshing: false
    // ---- tiers ---------------------------------------------------------------
    readonly property string detectedVramTier: app && app.snapshot ? Logic.vramTierForMb(app.snapshot.nvidia_gpu_vram_mb) : ""
    readonly property string vramTier: {
        const option = Logic.vramOptions.find((o) => {
            return o.id === vramOverride;
        });
        return option ? option.tier : detectedVramTier;
    }
    readonly property var queuedModels: Object.keys(selected).map((id) => {
        return models.find((m) => {
            return m.id === id;
        });
    }).filter(Boolean)
    /// Shown models, grouped by family (catalog order within one).
    readonly property var visibleModels: {
        const order = Logic.families(models).map((f) => {
            return f.value;
        });
        return Logic.filterModels(models, family, search).map((m, i) => {
            return ({
                "model": m,
                "i": i
            });
        }).sort((a, b) => {
            return (order.indexOf(a.model.family) - order.indexOf(b.model.family)) || (a.i - b.i);
        }).map((e) => {
            return e.model;
        });
    }
    /// RAM thresholds: the ones the queued models share, else the shown ones'.
    readonly property var ramThresholds: Logic.sharedCustomRamThresholds(queuedModels) || Logic.sharedCustomRamThresholds(visibleModels)
    readonly property real ramGb: {
        const option = Logic.ramOptions.find((o) => {
            return o.id === ramOverride;
        });
        if (option)
            return option.gb;

        return app && app.snapshot && app.snapshot.total_ram_gb ? app.snapshot.total_ram_gb : NaN;
    }
    readonly property string ramTier: isNaN(ramGb) ? "" : Logic.ramTierForGb(ramGb, ramThresholds)
    /// The queue: [{ model, variant, variantId }], by name.
    readonly property var queue: queuedModels.map((model) => {
        const variantId = currentVariantId(model);
        return {
            "model": model,
            "variantId": variantId,
            "variant": (model.variants || []).find((v) => {
                return v.id === variantId;
            }) || null
        };
    }).sort((a, b) => {
        return a.model.display_name.localeCompare(b.model.display_name);
    })
    readonly property real queueBytes: queue.reduce((sum, item) => {
        return sum + Logic.downloadBytes(item.model, item.variant, item.model.id, item.variantId, ramTier, choices);
    }, 0)

    // ---- selection -----------------------------------------------------------
    function currentVariantId(model) {
        const variants = model.variants || [];
        const pick = picked[model.id];
        if (pick && variants.some((v) => {
            return v.id === pick;
        }))
            return pick;

        return Logic.recommendedVariantId(model, vramTier);
    }

    function isQueued(model) {
        return Object.prototype.hasOwnProperty.call(selected, model.id);
    }

    function setQueued(model, on) {
        const next = Object.assign({
        }, selected);
        if (on)
            next[model.id] = currentVariantId(model);
        else
            delete next[model.id];
        selected = next;
    }

    function pickVariant(model, variantId) {
        const nextPicked = Object.assign({
        }, picked);
        nextPicked[model.id] = variantId;
        picked = nextPicked;
        if (isQueued(model)) {
            const next = Object.assign({
            }, selected);
            next[model.id] = variantId;
            selected = next;
        }
    }

    function setChoice(key, on) {
        const next = Object.assign({
        }, choices);
        next[key] = on;
        choices = next;
    }

    function clearQueue() {
        selected = {
        };
        choices = {
        };
    }

    // ---- actions -------------------------------------------------------------
    function refreshDestination() {
        if (!root) {
            destination = null;
            return ;
        }
        Api.call("get_effective_download_destination", {
            "comfyuiRoot": root
        }).then((d) => {
            return page.destination = d;
        }).catch(() => {
            return page.destination = null;
        });
    }

    function refreshCatalog() {
        refreshing = true;
        Api.call("refresh_catalog").then((catalog) => {
            page.app.catalog = catalog;
            page.app.log("Catalog refreshed: " + (catalog.models || []).length + " models.", "ok");
        }).catch((err) => {
            return page.app.log("Catalog refresh failed: " + err, "error");
        }).then(Api.after(() => {
            return page.refreshing = false;
        }));
    }

    function download() {
        if (Downloads.busy) {
            Downloads.cancel();
            return ;
        }
        if (!queue.length)
            return ;

        const items = queue.map((item) => {
            return ({
                "modelId": item.model.id,
                "variantId": item.variantId,
                "selectedArtifactKeys": Logic.artifactKeysForDownload(item.model, item.variant, item.model.id, item.variantId, ramTier, choices)
            });
        });
        Downloads.begin("model", "Starting the model download…", queueBytes);
        app.openPanel("downloads");
        Api.call("download_model_assets_batch", {
            "request": {
                "items": items,
                "ramTier": ramTier || null,
                "vramTier": vramTier || null,
                "comfyuiRoot": root
            }
        }).then(() => {
            return page.app.log("Downloading " + items.length + " model" + (items.length === 1 ? "" : "s") + ".");
        }).catch((err) => {
            Downloads.end();
            page.app.log(String(err), "error");
        });
    }

    color: Theme.canvas
    onRootChanged: refreshDestination()
    Component.onCompleted: refreshDestination()
    // Development aid: `--queue=<id>,<id>` queues those models once the
    // catalog is there (screenshots).
    onModelsChanged: {
        const flag = Qt.application.arguments.find((a) => {
            return a.startsWith("--queue=");
        });
        if (!flag || !models.length || Object.keys(selected).length)
            return ;

        for (const id of flag.slice(8).split(",")) {
            const model = models.find((m) => {
                return m.id === id;
            });
            if (model)
                setQueued(model, true);

        }
    }

    // ---- layout --------------------------------------------------------------
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        PageHeader {
            title: "Models"
            subtitle: !page.root ? "Choose a ComfyUI install first (ComfyUI section)" : page.destination ? "Downloads into " + page.destination.effective_root + (page.destination.uses_shared_default ? " (shared models folder)" : "") : page.root
            subtitleIcon: "hard-drive-2"

            ComboField {
                width: 170
                model: [{
                    "id": "",
                    "label": "VRAM: " + (Logic.vramTierLabels[page.detectedVramTier] || "unknown")
                }].concat(Logic.vramOptions)
                textRole: "label"
                valueRole: "id"
                onActivated: (index) => {
                    return page.vramOverride = model[index].id;
                }
                ToolTip.visible: hovered
                ToolTip.text: "The GPU memory variants are picked for. Detected unless you choose."
            }

            ComboField {
                width: 160
                model: [{
                    "id": "",
                    "label": "RAM: " + (page.app && page.app.snapshot && page.app.snapshot.total_ram_gb ? Math.round(page.app.snapshot.total_ram_gb) + " GB" : "unknown")
                }].concat(Logic.ramOptions)
                textRole: "label"
                valueRole: "id"
                onActivated: (index) => {
                    return page.ramOverride = model[index].id;
                }
                ToolTip.visible: hovered
                ToolTip.text: "The system memory files are picked for. Detected unless you choose."
            }

            FlatButton {
                text: page.refreshing ? "Refreshing…" : "Refresh catalog"
                enabled: !page.refreshing
                onClicked: page.refreshCatalog()
            }

            FlatButton {
                text: "Hugging Face…"
                onClicked: hfDialog.open()
            }

        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // ---- the catalog ----
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 12

                // Search and families.
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 28
                    Layout.rightMargin: 20
                    Layout.topMargin: 18
                    spacing: 10

                    TextField {
                        id: searchField

                        Layout.fillWidth: true
                        implicitHeight: 36
                        leftPadding: 36
                        placeholderText: "Search models"
                        placeholderTextColor: Theme.textFaint
                        color: Theme.text
                        font.pixelSize: Theme.px(14)
                        onTextChanged: page.search = text

                        background: Rectangle {
                            radius: 10
                            color: Theme.raised
                            border.color: searchField.activeFocus ? Theme.accent : Theme.hairline

                            Icon {
                                name: "search"
                                color: Theme.textFaint
                                implicitWidth: 16
                                implicitHeight: 16
                                x: 12
                                anchors.verticalCenter: parent.verticalCenter
                            }

                        }

                    }

                    Flow {
                        Layout.fillWidth: true
                        spacing: 6

                        Repeater {
                            model: [{
                                "value": "",
                                "label": "All"
                            }].concat(Logic.families(page.models))

                            delegate: FlatButton {
                                required property var modelData

                                text: modelData.label
                                variant: page.family === modelData.value ? "selected" : "outline"
                                font.pixelSize: Theme.px(12)
                                onClicked: page.family = modelData.value
                            }

                        }

                    }

                }

                // The models as tiles.
                Flickable {
                    id: modelGrid

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    contentHeight: familyColumn.implicitHeight + 20
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds

                    ColumnLayout {
                        id: familyColumn

                        x: 28
                        width: parent.width - 48
                        spacing: 6

                        // One grid, in family order (the names carry the
                        // family; the chips above filter by it).
                        GridLayout {
                            id: tileGrid

                            readonly property real tileWidth: (familyColumn.width - (columns - 1) * columnSpacing) / columns

                            Layout.fillWidth: true
                            Layout.topMargin: 8
                            columns: Math.max(1, Math.floor((familyColumn.width + columnSpacing) / (250 + columnSpacing)))
                            columnSpacing: 10
                            rowSpacing: 10

                            Repeater {
                                model: page.visibleModels

                                delegate: ModelTile {
                                    required property var modelData

                                    Layout.preferredWidth: tileGrid.tileWidth
                                    model: modelData
                                    ctl: page
                                }

                            }

                        }

                        Label {
                            Layout.topMargin: 40
                            Layout.alignment: Qt.AlignHCenter
                            visible: page.visibleModels.length === 0
                            text: !page.catalog ? "Loading the catalog…" : "No models match."
                            color: Theme.textFaint
                            font.pixelSize: Theme.px(13)
                        }

                    }

                    ScrollBar.vertical: ScrollBar {
                    }

                }

            }

            // ---- the queue, along the bottom ----
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: queueHead.height + (cards.visible ? cards.height + 12 : 0)
                color: Theme.subtle

                Rectangle {
                    width: parent.width
                    height: 1
                    color: Theme.hairline
                }

                RowLayout {
                    id: queueHead

                    x: 24
                    width: parent.width - 44
                    height: 58
                    spacing: 10

                    PanelButton {
                        visible: page.queue.length > 0
                        iconName: page.queueOpen ? "arrow-down-s" : "arrow-right-s"
                        tip: page.queueOpen ? "Hide the queue's files" : "Show the queue's files"
                        implicitWidth: 28
                        implicitHeight: 28
                        onClicked: page.queueOpen = !page.queueOpen
                    }

                    Label {
                        text: "Download queue"
                        color: Theme.text
                        font.pixelSize: Theme.px(14)
                        font.weight: Font.DemiBold
                    }

                    Label {
                        Layout.fillWidth: true
                        text: page.queue.length ? page.queue.length + " model" + (page.queue.length === 1 ? "" : "s") + " · " + Logic.formatFileSize(page.queueBytes) : "Tick models above to queue them."
                        color: Theme.textMuted
                        font.pixelSize: Theme.px(13)
                        elide: Text.ElideRight
                    }

                    FlatButton {
                        visible: page.queue.length > 0
                        text: "Clear"
                        outlined: false
                        accent: Theme.textMuted
                        onClicked: page.clearQueue()
                    }

                    FlatButton {
                        Layout.preferredWidth: 220
                        text: Downloads.busy && Downloads.kind === "model" ? "Cancel download" : page.queue.length ? "Download " + Logic.formatFileSize(page.queueBytes) : "Download"
                        variant: Downloads.busy && Downloads.kind === "model" ? "stop" : "primary"
                        font.pixelSize: Theme.px(14)
                        padding: 8
                        enabled: (Downloads.busy && Downloads.kind === "model") || (!Downloads.busy && page.queue.length > 0 && page.root !== "")
                        onClicked: page.download()
                    }

                }

                // The queued models, one wide card each.
                ListView {
                    id: cards

                    anchors.top: queueHead.bottom
                    x: 20
                    width: parent.width - 40
                    height: Math.min(contentHeight, 330)
                    visible: page.queueOpen && page.queue.length > 0
                    spacing: 8
                    clip: true
                    model: page.queue
                    boundsBehavior: Flickable.StopAtBounds

                    delegate: QueueCard {
                        required property var modelData

                        width: ListView.view.width
                        item: modelData
                        ctl: page
                    }

                    ScrollBar.vertical: ScrollBar {
                    }

                }

            }

        }

    }

    // Hugging Face token and the Xet downloader.
    AppDialog {
        id: hfDialog

        title: "Hugging Face"
        standardButtons: Dialog.Close
        // The Xet downloader's check (get_hf_xet_preflight): null, "checking" or its result.
        property var xetCheck: null

        function checkXet() {
            if (!(page.app && page.app.settings && page.app.settings.hf_xet_enabled)) {
                xetCheck = null;
                return ;
            }
            xetCheck = "checking";
            Api.call("get_hf_xet_preflight").then((result) => {
                return hfDialog.xetCheck = result;
            }).catch((err) => {
                return hfDialog.xetCheck = {
                    "hf_cli_available": false,
                    "detail": String(err)
                };
            });
        }

        onOpened: {
            tokenField.text = page.app && page.app.settings ? page.app.settings.hf_token || "" : "";
            checkXet();
        }

        ColumnLayout {
            width: 440
            spacing: 12

            Label {
                Layout.fillWidth: true
                text: "Most models download without one. A token is needed for gated models (you accept their license first) and private ones."
                color: Theme.textMuted
                font.pixelSize: Theme.px(12.5)
                wrapMode: Text.Wrap
            }

            StepList {
                Layout.fillWidth: true
                steps: ["Sign in at huggingface.co (or create a free account).", "Open Settings, then Access Tokens.", "Click Create new token, choose the Read type, name it (like ComfyPilot) and create it.", "Copy the token (it starts with hf_), paste it below and click Save.", "For a gated model, open its page on huggingface.co with the same account and accept its license."]
                linkText: "Open Hugging Face access tokens"
                linkUrl: "https://huggingface.co/settings/tokens"
            }

            RowLayout {
                Layout.fillWidth: true

                AppTextField {
                    id: tokenField

                    Layout.fillWidth: true
                    echoMode: TextInput.Password
                    placeholderText: "Paste your token (hf_…)"
                }

                FlatButton {
                    text: "Save"
                    onClicked: Api.call("save_hf_token", {
                        "token": tokenField.text
                    }).then((settings) => {
                        page.app.settings = settings;
                        page.app.log(tokenField.text.trim() ? "Hugging Face token saved." : "Hugging Face token removed.", "ok");
                    }).catch((err) => {
                        return page.app.log("Saving the token failed: " + err, "error");
                    })
                }

            }

            Label {
                Layout.fillWidth: true
                text: "It's sent only to huggingface.co, and kept in your system keychain when there is one. Leave it empty and save to remove it."
                color: Theme.textFaint
                font.pixelSize: Theme.px(11.5)
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4

                AppSwitch {
                    checked: page.app && page.app.settings ? page.app.settings.hf_xet_enabled === true : false
                    onToggled: Api.call("set_hf_xet_enabled", {
                        "enabled": checked
                    }).then((settings) => {
                        page.app.settings = settings;
                        hfDialog.checkXet();
                    }).catch((err) => {
                        return page.app.log("Xet setting failed: " + err, "error");
                    })
                }

                Label {
                    Layout.fillWidth: true
                    text: "Use Hugging Face Xet (experimental): can be faster, but can stall on some networks."
                    color: Theme.text
                    font.pixelSize: Theme.px(13)
                    wrapMode: Text.Wrap
                }

            }

            // Whether Xet can work here: the hf tool (uvx hf or hf) and hf_xet.
            RowLayout {
                readonly property bool ready: hfDialog.xetCheck !== null && hfDialog.xetCheck !== "checking" && hfDialog.xetCheck.hf_cli_available === true && hfDialog.xetCheck.hf_xet_installed === true

                Layout.fillWidth: true
                Layout.leftMargin: 44
                visible: hfDialog.xetCheck !== null
                spacing: 8

                Icon {
                    Layout.alignment: Qt.AlignTop
                    Layout.topMargin: 1
                    visible: hfDialog.xetCheck !== "checking"
                    name: parent.ready ? "checkbox-circle" : "alert"
                    color: parent.ready ? Theme.success : Theme.warning
                    implicitWidth: 14
                    implicitHeight: 14
                }

                Label {
                    Layout.fillWidth: true
                    text: hfDialog.xetCheck === "checking" ? "Checking the Xet downloader…" : hfDialog.xetCheck ? String(hfDialog.xetCheck.detail || "") : ""
                    textFormat: Text.PlainText
                    color: hfDialog.xetCheck === "checking" ? Theme.textFaint : parent.ready ? Theme.textMuted : Theme.warning
                    font.pixelSize: Theme.px(12)
                    wrapMode: Text.Wrap
                }

            }

        }

    }

    // A model as a tile: tick box and name, a line about the chosen variant
    // (its note, and whether it fits the VRAM), and the variant picker.
    // Clicking the tile queues or unqueues it.
    component ModelTile: Rectangle {
        id: tile

        property var model
        property var ctl
        readonly property var variants: ctl ? Logic.variantsForModel(model, ctl.vramTier) : []
        readonly property bool alwaysOnly: variants.length === 0 && Logic.modelHasAlwaysArtifacts(model)
        readonly property bool queueable: variants.length > 0 || alwaysOnly
        readonly property bool queued: ctl ? ctl.isQueued(model) : false
        readonly property var variant: ctl ? variants.find((v) => {
            return v.id === ctl.currentVariantId(model);
        }) || null : null
        readonly property string fitText: {
            if (!variant || !ctl || !ctl.vramTier)
                return "";

            const want = Logic.tierStrength[variant.tier], have = Logic.tierStrength[ctl.vramTier];
            if (want === undefined || have === undefined)
                return "";

            return want > have ? "may need more VRAM" : "fits your GPU";
        }

        implicitHeight: 92
        radius: 10
        color: queued ? Theme.accentSoft : tileHover.hovered && queueable ? Theme.controlFillHover : Theme.raised
        border.color: queued ? Theme.alpha(Theme.primary, 0.6) : Theme.hairline
        opacity: queueable ? 1 : 0.6

        HoverHandler {
            id: tileHover

            cursorShape: tile.queueable ? Qt.PointingHandCursor : Qt.ArrowCursor
        }

        TapHandler {
            enabled: tile.queueable
            onTapped: tile.ctl.setQueued(tile.model, !tile.queued)
        }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 10
            anchors.leftMargin: 12
            spacing: 4

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                AppCheckBox {
                    padding: 0
                    checked: tile.queued
                    enabled: tile.queueable
                    onToggled: tile.ctl.setQueued(tile.model, checked)
                }

                Label {
                    Layout.fillWidth: true
                    text: tile.model.display_name
                    color: Theme.text
                    font.pixelSize: Theme.px(13.5)
                    font.weight: Font.Medium
                    elide: Text.ElideRight
                }

            }

            Label {
                Layout.fillWidth: true
                text: tile.alwaysOnly ? "All required files" : !tile.queueable ? "Nothing to download yet" : [tile.variant ? tile.variant.note : "", tile.fitText].filter(Boolean).join(" · ")
                color: tile.fitText === "may need more VRAM" ? Theme.warning : Theme.textMuted
                font.pixelSize: Theme.px(11.5)
                elide: Text.ElideRight
            }

            ComboField {
                Layout.fillWidth: true
                implicitHeight: 28
                visible: tile.variants.length > 0
                font.pixelSize: Theme.px(12)
                model: tile.variants.map((v) => {
                    return {
                        "id": v.id,
                        "label": Logic.modelVariantLabel(v, tile.ctl.vramTier)
                    };
                })
                textRole: "label"
                valueRole: "id"
                displayText: tile.variant ? [tile.variant.model_size, tile.variant.quantization, Logic.vramTierLabels[tile.variant.tier] || ""].filter(Boolean).join(" · ") : ""
                onActivated: (index) => {
                    return tile.ctl.pickVariant(tile.model, tile.variants[index].id);
                }

                Binding on currentIndex {
                    value: Math.max(0, tile.variants.findIndex((v) => {
                        return tile.ctl && v.id === tile.ctl.currentVariantId(tile.model);
                    }))
                }

            }

        }

    }

    // One queued model, laid out across the width: a header line, then its
    // files in sections (the model's own files, then the additional files,
    // groups with the same label under one heading), packed into columns
    // masonry-style: each section goes under the shortest column so far, so a
    // long list doesn't leave the other columns with empty space below them.
    component QueueCard: Rectangle {
        id: card

        property var item
        property var ctl
        readonly property var ownFiles: ctl ? Logic.selectedVariantArtifacts(item.variant, ctl.ramTier) : []
        readonly property var groups: ctl ? Logic.alwaysArtifactGroupsForModel(item.model, ctl.ramTier) : []
        readonly property var sections: {
            const out = [];
            if (ownFiles.length)
                out.push({
                    "label": "Model",
                    "own": true,
                    "groups": [{
                        "artifacts": ownFiles
                    }]
                });
            for (const group of groups) {
                const same = out.find((section) => {
                    return !section.own && section.label === group.label;
                });
                if (same)
                    same.groups.push(group);
                else
                    out.push({
                        "label": group.label,
                        "own": false,
                        "groups": [group]
                    });
            }
            return out;
        }
        // As many ~300 px columns as fit (no more than there are sections).
        readonly property int columns: Math.max(1, Math.min(sections.length, Math.floor((width - 32 + 24) / 324)))
        readonly property real columnWidth: (width - 32 - (columns - 1) * 24) / columns
        readonly property var packed: {
            const cols = [], heights = [];
            for (let i = 0; i < columns; i++) {
                cols.push([]);
                heights.push(0);
            }
            for (const section of sections) {
                // Height in lines: the heading, a line per file, a bit more for a hint line.
                let lines = 1.5;
                for (const group of section.groups)
                    for (const artifact of group.artifacts)
                        lines += artifact.ram_bucket || Logic.artifactRuntimeRamBytes(artifact) ? 1.8 : 1.1;
                const k = heights.indexOf(Math.min(...heights));
                cols[k].push(section);
                heights[k] += lines + 1;
            }
            return cols;
        }

        implicitHeight: cardColumn.implicitHeight + 24
        radius: 12
        color: Theme.raised
        border.color: Theme.hairline

        ColumnLayout {
            id: cardColumn

            x: 16
            y: 12
            width: parent.width - 32
            spacing: 10

            RowLayout {
                Layout.fillWidth: true
                spacing: 10

                Label {
                    text: card.item.model.display_name
                    color: Theme.text
                    font.pixelSize: Theme.px(14)
                    font.weight: Font.DemiBold
                }

                Label {
                    Layout.fillWidth: true
                    text: card.item.variant ? Logic.modelVariantLabel(card.item.variant, "") : "All required files"
                    color: Theme.textMuted
                    font.pixelSize: Theme.px(12.5)
                    elide: Text.ElideRight
                }

                Label {
                    text: card.ctl ? Logic.formatFileSize(Logic.downloadBytes(card.item.model, card.item.variant, card.item.model.id, card.item.variantId, card.ctl.ramTier, card.ctl.choices)) : ""
                    color: Theme.textMuted
                    font.family: Theme.mono
                    font.pixelSize: Theme.px(12)
                }

                PanelButton {
                    iconName: "close"
                    tip: "Remove from the queue"
                    implicitWidth: 26
                    implicitHeight: 26
                    onClicked: card.ctl.setQueued(card.item.model, false)
                }

            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 24

                Repeater {
                    model: card.packed

                    delegate: ColumnLayout {
                        id: column

                        required property var modelData

                        Layout.preferredWidth: card.columnWidth
                        Layout.maximumWidth: card.columnWidth
                        Layout.alignment: Qt.AlignTop
                        spacing: 14

                        Repeater {
                            model: column.modelData

                            delegate: ColumnLayout {
                                id: section

                                required property var modelData

                                Layout.fillWidth: true
                                spacing: 3

                                Label {
                                    Layout.fillWidth: true
                                    text: section.modelData.label
                                    color: Theme.label
                                    font.pixelSize: Theme.px(11)
                                    font.weight: Font.Medium
                                    font.capitalization: Font.AllUppercase
                                    font.letterSpacing: Theme.px(11) * 0.06
                                    elide: Text.ElideRight
                                }

                                Repeater {
                                    model: section.modelData.groups

                                    delegate: ColumnLayout {
                                        id: group

                                        required property var modelData

                                        Layout.fillWidth: true
                                        spacing: 3

                                        Repeater {
                                            model: group.modelData.artifacts

                                            delegate: FileLine {
                                                required property var modelData

                                                Layout.fillWidth: true
                                                artifact: modelData
                                                fixed: section.modelData.own
                                                choiceKey: section.modelData.own ? "" : Logic.artifactChoiceStateKey(card.item.model.id, card.item.variantId, modelData)
                                                checked: section.modelData.own || !card.ctl ? true : Logic.artifactChoiceChecked(card.ctl.choices, card.item.model.id, card.item.variantId, modelData, card.ctl.ramTier, group.modelData)
                                                hint: !section.modelData.own && modelData.ram_bucket && card.ctl ? Logic.ramBucketLabel(modelData.ram_bucket, modelData, card.ctl.ramThresholds) : ""
                                                onChosen: (on) => {
                                                    return card.ctl.setChoice(choiceKey, on);
                                                }
                                            }

                                        }

                                    }

                                }

                            }

                        }

                    }

                }

            }

        }

    }

    // A file in the queue: always downloaded (`fixed`), or a box to tick.
    component FileLine: RowLayout {
        id: line

        property var artifact
        property bool fixed: false
        property bool checked: true
        property string choiceKey: ""
        property string hint: ""

        signal chosen(bool on)

        spacing: 8

        AppCheckBox {
            visible: !line.fixed
            checked: line.checked
            onToggled: line.chosen(checked)
        }

        // Lined up with the check boxes (28 px wide, box centred at 10 px).
        Icon {
            visible: line.fixed
            name: "check"
            color: Theme.primary
            implicitWidth: 14
            implicitHeight: 14
            Layout.leftMargin: 3
            Layout.rightMargin: 11
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 0

            Label {
                Layout.fillWidth: true
                text: Logic.artifactDisplayBaseName(line.artifact)
                color: line.fixed || line.checked ? Theme.text : Theme.textMuted
                font.pixelSize: Theme.px(12.5)
                elide: Text.ElideMiddle
            }

            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: [line.hint, Logic.artifactRuntimeRamBytes(line.artifact) ? "~" + Logic.formatFileSize(Logic.artifactRuntimeRamBytes(line.artifact)) + " RAM while running" : ""].filter(Boolean).join(" · ")
                color: Theme.textFaint
                font.pixelSize: Theme.px(11)
                elide: Text.ElideRight
            }

        }

        Label {
            text: Logic.formatFileSize(Logic.artifactSizeBytes(line.artifact))
            color: Theme.textFaint
            font.family: Theme.mono
            font.pixelSize: Theme.px(11)
        }

    }

}
