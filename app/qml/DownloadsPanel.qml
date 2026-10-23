import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import ArcticHelper

// The downloads panel: the running download with each file's progress on
// the left, what finished on the right (one column when it's narrow).
Rectangle {
    id: panel

    /// Its own title bar (off in the bottom dock, whose bar shows the title).
    property bool header: true
    readonly property bool wide: width >= 760

    signal closeRequested()

    color: header ? Theme.canvas : Theme.subtle

    component ProgressBarLine: Rectangle {
        property real value: 0
        property color fill: Theme.primary

        implicitHeight: 4
        radius: 2
        color: Theme.controlFill

        Rectangle {
            width: parent.width * Math.max(0, Math.min(1, parent.value))
            height: parent.height
            radius: 2
            color: parent.fill
        }

    }

    component SectionLabel: Label {
        color: Theme.label
        font.pixelSize: Theme.px(12)
        font.weight: Font.Medium
        font.capitalization: Font.AllUppercase
        font.letterSpacing: Theme.px(12) * 0.07
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        PanelHeader {
            visible: panel.header
            title: "Downloads"
            subtitle: Downloads.status
            onCloseRequested: panel.closeRequested()

            PanelButton {
                visible: Downloads.busy
                iconName: "stop"
                tip: "Cancel the download"
                onClicked: Downloads.cancel()
            }

            PanelButton {
                visible: !Downloads.busy && Downloads.completed.count > 0
                iconName: "delete-bin"
                tip: "Clear the finished list"
                onClicked: Downloads.completed.clear()
            }

        }

        GridLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: 16
            Layout.topMargin: 12
            columns: panel.wide ? 2 : 1
            columnSpacing: 28
            rowSpacing: 16

            // ---- now ----
            Flickable {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 3
                contentHeight: nowColumn.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                ColumnLayout {
                    id: nowColumn

                    width: parent.width
                    spacing: 12

                    RowLayout {
                        Layout.fillWidth: true

                        SectionLabel {
                            text: Downloads.busy ? "Downloading" : "Now"
                        }

                        Label {
                            Layout.fillWidth: true
                            horizontalAlignment: Text.AlignRight
                            visible: Downloads.busy && Downloads.totalBytes > 0
                            text: Math.round(Downloads.fraction * 100) + "%  ·  " + Downloads.formatBytes(Downloads.receivedBytes) + " / " + Downloads.formatBytes(Downloads.totalBytes)
                            color: Theme.textMuted
                            font.family: Theme.mono
                            font.pixelSize: Theme.px(11)
                        }

                    }

                    ProgressBarLine {
                        Layout.fillWidth: true
                        visible: Downloads.busy && Downloads.totalBytes > 0
                        value: Downloads.fraction
                    }

                    Label {
                        Layout.fillWidth: true
                        text: Downloads.status
                        color: Theme.textMuted
                        font.pixelSize: Theme.px(13)
                        elide: Text.ElideMiddle
                    }

                    Repeater {
                        model: Downloads.transfers

                        delegate: ColumnLayout {
                            id: transfer

                            required property string artifact
                            required property real received
                            required property real size
                            required property string phase

                            Layout.fillWidth: true
                            spacing: 5

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8

                                Icon {
                                    name: transfer.phase === "verifying" ? "shield-check" : "download-2"
                                    color: Theme.textMuted
                                    implicitWidth: 14
                                    implicitHeight: 14
                                }

                                Label {
                                    Layout.fillWidth: true
                                    text: transfer.artifact + (transfer.phase === "verifying" ? "  (checking)" : "")
                                    color: Theme.text
                                    font.pixelSize: Theme.px(13)
                                    elide: Text.ElideMiddle
                                }

                                Label {
                                    text: transfer.size > 0 ? Downloads.formatBytes(transfer.received) + " / " + Downloads.formatBytes(transfer.size) : Downloads.formatBytes(transfer.received)
                                    color: Theme.textFaint
                                    font.family: Theme.mono
                                    font.pixelSize: Theme.px(11)
                                }

                            }

                            ProgressBarLine {
                                Layout.fillWidth: true
                                value: transfer.size > 0 ? transfer.received / transfer.size : 0
                                fill: transfer.phase === "verifying" ? Theme.success : Theme.primary
                            }

                        }

                    }

                }

                ScrollBar.vertical: ScrollBar {
                }

            }

            // ---- finished ----
            Flickable {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 2
                contentHeight: doneColumn.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                ColumnLayout {
                    id: doneColumn

                    width: parent.width
                    spacing: 6

                    SectionLabel {
                        text: "Finished"
                    }

                    Label {
                        visible: Downloads.completed.count === 0
                        text: "Nothing yet."
                        color: Theme.textFaint
                        font.pixelSize: Theme.px(13)
                    }

                    Repeater {
                        model: Downloads.completed

                        delegate: RowLayout {
                            id: done

                            required property string name
                            required property string folder
                            required property bool failed
                            required property string message

                            Layout.fillWidth: true
                            spacing: 8

                            Icon {
                                name: done.failed ? "close-circle" : "checkbox-circle"
                                color: done.failed ? Theme.error : Theme.success
                                implicitWidth: 14
                                implicitHeight: 14
                            }

                            Label {
                                Layout.fillWidth: true
                                text: done.name
                                color: Theme.textMuted
                                font.pixelSize: Theme.px(13)
                                elide: Text.ElideMiddle
                                ToolTip.visible: doneHover.hovered && (done.message !== "" || done.folder !== "")
                                ToolTip.text: done.failed ? done.message : done.folder

                                HoverHandler {
                                    id: doneHover
                                }

                            }

                            PanelButton {
                                visible: done.folder !== ""
                                iconName: "folder-open"
                                tip: "Open the folder"
                                implicitWidth: 26
                                implicitHeight: 26
                                onClicked: Api.call("open_folder", {
                                    "path": done.folder
                                })
                            }

                        }

                    }

                }

                ScrollBar.vertical: ScrollBar {
                }

            }

        }

    }

}
