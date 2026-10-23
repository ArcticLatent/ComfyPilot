import QtQuick
pragma Singleton

// The app's colours, sizes and fonts: Harlo's theme roles with its
// "harlo-dark" and "harlo-light" palettes, the accent in the ComfyPilot
// logo's purple (deeper in the light theme, for contrast on white). Screens
// use the roles (canvas, chrome, raised, hairline, text levels, accent…),
// not the raw palette.
QtObject {
    id: theme

    /// "dark" or "light": the switcher in the left rail (Main keeps it).
    property string mode: "dark"
    readonly property bool isLight: mode === "light"

    // ---- palette (harlo-dark / harlo-light) ------------------------------------
    readonly property color background: isLight ? "#ffffff" : "#212227"
    readonly property color sidebar: isLight ? "#f4f5f7" : "#141519"
    readonly property color foreground: isLight ? "#1d1e22" : "#ecedef"
    readonly property color muted: isLight ? "#6b6e76" : "#8e919a"
    readonly property color elevated: isLight ? "#f7f8fa" : "#26272d"
    readonly property color subtle: isLight ? "#eef0f3" : "#1e1f24"
    readonly property color divider: isLight ? "#eceef1" : "#2b2c32"
    readonly property color hover: isLight ? "#0a000000" : "#0dffffff"
    readonly property color pressed: isLight ? "#14000000" : "#17ffffff"
    readonly property color selection: isLight ? "#337c4fd6" : "#40bd93f9"
    // The accent: the ComfyPilot logo's purple (Dracula); deeper on white.
    readonly property color primary: isLight ? "#7c4fd6" : "#bd93f9"
    readonly property color primaryHover: isLight ? "#6c40c4" : "#cba8fb"
    readonly property color error: isLight ? "#d64545" : "#f26d6d"
    readonly property color warning: isLight ? "#c7761c" : "#f2a65a"
    readonly property color success: isLight ? "#1f9d61" : "#3dd68c"
    readonly property color info: primary
    readonly property color border: blend(background, divider, 1)
    // Buttons (ControlSurface): a fill a step off the page and a border
    // that stands out from it.
    readonly property color controlFill: blend(background, foreground, isLight ? 0.045 : 0.07)
    readonly property color controlFillHover: blend(background, foreground, isLight ? 0.08 : 0.11)
    readonly property color controlFillDown: blend(background, foreground, isLight ? 0.11 : 0.05)
    readonly property color controlBorder: blend(background, foreground, isLight ? 0.1 : 0.09)
    readonly property color controlBorderHover: blend(background, foreground, isLight ? 0.18 : 0.16)
    // ---- roles: what screens are built from --------------------------------
    /// The main area (pages, panels).
    readonly property color canvas: background
    /// The sidebar, rails and the window around the panels.
    readonly property color chrome: sidebar
    /// Cards on the canvas.
    readonly property color raised: blend(background, foreground, 0.035)
    /// Fields on the chrome (search).
    readonly property color well: blend(sidebar, foreground, isLight ? 0.05 : 0.045)
    /// Lines between sections: barely there.
    readonly property color hairline: blend(background, foreground, isLight ? 0.08 : 0.075)
    /// Text: primary, secondary (names, metadata), faint (times, hints).
    readonly property color text: foreground
    readonly property color textMuted: muted
    readonly property color textFaint: blend(muted, background, 0.32)
    /// Section labels: small, uppercase, spaced.
    readonly property color label: blend(muted, background, 0.12)
    /// Sidebar rows: hovered, and the selected one.
    readonly property color rowHover: blend(sidebar, foreground, 0.04)
    readonly property color rowSelected: blend(sidebar, foreground, isLight ? 0.075 : 0.085)
    readonly property color accent: primary
    /// The accent as a quiet fill (badges, soft highlights).
    readonly property color accentSoft: alpha(primary, isLight ? 0.12 : 0.17)
    /// Text on a primary-coloured fill (buttons, check marks). (Not `onPrimary`:
    /// QML takes a name like that for a signal handler and never binds it.)
    readonly property color primaryText: isLight ? "#ffffff" : luminance(primary) > 0.55 ? "#1a1a1a" : "#1a1310"
    // ---- motion --------------------------------------------------------------
    readonly property int motion: 160
    /// The "No preview" picture for LoRAs and workflows without one.
    readonly property string noPreview: isLight ? "qrc:/qt/qml/ArcticHelper/qml/assets/no-preview-light.jpg" : "qrc:/qt/qml/ArcticHelper/qml/assets/no-preview.jpg"
    /// The window's content, which dialog scrims blur (set by Main).
    property Item backdrop: null
    readonly property int hoverMotion: Math.round(motion * 0.7)
    readonly property int paneMotion: Math.round(motion * 1.4)
    // ---- type ----------------------------------------------------------------
    /// Geist for the interface, JetBrains Mono for logs and paths (both bundled).
    readonly property string uiFont: "Geist"
    readonly property string mono: "JetBrains Mono"

    function blend(a, b, t) {
        return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1);
    }

    function luminance(col) {
        return 0.2126 * col.r + 0.7152 * col.g + 0.0722 * col.b;
    }

    function alpha(col, a) {
        return Qt.rgba(col.r, col.g, col.b, a);
    }

    function px(n) {
        return Math.round(n);
    }

    function statusColor(status) {
        switch (status) {
        case "busy":
        case "starting":
            return primary;
        case "warn":
        case "input":
            return warning;
        case "info":
        case "unread":
            return info;
        case "live":
        case "running":
        case "ok":
            return success;
        case "error":
        case "exited":
            return error;
        default:
            return muted;
        }
    }

}
