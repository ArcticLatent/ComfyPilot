import ArcticHelper
import QtQuick
import QtQuick.Controls

// ComboBox in the app's colours (the Basic style draws it light). From
// Harlo's ComboField. Works with a plain list of strings or with
// `textRole`/`valueRole` over a list of objects.
ComboBox {
    id: box

    implicitHeight: 32
    font.pixelSize: Theme.px(13)
    rightPadding: 26

    contentItem: Label {
        leftPadding: 10
        rightPadding: 4
        text: box.displayText
        color: Theme.foreground
        font.pixelSize: box.font.pixelSize
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    indicator: Icon {
        x: box.width - width - 9
        anchors.verticalCenter: parent.verticalCenter
        name: "arrow-down-s"
        color: Theme.muted
        implicitWidth: 14
        implicitHeight: 14
    }

    // The app's button surface; the accent border while it has the focus.
    background: ControlSurface {
        control: box
        radius: 7

        Rectangle {
            anchors.fill: parent
            radius: 7
            color: "transparent"
            border.color: Theme.primary
            visible: box.visualFocus
        }

    }

    delegate: ItemDelegate {
        id: row

        required property int index
        required property var modelData

        width: ListView.view.width
        highlighted: box.highlightedIndex === index

        contentItem: Label {
            // An option object (with textRole) or a plain string. (A JS array
            // model reaches QML as a list, so test the option, not the model.)
            text: box.textRole && row.modelData !== null && typeof row.modelData === "object" ? String(row.modelData[box.textRole]) : String(row.modelData)
            color: row.index === box.currentIndex ? Theme.primary : Theme.foreground
            font.pixelSize: box.font.pixelSize
            elide: Text.ElideRight
        }

        background: Rectangle {
            radius: 5
            color: row.highlighted ? Theme.hover : "transparent"
        }

    }

    popup: Popup {
        y: box.height + 3
        width: box.width
        implicitHeight: Math.min(contentItem.implicitHeight + 8, 340)
        padding: 4

        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: box.popup.visible ? box.delegateModel : null
            currentIndex: box.highlightedIndex

            ScrollBar.vertical: ScrollBar {
            }

        }

        background: PopupSurface {
            radius: 9
        }

        enter: Transition {
            NumberAnimation {
                property: "opacity"
                from: 0
                to: 1
                duration: Math.round(Theme.motion * 0.6)
            }

        }

    }

}
