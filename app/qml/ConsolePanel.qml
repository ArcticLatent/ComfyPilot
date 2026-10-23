import ArcticHelper
import QtQuick
import QtQuick.Controls

// ComfyUI's output as a console: compact monospace lines on a dark ground,
// terminal colours kept (ConsoleFormat.js), warnings and errors marked with
// a bar on the left and tinted, the time on the right only when it changes.
// Every line has a bar in its colour: its level's (warning, error, success),
// else its own (terminal colour or "[tag]"), else a dim neutral one.
// It follows new output unless scrolled up.
//
// A bar along the top filters by level (All, Errors, Warnings, Info, with
// counts) and by text. Filtering shows a copy of the matching lines (`shown`),
// kept up to date as output arrives; `lines` itself is never changed.
//
// `lines` is a ListModel of { time, showTime, html, plain, level, accent }.
Rectangle {
    id: panel

    property ListModel lines
    property string emptyText: "Nothing yet."

    /// "all", "error", "warn" or "info" (everything else).
    property string filter: "all"
    property string search: ""
    readonly property bool filtering: filter !== "all" || search !== ""
    property int errorCount: 0
    property int warnCount: 0
    readonly property int totalCount: lines ? lines.count : 0

    function levelColor(level) {
        return level === "error" ? Theme.error : level === "warn" ? Theme.warning : level === "ok" ? Theme.success : level === "debug" ? Theme.textFaint : Theme.text;
    }

    function matches(level, plain) {
        if (filter === "error" && level !== "error")
            return false;
        if (filter === "warn" && level !== "warn")
            return false;
        if (filter === "info" && (level === "error" || level === "warn"))
            return false;
        return search === "" || plain.toLowerCase().includes(search.toLowerCase());
    }

    /// Adds line `i` of `lines` to `shown` if it matches (the time shows when it
    /// differs from the line shown before it).
    function addShown(i) {
        const line = lines.get(i);
        if (!matches(line.level, line.plain))
            return ;

        const last = shown.count ? shown.get(shown.count - 1) : null;
        shown.append({
            "time": line.time,
            "showTime": !last || last.time !== line.time,
            "html": line.html,
            "plain": line.plain,
            "level": line.level,
            "accent": line.accent
        });
    }

    function rebuild() {
        shown.clear();
        // Not `filtering`: in onFilterChanged its binding isn't updated yet.
        if (filter !== "all" || search !== "")
            for (let i = 0; i < lines.count; i++) addShown(i)
        view.follow = true;
        Qt.callLater(view.showEnd);
    }

    function recount() {
        let errors = 0, warnings = 0;
        for (let i = 0; i < lines.count; i++) {
            const level = lines.get(i).level;
            if (level === "error")
                errors++;
            else if (level === "warn")
                warnings++;
        }
        errorCount = errors;
        warnCount = warnings;
    }

    color: Theme.chrome
    onFilterChanged: rebuild()
    onSearchChanged: rebuild()

    ListModel {
        id: shown
    }

    Connections {
        function onRowsInserted(parent, first, last) {
            for (let i = first; i <= last; i++) {
                const level = panel.lines.get(i).level;
                if (level === "error")
                    panel.errorCount++;
                else if (level === "warn")
                    panel.warnCount++;
                if (panel.filtering)
                    panel.addShown(i);

            }
        }

        function onRowsRemoved() {
            panel.recount();
            if (panel.filtering)
                panel.rebuild();

        }

        function onModelReset() {
            panel.recount();
            panel.rebuild();
        }

        target: panel.lines
    }

    // A level filter: its name, a count, and a dot in its colour.
    component FilterChip: Rectangle {
        id: chip

        property string key
        property string label
        property int count: 0
        property color dot: "transparent"
        readonly property bool active: panel.filter === key

        implicitWidth: chipRow.implicitWidth + 18
        implicitHeight: 26
        radius: 6
        color: active ? Theme.rowSelected : chipHover.hovered ? Theme.hover : "transparent"

        Row {
            id: chipRow

            anchors.centerIn: parent
            spacing: 6

            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                visible: chip.dot.a > 0
                width: 6
                height: 6
                radius: 3
                color: chip.dot
            }

            Label {
                anchors.verticalCenter: parent.verticalCenter
                text: chip.label
                color: chip.active ? Theme.text : Theme.textMuted
                font.pixelSize: Theme.px(12)
                font.weight: chip.active ? Font.Medium : Font.Normal
            }

            Label {
                anchors.verticalCenter: parent.verticalCenter
                text: chip.count
                color: chip.count > 0 && chip.dot.a > 0 ? chip.dot : Theme.textFaint
                font.family: Theme.mono
                font.pixelSize: Theme.px(11)
            }

        }

        HoverHandler {
            id: chipHover

            cursorShape: Qt.PointingHandCursor
        }

        TapHandler {
            onTapped: panel.filter = chip.key
        }

    }

    // The filter bar.
    Rectangle {
        id: filterBar

        width: parent.width
        height: 40
        color: Theme.chrome

        Row {
            anchors.left: parent.left
            anchors.leftMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2

            FilterChip {
                key: "all"
                label: "All"
                count: panel.totalCount
            }

            FilterChip {
                key: "error"
                label: "Errors"
                count: panel.errorCount
                dot: Theme.error
            }

            FilterChip {
                key: "warn"
                label: "Warnings"
                count: panel.warnCount
                dot: Theme.warning
            }

            FilterChip {
                key: "info"
                label: "Info"
                count: panel.totalCount - panel.errorCount - panel.warnCount
            }

        }

        AppTextField {
            id: searchField

            anchors.right: parent.right
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(260, parent.width * 0.35)
            implicitHeight: 28
            leftPadding: 30
            font.pixelSize: Theme.px(12)
            placeholderText: "Filter output"
            onTextChanged: searchDelay.restart()

            Icon {
                x: 10
                anchors.verticalCenter: parent.verticalCenter
                name: "search"
                color: Theme.textFaint
                implicitWidth: 13
                implicitHeight: 13
            }

            Timer {
                id: searchDelay

                interval: 200
                onTriggered: panel.search = searchField.text.trim()
            }

        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.hairline
        }

    }

    ListView {
        id: view

        // Scrolled to the end: new lines keep it there.
        property bool follow: true

        /// The newest line in view (also after switching between all and
        /// filtered lines, whose old scroll position may be past the end).
        function showEnd() {
            forceLayout();
            positionViewAtEnd();
        }

        anchors.top: filterBar.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        clip: true
        model: panel.filtering ? shown : panel.lines
        topMargin: 6
        bottomMargin: 8
        boundsBehavior: Flickable.StopAtBounds
        onCountChanged: if (follow)
            Qt.callLater(positionViewAtEnd)
        onModelChanged: {
            follow = true;
            Qt.callLater(showEnd);
        }
        // Opening or resizing the dock keeps the newest output in view.
        onHeightChanged: if (follow)
            Qt.callLater(positionViewAtEnd)
        onMovementEnded: follow = atYEnd

        ScrollBar.vertical: ScrollBar {
            onPressedChanged: if (!pressed)
                view.follow = view.atYEnd
        }

        delegate: Item {
            id: row

            required property string time
            required property bool showTime
            required property string html
            required property string level
            required property string accent

            width: ListView.view.width
            height: lineText.implicitHeight + 3

            Rectangle {
                x: 8
                width: 2
                height: parent.height
                color: row.level !== "" && row.level !== "debug" ? panel.levelColor(row.level) : row.accent !== "" ? row.accent : Theme.textFaint
            }

            Text {
                id: lineText

                x: 18
                y: 1
                // Room for the time on the right only when this line shows it.
                width: parent.width - x - 16 - (row.showTime ? timeLabel.implicitWidth + 16 : 0)
                text: row.html
                textFormat: Text.StyledText
                wrapMode: Text.WrapAtWordBoundaryOrAnywhere
                color: panel.levelColor(row.level)
                opacity: row.level === "" ? 0.92 : 1
                font.family: Theme.mono
                font.pixelSize: Theme.px(12)
                lineHeight: 1.12
            }

            Label {
                id: timeLabel

                anchors.right: parent.right
                anchors.rightMargin: 16
                y: 2
                visible: row.showTime
                text: row.time
                color: Theme.textFaint
                font.family: Theme.mono
                font.pixelSize: Theme.px(10.5)
            }

        }

    }

    // Over the list (not in it, where it would scroll away).
    Label {
        anchors.centerIn: view
        visible: view.count === 0
        text: panel.filtering && panel.totalCount > 0 ? "No lines match this filter." : panel.emptyText
        color: Theme.textFaint
        font.pixelSize: Theme.px(13)
    }

}
