import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Installing a new ComfyUI: where, which GPU and torch stack, what comes
// with it, a preflight check, and the install itself.
Item {
    id: view

    /// The ComfyPage (state and actions).
    property var ctl
    readonly property bool cancelable: ctl.installBusy

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        PageHeader {
            title: "Install ComfyUI"
            subtitle: view.ctl.installBase ? "A new ComfyUI folder in " + view.ctl.installBase : "Choose where to install it"
            subtitleIcon: "add-circle"

            FlatButton {
                visible: view.ctl.hasInstall
                text: "Back to " + (view.ctl.install ? view.ctl.install.name : "install")
                outlined: false
                accent: Theme.textMuted
                onClicked: view.ctl.view = "manage"
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

                // An install that stopped partway.
                Rectangle {
                    Layout.fillWidth: true
                    visible: view.ctl.resumeState !== null && !view.ctl.installBusy
                    implicitHeight: resumeRow.implicitHeight + 24
                    radius: 12
                    color: Theme.alpha(Theme.warning, 0.08)
                    border.color: Theme.alpha(Theme.warning, 0.35)

                    RowLayout {
                        id: resumeRow

                        x: 14
                        y: 12
                        width: parent.width - 28
                        spacing: 10

                        Icon {
                            name: "alert"
                            color: Theme.warning
                            implicitWidth: 16
                            implicitHeight: 16
                        }

                        Label {
                            Layout.fillWidth: true
                            text: view.ctl.resumeState ? view.ctl.resumeState.summary || "An install didn't finish." : ""
                            color: Theme.text
                            font.pixelSize: Theme.px(13)
                            wrapMode: Text.Wrap
                        }

                        FlatButton {
                            text: "Start fresh"
                            onClicked: view.ctl.startInstall(true)
                        }

                        FlatButton {
                            text: "Resume"
                            variant: "primary"
                            onClicked: view.ctl.startInstall(false)
                        }

                    }

                }

                SectionCard {
                    title: "Location"
                    description: "ComfyUI goes into a new ComfyUI folder (ComfyUI-2, -3… when there's one already) inside this folder."

                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.margins: 16
                        spacing: 10

                        Label {
                            text: "Install folder"
                            color: Theme.text
                            font.pixelSize: Theme.px(14)
                        }

                        FolderField {
                            Layout.fillWidth: true
                            path: view.ctl.installBase
                            placeholder: "For example your Documents folder"
                            dialogTitle: "Choose where to install ComfyUI"
                            onPicked: (path) => {
                                return view.ctl.chooseBase(path);
                            }
                        }

                        Label {
                            Layout.topMargin: 6
                            text: "Shared models folder (optional)"
                            color: Theme.text
                            font.pixelSize: Theme.px(14)
                        }

                        FolderField {
                            Layout.fillWidth: true
                            path: view.ctl.sharedModelsPath
                            placeholder: "None: models go into the new ComfyUI"
                            dialogTitle: "Choose a shared models folder"
                            onPicked: (path) => {
                                return view.ctl.saveSharedModels(path, true);
                            }
                        }

                        RowLayout {
                            visible: view.ctl.sharedModelsPath !== ""

                            AppSwitch {
                                checked: view.ctl.sharedModelsDefault
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
                                onClicked: view.ctl.saveSharedModels("", false)
                            }

                        }

                    }

                }

                SectionCard {
                    title: "GPU and PyTorch"
                    description: view.ctl.platformName + ": the torch stack is chosen for your GPU and driver."

                    GridLayout {
                        Layout.fillWidth: true
                        Layout.margins: 16
                        columns: 2
                        rowSpacing: 10
                        columnSpacing: 14

                        Label {
                            text: "GPU"
                            color: Theme.text
                            font.pixelSize: Theme.px(14)
                        }

                        ComboField {
                            id: gpuBox

                            Layout.fillWidth: true
                            model: view.ctl.gpuOptions
                            textRole: "label"
                            valueRole: "value"
                            onActivated: (index) => {
                                return view.ctl.setGpuSelection(view.ctl.gpuOptions[index].value);
                            }

                            Binding on currentIndex {
                                value: Math.max(0, view.ctl.gpuOptions.findIndex((o) => {
                                    return o.value === view.ctl.gpuSelection;
                                }))
                            }

                        }

                        Label {
                            text: "PyTorch"
                            color: Theme.text
                            font.pixelSize: Theme.px(14)
                        }

                        ComboField {
                            Layout.fillWidth: true
                            model: view.ctl.profileOptions
                            textRole: "label"
                            valueRole: "value"
                            onActivated: (index) => {
                                return view.ctl.setTorchProfile(view.ctl.profileOptions[index].value);
                            }

                            Binding on currentIndex {
                                value: view.ctl.profileOptions.findIndex((o) => {
                                    return o.value === view.ctl.torchProfile;
                                })
                            }

                        }

                        Item {
                            implicitWidth: 1
                        }

                        Label {
                            Layout.fillWidth: true
                            text: {
                                const reco = view.ctl.recommendation;
                                if (!reco)
                                    return "Detecting your GPU…";

                                return "Recommended: " + reco.torch_label + (reco.gpu_name ? " for " + reco.gpu_name : "") + (view.ctl.torchLocked && reco.torch_profile !== view.ctl.torchProfile ? " (you chose another)" : "");
                            }
                            color: Theme.textMuted
                            font.pixelSize: Theme.px(12)
                            wrapMode: Text.Wrap
                        }

                    }

                    // ROCm / Intel XPU drivers (Linux): check and guided setup.
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.bottomMargin: 16
                        visible: view.ctl.guidedTarget !== null && !(view.ctl.guidedStatus && view.ctl.guidedStatus.ready)
                        implicitHeight: guidedRow.implicitHeight + 20
                        radius: 10
                        color: Theme.subtle
                        border.color: Theme.hairline

                        RowLayout {
                            id: guidedRow

                            x: 12
                            y: 10
                            width: parent.width - 24
                            spacing: 10

                            Label {
                                Layout.fillWidth: true
                                text: view.ctl.guidedTarget ? view.ctl.guidedTarget.name + ": " + (view.ctl.guidedStatus && view.ctl.guidedStatus.detail ? view.ctl.guidedStatus.detail : "not checked yet.") : ""
                                color: Theme.text
                                font.pixelSize: Theme.px(13)
                                wrapMode: Text.Wrap
                            }

                            FlatButton {
                                text: view.ctl.guidedBusy ? "Working…" : "Check"
                                enabled: !view.ctl.guidedBusy
                                onClicked: view.ctl.refreshGuided(true)
                            }

                            FlatButton {
                                text: "Guided setup"
                                variant: "primary"
                                enabled: !view.ctl.guidedBusy && !(view.ctl.guidedStatus && view.ctl.guidedStatus.supported === false)
                                onClicked: view.ctl.runGuidedSetup()
                            }

                        }

                    }

                }

                SectionCard {
                    title: "Add-ons"
                    description: "Only one attention backend at a time. You can change these later."

                    OptionGrid {
                        ctl: view.ctl
                        defs: view.ctl.addonDefs
                        available: !view.ctl.installBusy
                    }

                }

                SectionCard {
                    title: "Custom nodes"

                    OptionGrid {
                        ctl: view.ctl
                        defs: view.ctl.nodeDefs
                        available: !view.ctl.installBusy
                    }

                }

                LaunchOptions {
                    ctl: view.ctl
                }

            }

            ScrollBar.vertical: ScrollBar {
            }

        }

        // The install button, always in reach, with what the system check found.
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 64
            color: Theme.canvas

            Rectangle {
                width: parent.width
                height: 1
                color: Theme.hairline
            }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 28
                anchors.rightMargin: 24
                spacing: 10

                BusyIndicator {
                    visible: view.ctl.installBusy || view.ctl.preflightRunning
                    running: visible
                    implicitWidth: 20
                    implicitHeight: 20
                }

                Icon {
                    visible: !view.ctl.installBusy && !view.ctl.preflightRunning && view.ctl.preflight !== null
                    name: view.ctl.preflightBlocks ? "close-circle" : view.ctl.preflightIssues.length ? "alert" : "checkbox-circle"
                    color: view.ctl.preflightBlocks ? Theme.error : view.ctl.preflightIssues.length ? Theme.warning : Theme.success
                    implicitWidth: 17
                    implicitHeight: 17
                }

                Label {
                    Layout.fillWidth: true
                    text: {
                        const c = view.ctl;
                        if (c.installBusy)
                            return "Installing… progress is in the Activity panel.";

                        if (c.preflightRunning)
                            return "Checking your system…";

                        if (!c.installBase)
                            return "Choose where to install ComfyUI.";

                        if (!c.preflight)
                            return "Installs with " + c.profileLabel(c.torchProfile);

                        if (c.preflightBlocks) {
                            const first = c.preflightFailures[0];
                            return first ? first.title + ": " + first.detail : c.preflight.summary;
                        }
                        const notes = c.preflightIssues.length;
                        return "Ready to install with " + c.profileLabel(c.torchProfile) + (notes ? " · " + notes + (notes === 1 ? " note" : " notes") : "");
                    }
                    color: view.ctl.preflightBlocks && !view.ctl.installBusy ? Theme.error : Theme.textMuted
                    font.pixelSize: Theme.px(13)
                    elide: Text.ElideRight
                }

                FlatButton {
                    visible: view.ctl.preflight !== null && !view.ctl.installBusy
                    text: "Details"
                    outlined: false
                    accent: Theme.textMuted
                    onClicked: checksPopup.open()
                }

                FlatButton {
                    text: view.ctl.installBusy ? "Cancel install" : "Install ComfyUI"
                    variant: view.ctl.installBusy ? "stop" : "primary"
                    font.pixelSize: Theme.px(14)
                    padding: 8
                    enabled: view.ctl.installBusy || (view.ctl.installBase !== "" && !view.ctl.preflightBlocks && !view.ctl.preflightRunning)
                    onClicked: view.ctl.startInstall(false)
                    ToolTip.visible: hovered && view.ctl.preflightBlocks
                    ToolTip.text: "Fix the problems the system check found first."
                }

            }

            // Every check, above the bar.
            Popup {
                id: checksPopup

                x: parent.width - width - 24
                y: -height - 8
                width: Math.min(560, parent.width - 48)
                padding: 0

                background: PopupSurface {
                    radius: 12
                }

                contentItem: ColumnLayout {
                    spacing: 0

                    RowLayout {
                        Layout.fillWidth: true
                        Layout.margins: 14
                        Layout.bottomMargin: 8

                        Label {
                            Layout.fillWidth: true
                            text: "System check"
                            color: Theme.text
                            font.pixelSize: Theme.px(15)
                            font.weight: Font.DemiBold
                        }

                        FlatButton {
                            text: view.ctl.preflightRunning ? "Checking…" : "Check again"
                            enabled: !view.ctl.preflightRunning
                            onClicked: view.ctl.runPreflight()
                        }

                    }

                    Repeater {
                        model: view.ctl.preflight ? view.ctl.preflight.items : []

                        delegate: RowLayout {
                            id: check

                            required property var modelData
                            readonly property string status: String(modelData.status || "warn").toLowerCase()

                            Layout.fillWidth: true
                            Layout.leftMargin: 14
                            Layout.rightMargin: 14
                            Layout.bottomMargin: 8
                            spacing: 10

                            Icon {
                                Layout.alignment: Qt.AlignTop
                                Layout.topMargin: 1
                                name: check.status === "pass" || check.status === "ok" ? "checkbox-circle" : check.status === "fail" ? "close-circle" : "alert"
                                color: check.status === "pass" || check.status === "ok" ? Theme.success : check.status === "fail" ? Theme.error : Theme.warning
                                implicitWidth: 15
                                implicitHeight: 15
                            }

                            Label {
                                Layout.fillWidth: true
                                text: "<b>" + check.modelData.title + "</b>  " + check.modelData.detail
                                textFormat: Text.StyledText
                                color: Theme.text
                                font.pixelSize: Theme.px(12.5)
                                wrapMode: Text.Wrap
                            }

                        }

                    }

                    Item {
                        implicitHeight: 6
                    }

                }

            }

        }

    }

}
