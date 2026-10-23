// The Workflows section, in two views. Library: Arctic Latent's catalog
// workflows (on Patreon, or a direct download), and the dependency check:
// pick a workflow saved in the selected ComfyUI install, or any workflow
// file, to see the models and custom node packs it needs, which are
// missing, and get them. Civitai: a browser of Civitai's workflows that
// come as a .json file (CivitaiBrowser), downloaded into the install's
// workflows and handed to the check.

import ArcticHelper
import QtCore
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

// Workflow files come from anywhere: everything read from them is shown as
// plain text, and downloads and installs ask first.
Rectangle {
    id: page

    /// The window (settings, catalog, log, openPanel).
    property var app
    readonly property string root: app && app.settings ? app.settings.comfyui_root || "" : ""
    readonly property var workflows: app && app.catalog ? app.catalog.workflows || [] : []
    property var saved: []
    property bool savedLoading: false
    /// The checked workflow file, and its report (check_workflow_dependencies).
    property string checkedPath: ""
    property var report: null
    property bool checking: false
    property string checkError: ""
    /// The pack being installed ("" when none).
    property string installingPack: ""
    /// A download started here: check again when it ends.
    property bool downloadPending: false
    readonly property var missingModels: report ? report.models.filter((m) => {
        return !m.present;
    }) : []
    readonly property var missingPacks: report ? report.packs.filter((p) => {
        return !p.installed;
    }) : []
    property bool devChecked: false

    // ---- helpers ---------------------------------------------------------------
    function workflowName(w) {
        return String(w.display_name || w.workflow_name || w.name || w.title || w.id || "Workflow").trim();
    }

    function safeHttpUrl(url) {
        return /^https?:\/\/[^\s]+$/i.test(String(url || "").trim());
    }

    /// Where the workflow lives when it isn't a direct JSON download (a Patreon post).
    function externalUrl(w) {
        const direct = String(w.patreon_url || w.workflow_url || w.workflow_link_url || "").trim();
        if (direct)
            return safeHttpUrl(direct) ? direct : "";

        const legacy = String(w.workflow_json_url || "").trim();
        if (!safeHttpUrl(legacy))
            return "";

        const lower = legacy.toLowerCase();
        return lower.includes("patreon.com") || !lower.split(/[?#]/)[0].endsWith(".json") ? legacy : "";
    }

    function hostOf(url) {
        const m = String(url).match(/^https?:\/\/([^\/?#]+)/i);
        return m ? m[1] : String(url);
    }

    function baseName(path) {
        return String(path).split(/[\\/]/).filter(Boolean).pop() || String(path);
    }

    function localPath(url) {
        let s = decodeURIComponent(String(url).replace(/^file:\/\//, ""));
        if (/^\/[A-Za-z]:/.test(s))
            s = s.slice(1);

        return s;
    }

    // ---- loading ---------------------------------------------------------------
    function refreshSaved() {
        if (!root) {
            saved = [];
            return ;
        }
        savedLoading = true;
        Api.call("list_saved_workflows", {
            "comfyuiRoot": root
        }).then((list) => {
            return page.saved = list;
        }).catch(() => {
            return page.saved = [];
        }).then(Api.after(() => {
            return page.savedLoading = false;
        }));
    }

    function check(path) {
        if (!path)
            return ;

        checkedPath = path;
        checking = true;
        checkError = "";
        Api.call("check_workflow_dependencies", {
            "path": path,
            "comfyuiRoot": root || null
        }).then((result) => {
            if (path !== page.checkedPath)
                return ;

            // Missing first.
            result.models.sort((a, b) => {
                return Number(a.present) - Number(b.present);
            });
            result.packs.sort((a, b) => {
                return Number(a.installed) - Number(b.installed);
            });
            page.report = result;
        }).catch((err) => {
            if (path === page.checkedPath) {
                page.report = null;
                page.checkError = String(err);
            }
        }).then(Api.after(() => {
            return page.checking = false;
        }));
    }

    // ---- actions ---------------------------------------------------------------
    function getWorkflow(w) {
        const url = externalUrl(w);
        if (url) {
            Api.call("open_external_url", {
                "url": url
            }).catch((err) => {
                return page.app.log("Opening the workflow link failed: " + err, "error");
            });
            return ;
        }
        if (Downloads.busy)
            return ;

        Downloads.begin("workflow", "Starting " + workflowName(w) + "…", 0);
        downloadPending = true;
        app.openPanel("downloads");
        Api.call("download_workflow_asset", {
            "workflowId": w.id,
            "comfyuiRoot": root || null
        }).catch((err) => {
            page.downloadPending = false;
            Downloads.end();
            page.app.log(String(err), "error");
        });
    }

    function downloadModel(model) {
        confirm.ask("Download " + model.name + " from " + hostOf(model.url) + " into models/" + model.directory + "?", "Download").then((ok) => {
            if (!ok || Downloads.busy)
                return ;

            Downloads.begin("model", "Starting " + model.name + "…", 0);
            page.downloadPending = true;
            page.app.openPanel("downloads");
            Api.call("download_workflow_model", {
                "url": model.url,
                "directory": model.directory,
                "name": model.name,
                "comfyuiRoot": page.root || null
            }).catch((err) => {
                page.downloadPending = false;
                Downloads.end();
                page.app.log("Downloading " + model.name + " failed: " + err, "error");
            });
        });
    }

    function installPack(pack) {
        confirm.ask("Install the custom node pack " + pack.repository + "?\n\nComfyUI is stopped for the install and started again afterwards if it was running.", "Install").then((ok) => {
            if (!ok)
                return ;

            page.installingPack = pack.id;
            page.app.log("Installing " + pack.id + " from " + pack.repository + "…");
            Api.call("install_workflow_node_pack", {
                "repository": pack.repository,
                "comfyuiRoot": page.root || null
            }).then((message) => {
                page.app.log(message, "ok");
                page.check(page.checkedPath);
            }).catch((err) => {
                return page.app.log("Installing " + pack.id + " failed: " + err, "error");
            }).then(Api.after(() => {
                return page.installingPack = "";
            }));
        });
    }

    color: Theme.canvas
    Component.onCompleted: {
        const devView = app ? app.argValue("--workflows-view=") : "";
        if (devView)
            prefs.view = devView;

    }
    onVisibleChanged: {
        if (visible) {
            refreshSaved();
        }
    }
    onRootChanged: {
        report = null;
        checkedPath = "";
        checkError = "";
        if (visible)
            refreshSaved();

        // Development aid: `--check=<workflow.json>` checks that file (screenshots).
        const devCheck = app ? app.argValue("--check=") : "";
        if (root && devCheck && !devChecked) {
            devChecked = true;
            check(devCheck);
        }
    }

    Settings {
        id: prefs

        /// "civitai" (the default) or "library".
        property string view: "civitai"

        category: "workflows"
    }

    Connections {
        function onEnded(kind, outcome, message) {
            if (!page.downloadPending)
                return ;

            page.downloadPending = false;
            if (kind === "workflow")
                page.refreshSaved();
            else if (page.checkedPath)
                page.check(page.checkedPath);
        }

        target: Downloads
    }

    FileDialog {
        id: fileDialog

        title: "Choose a ComfyUI workflow"
        nameFilters: ["ComfyUI workflow (*.json)", "All files (*)"]
        onAccepted: page.check(page.localPath(selectedFile))
    }

    ConfirmDialog {
        id: confirm

        title: "Workflow"
    }

    // ---- layout ----------------------------------------------------------------
    StackLayout {
        anchors.fill: parent
        currentIndex: prefs.view === "civitai" ? 1 : 0

        ColumnLayout {
            spacing: 0

            PageHeader {
                title: "Workflows"
                subtitle: page.workflows.length + (page.workflows.length === 1 ? " workflow" : " workflows") + " in the catalog" + (page.root ? " · checking against " + page.baseName(page.root) : "")
                subtitleIcon: "node-tree"

                ViewSwitch {
                }

                FlatButton {
                    text: "Check a file…"
                    onClicked: fileDialog.open()
                }

            }

            Flickable {
                id: scroller

                Layout.fillWidth: true
                Layout.fillHeight: true
                contentHeight: body.implicitHeight + 48
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                ColumnLayout {
                    id: body

                    x: 28
                    y: 24
                    width: scroller.width - 48
                    spacing: 28

                    // ---- the catalog ----
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        visible: page.workflows.length > 0

                        SectionLabel {
                            text: "Arctic Latent workflows"
                        }

                        GridLayout {
                            id: catalogGrid

                            readonly property int cols: Math.max(1, Math.min(page.workflows.length, Math.floor((body.width + columnSpacing) / (420 + columnSpacing))))

                            Layout.fillWidth: true
                            columns: cols
                            columnSpacing: 14
                            rowSpacing: 14

                            Repeater {
                                model: page.workflows

                                // A preview on the left, the name and buttons on the right.
                                delegate: Rectangle {
                                    id: wcard

                                    required property var modelData
                                    readonly property string link: page.externalUrl(modelData)

                                    Layout.preferredWidth: (body.width - catalogGrid.columnSpacing * (catalogGrid.cols - 1)) / catalogGrid.cols
                                    Layout.maximumWidth: 560
                                    implicitHeight: 128
                                    radius: 12
                                    color: Theme.raised
                                    border.color: Theme.hairline

                                    Rectangle {
                                        id: wthumb

                                        x: 8
                                        y: 8
                                        width: (parent.height - 16) * 16 / 9
                                        height: parent.height - 16
                                        radius: 8
                                        color: Theme.subtle
                                        clip: true

                                        // The preview, or the "No preview" picture (also when it
                                        // can't be loaded).
                                        Image {
                                            property bool failed: false
                                            readonly property string url: page.safeHttpUrl(wcard.modelData.preview_image_url) ? wcard.modelData.preview_image_url : ""

                                            anchors.fill: parent
                                            source: url !== "" && !failed ? url : Theme.noPreview
                                            sourceSize.width: 400
                                            fillMode: Image.PreserveAspectCrop
                                            asynchronous: true
                                            onStatusChanged: if (status === Image.Error && url !== "")
                                                failed = true
                                        }

                                    }

                                    ColumnLayout {
                                        anchors.left: wthumb.right
                                        anchors.right: parent.right
                                        anchors.top: parent.top
                                        anchors.bottom: parent.bottom
                                        anchors.margins: 12
                                        anchors.leftMargin: 14
                                        spacing: 4

                                        Label {
                                            Layout.fillWidth: true
                                            text: page.workflowName(wcard.modelData)
                                            color: Theme.text
                                            font.pixelSize: Theme.px(14)
                                            font.weight: Font.Medium
                                            elide: Text.ElideRight
                                        }

                                        Label {
                                            Layout.fillWidth: true
                                            text: [wcard.modelData.family || "", wcard.link ? "on Patreon" : "direct download"].filter(Boolean).join(" · ")
                                            color: Theme.textMuted
                                            font.pixelSize: Theme.px(12)
                                            elide: Text.ElideRight
                                        }

                                        Item {
                                            Layout.fillHeight: true
                                        }

                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 6

                                            FlatButton {
                                                variant: "primary"
                                                text: wcard.link ? (wcard.link.toLowerCase().includes("patreon.com") ? "Get on Patreon" : "Open the page") : "Download"
                                                enabled: wcard.link !== "" || (!Downloads.busy && page.root !== "")
                                                onClicked: page.getWorkflow(wcard.modelData)
                                                ToolTip.visible: hovered && !wcard.link && page.root === ""
                                                ToolTip.text: "Choose a ComfyUI install first."
                                            }

                                            FlatButton {
                                                visible: page.safeHttpUrl(wcard.modelData.youtube_url)
                                                text: "Tutorial"
                                                onClicked: Api.call("open_external_url", {
                                                    "url": wcard.modelData.youtube_url
                                                })
                                            }

                                        }

                                    }

                                }

                            }

                        }

                        Label {
                            Layout.fillWidth: true
                            text: "Got a workflow? Save it in ComfyUI or pick its file, then check it below. More workflows: the Civitai view."
                            color: Theme.textFaint
                            font.pixelSize: Theme.px(12)
                        }

                    }

                    // ---- the check ----
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 10

                        SectionLabel {
                            text: "Check a workflow"
                        }

                        Label {
                            Layout.fillWidth: true
                            text: "The models and custom node packs a workflow needs, and which ones " + (page.root ? page.baseName(page.root) : "the selected install") + " is missing."
                            color: Theme.textFaint
                            font.pixelSize: Theme.px(12)
                            wrapMode: Text.Wrap
                        }

                        GridLayout {
                            Layout.fillWidth: true
                            columns: body.width >= 860 ? 2 : 1
                            columnSpacing: 14
                            rowSpacing: 14

                            // Saved workflows.
                            Rectangle {
                                Layout.preferredWidth: parent.columns === 2 ? 300 : -1
                                Layout.fillWidth: parent.columns === 1
                                Layout.alignment: Qt.AlignTop
                                implicitHeight: savedCol.implicitHeight + 16
                                radius: 12
                                color: Theme.raised
                                border.color: Theme.hairline

                                ColumnLayout {
                                    id: savedCol

                                    x: 8
                                    y: 8
                                    width: parent.width - 16
                                    spacing: 2

                                    RowLayout {
                                        Layout.fillWidth: true
                                        Layout.leftMargin: 6
                                        Layout.bottomMargin: 4

                                        Label {
                                            Layout.fillWidth: true
                                            text: "Saved in ComfyUI"
                                            color: Theme.textMuted
                                            font.pixelSize: Theme.px(12.5)
                                            font.weight: Font.Medium
                                        }

                                        PanelButton {
                                            iconName: "refresh"
                                            tip: "Look again"
                                            implicitWidth: 26
                                            implicitHeight: 26
                                            onClicked: page.refreshSaved()
                                        }

                                    }

                                    Label {
                                        Layout.fillWidth: true
                                        Layout.leftMargin: 6
                                        Layout.bottomMargin: 6
                                        visible: page.saved.length === 0
                                        text: !page.root ? "Choose a ComfyUI install to see its saved workflows." : page.savedLoading ? "Looking…" : "No workflows saved in this install yet (user/default/workflows)."
                                        color: Theme.textFaint
                                        font.pixelSize: Theme.px(12.5)
                                        wrapMode: Text.Wrap
                                    }

                                    ListView {
                                        Layout.fillWidth: true
                                        // Shorter when the report goes below it.
                                        Layout.preferredHeight: Math.min(contentHeight, body.width >= 860 ? 380 : 200)
                                        visible: page.saved.length > 0
                                        clip: true
                                        boundsBehavior: Flickable.StopAtBounds
                                        model: page.saved

                                        delegate: ItemDelegate {
                                            id: savedRow

                                            required property var modelData

                                            width: ListView.view.width
                                            implicitHeight: 32
                                            leftPadding: 8
                                            rightPadding: 8
                                            topPadding: 0
                                            bottomPadding: 0
                                            onClicked: page.check(savedRow.modelData.path)

                                            contentItem: Label {
                                                text: savedRow.modelData.name.replace(/\.json$/i, "")
                                                color: page.checkedPath === savedRow.modelData.path ? Theme.text : Theme.textMuted
                                                font.pixelSize: Theme.px(13)
                                                elide: Text.ElideMiddle
                                                verticalAlignment: Text.AlignVCenter
                                            }

                                            background: Rectangle {
                                                radius: 7
                                                color: page.checkedPath === savedRow.modelData.path ? Theme.rowSelected : savedRow.hovered ? Theme.rowHover : "transparent"
                                            }

                                        }

                                        ScrollBar.vertical: ScrollBar {
                                        }

                                    }

                                    FlatButton {
                                        Layout.fillWidth: true
                                        Layout.topMargin: 6
                                        text: "Check another file…"
                                        onClicked: fileDialog.open()
                                    }

                                }

                            }

                            // The report.
                            Rectangle {
                                Layout.fillWidth: true
                                Layout.alignment: Qt.AlignTop
                                implicitHeight: reportCol.implicitHeight + 32
                                radius: 12
                                color: Theme.raised
                                border.color: Theme.hairline

                                ColumnLayout {
                                    id: reportCol

                                    x: 16
                                    y: 16
                                    width: parent.width - 32
                                    spacing: 14

                                    // Nothing checked yet / checking / failed.
                                    Label {
                                        Layout.fillWidth: true
                                        visible: page.report === null
                                        text: page.checking ? "Checking " + page.baseName(page.checkedPath) + "…" : page.checkError ? "The check failed: " + page.checkError : "Pick a saved workflow or a workflow file to check it."
                                        color: page.checkError && !page.checking ? Theme.error : Theme.textFaint
                                        font.pixelSize: Theme.px(13)
                                        wrapMode: Text.Wrap
                                    }

                                    // Head: name, summary, check again.
                                    RowLayout {
                                        Layout.fillWidth: true
                                        visible: page.report !== null
                                        spacing: 12

                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 3

                                            Label {
                                                Layout.fillWidth: true
                                                text: page.report ? String(page.report.workflow) : ""
                                                textFormat: Text.PlainText
                                                color: Theme.text
                                                font.pixelSize: Theme.px(15)
                                                font.weight: Font.DemiBold
                                                elide: Text.ElideMiddle
                                            }

                                            Label {
                                                readonly property bool complete: page.report !== null && page.missingModels.length === 0 && page.missingPacks.length === 0 && page.report.unresolved_node_types.length === 0

                                                Layout.fillWidth: true
                                                text: !page.report ? "" : complete ? "Everything this workflow needs is installed." : [page.missingModels.length + " of " + page.report.models.length + " models missing", page.missingPacks.length + " of " + page.report.packs.length + " custom node packs missing"].join(" · ")
                                                color: complete ? Theme.success : Theme.textMuted
                                                font.pixelSize: Theme.px(12.5)
                                                wrapMode: Text.Wrap
                                            }

                                        }

                                        FlatButton {
                                            text: page.checking ? "Checking…" : "Check again"
                                            enabled: !page.checking
                                            onClicked: page.check(page.checkedPath)
                                        }

                                    }

                                    // Models.
                                    SectionLabel {
                                        visible: page.report !== null && page.report.models.length > 0
                                        text: "Models"
                                    }

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        visible: page.report !== null && page.report.models.length > 0
                                        spacing: 6

                                        Repeater {
                                            model: page.report ? page.report.models : []

                                            delegate: RowLayout {
                                                id: modelRow

                                                required property var modelData
                                                readonly property bool canDownload: !modelData.present && !!modelData.url && !!modelData.directory

                                                Layout.fillWidth: true
                                                Layout.minimumHeight: 28
                                                spacing: 8

                                                StatusIcon {
                                                    ok: modelRow.modelData.present
                                                }

                                                Label {
                                                    Layout.fillWidth: true
                                                    text: modelRow.modelData.name
                                                    textFormat: Text.PlainText
                                                    color: modelRow.modelData.present ? Theme.textMuted : Theme.text
                                                    font.pixelSize: Theme.px(13)
                                                    elide: Text.ElideMiddle
                                                }

                                                Chip {
                                                    visible: !!modelRow.modelData.directory
                                                    text: modelRow.modelData.directory || ""
                                                    Layout.maximumWidth: 180
                                                }

                                                Label {
                                                    visible: !modelRow.modelData.present && !modelRow.canDownload
                                                    text: modelRow.modelData.directory ? "No download link" : "Folder unknown"
                                                    color: Theme.textFaint
                                                    font.pixelSize: Theme.px(12)
                                                }

                                                FlatButton {
                                                    visible: modelRow.canDownload
                                                    text: modelRow.modelData.url_source === "catalog" ? "Download (catalog)" : "Download"
                                                    enabled: !Downloads.busy && page.root !== ""
                                                    onClicked: page.downloadModel(modelRow.modelData)
                                                    ToolTip.visible: hovered
                                                    ToolTip.text: String(modelRow.modelData.url)
                                                }

                                            }

                                        }

                                    }

                                    // Custom node packs.
                                    SectionLabel {
                                        visible: page.report !== null && page.report.packs.length > 0
                                        text: "Custom node packs"
                                    }

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        visible: page.report !== null && page.report.packs.length > 0
                                        spacing: 6

                                        Repeater {
                                            model: page.report ? page.report.packs : []

                                            delegate: RowLayout {
                                                id: packRow

                                                required property var modelData

                                                Layout.fillWidth: true
                                                Layout.minimumHeight: 28
                                                spacing: 8

                                                StatusIcon {
                                                    ok: packRow.modelData.installed
                                                }

                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    spacing: 1

                                                    Label {
                                                        Layout.fillWidth: true
                                                        text: packRow.modelData.id + (packRow.modelData.version ? "  " + packRow.modelData.version : "")
                                                        textFormat: Text.PlainText
                                                        color: packRow.modelData.installed ? Theme.textMuted : Theme.text
                                                        font.pixelSize: Theme.px(13)
                                                        elide: Text.ElideRight
                                                    }

                                                    Label {
                                                        Layout.fillWidth: true
                                                        text: packRow.modelData.node_types.join(", ")
                                                        textFormat: Text.PlainText
                                                        color: Theme.textFaint
                                                        font.pixelSize: Theme.px(11.5)
                                                        elide: Text.ElideRight
                                                    }

                                                }

                                                Label {
                                                    visible: !packRow.modelData.installed && !packRow.modelData.repository
                                                    text: "Not in the Comfy Registry"
                                                    color: Theme.textFaint
                                                    font.pixelSize: Theme.px(12)
                                                }

                                                FlatButton {
                                                    visible: !packRow.modelData.installed && !!packRow.modelData.repository
                                                    text: page.installingPack === packRow.modelData.id ? "Installing…" : "Install"
                                                    enabled: page.installingPack === "" && page.root !== ""
                                                    onClicked: page.installPack(packRow.modelData)
                                                    ToolTip.visible: hovered
                                                    ToolTip.text: String(packRow.modelData.repository)
                                                }

                                            }

                                        }

                                    }

                                    // Nodes no pack claims.
                                    Rectangle {
                                        Layout.fillWidth: true
                                        visible: page.report !== null && page.report.unresolved_node_types.length > 0
                                        implicitHeight: unresolvedLabel.implicitHeight + 20
                                        radius: 8
                                        color: Theme.alpha(Theme.warning, 0.08)
                                        border.color: Theme.alpha(Theme.warning, 0.35)

                                        Label {
                                            id: unresolvedLabel

                                            x: 12
                                            y: 10
                                            width: parent.width - 24
                                            text: page.report ? "Nodes the workflow doesn't name a pack for: " + page.report.unresolved_node_types.join(", ") + ". \"Install Missing Custom Nodes\" in ComfyUI-Manager can look them up." : ""
                                            textFormat: Text.PlainText
                                            color: Theme.warning
                                            font.pixelSize: Theme.px(12.5)
                                            wrapMode: Text.Wrap
                                        }

                                    }

                                    Label {
                                        Layout.fillWidth: true
                                        visible: page.report !== null && page.report.models.length === 0 && page.report.packs.length === 0
                                        text: "This workflow names no models or custom node packs."
                                        color: Theme.textFaint
                                        font.pixelSize: Theme.px(13)
                                    }

                                }

                            }

                        }

                    }

                }

                ScrollBar.vertical: ScrollBar {
                }

            }

        }

        CivitaiBrowser {
            app: page.app
            kind: "workflows"
            title: "Workflows"
            subtitle: "ComfyUI workflows on Civitai that come as a .json file"
            subtitleIcon: "node-tree"
            onCheckRequested: (path) => {
                prefs.view = "library";
                page.refreshSaved();
                page.check(path);
            }
            headerLeading: [
                ViewSwitch {
                }
            ]
        }

    }

    // ---- small pieces ----------------------------------------------------------
    component SectionLabel: Label {
        color: Theme.label
        font.pixelSize: Theme.px(12)
        font.weight: Font.Medium
        font.capitalization: Font.AllUppercase
        font.letterSpacing: Theme.px(12) * 0.07
    }

    /// Civitai | Library, in both views' headers.
    component ViewSwitch: Rectangle {
        implicitWidth: switchRow.implicitWidth + 6
        implicitHeight: 30
        radius: 8
        color: Theme.controlFill
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined

        Row {
            id: switchRow

            anchors.centerIn: parent
            spacing: 2

            Repeater {
                model: [{
                    "id": "civitai",
                    "label": "Civitai"
                }, {
                    "id": "library",
                    "label": "Library"
                }]

                delegate: Rectangle {
                    id: seg

                    required property var modelData
                    readonly property bool active: prefs.view === modelData.id

                    width: segLabel.implicitWidth + 24
                    height: 24
                    radius: 6
                    color: active ? Theme.rowSelected : segHover.hovered ? Theme.hover : "transparent"

                    Label {
                        id: segLabel

                        anchors.centerIn: parent
                        text: seg.modelData.label
                        color: seg.active ? Theme.text : Theme.textMuted
                        font.pixelSize: Theme.px(12.5)
                        font.weight: seg.active ? Font.Medium : Font.Normal
                    }

                    HoverHandler {
                        id: segHover

                        cursorShape: Qt.PointingHandCursor
                    }

                    TapHandler {
                        onTapped: prefs.view = seg.modelData.id
                    }

                }

            }

        }

    }

    component StatusIcon: Icon {
        property bool ok

        name: ok ? "checkbox-circle" : "close-circle"
        color: ok ? Theme.success : Theme.error
        implicitWidth: 15
        implicitHeight: 15
    }

}
