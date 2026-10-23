import ArcticHelper
import QtQuick
import QtQuick.Controls

CheckBox {
    id: cb

    font.pixelSize: Theme.px(13)
    spacing: 8
    padding: 2

    indicator: Rectangle {
        implicitWidth: 16
        implicitHeight: 16
        x: cb.leftPadding
        y: (cb.height - height) / 2
        radius: 4
        color: cb.checked ? Theme.primary : Theme.background
        border.color: cb.checked ? Theme.primary : cb.hovered ? Theme.muted : Theme.controlBorderHover

        Label {
            anchors.centerIn: parent
            visible: cb.checked
            text: "✓"
            color: Theme.primaryText
            font.pixelSize: Theme.px(11)
            font.bold: true
        }

    }

    contentItem: Label {
        leftPadding: cb.indicator.width + cb.spacing
        text: cb.text
        font: cb.font
        color: Theme.foreground
        opacity: cb.enabled ? 1 : 0.45
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.Wrap
    }

}
