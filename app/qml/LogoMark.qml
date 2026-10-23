import ArcticHelper
import QtQuick

// The ComfyPilot logo (mark and wordmark, assets/logo.svg), as the sidebar's
// head and on the startup screen. `logoHeight` sets its size; the width
// follows the logo's proportions. (Sizes come from `logoHeight` only: in a
// layout, a source size taken from the image's own width feeds back into it.)
Item {
    property real logoHeight: 26

    // The logo is 1126 × 240.
    implicitWidth: Math.round(logoHeight * 1126 / 240)
    implicitHeight: logoHeight

    Image {
        anchors.fill: parent
        // The wordmark in the theme's text colour (off-white or near-black).
        source: Theme.isLight ? "qrc:/qt/qml/ArcticHelper/qml/assets/logo-light.svg" : "qrc:/qt/qml/ArcticHelper/qml/assets/logo.svg"
        sourceSize: Qt.size(parent.implicitWidth * 2, parent.implicitHeight * 2)
        fillMode: Image.PreserveAspectFit
        smooth: true
    }

}
