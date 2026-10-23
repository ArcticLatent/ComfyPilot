import ArcticHelper
import QtQuick
import QtQuick.Controls

// The app's button: an outline (ControlSurface), or with
// `outlined: false` a plain label that shows a hover. `variant` picks the
// surface: outline, primary, selected, danger or stop.
Button {
    id: fb

    property color accent: Theme.foreground
    property bool outlined: true
    property string variant: "outline"
    property real radius: 8

    flat: true
    font.pixelSize: Theme.px(13)
    padding: 5
    // A little wider than high; a compact `padding` keeps it compact.
    horizontalPadding: padding + 4

    contentItem: Label {
        text: fb.text
        color: !fb.enabled ? Theme.muted : fb.outlined && fb.variant === "primary" ? Theme.primaryText : fb.outlined && fb.variant === "stop" ? "#ffffff" : fb.accent
        font: fb.font
        // Only when a layout squeezes the button.
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    background: Item {
        ControlSurface {
            anchors.fill: parent
            visible: fb.outlined
            control: fb
            variant: fb.variant
            radius: fb.radius
        }

        Rectangle {
            anchors.fill: parent
            visible: !fb.outlined
            radius: fb.radius
            color: fb.down ? Theme.pressed : fb.hovered ? Theme.hover : "transparent"

            Behavior on color {
                ColorAnimation {
                    duration: Theme.hoverMotion
                }

            }

        }

    }

}
