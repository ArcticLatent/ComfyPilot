import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The app's sections, the ComfyUI installs, and the system summary (laid
// out as Harlo's sidebar: logo and action, primary rows, grouped rows, a
// card at the bottom).
Rectangle {
    id: root

    /// The open section: "comfyui", "models", "loras" or "workflows".
    property string section: "comfyui"
    /// [{ name, root }] from list_comfyui_installations.
    property var installs: []
    /// The selected install's folder.
    property string selectedRoot: ""
    /// The folder of the ComfyUI that's running ("" when none).
    property string runningRoot: ""
    /// Small counts shown on the section rows.
    property int modelCount: 0
    property int workflowCount: 0
    /// The app's version, by the logo ("" until known).
    property string appVersion: ""
    /// GPU and RAM, one line each.
    property string gpuText: ""
    property string ramText: ""

    signal sectionPicked(string section)
    signal installPicked(string root)
    signal newInstallRequested()

    function samePath(a, b) {
        return String(a || "").replace(/\\/g, "/").replace(/\/+$/, "") === String(b || "").replace(/\\/g, "/").replace(/\/+$/, "");
    }

    color: Theme.chrome
    clip: true

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 14
            Layout.bottomMargin: 16

            LogoMark {
                Layout.leftMargin: 2
            }

            Label {
                Layout.alignment: Qt.AlignBottom
                Layout.bottomMargin: 2
                visible: root.appVersion !== ""
                text: "v" + root.appVersion
                color: Theme.textFaint
                font.family: Theme.mono
                font.pixelSize: Theme.px(10.5)
            }

            Item {
                Layout.fillWidth: true
            }

            FlatButton {
                text: root.width < 250 ? "+" : "+ Install"
                font.pixelSize: Theme.px(12)
                onClicked: root.newInstallRequested()
                ToolTip.visible: hovered
                ToolTip.text: "Install a new ComfyUI"
            }

        }

        // The sections, as Harlo's "New session" row.
        Repeater {
            model: [{
                "key": "comfyui",
                "icon": "server",
                "label": "ComfyUI",
                "hint": root.installs.length ? String(root.installs.length) : ""
            }, {
                "key": "models",
                "icon": "database-2",
                "label": "Models",
                "hint": root.modelCount ? String(root.modelCount) : ""
            }, {
                "key": "loras",
                "icon": "sparkling",
                "label": "LoRAs",
                "hint": "Civitai"
            }, {
                "key": "workflows",
                "icon": "node-tree",
                "label": "Workflows",
                "hint": root.workflowCount ? String(root.workflowCount) : ""
            }]

            delegate: Button {
                id: navRow

                required property var modelData
                readonly property bool current: root.section === modelData.key

                Layout.fillWidth: true
                Layout.leftMargin: 14
                Layout.rightMargin: 14
                Layout.bottomMargin: 2
                implicitHeight: 38
                flat: true
                hoverEnabled: true
                leftPadding: 12
                rightPadding: 12
                onClicked: root.sectionPicked(modelData.key)

                contentItem: RowLayout {
                    spacing: 10

                    Icon {
                        name: navRow.modelData.icon
                        color: navRow.current ? Theme.accent : navRow.hovered ? Theme.text : Theme.textMuted
                        implicitWidth: 17
                        implicitHeight: 17
                    }

                    Label {
                        Layout.fillWidth: true
                        text: navRow.modelData.label
                        color: navRow.current ? Theme.text : Theme.textMuted
                        font.pixelSize: Theme.px(14)
                        font.weight: navRow.current ? Font.Medium : Font.Normal
                    }

                    Label {
                        visible: root.width >= 220 && navRow.modelData.hint !== ""
                        text: navRow.modelData.hint
                        color: Theme.textFaint
                        font.pixelSize: Theme.px(12)
                    }

                }

                background: Rectangle {
                    radius: 10
                    color: navRow.current || navRow.down ? Theme.rowSelected : navRow.hovered ? Theme.rowHover : "transparent"

                    Behavior on color {
                        ColorAnimation {
                            duration: Theme.hoverMotion
                        }

                    }

                }

            }

        }

        // The installs: a group header and a row each.
        ItemDelegate {
            id: groupRow

            Layout.fillWidth: true
            Layout.topMargin: 15
            // As tall as an install row, highlight included (46 px, with
            // 3 px above and below it).
            implicitHeight: 52
            hoverEnabled: true
            leftInset: 10
            rightInset: 10
            topInset: 3
            bottomInset: 3
            leftPadding: 18
            rightPadding: 14
            // No vertical padding (the Basic style's 12 px would push the
            // content below the middle of the highlight).
            topPadding: 0
            bottomPadding: 0

            background: Rectangle {
                radius: 9
                color: groupRow.hovered ? Theme.rowHover : "transparent"
            }

            contentItem: RowLayout {
                spacing: 6

                Icon {
                    name: "folder"
                    color: Theme.label
                    implicitWidth: 13
                    implicitHeight: 13
                }

                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 2
                    text: "ComfyUI installs"
                    color: groupRow.hovered ? Theme.textMuted : Theme.label
                    font.pixelSize: Theme.px(12)
                    font.weight: Font.Medium
                    font.capitalization: Font.AllUppercase
                    font.letterSpacing: Theme.px(12) * 0.07
                    elide: Text.ElideRight
                }

                Label {
                    text: root.installs.length
                    color: Theme.textFaint
                    font.pixelSize: Theme.px(12)
                }

                FlatButton {
                    text: "+"
                    outlined: false
                    accent: Theme.textMuted
                    implicitWidth: 24
                    implicitHeight: 24
                    padding: 0
                    font.pixelSize: Theme.px(18)
                    ToolTip.visible: hovered
                    ToolTip.text: "Install a new ComfyUI"
                    onClicked: root.newInstallRequested()
                }

            }

        }

        ListView {
            id: installList

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: root.installs
            boundsBehavior: Flickable.StopAtBounds

            Label {
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: 28
                anchors.topMargin: 6
                visible: installList.count === 0
                text: "No installs found yet. Install ComfyUI, or choose the folder that holds yours on the ComfyUI page."
                color: Theme.textFaint
                font.pixelSize: Theme.px(12)
                wrapMode: Text.Wrap
            }

            ScrollBar.vertical: ScrollBar {
            }

            delegate: ItemDelegate {
                id: installRow

                required property var modelData
                readonly property bool active: root.samePath(modelData.root, root.selectedRoot)
                readonly property bool running: root.runningRoot !== "" && root.samePath(modelData.root, root.runningRoot)

                width: ListView.view.width
                // A 46 px highlight with 3 px above and below it, as the group row.
                height: 52
                hoverEnabled: true
                leftInset: 10
                rightInset: 10
                topInset: 3
                bottomInset: 3
                leftPadding: 18
                rightPadding: 18
                topPadding: 0
                bottomPadding: 0
                ToolTip.visible: hovered
                ToolTip.text: modelData.root
                ToolTip.delay: 700
                onClicked: root.installPicked(modelData.root)

                background: Rectangle {
                    radius: 9
                    color: installRow.active ? Theme.rowSelected : installRow.hovered ? Theme.rowHover : "transparent"

                    Behavior on color {
                        ColorAnimation {
                            duration: Theme.hoverMotion
                        }

                    }

                }

                contentItem: RowLayout {
                    spacing: 11

                    // A soft disc with the install's mark; its state on the corner.
                    Item {
                        implicitWidth: 26
                        implicitHeight: 26

                        Rectangle {
                            anchors.fill: parent
                            radius: width / 2
                            color: installRow.running ? Theme.alpha(Theme.success, 0.18) : Theme.accentSoft

                            Icon {
                                anchors.centerIn: parent
                                name: "server"
                                color: installRow.running ? Theme.success : Theme.accent
                                implicitWidth: 13
                                implicitHeight: 13
                            }

                        }

                        StatusDot {
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            status: installRow.running ? "running" : ""
                            border.color: Theme.chrome
                            border.width: 1.5
                            implicitWidth: 9
                            implicitHeight: 9
                            radius: 4.5
                        }

                    }

                    // The name, and under it the ComfyUI version and PyTorch build.
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 1

                        // The name, with "running" beside it (not over the info line).
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8

                            Label {
                                Layout.fillWidth: true
                                text: installRow.modelData.name
                                color: installRow.active ? Theme.text : Theme.textMuted
                                elide: Text.ElideRight
                                font.pixelSize: Theme.px(14)
                                font.weight: installRow.active ? Font.Medium : Font.Normal
                            }

                            Label {
                                visible: installRow.running
                                text: "running"
                                color: Theme.success
                                font.pixelSize: Theme.px(12)
                            }

                        }

                        Label {
                            Layout.fillWidth: true
                            visible: text !== ""
                            text: installRow.modelData.summary || ""
                            color: Theme.textFaint
                            elide: Text.ElideRight
                            font.pixelSize: Theme.px(11)
                        }

                    }

                }

            }

        }

        // The system: GPU and RAM, as Harlo's notice card.
        Rectangle {
            Layout.fillWidth: true
            Layout.leftMargin: 10
            Layout.rightMargin: 10
            Layout.bottomMargin: 10
            visible: root.gpuText !== "" || root.ramText !== ""
            implicitHeight: systemRow.implicitHeight + 20
            radius: 12
            color: Theme.raised
            border.color: Theme.hairline

            RowLayout {
                id: systemRow

                x: 12
                y: 10
                width: parent.width - 24
                spacing: 9

                Icon {
                    Layout.alignment: Qt.AlignTop
                    Layout.topMargin: 1
                    name: "cpu"
                    color: Theme.textMuted
                    implicitWidth: 16
                    implicitHeight: 16
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2

                    Label {
                        Layout.fillWidth: true
                        text: root.gpuText
                        color: Theme.text
                        font.pixelSize: Theme.px(13)
                        elide: Text.ElideRight
                    }

                    Label {
                        Layout.fillWidth: true
                        visible: root.ramText !== ""
                        text: root.ramText
                        color: Theme.textMuted
                        font.pixelSize: Theme.px(12)
                    }

                }

            }

        }

    }

}
