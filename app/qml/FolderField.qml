import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

// A folder path with a "Choose…" button (the desktop's folder dialog).
// `picked(path)` fires with a local path when one is chosen.
RowLayout {
    id: field

    property string path: ""
    property string placeholder: ""
    property string dialogTitle: "Choose a folder"
    property string chooseText: "Choose…"

    signal picked(string path)

    /// A local path from a dialog's file:// URL.
    function localPath(url) {
        let s = decodeURIComponent(String(url).replace(/^file:\/\//, ""));
        if (/^\/[A-Za-z]:/.test(s))
            s = s.slice(1);

        return s;
    }

    spacing: 8

    AppTextField {
        Layout.fillWidth: true
        text: field.path
        readOnly: true
        placeholderText: field.placeholder
        font.family: Theme.mono
        font.pixelSize: Theme.px(12)
    }

    FlatButton {
        text: field.chooseText
        onClicked: dialog.open()
    }

    FolderDialog {
        id: dialog

        title: field.dialogTitle
        onAccepted: field.picked(field.localPath(selectedFolder))
    }

}
