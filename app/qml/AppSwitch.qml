import ArcticHelper
import QtQuick
import QtQuick.Controls

// On/off switch in the app's colours (the off track a step brighter than
// Harlo's, to show on the raised cards).
Switch {
    id: sw

    padding: 0
    implicitWidth: 34
    implicitHeight: 20

    contentItem: Item {
    }

    indicator: Rectangle {
        implicitWidth: 34
        implicitHeight: 20
        y: (sw.height - height) / 2
        radius: height / 2
        color: sw.checked ? Theme.primary : Theme.controlBorderHover
        opacity: sw.enabled ? 1 : 0.5

        Rectangle {
            width: 14
            height: 14
            radius: 7
            y: 3
            x: sw.checked ? parent.width - width - 3 : 3
            color: sw.checked ? Theme.primaryText : Theme.muted

            Behavior on x {
                NumberAnimation {
                    duration: 120
                    easing.type: Easing.OutCubic
                }

            }

        }

    }

}
