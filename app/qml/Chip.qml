import ArcticHelper
import QtQuick
import QtQuick.Controls

// A small label pill. In a layout it can be narrower than its text, which
// then elides (give it Layout.fillWidth with Layout.maximumWidth: implicitWidth).
Rectangle {
    property alias text: label.text
    property color fg: Theme.muted

    radius: 4
    color: Theme.elevated
    border.color: Theme.border
    implicitWidth: label.implicitWidth + 14
    implicitHeight: 22
    clip: true

    Label {
        id: label

        x: 7
        width: parent.width - 14
        anchors.verticalCenter: parent.verticalCenter
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
        color: parent.fg
        font.pixelSize: Theme.px(12)
    }

}
