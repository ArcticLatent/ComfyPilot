import ArcticHelper
import QtQuick
import QtQuick.Layouts

// Options (ComfyPage definitions) in as many columns as fit, on a section's
// card.
GridLayout {
    id: grid

    /// The ComfyPage.
    property var ctl
    property var defs: []
    /// Whether the options can be changed at all right now.
    property bool available: true

    Layout.fillWidth: true
    Layout.margins: 8
    columns: Math.max(1, Math.floor((width + columnSpacing) / (220 + columnSpacing)))
    columnSpacing: 6
    rowSpacing: 2

    Repeater {
        model: grid.defs

        delegate: OptionTile {
            required property var modelData

            Layout.fillWidth: true
            Layout.preferredWidth: 1
            title: modelData.label
            description: modelData.description
            mono: modelData.mono === true
            checked: grid.ctl.valueOf(modelData)
            blockedReason: grid.ctl.blockedReason(modelData)
            busy: grid.ctl.busyKey === modelData.key
            enabled: grid.available
            onToggled: (on) => {
                return grid.ctl.toggle(modelData, on);
            }
        }

    }

}
