import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A group of settings on the canvas: a small uppercase label (as Harlo's
// section labels), an optional line about it, and its rows on a raised card.
ColumnLayout {
    id: section

    property string title
    property string description: ""
    /// Rows without the card behind them (for tiles, which are cards themselves).
    property bool plain: false
    /// Rows go on the card.
    default property alias rows: cardColumn.data
    /// Buttons next to the label.
    property alias actions: actionRow.data

    Layout.fillWidth: true
    spacing: 8

    RowLayout {
        Layout.fillWidth: true
        Layout.leftMargin: 2

        Label {
            Layout.fillWidth: true
            text: section.title
            color: Theme.label
            font.pixelSize: Theme.px(12)
            font.weight: Font.Medium
            font.capitalization: Font.AllUppercase
            font.letterSpacing: Theme.px(12) * 0.07
        }

        Row {
            id: actionRow

            spacing: 6
        }

    }

    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 2
        visible: section.description !== ""
        text: section.description
        color: Theme.textFaint
        font.pixelSize: Theme.px(12)
        wrapMode: Text.Wrap
    }

    Rectangle {
        Layout.fillWidth: true
        implicitHeight: cardColumn.implicitHeight
        radius: 12
        color: section.plain ? "transparent" : Theme.raised
        border.color: section.plain ? "transparent" : Theme.hairline

        ColumnLayout {
            id: cardColumn

            width: parent.width
            spacing: 0
        }

    }

}
