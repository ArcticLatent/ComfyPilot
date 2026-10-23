import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A side panel listing log lines, newest at the bottom: the app's activity
// or ComfyUI's output. `lines` is a ListModel of { time, text, level }.
Rectangle {
    id: panel

    property string title: "Activity"
    property string subtitle: ""
    property ListModel lines
    /// Code-like output (ComfyUI's): the monospace font.
    property bool mono: false
    property string emptyText: "Nothing yet."

    /// Its own title bar (off in the bottom dock, whose bar shows the title).
    property bool header: true

    signal closeRequested()

    color: header ? Theme.canvas : Theme.subtle

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        PanelHeader {
            visible: panel.header
            title: panel.title
            subtitle: panel.subtitle
            onCloseRequested: panel.closeRequested()

            PanelButton {
                iconName: "delete-bin"
                tip: "Clear"
                onClicked: panel.lines.clear()
            }

        }

        ListView {
            id: view

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: panel.lines
            topMargin: 10
            bottomMargin: 10
            spacing: 2
            boundsBehavior: Flickable.StopAtBounds
            onCountChanged: Qt.callLater(positionViewAtEnd)

            Label {
                anchors.centerIn: parent
                visible: view.count === 0
                text: panel.emptyText
                color: Theme.textFaint
                font.pixelSize: Theme.px(13)
            }

            ScrollBar.vertical: ScrollBar {
            }

            delegate: Item {
                id: row

                required property string time
                required property string text
                required property string level

                width: ListView.view.width
                height: lineRow.implicitHeight + 6

                RowLayout {
                    id: lineRow

                    x: 18
                    width: parent.width - 32
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 9

                    Rectangle {
                        Layout.alignment: Qt.AlignTop
                        Layout.topMargin: 6
                        implicitWidth: 6
                        implicitHeight: 6
                        radius: 3
                        color: Theme.statusColor(row.level)
                        opacity: row.level === "" ? 0.5 : 1
                    }

                    Label {
                        Layout.fillWidth: true
                        text: row.text
                        color: row.level === "error" ? Theme.error : row.level === "warn" ? Theme.warning : Theme.text
                        wrapMode: Text.Wrap
                        textFormat: Text.PlainText
                        font.family: panel.mono ? Theme.mono : Theme.uiFont
                        font.pixelSize: Theme.px(panel.mono ? 12 : 13)
                        lineHeight: 1.15
                    }

                    Label {
                        Layout.alignment: Qt.AlignTop
                        text: row.time
                        color: Theme.textFaint
                        font.family: Theme.mono
                        font.pixelSize: Theme.px(11)
                    }

                }

            }

        }

    }

}
