import ArcticHelper
import QtQuick
import QtQuick.Controls

// An icon button on a side panel's head (refresh, close): a soft square
// that lights up when hovered, or a round one (`round`, the close button).
Button {
    id: pb

    property string iconName
    property string tip
    property bool round: false

    flat: true
    implicitWidth: 32
    implicitHeight: 32
    padding: 0
    ToolTip.visible: hovered && tip !== ""
    ToolTip.text: tip
    ToolTip.delay: 400

    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: pb.iconName
            color: pb.hovered ? Theme.text : Theme.textMuted
            implicitWidth: pb.round ? 14 : 16
            implicitHeight: pb.round ? 14 : 16
        }

    }

    background: Rectangle {
        radius: pb.round ? width / 2 : 9
        color: pb.round ? (pb.down ? Theme.controlFillDown : pb.hovered ? Theme.controlFillHover : Theme.controlFill) : (pb.down ? Theme.rowSelected : pb.hovered ? Theme.rowHover : "transparent")

        Behavior on color {
            ColorAnimation {
                duration: Theme.hoverMotion
            }

        }

    }

}
