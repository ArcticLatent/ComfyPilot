import ArcticHelper
import QtQuick
import QtQuick.Controls

// Dialog footer: right-aligned DialogButtons on the dialog's background.
DialogButtonBox {
    alignment: Qt.AlignRight
    spacing: 8
    padding: 20
    topPadding: 8
    bottomPadding: 18

    background: Item {
    }

    delegate: DialogButton {
    }

}
