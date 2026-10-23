import ArcticHelper
import QtQuick
import QtQuick.Effects

// A popup's background, as Harlo's (GlassSurface without the glass): the
// surface colour with a faint light edge all round and a lighter line along
// the top, over a soft drop shadow (deeper for dialogs). Without graphics
// acceleration (Qt's software renderer runs no shader effects) the shadow is
// a few faint outlines.
Item {
    id: surface

    property real radius: 10
    /// "menu" (menus, pickers, popovers) or "dialog".
    property string kind: "menu"
    readonly property bool dialog: kind === "dialog"
    /// The surface colour: the window's for dialogs, a step up for menus.
    property color tint: dialog ? Theme.background : Theme.elevated
    /// Shader effects (soft shadows) can run.
    readonly property bool gpu: GraphicsInfo.api !== GraphicsInfo.Software

    // The shadow: the card's shape, blurred, a little smaller and lower.
    Rectangle {
        id: shadowShape

        anchors.fill: parent
        radius: surface.radius
        color: "#000000"
        visible: false
        layer.enabled: surface.gpu
    }

    MultiEffect {
        visible: surface.gpu
        anchors.fill: shadowShape
        source: shadowShape
        shadowEnabled: true
        shadowColor: Qt.rgba(0, 0, 0, Theme.isLight ? (surface.dialog ? 0.4 : 0.3) : (surface.dialog ? 0.85 : 0.75))
        shadowBlur: 1
        blurMax: surface.dialog ? 64 : 40
        shadowVerticalOffset: surface.dialog ? 22 : 14
        shadowScale: surface.dialog ? 0.95 : 0.96
    }

    // Without shaders: a few faint outlines below the card.
    Repeater {
        model: surface.gpu ? 0 : 3

        Rectangle {
            required property int index

            anchors.fill: parent
            anchors.margins: -(index + 1) * 2
            anchors.topMargin: -(index + 1)
            radius: surface.radius + (index + 1) * 2
            color: "transparent"
            border.width: 2
            border.color: Qt.rgba(0, 0, 0, (Theme.isLight ? 0.08 : 0.16) - index * (Theme.isLight ? 0.02 : 0.045))
        }

    }

    // The surface and its edge: a faint line all round, a lighter one on top.
    Rectangle {
        anchors.fill: parent
        radius: surface.radius
        color: surface.tint
        border.color: Theme.isLight ? Qt.rgba(0, 0, 0, 0.1) : Qt.rgba(1, 1, 1, surface.dialog ? 0.08 : 0.1)
    }

    Rectangle {
        visible: !Theme.isLight
        x: surface.radius
        y: 1
        width: surface.width - 2 * surface.radius
        height: 1
        color: Qt.rgba(1, 1, 1, surface.dialog ? 0.04 : 0.06)
    }

}
