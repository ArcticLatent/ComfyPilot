import ArcticHelper
import QtQuick
import QtQuick.Controls

TextField {
    id: field

    color: Theme.foreground
    placeholderTextColor: Theme.muted
    selectionColor: Theme.selection
    selectedTextColor: Theme.foreground
    font.pixelSize: Theme.px(13)
    leftPadding: 10
    rightPadding: 10
    implicitHeight: 32

    background: Rectangle {
        radius: 6
        color: Theme.background
        border.color: field.activeFocus ? Theme.primary : Theme.border
    }

}
