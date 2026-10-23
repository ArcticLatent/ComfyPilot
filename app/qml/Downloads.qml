// The running download (models, LoRAs, workflows; one at a time) and what
// finished, from the backend's download-progress events.
//   Downloads.begin("model", "Starting model download…")  then the command
//   Downloads.cancel()

import ArcticHelper
import QtQuick
pragma Singleton

// Rows are ListModels updated in place, so the panel's bars keep their
// delegates and only their values change.
QtObject {
    id: downloads

    property bool busy: false
    property string kind: ""
    /// A line about what's happening.
    property string status: "Nothing downloading."
    /// Files being downloaded: { key, artifact, received, size, phase }.
    property ListModel transfers
    property ListModel completed
    /// This batch: its expected size when known (so the overall bar doesn't
    /// jump back as later files start), bytes of the files already done, and
    /// the totals.
    property real expectedBytes: 0
    property real doneBytes: 0
    property real totalBytes: 0
    property real receivedBytes: 0
    readonly property real fraction: totalBytes > 0 ? Math.min(1, receivedBytes / totalBytes) : 0
    readonly property int activeCount: busy ? transfers.count : 0
    // If the backend doesn't confirm the cancel, stop showing it as running.
    property Timer cancelFallback
    property Connections _events

    /// A download batch ended: "ok", "cancelled" or "failed".
    signal ended(string kind, string outcome, string message)

    function begin(kind, label, expectedBytes) {
        downloads.expectedBytes = expectedBytes || 0;
        busy = true;
        downloads.kind = kind;
        status = label || "Starting…";
        transfers.clear();
        doneBytes = 0;
        recompute();
    }

    function end() {
        busy = false;
        transfers.clear();
        cancelFallback.stop();
        recompute();
    }

    function cancel() {
        status = "Cancelling…";
        Api.call("cancel_active_download").then((cancelled) => {
            if (cancelled)
                cancelFallback.restart();
            else
                downloads.end();
        }).catch(() => {
            return downloads.end();
        });
    }

    function recompute() {
        let size = 0;
        let received = 0;
        for (let i = 0; i < transfers.count; i++) {
            const t = transfers.get(i);
            size += t.size;
            received += Math.min(t.received, t.size || t.received);
        }
        totalBytes = Math.max(expectedBytes, doneBytes + size);
        receivedBytes = doneBytes + received;
    }

    function formatBytes(bytes) {
        let value = Number(bytes || 0);
        if (!isFinite(value) || value <= 0)
            return "0 B";

        const units = ["B", "KB", "MB", "GB", "TB"];
        let i = 0;
        while (value >= 1024 && i < units.length - 1) {
            value /= 1024;
            i += 1;
        }
        return (value >= 10 || i === 0 ? Math.round(value) : value.toFixed(1)) + " " + units[i];
    }

    function indexOfKey(key) {
        for (let i = 0; i < transfers.count; i++) if (transfers.get(i).key === key) {
            return i;
        }
        return -1;
    }

    function onProgress(p) {
        const kind = p.kind || "download";
        if (p.phase === "cancelled" || p.phase === "batch_finished" || p.phase === "batch_failed") {
            const outcome = p.phase === "cancelled" ? "cancelled" : p.phase === "batch_failed" ? "failed" : "ok";
            status = outcome === "ok" ? (p.message || "Done.") : outcome === "cancelled" ? "Cancelled." : (p.message || "Failed.");
            end();
            ended(kind, outcome, p.message || "");
            return ;
        }
        // Late events of a batch that already ended (a cancelled file
        // reporting its failure afterwards).
        if (!busy)
            return ;

        const key = kind + ":" + (p.index || "?") + ":" + (p.artifact || "item");
        const artifact = p.artifact || "file";
        let row = indexOfKey(key);
        if (p.phase === "finished" || p.phase === "failed") {
            if (row >= 0) {
                if (p.phase === "finished")
                    doneBytes += transfers.get(row).size;

                transfers.remove(row);
            }
            completed.insert(0, {
                "name": artifact,
                "folder": String(p.folder || ""),
                "failed": p.phase === "failed",
                "message": String(p.message || "")
            });
            if (completed.count > 100)
                completed.remove(100, completed.count - 100);

            status = p.phase === "failed" ? artifact + " failed: " + (p.message || "unknown error") : "Finished " + artifact;
            recompute();
            return ;
        }
        if (row < 0) {
            transfers.append({
                "key": key,
                "artifact": artifact,
                "received": 0,
                "size": Number(p.size || 0),
                "phase": "started"
            });
            row = transfers.count - 1;
        }
        const current = transfers.get(row);
        // Verification re-reads the file from the start.
        if (p.phase === "verifying" && current.phase !== "verifying")
            transfers.setProperty(row, "received", 0);

        if (p.phase)
            transfers.setProperty(row, "phase", p.phase);

        if (p.size !== null && p.size !== undefined && Number(p.size) > 0)
            transfers.setProperty(row, "size", Number(p.size));

        if (p.received !== null && p.received !== undefined)
            transfers.setProperty(row, "received", Math.max(transfers.get(row).received, Number(p.received || 0)));

        if (p.phase === "started")
            status = "Downloading " + (p.index || "?") + " of " + (p.total || "?") + ": " + artifact;
        else if (p.phase === "verifying")
            status = "Checking " + artifact;
        recompute();
    }

    transfers: ListModel {
    }
    /// Finished files, newest first: { name, folder, failed, message }.

    completed: ListModel {
    }

    cancelFallback: Timer {
        interval: 5000
        onTriggered: {
            if (downloads.busy) {
                downloads.status = "Cancelled.";
                const kind = downloads.kind;
                downloads.end();
                downloads.ended(kind, "cancelled", "");
            }
        }
    }

    _events: Connections {
        function onEvent(name, payload) {
            if (name === "download-progress")
                downloads.onProgress(payload || {
            });

        }

        target: Api
    }

}
