import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A small fact on the canvas: an icon, a label and its value.
Rectangle {
    id: tile

    property string icon
    property string label
    property string value
    property color valueColor: Theme.text

    Layout.fillWidth: true
    implicitHeight: 64
    radius: 12
    color: Theme.raised
    border.color: Theme.hairline

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 14
        spacing: 12

        Icon {
            name: tile.icon
            color: Theme.textMuted
            implicitWidth: 18
            implicitHeight: 18
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            Label {
                text: tile.label
                color: Theme.textMuted
                font.pixelSize: Theme.px(12)
            }

            Label {
                Layout.fillWidth: true
                text: tile.value
                color: tile.valueColor
                font.pixelSize: Theme.px(14)
                font.weight: Font.Medium
                elide: Text.ElideRight
            }

        }

    }

}
