import ArcticHelper
import QtQuick
import QtQuick.Controls

// Covers the window with the app's mark and a status line: while the first
// data loads at startup, and while an update installs. It fades out when
// `active` turns off, and takes all input while shown.
Rectangle {
    id: overlay

    property bool active: false
    property string status: "Getting things ready…"

    color: Theme.chrome
    opacity: active ? 1 : 0
    visible: opacity > 0

    Behavior on opacity {
        NumberAnimation {
            duration: Theme.paneMotion
            easing.type: Easing.OutCubic
        }

    }

    // Swallow clicks, wheel and hover while shown.
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.AllButtons
        onWheel: (wheel) => {
            return wheel.accepted = true;
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 14

        LogoMark {
            anchors.horizontalCenter: parent.horizontalCenter
            logoHeight: 52
        }

        Label {
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(implicitWidth, overlay.width - 64)
            text: overlay.status
            color: Theme.textMuted
            font.pixelSize: Theme.px(13)
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        // An indeterminate bar: a short segment sweeping across.
        Rectangle {
            id: track

            anchors.horizontalCenter: parent.horizontalCenter
            width: 180
            height: 3
            radius: 1.5
            color: Theme.controlFill
            clip: true

            Rectangle {
                id: sweep

                width: track.width * 0.35
                height: track.height
                radius: 1.5
                color: Theme.primary

                NumberAnimation on x {
                    from: -sweep.width
                    to: track.width
                    duration: 1100
                    loops: Animation.Infinite
                    running: overlay.visible
                    easing.type: Easing.InOutQuad
                }

            }

        }

    }

}
