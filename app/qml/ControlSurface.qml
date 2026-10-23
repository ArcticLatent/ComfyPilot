// A button's background: a soft fill a step off the page with a faint
// border, flat (no bevel or shadow). Colours come from the theme.

import ArcticHelper
import QtQuick

//   outline   the usual button
//   primary   filled with the accent colour (main actions)
//   selected  a chosen pill: tinted with the accent colour
//   danger    an outline that turns red when hovered
//   stop      filled red (Stop ComfyUI, Cancel download/install)
Item {
    id: surface

    /// The Button this is the background of (for hovered / down / enabled).
    property Item control: parent
    property string variant: "outline"
    property real radius: 8
    readonly property bool hovered: control && control.hovered === true
    readonly property bool down: control && (control.down === true || control.pressed === true)
    readonly property bool usable: !control || control.enabled
    readonly property bool primary: variant === "primary"
    readonly property bool selected: variant === "selected"
    readonly property bool danger: variant === "danger"
    readonly property bool stop: variant === "stop"
    // A red deep enough for white text.
    readonly property color stopRed: Qt.darker(Theme.error, 1.3)
    readonly property color fill: stop ? (down ? Qt.darker(stopRed, 1.12) : hovered ? Qt.lighter(stopRed, 1.12) : stopRed) : primary ? (down ? Qt.darker(Theme.primary, 1.12) : hovered ? Theme.primaryHover : Theme.primary) : selected ? mix(Theme.controlFill, Theme.primary, hovered ? 0.2 : 0.14) : danger && hovered ? mix(Theme.controlFill, Theme.error, 0.1) : down ? Theme.controlFillDown : hovered ? Theme.controlFillHover : Theme.controlFill
    readonly property color edge: stop ? stopRed : primary ? Theme.primary : selected ? Theme.primary : danger && hovered ? mix(Theme.controlBorder, Theme.error, 0.6) : hovered ? Theme.controlBorderHover : Theme.controlBorder

    function mix(a, b, t) {
        return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1);
    }

    opacity: usable ? 1 : 0.55

    Rectangle {
        anchors.fill: parent
        radius: surface.radius
        color: surface.fill
        border.color: surface.edge

        Behavior on color {
            ColorAnimation {
                duration: Theme.hoverMotion
            }

        }

        Behavior on border.color {
            ColorAnimation {
                duration: Theme.hoverMotion
            }

        }

    }

}
