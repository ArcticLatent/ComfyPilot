import ArcticHelper
import QtQuick
import QtQuick.Effects

// What a dialog lays over the window (its Overlay.modal), as Harlo's
// GlassScrim: the window dimmed toward the background colour and, with
// graphics acceleration, slightly blurred (Theme.backdrop is the window's
// content, set by Main).
Rectangle {
    id: scrim

    readonly property bool gpu: GraphicsInfo.api !== GraphicsInfo.Software
    readonly property bool blurred: gpu && visible && Theme.backdrop !== null

    color: gpu ? Qt.rgba(Theme.background.r, Theme.background.g, Theme.background.b, Theme.isLight ? 0.6 : 0.64) : Theme.isLight ? "#66000000" : "#99000000"

    // Under the dimming: the window, blurred (about 4 px).
    ShaderEffectSource {
        id: behind

        anchors.fill: parent
        visible: false
        sourceItem: scrim.blurred ? Theme.backdrop : null
        live: true
    }

    MultiEffect {
        z: -1
        anchors.fill: parent
        visible: scrim.blurred
        source: behind
        autoPaddingEnabled: false
        blurEnabled: true
        blurMax: 16
        blur: 0.25
    }

}
