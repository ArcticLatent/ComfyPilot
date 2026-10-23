import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The selected ComfyUI install: start/stop, update, its torch stack, and
// its add-ons, custom nodes, launch options and shared models folder.
Item {
    id: view

    /// The ComfyPage (state and actions).
    property var ctl

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        PageHeader {
            title: view.ctl.install ? view.ctl.install.name : "ComfyUI"
            subtitle: view.ctl.root
            subtitleIcon: "folder"

            FlatButton {
                text: "Open folder"
                onClicked: view.ctl.openFolder()
            }

            FlatButton {
                text: view.ctl.updateBusy ? "Updating…" : view.ctl.updateChecking ? "Checking…" : view.ctl.updateStatus && view.ctl.updateStatus.update_available ? "Update to " + view.ctl.updateStatus.latest_version : "Check for update"
                variant: view.ctl.updateStatus && view.ctl.updateStatus.update_available ? "selected" : "outline"
                enabled: !view.ctl.updateBusy && !view.ctl.updateChecking && !view.ctl.anyBusy
                onClicked: view.ctl.updateStatus && view.ctl.updateStatus.update_available ? view.ctl.updateComfy() : view.ctl.checkForUpdate()
                ToolTip.visible: hovered && view.ctl.updateStatus !== null
                ToolTip.text: view.ctl.updateStatus ? view.ctl.updateStatus.detail : ""
            }

            FlatButton {
                text: view.ctl.startingHere ? "Starting…" : view.ctl.running ? "Stop ComfyUI" : "Start ComfyUI"
                variant: view.ctl.running ? "stop" : "primary"
                // One ComfyUI at a time: another install's holds the port.
                enabled: !view.ctl.starting && !view.ctl.anyBusy && !view.ctl.otherRunning
                onClicked: view.ctl.startOrStop()
                ToolTip.visible: hovered && view.ctl.otherRunning
                ToolTip.text: view.ctl.nameOf(view.ctl.runningRoot) + " is running. Stop it first (select it in the sidebar)."
            }

        }

        Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: body.implicitHeight + 56
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            ColumnLayout {
                id: body

                x: 28
                y: 24
                width: Math.min(parent.width - 56, 1080)
                spacing: 26

                // The install at a glance.
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12

                    InfoTile {
                        icon: "pulse"
                        label: "Status"
                        value: view.ctl.startingHere ? "Starting…" : view.ctl.running ? "Running" : view.ctl.otherRunning ? "Stopped · " + view.ctl.nameOf(view.ctl.runningRoot) + " is running" : "Stopped"
                        valueColor: view.ctl.running ? Theme.success : view.ctl.startingHere ? Theme.primary : Theme.text
                    }

                    InfoTile {
                        icon: "code-box"
                        label: view.ctl.updateStatus && view.ctl.updateStatus.update_available ? "Version (" + view.ctl.updateStatus.latest_version + " available)" : "Version"
                        value: view.ctl.versionText
                    }

                    InfoTile {
                        icon: "stack"
                        label: "Torch stack"
                        value: view.ctl.addonState && view.ctl.addonState.torch_profile ? view.ctl.profileLabel(view.ctl.addonState.torch_profile) : view.ctl.addonStateLoading ? "Reading…" : "Unknown"
                    }

                }

                // Running: where it is.
                Rectangle {
                    Layout.fillWidth: true
                    visible: view.ctl.running && view.ctl.runtimeUrl !== ""
                    implicitHeight: 48
                    radius: 12
                    color: Theme.alpha(Theme.success, 0.08)
                    border.color: Theme.alpha(Theme.success, 0.3)

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 14
                        anchors.rightMargin: 8
                        spacing: 10

                        StatusDot {
                            status: "running"
                        }

                        Label {
                            Layout.fillWidth: true
                            text: "ComfyUI is running at " + view.ctl.runtimeUrl
                            color: Theme.text
                            font.pixelSize: Theme.px(13)
                            elide: Text.ElideRight
                        }

                        FlatButton {
                            text: "Open in browser"
                            onClicked: view.ctl.openComfy()
                        }

                    }

                }

                SectionCard {
                    title: "Add-ons"
                    description: "Installed into this ComfyUI's Python environment. Only one attention backend at a time."

                    OptionGrid {
                        ctl: view.ctl
                        defs: view.ctl.addonDefs
                        available: !view.ctl.anyBusy && view.ctl.addonState !== null
                    }

                }

                SectionCard {
                    title: "Custom nodes"
                    description: "Installed at the versions the ComfyPilot catalog pins."

                    OptionGrid {
                        ctl: view.ctl
                        defs: view.ctl.nodeDefs
                        available: !view.ctl.anyBusy && view.ctl.addonState !== null
                    }

                }

                LaunchOptions {
                    ctl: view.ctl
                    showAttentionFlags: true
                }

                SectionCard {
                    title: "Shared models folder"
                    description: "A folder of models several ComfyUI installs share (extra_model_paths.yaml)."

                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.margins: 16
                        spacing: 12

                        FolderField {
                            Layout.fillWidth: true
                            path: view.ctl.sharedModelsPath
                            placeholder: "None"
                            dialogTitle: "Choose the shared models folder"
                            onPicked: (path) => {
                                return view.ctl.saveSharedModels(path, true);
                            }
                        }

                        RowLayout {
                            Layout.fillWidth: true

                            AppSwitch {
                                checked: view.ctl.sharedModelsDefault
                                enabled: view.ctl.sharedModelsPath !== ""
                                onToggled: view.ctl.saveSharedModels(view.ctl.sharedModelsPath, checked)
                            }

                            Label {
                                Layout.fillWidth: true
                                text: "Download models into it by default"
                                color: Theme.textMuted
                                font.pixelSize: Theme.px(13)
                            }

                            FlatButton {
                                text: "Clear"
                                outlined: false
                                accent: Theme.textMuted
                                visible: view.ctl.sharedModelsPath !== ""
                                onClicked: view.ctl.saveSharedModels("", false)
                            }

                        }

                    }

                }

            }

            ScrollBar.vertical: ScrollBar {
            }

        }

    }

}
