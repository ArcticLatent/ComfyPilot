import ArcticHelper
import QtCore
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "ConsoleFormat.js" as Console

// The window: left rail | sidebar | page, as Harlo's, with the activity,
// downloads and ComfyUI output panels in a dock along the bottom.
ApplicationWindow {
    id: window

    property var settings: null
    property var snapshot: null
    property var catalog: null
    property var installs: []
    property int modelCount: 0
    property int workflowCount: 0
    property bool updateAvailable: false
    property bool checkingUpdates: false
    property string updateVersion: ""
    property string updateNotes: ""
    /// Updates come from a package manager: point to the release page.
    property bool updateManaged: false
    property bool updating: false
    /// Settings and installs have loaded (the startup screen goes).
    property bool startupReady: false
    /// Loading is taking a while: only then the startup screen shows (no flash).
    property bool startupSlow: false
    /// Started with `--hidden`: in the tray only (see hiddenStartCheck).
    readonly property bool startHidden: Qt.application.arguments.includes("--hidden")
    /// Quit was asked for: closing the window really closes.
    property bool quitting: false
    /// Inside a Python traceback in ComfyUI's output (its indented lines are errors too).
    property bool comfyInTraceback: false
    /// Terminal colours for ComfyUI's output: the logo's Dracula palette in
    /// the dark theme, deeper versions of it in the light one.
    readonly property var consolePalette: Theme.isLight ? ({
            "black": String(Theme.textFaint),
            "red": "#c62828",
            "green": "#1b8a4e",
            "yellow": "#9a6b00",
            "blue": "#7c4fd6",
            "magenta": "#b8327a",
            "cyan": "#0b7fa8",
            "white": String(Theme.text),
            "faint": String(Theme.textFaint),
            "tag": "#7c4fd6"
        }) : ({
            "black": String(Theme.textFaint),
            "red": "#ff6e6e",
            "green": "#50fa7b",
            "yellow": "#f1fa8c",
            "blue": "#bd93f9",
            "magenta": "#ff79c6",
            "cyan": "#8be9fd",
            "white": String(Theme.text),
            "faint": String(Theme.textFaint),
            "tag": "#bd93f9"
        })
    // The theme changed: colour the output again (from each line's raw text).
    onConsolePaletteChanged: {
        for (let i = 0; i < comfyLines.count; i++) {
            const line = Console.format(comfyLines.get(i).raw, consolePalette);
            comfyLines.setProperty(i, "html", line.html);
            comfyLines.setProperty(i, "accent", line.accent);
        }
        if (comfyConsole.filtering)
            comfyConsole.rebuild();

    }

    function timeNow() {
        return new Date().toLocaleTimeString(Qt.locale(), "HH:mm:ss");
    }

    function appendLine(model, text, level, max) {
        model.append({
            "time": timeNow(),
            "text": String(text),
            "level": level || ""
        });
        if (model.count > max)
            model.remove(0, model.count - max);

    }

    /// A line of ComfyUI's output: coloured and given a level (ConsoleFormat.js).
    function appendComfyLine(text) {
        const line = Console.format(text, consolePalette);
        if (line.plain.trim() === "")
            return ;

        let level = line.level;
        if (/^Traceback \(most recent call last\)/.test(line.plain.trim()))
            comfyInTraceback = true;
        else if (comfyInTraceback && /^\s/.test(line.plain))
            level = "error";
        else if (comfyInTraceback) {
            level = "error";
            comfyInTraceback = false; // the exception line ends it
        }
        const time = timeNow();
        const last = comfyLines.count ? comfyLines.get(comfyLines.count - 1) : null;
        comfyLines.append({
            "time": time,
            "showTime": !last || last.time !== time,
            "raw": String(text),
            "html": line.html,
            "plain": line.plain,
            "level": level,
            "accent": line.accent
        });
        if (comfyLines.count > 2000) {
            comfyLines.remove(0, comfyLines.count - 2000);
            comfyLines.setProperty(0, "showTime", true);
        }
    }

    function log(message, level) {
        appendLine(activityLines, message, level, 500);
    }

    function showWindow() {
        window.show();
        window.raise();
        window.requestActivate();
    }

    // Copies text with a hidden editor (QML has no clipboard API of its own).
    function copyText(text) {
        clipboardHelper.text = text;
        clipboardHelper.selectAll();
        clipboardHelper.copy();
        clipboardHelper.text = "";
    }

    function gpuSummary(snap) {
        if (!snap)
            return "";

        if (snap.nvidia_gpu_name)
            return snap.nvidia_gpu_name + (snap.nvidia_gpu_vram_mb ? " · " + Math.round(snap.nvidia_gpu_vram_mb / 1024) + " GB" : "");

        if (snap.amd_gpu_name)
            return snap.amd_gpu_name;

        if (snap.intel_gpu_name)
            return snap.intel_gpu_name;

        return snap.gpu_detection_pending ? "Detecting GPU..." : "No GPU detected";
    }

    function selectedInstall() {
        const root = settings ? settings.comfyui_root || "" : "";
        return installs.find((i) => {
            return sidebar.samePath(i.root, root);
        }) || null;
    }

    // GPU detection runs in the background at startup; ask again until it's done.
    function refreshSnapshot(attempt) {
        Api.call("get_app_snapshot").then((snap) => {
            const first = window.snapshot === null;
            window.snapshot = snap;
            if (first && snap.settings_notice) {
                log(snap.settings_notice, "warn");
                settingsNotice.text = snap.settings_notice;
                settingsNotice.open();
            }

            const noGpu = !snap.nvidia_gpu_name && !snap.amd_gpu_name && !snap.intel_gpu_name;
            if ((snap.gpu_detection_pending || noGpu) && attempt < 8)
                snapshotRetry.restartFor(attempt + 1);

        }).catch((err) => {
            return log("Couldn't read system info: " + err, "error");
        });
    }

    function refreshInstalls() {
        return Api.call("list_comfyui_installations", {
            "basePath": null
        }).then((list) => {
            return window.installs = list || [];
        }).catch(() => {
            return window.installs = [];
        });
    }

    function refreshSettings() {
        return Api.call("get_settings").then((settings) => {
            return window.settings = settings;
        }).catch(() => {
        });
    }

    /// Opens a side panel ("activity", "downloads", "comfyui-log").
    function openPanel(mode) {
        windowSettings.rightMode = mode;
    }

    function loadStartupData() {
        refreshSnapshot(0);
        Api.call("get_settings").then((settings) => {
            window.settings = settings;
            return refreshInstalls();
        }).catch((err) => {
            return log("Couldn't read settings: " + err, "error");
        }).then(Api.after(() => {
            return window.startupReady = true;
        }));
        Api.call("get_catalog").then((catalog) => {
            window.catalog = catalog;
            window.modelCount = (catalog.models || []).length;
            window.workflowCount = (catalog.workflows || []).length;
            log("Loaded " + window.modelCount + " models and " + window.workflowCount + " workflows.");
        }).catch((err) => {
            return log("Catalog unavailable: " + err, "error");
        });
    }

    /// Checks for an update; `quiet` (the startup check) logs only a found one.
    /// With an update already found, the button installs it instead.
    function checkUpdates(quiet) {
        if (updateAvailable && !quiet) {
            installUpdate();
            return;
        }
        checkingUpdates = true;
        Api.call("check_updates_now").then((result) => {
            updateAvailable = result.available === true;
            updateVersion = result.version || "";
            updateNotes = String(result.notes || "").trim();
            updateManaged = result.managed_externally === true;
            if (updateAvailable)
                log("Version " + updateVersion + " is available. Use the update button on the left to install it.", "ok");
            else if (!quiet)
                log(updateNotes || "You're on the latest version.");
        }).catch((err) => {
            if (!quiet)
                log("Update check failed: " + err, "error");

        }).then(Api.after(() => {
            return checkingUpdates = false;
        }));
    }

    function installUpdate() {
        if (updateManaged) {
            log(updateNotes || "Update this app through your package manager.");
            Api.call("open_external_url", {
                "url": "https://github.com/ArcticLatent/ComfyPilot/releases/latest"
            });
            return;
        }
        updateConfirm.ask("Version " + updateVersion + " is available." + (updateNotes ? "\n\n" + updateNotes : "") + "\n\nInstall it now? The app closes, updates and opens again." + (comfyPage.runningRoot !== "" ? " ComfyUI keeps running." : ""), "Install update").then((ok) => {
            if (!ok)
                return;
            window.updating = true;
            startupOverlay.status = "Installing version " + window.updateVersion + "… The app restarts when it's done.";
            Api.call("auto_update_startup").then((result) => {
                // Without an install the app keeps running (e.g. updates are turned off).
                if (!result || result.available !== true) {
                    window.updating = false;
                    log((result && result.notes) || "No update was installed.", "warn");
                }
            }).catch((err) => {
                window.updating = false;
                log(String(err), "error");
            });
        });
    }

    /// Quits for real (the tray's Quit; closing the window may only hide it).
    function quitApp() {
        quitting = true;
        Api.call("set_app_quitting").catch(() => {
        }).then(Api.after(() => {
            return Qt.quit();
        }));
    }

    /// A desktop notification, when the window isn't in front.
    function notify(body) {
        if (!window.active || !window.visible)
            tray.notify("ComfyPilot", body);

    }

    function copyDiagnostics() {
        Api.call("write_diagnostics_report").then((result) => {
            copyText(result.report);
            log("Diagnostics copied to the clipboard" + (result.path ? " and saved to " + result.path : "") + ".", "ok");
        }).catch((err) => {
            return log("Diagnostics failed: " + err, "error");
        });
    }

    function selectInstall(root) {
        return Api.call("set_comfyui_root", {
            "comfyuiRoot": root
        }).then((settings) => {
            window.settings = settings;
            windowSettings.section = "comfyui";
            comfyPage.view = "manage";
        }).catch((err) => {
            return log("Couldn't select " + root + ": " + err, "error");
        });
    }

    // Development aid: `--screenshot=<file.png>` saves the window a few
    // seconds after startup and quits (run with QT_QPA_PLATFORM=offscreen
    // to keep it off the screen).
    function argValue(prefix) {
        const flag = Qt.application.arguments.find((a) => {
            return a.startsWith(prefix);
        });
        return flag ? flag.slice(prefix.length) : "";
    }

    width: 1440
    height: 900
    minimumWidth: 960
    minimumHeight: 600
    objectName: "mainWindow" // found by the single-instance guard (arctic_qt.h)
    // `--hidden` starts in the tray (autostart); without a tray the window shows anyway.
    visible: !startHidden
    title: "ComfyPilot"
    color: Theme.chrome
    // Basic-style defaults (tooltips, menus, scroll bars) in the theme's colours.
    palette.window: Theme.background
    palette.windowText: Theme.foreground
    palette.base: Theme.background
    palette.alternateBase: Theme.subtle
    palette.text: Theme.foreground
    palette.button: Theme.elevated
    palette.buttonText: Theme.foreground
    palette.brightText: Theme.foreground
    palette.highlight: Theme.primary
    palette.highlightedText: Theme.primaryText
    palette.placeholderText: Theme.muted
    palette.toolTipBase: Theme.elevated
    palette.toolTipText: Theme.foreground
    palette.light: Theme.hover
    palette.midlight: Theme.border
    palette.mid: Theme.border
    palette.dark: Theme.subtle
    palette.shadow: "#000000"
    Component.onCompleted: {
        // What dialog scrims blur.
        Theme.backdrop = page;
        // `--section=` / `--panel=` open a page or panel (screenshots).
        // `--theme=light|dark` (screenshots).
        if (argValue("--theme="))
            windowSettings.theme = argValue("--theme=");

        if (argValue("--section="))
            windowSettings.section = argValue("--section=");

        if (argValue("--view=") !== "")
            comfyPage.view = argValue("--view=");

        if (argValue("--panel=") !== "")
            windowSettings.rightMode = argValue("--panel=") === "none" ? "" : argValue("--panel=");

        loadStartupData();
        if (argValue("--screenshot="))
            screenshotTimer.start();
        else
            updateCheckTimer.start();

    }

    // The window's own state, kept between runs.
    Settings {
        id: windowSettings

        property bool sidebarOpen: true
        property real sidebarWidth: 290
        property real dockHeight: 260
        property string rightMode: ""
        property string section: "comfyui"
        /// "dark" or "light" (the left rail's switcher).
        property string theme: "dark"
        /// The "still running in the tray" message was shown once.
        property bool trayHintShown: false

        category: "window"
    }

    ListModel {
        id: activityLines
    }

    ListModel {
        id: comfyLines
    }

    Binding {
        target: Theme
        property: "mode"
        value: windowSettings.theme === "light" ? "light" : "dark"
    }

    Timer {
        id: snapshotRetry

        property int nextAttempt: 0

        function restartFor(attempt) {
            nextAttempt = attempt;
            restart();
        }

        interval: 600
        onTriggered: window.refreshSnapshot(nextAttempt)
    }

    // A quiet update check a little after startup.
    Timer {
        id: updateCheckTimer

        interval: 5000
        onTriggered: window.checkUpdates(true)
    }

    // The startup screen shows when loading takes a moment, and goes even if
    // loading hangs.
    Timer {
        interval: 300
        running: true
        onTriggered: window.startupSlow = true
    }

    Timer {
        interval: 4000
        running: true
        onTriggered: window.startupReady = true
    }

    // With a tray, closing hides the window (once with a note); otherwise,
    // or after Quit, the app quits.
    onClosing: (close) => {
        if (!quitting && tray.shown) {
            close.accepted = false;
            window.hide();
            if (!windowSettings.trayHintShown) {
                windowSettings.trayHintShown = true;
                tray.notify("ComfyPilot", "Still running in the tray. Use Quit in its menu to close it.");
            }
            return;
        }
        quitting = true;
        Qt.quit();
    }

    // A hidden start needs a tray to come back from; without one, show the window.
    Timer {
        id: hiddenStartCheck

        interval: 2000
        running: window.startHidden
        onTriggered: {
            if (!tray.shown)
                window.showWindow();

        }
    }

    AppTray {
        id: tray

        onShowRequested: window.showWindow()
        onQuitRequested: window.quitApp()
    }

    Connections {
        function onEnded(kind, outcome, message) {
            if (outcome === "ok")
                window.notify(message || "Download finished.");
            else if (outcome === "failed")
                window.notify(message || "Download failed.");
        }

        target: Downloads
    }

    ConfirmDialog {
        id: updateConfirm

        title: "Update ComfyPilot"
    }

    AppDialog {
        id: settingsNotice

        property alias text: settingsNoticeLabel.text

        title: "Your settings were reset"
        standardButtons: Dialog.Ok

        Label {
            id: settingsNoticeLabel

            width: 420
            color: Theme.textMuted
            font.pixelSize: Theme.px(13)
            wrapMode: Text.Wrap
            lineHeight: 1.25
        }

    }

    Timer {
        id: screenshotTimer

        // `--screenshot-delay=<ms>` waits longer (pages that load images).
        interval: Number(window.argValue("--screenshot-delay=")) || 6000
        // The whole window, popups and dialogs included.
        onTriggered: {
            Backend.saveWindowImage(window.argValue("--screenshot="));
            Qt.quit();
        }
    }

    Connections {
        function onEvent(name, payload) {
            switch (name) {
            case "show-window":
                window.showWindow();
                break;
            case "app-exit":
                Qt.quit();
                break;
            case "notify":
                window.log(payload.body);
                window.notify(payload.body);
                break;
            case "tray-status":
                tray.applyStatus(payload);
                break;
            case "update-state":
                if (payload.message) {
                    window.log(payload.message);
                    if (window.updating)
                        startupOverlay.status = payload.message;

                }
                break;
            case "download-progress":
                if (payload.message)
                    window.log("[" + payload.kind + "] " + payload.message, payload.phase === "batch_failed" ? "error" : "");

                break;
            case "comfyui-install-progress":
                if (payload.message)
                    window.log(payload.message, payload.phase === "warn" ? "warn" : payload.phase === "error" || payload.phase === "failed" ? "error" : "");

                break;
            case "comfyui-runtime":
                if (payload.message)
                    window.log(payload.message, payload.phase.endsWith("failed") ? "error" : payload.phase === "started" ? "ok" : "");

                break;
            case "comfyui-runtime-log":
                window.appendComfyLine(payload.text);
                break;
            }
        }

        target: Api
    }

    TextEdit {
        id: clipboardHelper

        visible: false
    }

    Item {
        id: page

        anchors.fill: parent

        LeftRail {
            id: leftRail

            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            sidebarOpen: windowSettings.sidebarOpen
            updateAvailable: window.updateAvailable
            checkingUpdates: window.checkingUpdates
            onSidebarToggled: windowSettings.sidebarOpen = !windowSettings.sidebarOpen
            onCheckUpdatesRequested: window.checkUpdates()
            lightTheme: Theme.isLight
            onThemeToggled: windowSettings.theme = Theme.isLight ? "dark" : "light"
            onDiagnosticsRequested: window.copyDiagnostics()
            onLogsRequested: Api.call("open_log_folder").catch((err) => {
                return window.log("Open logs failed: " + err, "error");
            })
        }

        SplitView {
            id: split

            anchors.left: leftRail.right
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            onResizingChanged: {
                if (!resizing && sidebar.visible)
                    windowSettings.sidebarWidth = sidebar.width;

            }

            Sidebar {
                id: sidebar

                visible: windowSettings.sidebarOpen
                SplitView.preferredWidth: windowSettings.sidebarWidth
                SplitView.minimumWidth: 200
                SplitView.maximumWidth: 460
                section: windowSettings.section
                installs: window.installs
                selectedRoot: window.settings ? window.settings.comfyui_root || "" : ""
                runningRoot: comfyPage.runningRoot
                modelCount: window.modelCount
                workflowCount: window.workflowCount
                appVersion: window.snapshot ? window.snapshot.version || "" : ""
                gpuText: window.gpuSummary(window.snapshot)
                ramText: window.snapshot && window.snapshot.total_ram_gb ? Math.round(window.snapshot.total_ram_gb) + " GB RAM" : ""
                onSectionPicked: (section) => {
                    return windowSettings.section = section;
                }
                onInstallPicked: (root) => {
                    return window.selectInstall(root);
                }
                onNewInstallRequested: {
                    windowSettings.section = "comfyui";
                    comfyPage.view = "install";
                }
            }

            // The page, the open bottom panel under it, and the dock's bar.
            ColumnLayout {
                SplitView.fillWidth: true
                SplitView.minimumWidth: 420
                spacing: 0

                SplitView {
                    id: mainSplit

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    orientation: Qt.Vertical
                    onResizingChanged: {
                        if (!resizing && dockPanel.visible)
                            windowSettings.dockHeight = dockPanel.height;

                    }

                    StackLayout {
                        SplitView.fillHeight: true
                        SplitView.minimumHeight: 240
                        currentIndex: ["comfyui", "models", "loras", "workflows"].indexOf(windowSettings.section)

                        ComfyPage {
                            id: comfyPage

                            app: window
                        }

                        ModelsPage {
                            app: window
                        }

                        CivitaiBrowser {
                            app: window
                            kind: "loras"
                        }

                        WorkflowsPage {
                            app: window
                        }

                    }

                    Item {
                        id: dockPanel

                        visible: windowSettings.rightMode !== ""
                        SplitView.preferredHeight: windowSettings.dockHeight
                        SplitView.minimumHeight: 120

                        ActivityPanel {
                            anchors.fill: parent
                            visible: windowSettings.rightMode === "activity"
                            header: false
                            lines: activityLines
                            emptyText: "Nothing yet."
                        }

                        DownloadsPanel {
                            anchors.fill: parent
                            visible: windowSettings.rightMode === "downloads"
                            header: false
                        }

                        ConsolePanel {
                            id: comfyConsole

                            anchors.fill: parent
                            visible: windowSettings.rightMode === "comfyui-log"
                            lines: comfyLines
                            emptyText: "ComfyUI hasn't printed anything yet."
                        }

                    }

                    handle: Rectangle {
                        implicitHeight: 1
                        color: SplitHandle.hovered || SplitHandle.pressed ? Theme.primary : Theme.border
                    }

                }

                DockBar {
                    Layout.fillWidth: true
                    mode: windowSettings.rightMode
                    comfyRunning: comfyPage.runningRoot !== ""
                    onPicked: (mode) => {
                        return windowSettings.rightMode = mode;
                    }

                    PanelButton {
                        visible: windowSettings.rightMode === "downloads" && Downloads.busy
                        iconName: "stop"
                        tip: "Cancel the download"
                        implicitWidth: 26
                        implicitHeight: 26
                        onClicked: Downloads.cancel()
                    }

                    PanelButton {
                        visible: windowSettings.rightMode === "comfyui-log" && comfyLines.count > 0
                        iconName: "clipboard"
                        tip: "Copy ComfyUI's output"
                        implicitWidth: 26
                        implicitHeight: 26
                        onClicked: {
                            const text = [];
                            for (let i = 0; i < comfyLines.count; i++)
                                text.push(comfyLines.get(i).plain);
                            window.copyText(text.join("\n"));
                            window.log("Copied ComfyUI's output (" + comfyLines.count + " lines).", "ok");
                        }
                    }

                    PanelButton {
                        visible: (windowSettings.rightMode === "downloads" && !Downloads.busy && Downloads.completed.count > 0) || windowSettings.rightMode === "activity" || windowSettings.rightMode === "comfyui-log"
                        iconName: "delete-bin"
                        tip: "Clear"
                        implicitWidth: 26
                        implicitHeight: 26
                        onClicked: {
                            if (windowSettings.rightMode === "downloads")
                                Downloads.completed.clear();
                            else if (windowSettings.rightMode === "activity")
                                activityLines.clear();
                            else {
                                comfyLines.clear();
                                window.comfyInTraceback = false;
                            }
                        }
                    }

                }

            }

            handle: Rectangle {
                implicitWidth: 1
                color: SplitHandle.hovered || SplitHandle.pressed ? Theme.primary : Theme.border
            }

        }

        // Last, so it covers everything.
        StartupOverlay {
            id: startupOverlay

            anchors.fill: parent
            active: (window.startupSlow && !window.startupReady) || window.updating
        }

    }

}
