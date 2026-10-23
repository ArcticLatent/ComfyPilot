import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Numbered steps (a token's how-to), each a line of text by a small numbered
// disc, with an optional button that opens the page the steps start on.
ColumnLayout {
    id: list

    /// The steps, in order (plain text).
    property var steps: []
    /// A button under the steps that opens `linkUrl` in the browser.
    property string linkText: ""
    property string linkUrl: ""

    spacing: 8

    Repeater {
        model: list.steps

        delegate: RowLayout {
            id: step

            required property string modelData
            required property int index

            Layout.fillWidth: true
            spacing: 10

            Rectangle {
                Layout.alignment: Qt.AlignTop
                implicitWidth: 20
                implicitHeight: 20
                radius: 10
                color: Theme.accentSoft

                Label {
                    anchors.centerIn: parent
                    text: step.index + 1
                    color: Theme.primary
                    font.pixelSize: Theme.px(11)
                    font.weight: Font.DemiBold
                }

            }

            Label {
                Layout.fillWidth: true
                Layout.topMargin: 1
                text: step.modelData
                textFormat: Text.PlainText
                color: Theme.text
                font.pixelSize: Theme.px(12.5)
                wrapMode: Text.Wrap
                lineHeight: 1.2
            }

        }

    }

    FlatButton {
        visible: list.linkUrl !== ""
        Layout.leftMargin: 30
        text: list.linkText + "  ↗"
        onClicked: Api.call("open_external_url", {
            "url": list.linkUrl
        })
    }

}
