import ArcticHelper
import QtQuick
import QtQuick.Controls

// A small count bubble for the top-right corner of a button.
// Place it in the button and anchor it: `anchors.top: parent.top;
// anchors.right: parent.right`. Hidden when the count is 0.
Rectangle {
    property int count: 0

    visible: count > 0
    height: 16
    width: Math.max(16, label.implicitWidth + 8)
    radius: 8
    color: Theme.primary
    border.color: Theme.background
    border.width: 1.5
    anchors.topMargin: -6
    anchors.rightMargin: -6
    z: 5

    Label {
        id: label

        anchors.centerIn: parent
        text: parent.count > 99 ? "99+" : parent.count
        color: Theme.background
        font.pixelSize: Theme.px(10)
        font.bold: true
    }

}
