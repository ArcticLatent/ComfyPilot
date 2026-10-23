import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import ArcticHelper

// The bar along the bottom of the main area: tabs for the bottom panels
// (Activity, Downloads, ComfyUI output) with a word on each's state, and the
// open panel's buttons on the right. Clicking the open tab closes it.
Rectangle {
    id: bar
    /// The open panel ("" when closed).
    property string mode: ""
    property bool comfyRunning: false
    /// The open panel's buttons.
    default property alias actions: actionRow.data
    signal picked(string mode)

    implicitHeight: 34
    color: Theme.chrome
    Rectangle {
        width: parent.width
        height: 1
        color: Theme.hairline
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 2

        DockTab {
            mode: "activity"
            iconName: "list-check-2"
            label: "Activity"
        }
        DockTab {
            mode: "downloads"
            iconName: "download-2"
            label: Downloads.busy && Downloads.totalBytes > 0 ? "Downloads · " + Math.round(Downloads.fraction * 100) + "%" : "Downloads"
            highlight: Downloads.busy
            // A thin progress line under the tab while downloading.
            progress: Downloads.busy ? Downloads.fraction : -1
        }
        DockTab {
            mode: "comfyui-log"
            iconName: "terminal-box"
            label: "ComfyUI output"
            dot: bar.comfyRunning
        }
        Item {
            Layout.fillWidth: true
        }
        Row {
            id: actionRow
            spacing: 2
        }
        PanelButton {
            visible: bar.mode !== ""
            iconName: "arrow-down-s"
            tip: "Close the panel"
            implicitWidth: 26
            implicitHeight: 26
            onClicked: bar.picked("")
        }
    }

    component DockTab: Item {
        id: tab
        property string mode
        property string iconName
        property string label
        property bool highlight: false
        property bool dot: false
        property real progress: -1
        readonly property bool active: bar.mode === mode
        implicitWidth: tabRow.implicitWidth + 20
        Layout.fillHeight: true

        Rectangle {
            anchors.fill: parent
            anchors.topMargin: 4
            anchors.bottomMargin: 4
            radius: 7
            color: tab.active ? Theme.rowSelected : hover.hovered ? Theme.rowHover : "transparent"
            Behavior on color {
                ColorAnimation {
                    duration: Theme.hoverMotion
                }
            }
        }
        Row {
            id: tabRow
            anchors.centerIn: parent
            spacing: 7
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                name: tab.iconName
                color: tab.active || tab.highlight ? Theme.accent : Theme.textMuted
                implicitWidth: 15
                implicitHeight: 15
            }
            Label {
                anchors.verticalCenter: parent.verticalCenter
                text: tab.label
                color: tab.active ? Theme.text : Theme.textMuted
                font.pixelSize: Theme.px(12.5)
            }
            StatusDot {
                anchors.verticalCenter: parent.verticalCenter
                status: tab.dot ? "running" : ""
            }
        }
        Rectangle {
            visible: tab.progress >= 0
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 3
            x: 10
            width: (parent.width - 20) * Math.max(0, Math.min(1, tab.progress))
            height: 2
            radius: 1
            color: Theme.primary
        }
        HoverHandler {
            id: hover
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            onTapped: bar.picked(tab.active ? "" : tab.mode)
        }
    }
}
