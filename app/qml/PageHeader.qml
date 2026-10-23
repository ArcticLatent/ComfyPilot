import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A page's head on the canvas (as Harlo's chat header): the title, a line
// under it with a small icon, and the page's own buttons on the right, over
// a hairline.
Item {
    id: head

    property string title
    property string subtitle
    property string subtitleIcon: ""
    default property alias actions: actionRow.data

    Layout.fillWidth: true
    implicitHeight: 76

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.hairline
    }

    ColumnLayout {
        anchors.left: parent.left
        anchors.leftMargin: 28
        anchors.right: actionRow.left
        anchors.rightMargin: 12
        anchors.verticalCenter: parent.verticalCenter
        spacing: 5

        Label {
            Layout.fillWidth: true
            text: head.title
            color: Theme.text
            font.pixelSize: Theme.px(19)
            font.weight: Font.DemiBold
            font.letterSpacing: -0.2
            elide: Text.ElideRight
        }

        RowLayout {
            visible: head.subtitle !== ""
            spacing: 6

            Icon {
                visible: head.subtitleIcon !== ""
                name: head.subtitleIcon
                color: Theme.textMuted
                implicitWidth: 13
                implicitHeight: 13
            }

            Label {
                Layout.fillWidth: true
                text: head.subtitle
                color: Theme.textMuted
                font.pixelSize: Theme.px(13)
                elide: Text.ElideMiddle
            }

        }

    }

    Row {
        id: actionRow

        anchors.right: parent.right
        anchors.rightMargin: 24
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8
    }

}
