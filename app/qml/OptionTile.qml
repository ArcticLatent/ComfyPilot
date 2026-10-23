import ArcticHelper
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// One switchable option, compact: a switch and its name; what it is (and
// why it's unavailable) on hover. A blocked option is dimmed with a small
// warning mark. Clicking anywhere on it flips it.
Item {
    id: item

    property string title
    property string description: ""
    /// Why it can't be changed right now ("" when it can).
    property string blockedReason: ""
    property bool checked: false
    property bool busy: false
    property bool mono: false
    readonly property bool usable: blockedReason === "" && enabled && !busy

    signal toggled(bool checked)

    implicitHeight: 36
    ToolTip.visible: hover.hovered && (description !== "" || blockedReason !== "")
    ToolTip.text: blockedReason !== "" ? (description !== "" ? description + "\n\n" : "") + blockedReason : description
    ToolTip.delay: 400

    Rectangle {
        anchors.fill: parent
        radius: 8
        color: hover.hovered && item.usable ? Theme.rowHover : "transparent"

        Behavior on color {
            ColorAnimation {
                duration: Theme.hoverMotion
            }

        }

    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        spacing: 10

        Item {
            implicitWidth: 34
            implicitHeight: 20

            AppSwitch {
                anchors.fill: parent
                visible: !item.busy
                checked: item.checked
                enabled: item.usable
                // Shows the real state: asks for the change and snaps back
                // until the state itself changes.
                onToggled: {
                    const wanted = checked;
                    checked = Qt.binding(() => {
                        return item.checked;
                    });
                    item.toggled(wanted);
                }
            }

            BusyIndicator {
                anchors.centerIn: parent
                visible: item.busy
                running: item.busy
                width: 20
                height: 20
            }

        }

        Label {
            Layout.fillWidth: true
            text: item.title
            color: item.blockedReason !== "" ? Theme.textFaint : item.checked ? Theme.text : Theme.textMuted
            font.family: item.mono ? Theme.mono : Theme.uiFont
            font.pixelSize: Theme.px(item.mono ? 12.5 : 13.5)
            elide: Text.ElideRight
        }

        Icon {
            visible: item.blockedReason !== ""
            name: "alert"
            color: Theme.warning
            implicitWidth: 14
            implicitHeight: 14
        }

    }

    HoverHandler {
        id: hover

        cursorShape: item.usable ? Qt.PointingHandCursor : Qt.ArrowCursor
    }

    TapHandler {
        enabled: item.usable
        onTapped: item.toggled(!item.checked)
    }

}
