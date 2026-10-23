.pragma library

// Turns a line of ComfyUI's output into something readable: terminal colour
// codes (ANSI SGR) become real colours, other escape codes go, a progress bar
// redrawn with "\r" keeps only its last state, and the line gets a level from
// what it says (not from the stream: Python logs everything to stderr).
//
// format(text, palette) → { plain, html, level, accent }: `accent` is the
// line's own colour (its first terminal colour, or its "[tag]" colour; ""
// for a plain line), for the bar beside it. `palette` maps the
// terminal's colour names (black, red, green, yellow, blue, magenta, cyan,
// white, faint, tag) to colour strings; `html` is for Text.StyledText.

const SGR_COLORS = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/// HTML-escaped, with spaces kept (rich text would collapse indentation).
function escapeHtml(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/\t/g, "    ").replace(/ {2,}|^ /g, (m) => {
        return "&nbsp;".repeat(m.length);
    });
}

/// The text as a terminal would show it at the end of the line: after the
/// last "\r" (progress bars redraw themselves that way).
function lastRedraw(text) {
    const parts = String(text).replace(/\r\n/g, "\n").split("\r");
    for (let i = parts.length - 1; i >= 0; i--) {
        if (parts[i].trim() !== "")
            return parts[i];
    }
    return parts[parts.length - 1];
}

/// "error", "warn", "ok" (a finished prompt, the server's address), "debug" or "".
function levelOf(plain) {
    const t = plain.trim();
    if (/\b(ERROR|CRITICAL|FATAL)\b/.test(t) || /^Traceback \(most recent call last\)/.test(t)
            || /^[A-Za-z_.]*(Error|Exception)\b:?/.test(t) || /^File "/.test(t) || /^!!! Exception/.test(t))
        return "error";
    if (/\b(WARNING|WARN)\b/.test(t) || /\b\w*Warning:/.test(t))
        return "warn";
    if (/^Prompt executed in\b/.test(t) || /^To see the GUI go to\b/.test(t) || /^Starting server\b/.test(t))
        return "ok";
    if (/\bDEBUG\b/.test(t))
        return "debug";
    return "";
}

/// Colours the leading "[tag]" of a line without colour codes of its own.
function tagged(html, palette) {
    return html.replace(/^(\s*)(\[[^\]\s][^\]]{0,40}\])/, (m, space, tag) => space + '<font color="' + palette.tag + '">' + tag + "</font>");
}

function format(text, palette) {
    let raw = lastRedraw(text);
    // Escape codes other than colours (cursor moves, clears, titles).
    raw = raw.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "").replace(/\x1b\[[0-9;?]*[A-La-ln-z]/g, "");
    const plain = raw.replace(/\x1b\[[0-9;]*m/g, "").replace(/\x1b/g, "");
    let html = "";
    let color = "", bold = false, dim = false, coloured = false, accent = "";
    const pieces = raw.split(/(\x1b\[[0-9;]*m)/);
    for (const piece of pieces) {
        const sgr = /^\x1b\[([0-9;]*)m$/.exec(piece);
        if (sgr) {
            const codes = sgr[1] === "" ? [0] : sgr[1].split(";").map(Number);
            for (let i = 0; i < codes.length; i++) {
                const c = codes[i];
                if (c === 0) {
                    color = "";
                    bold = false;
                    dim = false;
                } else if (c === 1)
                    bold = true;
                else if (c === 2)
                    dim = true;
                else if (c === 22) {
                    bold = false;
                    dim = false;
                } else if (c === 39)
                    color = "";
                else if (c >= 30 && c <= 37)
                    color = palette[SGR_COLORS[c - 30]];
                else if (c >= 90 && c <= 97)
                    color = palette[SGR_COLORS[c - 90]];
                else if (c === 38 || c === 48)
                    i += codes[i + 1] === 5 ? 2 : 4; // 256-colour / RGB: skip
            }
            continue;
        }
        const chunk = escapeHtml(piece.replace(/\x1b/g, ""));
        if (chunk === "")
            continue;
        const c = dim && !color ? palette.faint : color;
        if (c || bold) {
            coloured = coloured || !!c;
            if (c && !accent && piece.trim() !== "")
                accent = c;
            html += (bold ? "<b>" : "") + (c ? '<font color="' + c + '">' : "") + chunk + (c ? "</font>" : "") + (bold ? "</b>" : "");
        } else {
            html += chunk;
        }
    }
    if (!coloured) {
        const withTag = tagged(html, palette);
        if (withTag !== html)
            accent = palette.tag;
        html = withTag;
    }
    return {
        "plain": plain,
        "html": html,
        "level": levelOf(plain),
        "accent": accent
    };
}
