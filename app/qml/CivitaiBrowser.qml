import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtMultimedia
import QtCore
import ArcticHelper

// A Civitai browser: the LoRAs section (`kind: "loras"`) and the Workflows
// section's Civitai view (`kind: "workflows"`, only workflows with a .json
// file). Cards by sort/period/base model or search, more as you scroll; a
// card opens its details (gallery, versions, files, trigger words for a
// LoRA) and downloads the chosen file: a LoRA into models/loras/<base
// model>, a workflow into ComfyUI's user/default/workflows.
//
// Mature content is filtered in the backend: with the Mature content switch
// off (a setting shared by both browsers), Civitai's mature models and
// previews never reach the page.
Rectangle {
    id: page

    /// The window (settings, log, copyText, openPanel).
    property var app
    /// "loras" or "workflows".
    property string kind: "loras"
    property string title: "LoRAs"
    property string subtitle: "From Civitai"
    property string subtitleIcon: "sparkling"
    /// Put first among the header's buttons (the Workflows view switch).
    property alias headerLeading: headerLeadingRow.data
    /// A saved workflow to check (Workflows: "Check what it needs").
    signal checkRequested(string path)

    readonly property bool workflows: kind === "workflows"
    readonly property string noun: workflows ? "workflow" : "LoRA"
    readonly property string nouns: workflows ? "workflows" : "LoRAs"
    /// The Downloads kind of this browser's downloads.
    readonly property string downloadKind: workflows ? "workflow" : "lora"
    readonly property bool downloading: Downloads.busy && Downloads.kind === downloadKind
    readonly property string root: app && app.settings ? app.settings.comfyui_root || "" : ""
    readonly property bool showMature: app && app.settings ? app.settings.civitai_show_mature === true : false

    property bool loaded: false
    property bool loading: false
    property int requestSeq: 0
    property string nextCursor: ""
    property string errorText: ""
    property var sorts: ["Most Downloaded", "Highest Rated", "Newest"]
    property var periods: ["AllTime", "Year", "Month", "Week", "Day"]
    property var baseModels: []
    /// Lowercased file names already there (LoRAs: under models/loras), with
    /// their paths for workflows (or `true`).
    property var installed: ({})

    /// The open LoRA or workflow, or null for the grid.
    property var details: null
    property bool detailLoading: false
    property string detailError: ""
    property int versionIndex: 0
    property int fileIndex: 0
    property int mediaIndex: 0
    /// The whole description is shown (else its start and "Show more").
    property bool descriptionExpanded: false
    /// Descriptions longer than this start collapsed.
    readonly property int descriptionLimit: 600
    readonly property string description: details ? details.description : ""
    readonly property bool descriptionLong: description.length > descriptionLimit + 80

    readonly property var periodLabels: ({
            "AllTime": "All time",
            "Year": "This year",
            "Month": "This month",
            "Week": "This week",
            "Day": "Today"
        })

    color: Theme.canvas

    Settings {
        id: prefs

        category: page.workflows ? "civitai-workflows" : "loras"
        property string sort: "Most Downloaded"
        property string period: "Week"
        property string baseModel: ""
    }

    ListModel {
        id: cards
    }

    /// A preview image, decoded at about the size it's shown.
    /// A preview image, decoded at about the size it's shown. Without a
    /// `url`, or when it can't be loaded, the "No preview" picture.
    component Preview: Image {
        property int fullWidth: 420
        property string url: ""
        property bool failed: false
        readonly property bool placeholder: url === "" || failed

        source: placeholder ? Theme.noPreview : url
        sourceSize.width: fullWidth
        asynchronous: true
        smooth: true
        onUrlChanged: failed = false
        onStatusChanged: if (status === Image.Error && !placeholder)
            failed = true
    }

    // ---- helpers ---------------------------------------------------------------
    function compact(n) {
        n = Number(n || 0);
        if (n >= 1000000)
            return (n / 1000000).toFixed(1) + "M";
        if (n >= 1000)
            return (n / 1000).toFixed(1) + "k";
        return String(n);
    }
    function isInstalled(name) {
        return !!name && !!installed[String(name).toLowerCase()];
    }
    function folderFor(version) {
        const folder = String(version && version.base_model || "").toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
        return folder || "civitai-" + (details ? details.id : "");
    }
    readonly property var version: details && details.versions.length ? details.versions[Math.min(versionIndex, details.versions.length - 1)] : null
    readonly property var files: version ? version.files.slice().sort((a, b) => Number(b.primary) - Number(a.primary)) : []
    readonly property var file: files.length ? files[Math.min(fileIndex, files.length - 1)] : null
    readonly property var media: version ? version.media : []
    readonly property var shownMedia: media.length ? media[Math.min(mediaIndex, media.length - 1)] : null

    // ---- loading ---------------------------------------------------------------
    function refreshInstalled() {
        if (workflows) {
            if (!root) {
                installed = {};
                return;
            }
            Api.call("list_saved_workflows", {
                comfyuiRoot: root
            }).then(list => {
                const map = {};
                for (const w of list)
                    map[String(w.path).split(/[\\/]/).pop().toLowerCase()] = w.path;
                page.installed = map;
            }).catch(() => page.installed = {});
            return;
        }
        Api.call("list_installed_lora_files", {
            comfyuiRoot: root || null
        }).then(names => {
            const set = {};
            for (const n of names)
                set[n] = true;
            page.installed = set;
        }).catch(() => page.installed = {});
    }

    function loadPage(reset) {
        if (loading && !reset)
            return;
        const seq = ++requestSeq;
        loading = true;
        errorText = "";
        if (reset) {
            nextCursor = "";
            cards.clear();
            grid.contentY = 0;
        }
        Api.call(workflows ? "browse_civitai_workflows" : "browse_civitai_loras", {
            query: {
                sort: prefs.sort,
                period: prefs.period,
                baseModel: prefs.baseModel || null,
                query: searchField.text.trim() || null,
                cursor: reset ? null : nextCursor || null
            }
        }).then(result => {
            if (seq !== page.requestSeq)
                return;
            for (const item of result.items || []) {
                cards.append({
                    loraId: item.id,
                    name: item.name,
                    creator: item.creator || "",
                    baseModel: item.base_model || "",
                    downloads: item.downloads || 0,
                    likes: item.likes || 0,
                    thumbUrl: item.thumb ? item.thumb.thumb_url : "",
                    thumbMature: item.thumb ? item.thumb.mature === true : false,
                    isVideo: item.thumb ? !!item.thumb.video_url : false,
                    fileName: item.file_name || ""
                });
            }
            page.nextCursor = result.next_cursor && (result.items || []).length ? result.next_cursor : "";
            // Development aid: `--lora=<n>` / `--civitai-workflow=<n>` opens the nth card (screenshots).
            const devCard = page.app ? page.app.argValue(page.workflows ? "--civitai-workflow=" : "--lora=") : "";
            if (reset && devCard !== "" && cards.count > Number(devCard))
                page.openDetail(cards.get(Number(devCard)).loraId);
        }).catch(err => {
            if (seq === page.requestSeq)
                page.errorText = "Couldn't load " + page.nouns + " from Civitai: " + err;
        }).then(Api.after(() => {
            if (seq !== page.requestSeq)
                return;
            page.loading = false;
            Qt.callLater(page.loadMoreIfNeeded);
        }));
    }

    /// The next page when the end of the grid is (nearly) in view.
    function loadMoreIfNeeded() {
        if (nextCursor && !loading && visible && stack.currentIndex === 0 && grid.contentY + grid.height > grid.contentHeight - grid.cellHeight * 2)
            loadPage(false);
    }

    function ensureLoaded() {
        refreshInstalled();
        if (loaded)
            return;
        loaded = true;
        Api.call("get_lora_browser_options").then(options => {
            page.sorts = options.sorts;
            page.periods = options.periods;
        }).catch(() => {});
        Api.call("get_civitai_base_models").then(list => page.baseModels = list).catch(() => page.baseModels = []);
        loadPage(true);
    }
    onVisibleChanged: if (visible)
        ensureLoaded()

    function reload() {
        if (loaded)
            loadPage(true);
    }

    /// The description's start: up to `descriptionLimit` characters, cut at a
    /// paragraph or word break.
    function descriptionStart() {
        const head = description.slice(0, descriptionLimit);
        const paragraph = head.lastIndexOf("\n\n");
        const cut = paragraph > descriptionLimit * 0.6 ? paragraph : Math.max(head.lastIndexOf(" "), descriptionLimit * 0.8);
        return head.slice(0, cut).replace(/[\s.,;:]+$/, "") + "…";
    }

    function openDetail(loraId) {
        details = null;
        descriptionExpanded = false;
        detailError = "";
        detailLoading = true;
        versionIndex = 0;
        fileIndex = 0;
        mediaIndex = 0;
        const requested = loraId;
        Api.call(workflows ? "get_civitai_workflow" : "get_civitai_lora", {
            modelId: loraId
        }).then(result => {
            if (detailLoading && requested === loraId)
                page.details = result;
        }).catch(err => page.detailError = "Couldn't load this " + page.noun + ": " + err).then(Api.after(() => page.detailLoading = false));
        stack.currentIndex = 1;
    }
    function closeDetail() {
        details = null;
        detailLoading = false;
        stack.currentIndex = 0;
    }

    function setMature(on) {
        Api.call("set_civitai_show_mature", {
            enabled: on
        }).then(settings => page.app.settings = settings).catch(err => page.app.log("Mature content setting failed: " + err, "error"));
    }
    // Both browsers follow the setting: the visible one reloads now, the
    // other when it's shown.
    onShowMatureChanged: {
        if (!loaded)
            return;
        closeDetail();
        loaded = false;
        if (visible)
            ensureLoaded();
    }

    function download() {
        if (Downloads.busy) {
            Downloads.cancel();
            return;
        }
        if (!details || !version || !file)
            return;
        Downloads.begin(downloadKind, "Starting " + details.name + "…", file.size_bytes || 0);
        app.openPanel("downloads");
        Api.call(workflows ? "download_civitai_workflow" : "download_civitai_lora", {
            modelId: details.id,
            versionId: version.id,
            fileId: file.id,
            comfyuiRoot: root || null
        }).then(() => page.app.log("Downloading the " + page.noun + " " + page.details.name + " (" + page.file.name + ").")).catch(err => {
            Downloads.end();
            page.app.log(String(err), "error");
        });
    }

    Connections {
        target: Downloads

        function onEnded(kind, outcome, message) {
            if (kind === page.downloadKind)
                page.refreshInstalled();
        }
    }

    onRootChanged: if (loaded)
        refreshInstalled()

    // ---- layout ----------------------------------------------------------------
    StackLayout {
        id: stack

        anchors.fill: parent

        // ---- the grid ----
        ColumnLayout {
            spacing: 0

            PageHeader {
                title: page.title
                subtitle: page.subtitle
                subtitleIcon: page.subtitleIcon

                Row {
                    id: headerLeadingRow

                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                }

                Row {
                    spacing: 8
                    anchors.verticalCenter: parent.verticalCenter

                    AppSwitch {
                        anchors.verticalCenter: parent.verticalCenter
                        checked: page.showMature
                        onToggled: {
                            page.setMature(checked);
                            // Toggling replaced the binding; follow the setting again.
                            checked = Qt.binding(() => page.showMature);
                        }
                        ToolTip.visible: hovered
                        ToolTip.text: "Show " + page.nouns + " and previews Civitai rates above PG."
                    }

                    Label {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Mature content"
                        color: Theme.textMuted
                        font.pixelSize: Theme.px(13)
                    }

                }

                FlatButton {
                    text: "Civitai token…"
                    onClicked: tokenDialog.open()
                }

            }

            // Search and filters.
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: 28
                Layout.rightMargin: 20
                Layout.topMargin: 16
                Layout.bottomMargin: 12
                spacing: 8

                TextField {
                    id: searchField

                    Layout.fillWidth: true
                    implicitHeight: 34
                    leftPadding: 36
                    placeholderText: "Search Civitai " + page.nouns + " by name, tag or creator"
                    placeholderTextColor: Theme.textFaint
                    color: Theme.text
                    font.pixelSize: Theme.px(13.5)
                    onTextChanged: searchTimer.restart()
                    onAccepted: {
                        searchTimer.stop();
                        page.reload();
                    }

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

                    Timer {
                        id: searchTimer

                        interval: 450
                        onTriggered: page.reload()
                    }

                }

                ComboField {
                    Layout.preferredWidth: 170
                    model: page.sorts
                    currentIndex: Math.max(0, page.sorts.indexOf(prefs.sort))
                    onActivated: (index) => {
                        prefs.sort = page.sorts[index];
                        page.reload();
                    }
                }

                ComboField {
                    Layout.preferredWidth: 140
                    enabled: searchField.text.trim() === ""
                    model: page.periods.map((p) => {
                        return {
                            "id": p,
                            "label": page.periodLabels[p] || p
                        };
                    })
                    textRole: "label"
                    valueRole: "id"
                    currentIndex: Math.max(0, page.periods.indexOf(prefs.period))
                    onActivated: (index) => {
                        prefs.period = page.periods[index];
                        page.reload();
                    }
                    ToolTip.visible: hovered
                    ToolTip.text: enabled ? "Period for downloads and ratings." : "Searches always cover all time."
                }

                ComboField {
                    Layout.preferredWidth: 200
                    model: ["All base models"].concat(page.baseModels)
                    currentIndex: Math.max(0, page.baseModels.indexOf(prefs.baseModel) + 1)
                    onActivated: (index) => {
                        prefs.baseModel = index === 0 ? "" : page.baseModels[index - 1];
                        page.reload();
                    }
                }

            }

            Label {
                Layout.fillWidth: true
                Layout.leftMargin: 28
                Layout.rightMargin: 20
                visible: page.errorText !== ""
                text: page.errorText
                color: Theme.error
                font.pixelSize: Theme.px(13)
                wrapMode: Text.Wrap
            }

            GridView {
                id: grid

                readonly property int columns: Math.max(1, Math.floor(width / 210))

                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: 22
                Layout.rightMargin: 16
                cellWidth: width / columns
                cellHeight: cellWidth * 4 / 3 + 74
                clip: true
                model: cards
                boundsBehavior: Flickable.StopAtBounds
                cacheBuffer: 600
                // More as the end comes into view.
                onContentYChanged: page.loadMoreIfNeeded()
                onHeightChanged: page.loadMoreIfNeeded()

                delegate: Item {
                    id: card

                    required property int loraId
                    required property string name
                    required property string creator
                    required property string baseModel
                    required property real downloads
                    required property real likes
                    required property string thumbUrl
                    required property bool thumbMature
                    required property bool isVideo
                    required property string fileName

                    width: grid.cellWidth
                    height: grid.cellHeight

                    Rectangle {
                        anchors.fill: parent
                        anchors.margins: 6
                        radius: 12
                        color: cardHover.hovered ? Theme.controlFillHover : Theme.raised
                        border.color: page.isInstalled(card.fileName) ? Theme.alpha(Theme.success, 0.55) : Theme.hairline
                        clip: true

                        Rectangle {
                            id: thumbBox

                            width: parent.width
                            height: width * 4 / 3
                            color: Theme.subtle
                            radius: 12

                            Preview {
                                anchors.fill: parent
                                url: card.thumbUrl
                                fillMode: Image.PreserveAspectCrop
                            }

                            Row {
                                anchors.top: parent.top
                                anchors.left: parent.left
                                anchors.margins: 8
                                spacing: 4

                                Chip {
                                    visible: card.isVideo
                                    text: "Video"
                                }

                                Chip {
                                    visible: card.thumbMature
                                    text: "Mature"
                                    fg: Theme.warning
                                }

                            }

                            Chip {
                                anchors.top: parent.top
                                anchors.right: parent.right
                                anchors.margins: 8
                                visible: page.isInstalled(card.fileName)
                                text: page.workflows ? "Saved" : "Installed"
                                fg: Theme.success
                            }

                        }

                        ColumnLayout {
                            anchors.top: thumbBox.bottom
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.margins: 10
                            anchors.topMargin: 8
                            spacing: 2

                            Label {
                                Layout.fillWidth: true
                                text: card.name
                                color: Theme.text
                                font.pixelSize: Theme.px(13)
                                font.weight: Font.Medium
                                elide: Text.ElideRight
                            }

                            Label {
                                Layout.fillWidth: true
                                text: [card.baseModel, card.creator ? "by " + card.creator : ""].filter(Boolean).join(" · ")
                                color: Theme.textMuted
                                font.pixelSize: Theme.px(11.5)
                                elide: Text.ElideRight
                            }

                            Label {
                                Layout.fillWidth: true
                                text: page.compact(card.downloads) + " downloads · " + page.compact(card.likes) + " likes"
                                color: Theme.textFaint
                                font.pixelSize: Theme.px(11)
                                elide: Text.ElideRight
                            }

                        }

                    }

                    HoverHandler {
                        id: cardHover

                        cursorShape: Qt.PointingHandCursor
                    }

                    TapHandler {
                        onTapped: page.openDetail(card.loraId)
                    }

                }

                footer: Item {
                    width: grid.width
                    height: 60

                    // (A text, not a BusyIndicator: the Basic one doesn't show on the dark canvas.)
                    Label {
                        anchors.centerIn: parent
                        visible: page.loading || (cards.count === 0 && page.loaded && page.errorText === "")
                        text: page.loading ? (cards.count === 0 ? "Loading " + page.nouns + " from Civitai…" : "Loading more…") : "No " + page.nouns + " match these filters."
                        color: Theme.textFaint
                        font.pixelSize: Theme.px(13)
                    }

                }

                ScrollBar.vertical: ScrollBar {
                }

            }

        }

        // ---- one LoRA or workflow ----
        ColumnLayout {
            spacing: 0

            PageHeader {
                title: page.details ? page.details.name : (page.detailLoading ? "Loading…" : page.noun)
                subtitle: page.details ? [page.details.creator ? "by " + page.details.creator : "", page.compact(page.details.downloads) + " downloads", page.compact(page.details.likes) + " likes"].filter(Boolean).join(" · ") : ""
                subtitleIcon: "sparkling"

                FlatButton {
                    text: "← Back"
                    onClicked: page.closeDetail()
                }

                FlatButton {
                    text: "Open on Civitai"
                    enabled: page.details !== null
                    onClicked: Api.call("open_external_url", {
                        "url": page.details.page_url
                    })
                }

            }

            Label {
                Layout.margins: 28
                visible: page.detailError !== ""
                text: page.detailError
                color: Theme.error
                font.pixelSize: Theme.px(13)
            }

            Label {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 60
                visible: page.detailLoading
                text: "Loading from Civitai…"
                color: Theme.textFaint
                font.pixelSize: Theme.px(13)
            }

            Flickable {
                Layout.fillWidth: true
                Layout.fillHeight: true
                visible: page.details !== null
                contentHeight: detailBody.implicitHeight + 48
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                RowLayout {
                    id: detailBody

                    x: 28
                    y: 24
                    width: parent.width - 56
                    spacing: 28

                    // The gallery.
                    ColumnLayout {
                        Layout.alignment: Qt.AlignTop
                        Layout.preferredWidth: 5
                        Layout.fillWidth: true
                        spacing: 8

                        Rectangle {
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(560, width * 1.1)
                            radius: 12
                            color: Theme.subtle
                            clip: true

                            Preview {
                                id: mainImage

                                anchors.fill: parent
                                url: page.shownMedia ? page.shownMedia.image_url : ""
                                fullWidth: 1024
                                // The placeholder fills the frame (it's centred); real images fit.
                                fillMode: placeholder ? Image.PreserveAspectCrop : Image.PreserveAspectFit
                            }

                            // A video preview plays here, muted and looping, over its still
                            // (shown until the first frame). Only while this view shows it:
                            // leaving it stops playback and the download.
                            MediaPlayer {
                                id: player

                                readonly property string videoUrl: page.visible && stack.currentIndex === 1 && page.shownMedia && page.shownMedia.video_url ? page.shownMedia.video_url : ""
                                readonly property bool playing: playbackState === MediaPlayer.PlayingState

                                videoOutput: videoView
                                loops: MediaPlayer.Infinite
                                // Source and play together (a binding on `source` could
                                // update after play() and leave it stopped).
                                onVideoUrlChanged: {
                                    stop();
                                    source = videoUrl;
                                    if (videoUrl !== "")
                                        play();

                                }

                                audioOutput: AudioOutput {
                                    id: playerAudio

                                    muted: true
                                }

                            }

                            VideoOutput {
                                id: videoView

                                anchors.fill: parent
                                fillMode: VideoOutput.PreserveAspectFit
                                visible: player.videoUrl !== "" && player.hasVideo && player.error === MediaPlayer.NoError
                            }

                            // Play/pause, sound, and the video in the browser.
                            Rectangle {
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                anchors.margins: 10
                                visible: page.shownMedia !== null && !!page.shownMedia.video_url
                                width: videoControls.implicitWidth + 12
                                height: 36
                                radius: 9
                                color: Theme.alpha("#000000", 0.55)

                                Row {
                                    id: videoControls

                                    anchors.centerIn: parent
                                    spacing: 2

                                    Label {
                                        anchors.verticalCenter: parent.verticalCenter
                                        rightPadding: 6
                                        leftPadding: 6
                                        visible: text !== ""
                                        text: player.error !== MediaPlayer.NoError ? "Can't play it here" : player.mediaStatus === MediaPlayer.LoadingMedia || player.mediaStatus === MediaPlayer.BufferingMedia || player.mediaStatus === MediaPlayer.StalledMedia ? "Loading video…" : ""
                                        color: "#ffffff"
                                        opacity: 0.8
                                        font.pixelSize: Theme.px(12)
                                    }

                                    PanelButton {
                                        visible: player.error === MediaPlayer.NoError
                                        iconName: player.playing ? "pause" : "play"
                                        tip: player.playing ? "Pause" : "Play"
                                        implicitWidth: 28
                                        implicitHeight: 28
                                        onClicked: player.playing ? player.pause() : player.play()
                                    }

                                    PanelButton {
                                        visible: player.error === MediaPlayer.NoError
                                        iconName: playerAudio.muted ? "volume-mute" : "volume-up"
                                        tip: playerAudio.muted ? "Sound on" : "Sound off"
                                        implicitWidth: 28
                                        implicitHeight: 28
                                        onClicked: playerAudio.muted = !playerAudio.muted
                                    }

                                    PanelButton {
                                        iconName: "external-link"
                                        tip: "Open the video in the browser"
                                        implicitWidth: 28
                                        implicitHeight: 28
                                        onClicked: Api.call("open_external_url", {
                                            "url": page.shownMedia.video_url
                                        })
                                    }

                                }

                            }

                        }

                        ListView {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 64
                            visible: page.media.length > 1
                            orientation: ListView.Horizontal
                            spacing: 6
                            clip: true
                            model: page.media

                            delegate: Rectangle {
                                id: thumb

                                required property var modelData
                                required property int index

                                width: 64
                                height: 64
                                radius: 8
                                color: Theme.subtle
                                border.color: page.mediaIndex === thumb.index ? Theme.accent : "transparent"
                                border.width: 2
                                clip: true

                                Preview {
                                    anchors.fill: parent
                                    anchors.margins: 2
                                    source: thumb.modelData.thumb_url
                                    fullWidth: 128
                                    fillMode: Image.PreserveAspectCrop
                                }

                                TapHandler {
                                    onTapped: page.mediaIndex = thumb.index
                                }

                            }

                        }

                    }

                    // Version, file, trigger words, download, description.
                    ColumnLayout {
                        Layout.alignment: Qt.AlignTop
                        Layout.preferredWidth: 4
                        Layout.fillWidth: true
                        spacing: 12

                        GridLayout {
                            Layout.fillWidth: true
                            columns: 2
                            columnSpacing: 12
                            rowSpacing: 8

                            Label {
                                text: "Version"
                                color: Theme.textMuted
                                font.pixelSize: Theme.px(13)
                            }

                            ComboField {
                                Layout.fillWidth: true
                                enabled: page.details && page.details.versions.length > 1
                                model: page.details ? page.details.versions.map((v) => {
                                    return {
                                        "id": v.id,
                                        "label": v.name + (v.base_model ? "  (" + v.base_model + ")" : "")
                                    };
                                }) : []
                                textRole: "label"
                                valueRole: "id"
                                currentIndex: page.versionIndex
                                onActivated: (index) => {
                                    page.versionIndex = index;
                                    page.fileIndex = 0;
                                    page.mediaIndex = 0;
                                }
                            }

                            Label {
                                text: "File"
                                color: Theme.textMuted
                                font.pixelSize: Theme.px(13)
                            }

                            ComboField {
                                Layout.fillWidth: true
                                enabled: page.files.length > 1
                                model: page.files.map((f) => {
                                    return {
                                        "id": f.id,
                                        "label": f.name + (f.size_bytes ? "  (" + Downloads.formatBytes(f.size_bytes) + ")" : "")
                                    };
                                })
                                textRole: "label"
                                valueRole: "id"
                                currentIndex: page.fileIndex
                                onActivated: (index) => {
                                    return page.fileIndex = index;
                                }
                            }

                        }

                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: facts.implicitHeight + 24
                            radius: 12
                            color: Theme.raised
                            border.color: Theme.hairline

                            GridLayout {
                                id: facts

                                x: 14
                                y: 12
                                width: parent.width - 28
                                columns: 2
                                columnSpacing: 14
                                rowSpacing: 6

                                Label {
                                    text: "Base model"
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.px(12.5)
                                }

                                Label {
                                    Layout.fillWidth: true
                                    text: page.version && page.version.base_model ? page.version.base_model : "-"
                                    color: Theme.text
                                    font.pixelSize: Theme.px(12.5)
                                }

                                Label {
                                    text: "Size"
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.px(12.5)
                                }

                                Label {
                                    Layout.fillWidth: true
                                    text: page.file && page.file.size_bytes ? Downloads.formatBytes(page.file.size_bytes) : "unknown"
                                    color: Theme.text
                                    font.pixelSize: Theme.px(12.5)
                                }

                                Label {
                                    text: "Saved to"
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.px(12.5)
                                }

                                Label {
                                    Layout.fillWidth: true
                                    text: !page.file ? "-" : page.workflows ? "user/default/workflows/" + page.file.name : "models/loras/" + page.folderFor(page.version) + "/" + page.file.name
                                    color: Theme.text
                                    font.family: Theme.mono
                                    font.pixelSize: Theme.px(11.5)
                                    elide: Text.ElideMiddle
                                }

                                Label {
                                    text: "Published"
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.px(12.5)
                                    visible: page.version && page.version.published_at
                                }

                                Label {
                                    Layout.fillWidth: true
                                    visible: page.version && page.version.published_at
                                    text: page.version && page.version.published_at ? String(page.version.published_at).slice(0, 10) : ""
                                    color: Theme.text
                                    font.pixelSize: Theme.px(12.5)
                                }

                            }

                        }

                        Label {
                            visible: !page.workflows
                            text: "Trigger words"
                            color: Theme.label
                            font.pixelSize: Theme.px(12)
                            font.weight: Font.Medium
                            font.capitalization: Font.AllUppercase
                            font.letterSpacing: Theme.px(12) * 0.07
                        }

                        Flow {
                            Layout.fillWidth: true
                            visible: !page.workflows
                            spacing: 6

                            Repeater {
                                model: page.version ? page.version.trained_words : []

                                delegate: FlatButton {
                                    required property string modelData

                                    text: modelData
                                    font.pixelSize: Theme.px(12)
                                    ToolTip.visible: hovered
                                    ToolTip.text: "Copy"
                                    onClicked: {
                                        page.app.copyText(modelData);
                                        page.app.log("Copied “" + modelData + "”.", "ok");
                                    }
                                }

                            }

                            Label {
                                visible: !page.version || page.version.trained_words.length === 0
                                text: "None"
                                color: Theme.textFaint
                                font.pixelSize: Theme.px(12.5)
                            }

                        }

                        Rectangle {
                            Layout.fillWidth: true
                            visible: page.version && page.version.restricted
                            implicitHeight: restrictedLabel.implicitHeight + 16
                            radius: 8
                            color: Theme.alpha(Theme.warning, 0.08)
                            border.color: Theme.alpha(Theme.warning, 0.35)

                            Label {
                                id: restrictedLabel

                                x: 10
                                y: 8
                                width: parent.width - 20
                                text: "This version is early access or paid on Civitai. The download needs a Civitai token from an account that has access."
                                color: Theme.warning
                                font.pixelSize: Theme.px(12)
                                wrapMode: Text.Wrap
                            }

                        }

                        FlatButton {
                            Layout.fillWidth: true
                            text: page.downloading ? "Cancel download" : page.file && page.isInstalled(page.file.name) ? (page.workflows ? "Saved" : "Installed") + " · download again" : "Download " + page.noun
                            variant: page.downloading ? "stop" : page.file && page.isInstalled(page.file.name) && page.workflows ? "outline" : "primary"
                            font.pixelSize: Theme.px(14)
                            padding: 8
                            enabled: page.downloading || (!Downloads.busy && page.file !== null && page.root !== "")
                            onClicked: page.download()
                            ToolTip.visible: hovered && page.root === ""
                            ToolTip.text: "Choose a ComfyUI install first."
                        }

                        // Civitai doesn't say which files need a login; many do.
                        Label {
                            Layout.fillWidth: true
                            visible: !page.downloading && !(page.app && page.app.settings && page.app.settings.civitai_token)
                            text: "Many Civitai downloads need a login. If this one fails, add your Civitai token (Civitai token… above the list)."
                            color: Theme.textFaint
                            font.pixelSize: Theme.px(12)
                            wrapMode: Text.Wrap
                        }

                        // A saved workflow: on to the dependency check.
                        FlatButton {
                            Layout.fillWidth: true
                            visible: page.workflows && page.file !== null && page.isInstalled(page.file.name) && !page.downloading
                            text: "Check what it needs"
                            variant: "primary"
                            font.pixelSize: Theme.px(14)
                            padding: 8
                            onClicked: page.checkRequested(page.installed[page.file.name.toLowerCase()])
                        }

                        Flow {
                            Layout.fillWidth: true
                            spacing: 6

                            Repeater {
                                model: page.details ? page.details.tags : []

                                delegate: Chip {
                                    required property string modelData

                                    text: modelData
                                }

                            }

                        }

                        // The description: long ones start with their beginning and "Show more".
                        Label {
                            Layout.fillWidth: true
                            text: page.descriptionLong && !page.descriptionExpanded ? page.descriptionStart() : page.description
                            textFormat: Text.PlainText
                            color: Theme.textMuted
                            font.pixelSize: Theme.px(13)
                            wrapMode: Text.Wrap
                            lineHeight: 1.3
                        }

                        Label {
                            visible: page.descriptionLong
                            text: page.descriptionExpanded ? "Show less" : "Show more"
                            color: moreHover.hovered ? Theme.primaryHover : Theme.primary
                            font.pixelSize: Theme.px(13)
                            font.weight: Font.Medium

                            HoverHandler {
                                id: moreHover

                                cursorShape: Qt.PointingHandCursor
                            }

                            TapHandler {
                                onTapped: page.descriptionExpanded = !page.descriptionExpanded
                            }

                        }

                    }

                }

                ScrollBar.vertical: ScrollBar {
                }

            }

        }

    }

    // The Civitai token.
    AppDialog {
        id: tokenDialog

        title: "Civitai token"
        standardButtons: Dialog.Close
        onOpened: tokenField.text = page.app && page.app.settings ? page.app.settings.civitai_token || "" : ""

        ColumnLayout {
            width: 440
            spacing: 12

            Label {
                Layout.fillWidth: true
                text: "Many LoRAs and workflows on Civitai can only be downloaded while signed in. An API key lets ComfyPilot download them for you."
                color: Theme.textMuted
                font.pixelSize: Theme.px(12.5)
                wrapMode: Text.Wrap
            }

            StepList {
                Layout.fillWidth: true
                steps: ["Sign in at civitai.com (or create a free account).", "Open your account settings: your avatar, then Settings.", "Scroll down to API Keys and click Add API key.", "Give it a name, like ComfyPilot, and click Save.", "Copy the key (Civitai shows it only once), paste it below and click Save."]
                linkText: "Open Civitai account settings"
                linkUrl: "https://civitai.com/user/account"
            }

            RowLayout {
                Layout.fillWidth: true

                AppTextField {
                    id: tokenField

                    Layout.fillWidth: true
                    echoMode: TextInput.Password
                    placeholderText: "Paste your Civitai API key"
                }

                FlatButton {
                    text: "Save"
                    onClicked: Api.call("save_civitai_token", {
                        "token": tokenField.text
                    }).then((settings) => {
                        page.app.settings = settings;
                        page.app.log(tokenField.text.trim() ? "Civitai token saved." : "Civitai token removed.", "ok");
                    }).catch((err) => {
                        return page.app.log("Saving the token failed: " + err, "error");
                    })
                }

            }

            Label {
                Layout.fillWidth: true
                text: "It's sent only to Civitai, and kept in your system keychain when there is one. Leave it empty and save to remove it."
                color: Theme.textFaint
                font.pixelSize: Theme.px(11.5)
                wrapMode: Text.Wrap
            }

        }

    }

}
