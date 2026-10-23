import ArcticHelper
import QtQuick

// busy/starting pulse; input, unread and live are steady dots.
Rectangle {
    id: dot

    property string status: ""
    property real pulse: 1

    implicitWidth: 8
    implicitHeight: 8
    radius: 4
    visible: status !== ""
    color: Theme.statusColor(status)
    opacity: (status === "live" ? 0.6 : 1) * pulse

    SequentialAnimation on pulse {
        running: dot.status === "busy" || dot.status === "starting"
        loops: Animation.Infinite
        alwaysRunToEnd: true

        NumberAnimation {
            to: 0.3
            duration: 600
        }

        NumberAnimation {
            to: 1
            duration: 600
        }

    }

}
