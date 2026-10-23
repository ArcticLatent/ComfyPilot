import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Asks a yes/no question:
//   confirmDialog.ask("Remove 'X'?", "Remove", true).then(ok => ...)
AppDialog {
    id: dlg

    property string message: ""
    property string acceptText: "OK"
    property bool destructive: false
    property var _resolve: null

    function ask(message, acceptText, destructive) {
        if (_resolve)
            _resolve(false);

        dlg.message = message;
        dlg.acceptText = acceptText || "OK";
        dlg.destructive = destructive === true;
        return new Promise((resolve) => {
            dlg._resolve = resolve;
            dlg.open();
        });
    }

    function finish(ok) {
        const resolve = _resolve;
        _resolve = null;
        if (resolve)
            resolve(ok);

    }

    title: "Are you sure?"
    onAccepted: finish(true)
    onRejected: finish(false)

    Label {
        width: 400
        text: dlg.message
        color: Theme.textMuted
        font.pixelSize: Theme.px(13)
        wrapMode: Text.Wrap
        lineHeight: 1.25
    }

    footer: AppButtonBox {
        DialogButton {
            text: "Cancel"
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }

        DialogButton {
            text: dlg.acceptText
            DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            accent: !dlg.destructive
            destructive: dlg.destructive
        }

    }

}
