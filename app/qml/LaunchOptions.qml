import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// How ComfyUI starts: pinned memory, launch flags, port, extra arguments,
// and whether its output shows in the app. These are app settings, so they
// apply at once (from the next start) in both views.
SectionCard {
    id: section

    /// The ComfyPage.
    property var ctl
    /// The --use-*-attention flags (only meaningful for an installed backend).
    property bool showAttentionFlags: false
    readonly property var settings: ctl.app ? ctl.app.settings : null

    function save(command, args, done) {
        Api.call(command, args).then((settings) => {
            ctl.app.settings = settings;
            if (done)
                ctl.log(done);

        }).catch((err) => {
            return ctl.log("Saving failed: " + err, "error");
        });
    }

    title: "Launch options"
    description: "Used the next time ComfyUI starts from this app."

    OptionGrid {
        ctl: section.ctl
        defs: section.ctl.flagDefs.filter((def) => {
            return section.showAttentionFlags || def.kind !== "attentionFlag";
        })
        available: !section.ctl.anyBusy
    }

    // Port, extra arguments and ComfyUI's output, under a hairline.
    Rectangle {
        Layout.fillWidth: true
        Layout.leftMargin: 16
        Layout.rightMargin: 16
        Layout.topMargin: 4
        implicitHeight: 1
        color: Theme.hairline
    }

    GridLayout {
        Layout.fillWidth: true
        Layout.margins: 16
        columns: 3
        rowSpacing: 10
        columnSpacing: 10

        Label {
            text: "Port"
            color: Theme.text
            font.pixelSize: Theme.px(14)
        }

        AppTextField {
            id: portField

            Layout.preferredWidth: 120
            text: section.settings ? String(section.settings.comfyui_port || 8188) : "8188"
            font.family: Theme.mono
            onAccepted: savePort.clicked()

            validator: IntValidator {
                bottom: 1024
                top: 65535
            }

        }

        FlatButton {
            id: savePort

            Layout.alignment: Qt.AlignLeft
            text: "Save"
            enabled: portField.acceptableInput && section.settings && Number(portField.text) !== section.settings.comfyui_port
            onClicked: section.save("set_comfyui_port", {
                "port": Number(portField.text)
            }, "ComfyUI port set to " + portField.text + "; it applies the next time ComfyUI starts.")
        }

        Label {
            text: "Extra arguments"
            color: Theme.text
            font.pixelSize: Theme.px(14)
        }

        AppTextField {
            id: argsField

            Layout.fillWidth: true
            text: section.settings ? section.settings.comfyui_custom_launch_args || "" : ""
            placeholderText: "Other ComfyUI options, like --preview-method auto"
            font.family: Theme.mono
            onAccepted: saveArgs.clicked()
        }

        FlatButton {
            id: saveArgs

            text: "Save"
            enabled: section.settings && argsField.text.trim() !== (section.settings.comfyui_custom_launch_args || "")
            onClicked: section.save("set_comfyui_custom_launch_args", {
                "customLaunchArgs": argsField.text
            }, argsField.text.trim() ? "Extra arguments saved." : "Extra arguments cleared.")
        }

    }

    OptionRow {
        title: "Show ComfyUI's output in the app"
        description: "Its console output in the ComfyUI output tab of the bottom bar, from the next start."
        checked: section.settings ? section.settings.comfyui_show_runtime_logs !== false : true
        onToggled: (on) => {
            return section.save("set_comfyui_show_runtime_logs", {
                "enabled": on
            }, "");
        }
    }

}
