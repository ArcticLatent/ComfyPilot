import ArcticHelper
import QtQuick
import QtQuick.Controls

// Dialog footer button. Accept/Yes/Apply roles are filled with the accent
// colour, Destructive is red; `accent` forces the filled look.
Button {
    id: b

    readonly property int role: DialogButtonBox.buttonRole
    property bool accent: role === DialogButtonBox.AcceptRole || role === DialogButtonBox.YesRole || role === DialogButtonBox.ApplyRole
    property bool destructive: role === DialogButtonBox.DestructiveRole

    flat: true
    implicitHeight: 32
    leftPadding: 14
    rightPadding: 14
    font.pixelSize: Theme.px(13)

    contentItem: Label {
        text: b.text
        font: b.font
        color: b.accent ? Theme.primaryText : b.destructive ? Theme.error : Theme.foreground
        opacity: b.enabled ? 1 : 0.45
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    background: ControlSurface {
        control: b
        variant: b.accent ? "primary" : b.destructive ? "danger" : "outline"
    }

}
