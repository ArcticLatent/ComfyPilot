import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A side panel's head, as the mockup's: its title, what it's about in grey
// under it, its own buttons (`actions`: refresh…) and a round close button,
// over a hairline.
Item {
    id: head

    property string title
    property string subtitle
    default property alias actions: actionRow.data

    signal closeRequested()

    Layout.fillWidth: true
    Layout.minimumWidth: 0
    implicitHeight: Math.max(textCol.implicitHeight, 32) + 34

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.hairline
    }

    ColumnLayout {
        id: textCol

        x: 18
        width: Math.max(0, closeButton.x - actionRow.width - x - 10)
        anchors.verticalCenter: parent.verticalCenter
        spacing: 3

        Label {
            Layout.fillWidth: true
            text: head.title
            color: Theme.text
            font.pixelSize: Theme.px(18)
            font.weight: Font.DemiBold
            font.letterSpacing: -0.2
            elide: Text.ElideRight
        }

        Label {
            Layout.fillWidth: true
            visible: head.subtitle !== ""
            text: head.subtitle
            color: Theme.textMuted
            font.pixelSize: Theme.px(13)
            elide: Text.ElideMiddle
        }

    }

    Row {
        id: actionRow

        anchors.right: closeButton.left
        anchors.rightMargin: 6
        anchors.verticalCenter: parent.verticalCenter
        spacing: 2
    }

    PanelButton {
        id: closeButton

        anchors.right: parent.right
        anchors.rightMargin: 14
        anchors.verticalCenter: parent.verticalCenter
        round: true
        iconName: "close"
        tip: "Close"
        onClicked: head.closeRequested()
    }

}
