import ArcticHelper
import QtQuick
import QtQuick.Controls

// Vertical icon bar at the window's left edge (as Harlo's): the sidebar's
// toggle at the top; app-wide actions at the bottom.
Rectangle {
    id: root

    property bool sidebarOpen: true
    property bool updateAvailable: false
    property bool checkingUpdates: false
    property bool lightTheme: false

    signal sidebarToggled()
    signal checkUpdatesRequested()
    signal diagnosticsRequested()
    signal logsRequested()
    signal themeToggled()

    implicitWidth: 52
    color: Theme.chrome

    Rectangle {
        anchors.right: parent.right
        width: 1
        height: parent.height
        color: Theme.hairline
    }

    RailButton {
        anchors.top: parent.top
        anchors.topMargin: 10
        iconName: "layout-column"
        tip: (root.sidebarOpen ? "Hide" : "Show") + " the sidebar"
        onClicked: root.sidebarToggled()
    }

    Column {
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 10
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: 6

        // Light or dark: the icon of the theme it switches to.
        RailButton {
            iconName: root.lightTheme ? "moon" : "sun"
            tip: root.lightTheme ? "Dark theme" : "Light theme"
            onClicked: root.themeToggled()
        }

        RailButton {
            iconName: "clipboard"
            tip: "Copy diagnostics for a bug report"
            onClicked: root.diagnosticsRequested()
        }

        RailButton {
            iconName: "file-list-3"
            tip: "Open the log folder"
            onClicked: root.logsRequested()
        }

        RailButton {
            iconName: root.updateAvailable ? "download-2" : "refresh"
            tip: root.checkingUpdates ? "Checking for updates..." : root.updateAvailable ? "Install the update" : "Check for updates"
            active: root.updateAvailable
            enabled: !root.checkingUpdates
            onClicked: root.checkUpdatesRequested()

            Rectangle {
                visible: root.updateAvailable
                width: 8
                height: 8
                radius: 4
                color: Theme.success
                border.color: Theme.chrome
                border.width: 1.5
                anchors.top: parent.top
                anchors.right: parent.right
                anchors.margins: 5
            }

        }

    }

    component RailButton: Button {
        id: rb

        property string iconName
        property string tip
        property bool active: false

        anchors.horizontalCenter: parent ? parent.horizontalCenter : undefined
        flat: true
        implicitWidth: 38
        implicitHeight: 38
        padding: 0
        ToolTip.visible: hovered
        ToolTip.text: tip
        ToolTip.delay: 400

        contentItem: Item {
            Icon {
                anchors.centerIn: parent
                name: rb.iconName
                implicitWidth: 19
                implicitHeight: 19
                color: rb.active ? Theme.accent : rb.hovered ? Theme.text : Theme.textMuted
            }

        }

        background: Rectangle {
            radius: 10
            color: rb.active || rb.down ? Theme.rowSelected : rb.hovered ? Theme.rowHover : "transparent"

            Behavior on color {
                ColorAnimation {
                    duration: Theme.hoverMotion
                }

            }

        }

    }

}
