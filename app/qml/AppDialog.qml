import ArcticHelper
import QtQuick
import QtQuick.Controls

// Modal dialog in the app's style, as Harlo's AppDialog: a card with a soft
// shadow and a light edge (PopupSurface) over the dimmed, blurred window
// (PopupScrim); it fades and grows in.
// Standard buttons are drawn with DialogButton; a custom `footer` can use
// DialogButton directly.
Dialog {
    id: dlg

    modal: true
    anchors.centerIn: Overlay.overlay
    padding: 20
    topPadding: title !== "" ? 6 : 20
    bottomPadding: 12

    background: PopupSurface {
        radius: 14
        kind: "dialog"
    }

    Overlay.modal: PopupScrim {
    }

    // Long titles elide instead of widening the dialog.
    header: Item {
        visible: dlg.title !== ""
        implicitWidth: Math.min(titleMetrics.advanceWidth + 2, 520) + 40
        implicitHeight: titleLabel.implicitHeight

        TextMetrics {
            id: titleMetrics

            text: dlg.title
            font: titleLabel.font
        }

        Label {
            id: titleLabel

            anchors.fill: parent
            text: dlg.title
            leftPadding: 20
            rightPadding: 20
            topPadding: 18
            bottomPadding: 6
            color: Theme.foreground
            font.pixelSize: Theme.px(15)
            font.bold: true
            elide: Text.ElideMiddle
        }

    }

    footer: AppButtonBox {
    }

    enter: Transition {
        NumberAnimation {
            property: "opacity"
            from: 0
            to: 1
            duration: Math.round(Theme.motion * 0.7)
        }

        NumberAnimation {
            property: "scale"
            from: 0.97
            to: 1
            duration: Math.round(Theme.motion * 0.7)
            easing.type: Easing.OutCubic
        }

    }

    exit: Transition {
        NumberAnimation {
            property: "opacity"
            from: 1
            to: 0
            duration: Math.round(Theme.motion * 0.5)
        }

    }

}
