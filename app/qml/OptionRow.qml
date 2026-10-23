import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// One switchable option on a SectionCard: its name, a line about it, and a
// switch (a spinner while it's being applied). Rows after the first get a
// hairline above them.
Item {
    id: row

    property string title
    property string description: ""
    /// Why it can't be changed right now ("" when it can).
    property string blockedReason: ""
    property bool checked: false
    property bool busy: false
    property bool mono: false
    property bool first: false
    readonly property bool usable: blockedReason === "" && enabled

    signal toggled(bool checked)

    Layout.fillWidth: true
    implicitHeight: Math.max(textCol.implicitHeight, 24) + 22

    Rectangle {
        visible: !row.first
        width: parent.width - 32
        x: 16
        height: 1
        color: Theme.hairline
    }

    ColumnLayout {
        id: textCol

        anchors.left: parent.left
        anchors.leftMargin: 16
        anchors.right: control.left
        anchors.rightMargin: 16
        anchors.verticalCenter: parent.verticalCenter
        spacing: 3

        Label {
            Layout.fillWidth: true
            text: row.title
            color: row.usable ? Theme.text : Theme.textMuted
            font.family: row.mono ? Theme.mono : Theme.uiFont
            font.pixelSize: Theme.px(row.mono ? 13 : 14)
            elide: Text.ElideRight
        }

        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: row.blockedReason !== "" ? row.blockedReason : row.description
            color: row.blockedReason !== "" ? Theme.warning : Theme.textMuted
            font.pixelSize: Theme.px(12)
            wrapMode: Text.Wrap
            lineHeight: 1.2
        }

    }

    Item {
        id: control

        anchors.right: parent.right
        anchors.rightMargin: 16
        anchors.verticalCenter: parent.verticalCenter
        width: 34
        height: 20

        AppSwitch {
            anchors.fill: parent
            visible: !row.busy
            checked: row.checked
            enabled: row.usable
            // The switch shows the real state: it asks for the change and
            // snaps back until the state itself changes (or the user cancels).
            onToggled: {
                const wanted = checked;
                checked = Qt.binding(() => {
                    return row.checked;
                });
                row.toggled(wanted);
            }
        }

        BusyIndicator {
            anchors.centerIn: parent
            visible: row.busy
            running: row.busy
            width: 22
            height: 22
        }

    }

}
