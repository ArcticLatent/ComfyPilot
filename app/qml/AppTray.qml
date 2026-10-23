import ArcticHelper
import QtQuick
import Qt.labs.platform as Platform

// The system tray icon (Qt Labs Platform): the ComfyPilot mark, its C green
// while ComfyUI runs (assets/icon-running.svg). A click shows the window; the
// menu shows the window, starts, stops or restarts ComfyUI and quits. Desktop
// notifications go through it (`notify`), so there are none without a tray.
//
// The backend says whether there should be a tray (`get_tray_status`, and
// `tray-status` events, which Main passes to `applyStatus`;
// ARCTIC_ENABLE_TRAY=0 turns it off on Linux) and whether ComfyUI is running.
// (SystemTrayIcon takes no child items, so no Connections in here.)
Platform.SystemTrayIcon {
    id: tray

    /// The backend wants a tray (and the desktop has one: `available`).
    property bool wanted: false
    property bool running: false
    property string tooltipText: "ComfyPilot"
    readonly property bool shown: wanted && available

    signal showRequested()
    signal quitRequested()

    /// A desktop notification (when there is a tray).
    function notify(title, body) {
        if (shown && supportsMessages)
            showMessage(title, body);
    }

    function applyStatus(status) {
        wanted = status.enabled === true;
        running = status.running === true;
        tooltipText = status.tooltip || "ComfyPilot";
    }

    visible: shown
    icon.source: running ? "qrc:/qt/qml/ArcticHelper/qml/assets/icon-running.svg" : "qrc:/qt/qml/ArcticHelper/qml/assets/icon.svg"
    tooltip: tooltipText
    Component.onCompleted: Api.call("get_tray_status").then((status) => {
        return tray.applyStatus(status);
    }).catch(() => {
    })
    onActivated: (reason) => {
        if (reason === Platform.SystemTrayIcon.Trigger || reason === Platform.SystemTrayIcon.DoubleClick)
            tray.showRequested();

    }

    menu: Platform.Menu {
        Platform.MenuItem {
            text: "Show ComfyPilot"
            onTriggered: tray.showRequested()
        }

        Platform.MenuSeparator {
        }

        Platform.MenuItem {
            text: tray.running ? "ComfyUI is running" : "ComfyUI is stopped"
            enabled: false
        }

        Platform.MenuItem {
            text: "Start ComfyUI"
            enabled: !tray.running
            onTriggered: Api.call("tray_start_comfyui")
        }

        Platform.MenuItem {
            text: "Restart ComfyUI"
            enabled: tray.running
            onTriggered: Api.call("tray_restart_comfyui")
        }

        Platform.MenuItem {
            text: "Stop ComfyUI"
            enabled: tray.running
            onTriggered: Api.call("tray_stop_comfyui")
        }

        Platform.MenuSeparator {
        }

        Platform.MenuItem {
            text: "Quit"
            onTriggered: tray.quitRequested()
        }

    }

}
