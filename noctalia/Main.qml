import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Widgets

Item {
    id: root

    property var pluginApi: null

    // ── Job state model ───────────────────────────────────────────────────────
    // { "jobId": { state, metadata, current, total, stageName, promptText, finishedValue } }
    property var jobs: ({})
    readonly property int jobsCount: Object.keys(jobs).length

    // Array version of jobs for ListView/Repeater binding in Panel.
    readonly property var jobsList: {
        var keys = Object.keys(jobs);
        var arr = [];
        for (var i = 0; i < keys.length; i++) {
            var j = Object.assign({}, jobs[keys[i]]);
            j.jobId = parseInt(keys[i]);
            arr.push(j);
        }
        return arr;
    }

    readonly property bool hasAnimatedJobs: {
        var vals = Object.values(jobs);
        for (var i = 0; i < vals.length; i++) {
            var spec = colorSpecForState(vals[i].state);
            if (spec && spec.type !== "static")
                return true;
        }
        return false;
    }

    // ── Active prompt for the notification overlay ────────────────────────────
    // Tracks the most recent prompt so the overlay can display it.
    property int activePromptJobId: -1
    property string activePromptText: ""
    property bool promptOverlayVisible: false

    // ── Daemon color defaults ────────────────────────────────────────────────
    // Fetched from oryx-jobs via GetColors on startup and on reconnect.
    // Each value is a spec object: { type, color, [colors], [periodMs] }
    property var daemonColors: ({})

    // ── Global animation clock ───────────────────────────────────────────────
    // A single elapsed-ms counter shared by all animations, mirroring the
    // daemon's global epoch approach so same-period animations stay in phase.
    property real animElapsedMs: 0

    Timer {
        interval: 30
        running: root.hasAnimatedJobs
        repeat: true
        onTriggered: root.animElapsedMs += interval
    }

    // ── Color spec resolution ────────────────────────────────────────────────

    // Map from job state name to settings key.
    readonly property var stateKeyMap: ({
        "created":  "colorStarted",
        "started":  "colorStarted",
        "stage":    "colorStageDefault",
        "prompt":   "colorPromptWaiting",
        "finished": "colorFinishedDefault"
    })

    /// Normalize a setting value to a spec object.
    /// Accepts bare hex strings (backward compat) and spec objects.
    function normalizeSpec(v) {
        if (!v) return null;
        if (typeof v === "string")
            return { type: "static", color: v };
        if (typeof v === "object" && v.type && v.color)
            return v;
        return null;
    }

    /// Return the full color spec for a settings key.
    /// Fallback chain: pluginSettings -> daemonColors -> manifest defaults.
    function colorSpec(key) {
        // Daemon key: strip "color" prefix and lower-case first char
        var dk = key.replace(/^color/, "");
        dk = dk.charAt(0).toLowerCase() + dk.slice(1);

        var ps = normalizeSpec(pluginApi?.pluginSettings?.[key]);
        if (ps) return ps;

        var dc = normalizeSpec(daemonColors[dk]);
        if (dc) return dc;

        var ms = normalizeSpec(
            pluginApi?.manifest?.metadata?.defaultSettings?.[key]);
        if (ms) return ms;

        return { type: "static", color: "#888888" };
    }

    /// Return the color spec for a given job state.
    function colorSpecForState(state) {
        var key = stateKeyMap[state];
        if (key) return colorSpec(key);
        return colorSpec("colorIdle");
    }

    /// Compute animation opacity for a given job state.
    /// Static specs return 1.0; breathe/bounce specs oscillate 0.15..1.0.
    function animOpacity(state) {
        var spec = colorSpecForState(state);
        if (!spec || spec.type === "static")
            return 1.0;

        var period = spec.periodMs || (spec.type === "breathe" ? 1500 : 2000);
        var t = (animElapsedMs % period) / period;

        if (spec.type === "breathe") {
            // Sine wave brightness: min 0.15, max 1.0
            var phase = t * 2.0 * Math.PI;
            return 0.15 + 0.85 * ((Math.sin(phase) + 1.0) / 2.0);
        }

        if (spec.type === "bounce") {
            // Triangle wave: 0->1 first half, 1->0 second half
            var pos = t < 0.5 ? t * 2.0 : (1.0 - t) * 2.0;
            return 0.15 + 0.85 * pos;
        }

        return 1.0;
    }

    // ── Color utilities ───────────────────────────────────────────────────────

    function parseHex(hex) {
        var s = (hex || "#888888").replace("#", "");
        return [
            parseInt(s.substring(0, 2), 16),
            parseInt(s.substring(2, 4), 16),
            parseInt(s.substring(4, 6), 16)
        ];
    }

    function lerpColor(hexA, hexB, t) {
        var a = parseHex(hexA);
        var b = parseHex(hexB);
        var r = Math.round(a[0] + (b[0] - a[0]) * t);
        var g = Math.round(a[1] + (b[1] - a[1]) * t);
        var bl = Math.round(a[2] + (b[2] - a[2]) * t);
        return "#" + ("0" + r.toString(16)).slice(-2)
                   + ("0" + g.toString(16)).slice(-2)
                   + ("0" + bl.toString(16)).slice(-2);
    }

    // ── WCAG contrast utilities ──────────────────────────────────────────────

    /// Linearize an sRGB channel value (0-255) for luminance calculation.
    function srgbLinear(c) {
        var s = c / 255.0;
        return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
    }

    /// Relative luminance of an [r, g, b] array (0-255 each). Returns 0.0-1.0.
    function relativeLuminance(rgb) {
        return 0.2126 * srgbLinear(rgb[0])
             + 0.7152 * srgbLinear(rgb[1])
             + 0.0722 * srgbLinear(rgb[2]);
    }

    /// WCAG contrast ratio between two luminance values. Returns 1.0-21.0.
    function contrastRatio(lum1, lum2) {
        var lighter = Math.max(lum1, lum2);
        var darker  = Math.min(lum1, lum2);
        return (lighter + 0.05) / (darker + 0.05);
    }

    /// Parse a QML color (which may be "#AARRGGBB", "#RRGGBB", or a named
    /// color) into an [r, g, b] array.
    function colorToRgb(c) {
        var s = c.toString().replace("#", "");
        // QML color.toString() returns "#aarrggbb" (8 chars)
        if (s.length === 8) s = s.substring(2); // strip alpha
        return [
            parseInt(s.substring(0, 2), 16),
            parseInt(s.substring(2, 4), 16),
            parseInt(s.substring(4, 6), 16)
        ];
    }

    function rgbToHex(rgb) {
        return "#" + ("0" + rgb[0].toString(16)).slice(-2)
                   + ("0" + rgb[1].toString(16)).slice(-2)
                   + ("0" + rgb[2].toString(16)).slice(-2);
    }

    /// Ensure `fgColor` has at least 3:1 contrast against `bgColor`.
    /// If not, mix toward white (on dark bg) or black (on light bg)
    /// until the threshold is met. Both arguments are QML color values.
    function ensureContrast(fgColor, bgColor) {
        var fg = colorToRgb(fgColor);
        var bg = colorToRgb(bgColor);
        var bgLum = relativeLuminance(bg);
        var fgLum = relativeLuminance(fg);

        if (contrastRatio(fgLum, bgLum) >= 3.0)
            return fgColor;

        // Mix toward white on dark backgrounds, black on light.
        var target = bgLum > 0.5 ? [0, 0, 0] : [255, 255, 255];
        for (var t = 0.1; t <= 1.0; t += 0.1) {
            var mixed = [
                Math.round(fg[0] + (target[0] - fg[0]) * t),
                Math.round(fg[1] + (target[1] - fg[1]) * t),
                Math.round(fg[2] + (target[2] - fg[2]) * t)
            ];
            if (contrastRatio(relativeLuminance(mixed), bgLum) >= 3.0)
                return rgbToHex(mixed);
        }
        return rgbToHex(target);
    }

    /// Return white or black depending on which has better contrast against bg.
    function contrastTextColor(bgColor) {
        var bg = colorToRgb(bgColor);
        var bgLum = relativeLuminance(bg);
        var whiteRatio = contrastRatio(1.0, bgLum);
        var blackRatio = contrastRatio(bgLum, 0.0);
        return whiteRatio >= blackRatio ? "#FFFFFF" : "#000000";
    }

    /// Return a darkened or lightened variant of a color for hover state.
    /// Darkens light colors, lightens dark colors.
    function hoverVariant(c) {
        var rgb = colorToRgb(c);
        var lum = relativeLuminance(rgb);
        return lum > 0.3 ? Qt.darker(c, 1.3) : Qt.lighter(c, 1.5);
    }

    // ── Job color resolution ──────────────────────────────────────────────────

    function resolveColor(job) {
        if (!job) return colorSpec("colorIdle").color;
        switch (job.state) {
        case "created":
        case "started":
            return colorSpec("colorStarted").color;
        case "progress":
            var t = job.total > 0
                ? Math.max(0, Math.min(1, job.current / job.total))
                : 0;
            return lerpColor(colorSpec("colorProgressStart").color,
                             colorSpec("colorProgressEnd").color, t);
        case "stage":
            return colorSpec("colorStageDefault").color;
        case "prompt":
            return colorSpec("colorPromptWaiting").color;
        case "finished":
            return colorSpec("colorFinishedDefault").color;
        default:
            return colorSpec("colorIdle").color;
        }
    }

    function iconForState(state) {
        switch (state) {
        case "created":        return "circle-plus";
        case "started":        return "player-play-filled";
        case "progress":       return "progress";
        case "stage":          return "stack-2";
        case "prompt":         return "question-mark";
        case "finished":       return "circle-check";
        default:               return "circle";
        }
    }

    // ── busctl JSON signal parser ─────────────────────────────────────────────

    // busctl --json=short wraps a{sv} dict values as:
    //   { "key": { "type": "s", "data": "value" }, ... }
    // or for nested variants:
    //   { "key": { "type": "v", "data": { "type": "s", "data": "value" } } }
    function extractVariantValue(v) {
        if (v === null || v === undefined) return null;
        if (typeof v !== "object") return v;
        // Unwrap nested variants
        if (v.type === "v" && v.data !== undefined)
            return extractVariantValue(v.data);
        if (v.data !== undefined)
            return v.data;
        return v;
    }

    // Parse the a{sv} metadata dict from busctl JSON payload.
    // busctl represents a{sv} as an object where each value is { type, data }.
    function parseSvDict(raw) {
        if (!raw || typeof raw !== "object") return {};
        var result = {};
        var keys = Object.keys(raw);
        for (var i = 0; i < keys.length; i++) {
            result[keys[i]] = extractVariantValue(raw[keys[i]]);
        }
        return result;
    }

    /// Parse the GetColors response into daemonColors.
    /// The response is a{sa{sv}}: outer dict keyed by color name,
    /// each value is a{sv} with type/color/colors/periodMs.
    function parseGetColorsResponse(line) {
        var msg;
        try { msg = JSON.parse(line); } catch (e) { return; }

        // busctl --json=short returns the response payload
        var payload = msg;
        if (msg.payload) payload = msg.payload;
        if (!payload || !payload.data) return;

        // For a method reply, data is an array of return values.
        // GetColors returns a single a{sa{sv}}.
        var outer = payload.data;
        if (Array.isArray(outer) && outer.length >= 1)
            outer = outer[0];
        if (!outer || typeof outer !== "object") return;

        var result = {};
        var keys = Object.keys(outer);
        for (var i = 0; i < keys.length; i++) {
            var inner = parseSvDict(outer[keys[i]]);
            // Build a normalized spec from the parsed dict
            var spec = { type: inner.type || "static", color: inner.color || "#888888" };
            if (inner.colors && Array.isArray(inner.colors))
                spec.colors = inner.colors;
            if (inner.periodMs !== undefined)
                spec.periodMs = parseFloat(inner.periodMs);
            result[keys[i]] = spec;
        }
        daemonColors = result;
    }

    /// Strip known state-specific keys from a dict and return the rest as
    /// pure metadata.  The stripped keys depend on the state.
    function extractStateMetadata(state, dict) {
        var meta = Object.assign({}, dict);
        switch (state) {
        case "progress":       delete meta.current; delete meta.total; break;
        case "stage":          delete meta.name; break;
        case "prompt":         delete meta.question; break;
        case "prompt_resolved": delete meta.accepted; break;
        case "finished":       delete meta.status; break;
        // created, started: entire dict is metadata
        }
        return meta;
    }

    function parseSignalLine(line) {
        var msg;
        try { msg = JSON.parse(line); } catch (e) { return; }

        if (msg.type !== "signal") return;
        if (msg.interface !== "zsa.oryx.Jobs") return;

        var payload = msg.payload;
        if (!payload || !payload.data) return;
        var data = payload.data;

        // ── MetadataChanged signal: (ua{sv}) ─────────────────────────────
        if (msg.member === "MetadataChanged") {
            if (data.length < 2) return;
            var mcJobId = String(data[0]);
            var mcDict = parseSvDict(data[1]);
            var mcExisting = jobs[mcJobId];
            if (!mcExisting) return;

            var mcUpdated = Object.assign({}, jobs);
            var mcJob = Object.assign({}, mcExisting);
            mcJob.creationMetadata = mcDict;
            mcJob.metadata = Object.assign({}, mcJob.creationMetadata, mcJob.stateMetadata);
            mcUpdated[mcJobId] = mcJob;
            jobs = mcUpdated;
            return;
        }

        // ── StateChanged signal: (usa{sv}) ───────────────────────────────
        if (msg.member !== "StateChanged") return;
        if (data.length < 2) return;

        var jobId = String(data[0]);
        var state = data[1];
        var dict = data.length > 2 ? parseSvDict(data[2]) : {};

        // "cleared" means the job slot has been freed
        if (state === "cleared") {
            var pruned = Object.assign({}, jobs);
            delete pruned[jobId];
            jobs = pruned;

            if (parseInt(jobId) === activePromptJobId)
                hidePromptOverlay();
            return;
        }

        // "prompt_resolved" is transient; transition the job back to started
        // and hide the prompt overlay.
        if (state === "prompt_resolved") {
            var existing = jobs[jobId];
            if (existing) {
                var resolved = Object.assign({}, jobs);
                var rJob = Object.assign({}, existing);
                rJob.state = "started";
                rJob.stateMetadata = extractStateMetadata("prompt_resolved", dict);
                rJob.metadata = Object.assign({}, rJob.creationMetadata, rJob.stateMetadata);
                resolved[jobId] = rJob;
                jobs = resolved;
            }
            if (parseInt(jobId) === activePromptJobId)
                hidePromptOverlay();
            return;
        }

        // Build or update the job entry
        var job = Object.assign({}, jobs[jobId] || {
            state: "",
            creationMetadata: {},
            stateMetadata: {},
            metadata: {},
            current: 0,
            total: 0,
            stageName: "",
            promptText: "",
            finishedValue: null
        });

        job.state = state;

        // Extract state-specific fields
        if (state === "created") {
            job.creationMetadata = dict;
        } else if (state === "progress") {
            job.current = dict.current !== undefined ? parseInt(dict.current) : 0;
            job.total   = dict.total   !== undefined ? parseInt(dict.total)   : 0;
        } else if (state === "stage") {
            job.stageName = dict.name || "";
        } else if (state === "prompt") {
            job.promptText = dict.question || "";
            showPromptOverlay(parseInt(jobId), job.promptText);
        } else if (state === "finished") {
            job.finishedValue = dict.status !== undefined ? dict.status : null;
        }

        // Store per-state metadata and merge with creation metadata
        job.stateMetadata = extractStateMetadata(state, dict);
        job.metadata = Object.assign({}, job.creationMetadata, job.stateMetadata);

        var updated = Object.assign({}, jobs);
        updated[jobId] = job;
        jobs = updated;
    }

    // ── Prompt overlay control ────────────────────────────────────────────────

    function showPromptOverlay(jobId, text) {
        activePromptJobId = jobId;
        activePromptText = text;
        promptOverlayVisible = true;
    }

    function hidePromptOverlay() {
        promptOverlayVisible = false;
        activePromptJobId = -1;
        activePromptText = "";
    }

    // ── Fetch daemon colors ──────────────────────────────────────────────────

    function fetchDaemonColors() {
        getColorsProcess.exec([
            "busctl", "--user", "--json=short", "call",
            "zsa.oryx.Jobs", "/zsa/oryx/Jobs", "zsa.oryx.Jobs",
            "GetColors"
        ]);
    }

    Process {
        id: getColorsProcess
        stdout: SplitParser {
            onRead: line => root.parseGetColorsResponse(line)
        }
    }

    // ── Fetch existing jobs on startup ────────────────────────────────────────

    function fetchJobs() {
        getJobsProcess.exec([
            "busctl", "--user", "--json=short", "call",
            "zsa.oryx.Jobs", "/zsa/oryx/Jobs", "zsa.oryx.Jobs",
            "GetJobs"
        ]);
    }

    /// Parse the GetJobs response and merge into the jobs model.
    /// Only adds jobs that the monitor hasn't already delivered (monitor wins).
    function parseGetJobsResponse(line) {
        var msg;
        try { msg = JSON.parse(line); } catch (e) { return; }

        var payload = msg;
        if (msg.payload) payload = msg.payload;
        if (!payload || !payload.data) return;

        var outer = payload.data;
        if (Array.isArray(outer) && outer.length >= 1)
            outer = outer[0];
        if (!outer || typeof outer !== "object") return;

        var merged = Object.assign({}, jobs);
        var ids = Object.keys(outer);
        for (var i = 0; i < ids.length; i++) {
            var jobId = ids[i];

            // Monitor signals win — skip jobs we already know about.
            if (merged[jobId] !== undefined) continue;

            var tuple = outer[jobId];
            if (!Array.isArray(tuple) || tuple.length < 3) continue;

            var state     = tuple[0];
            var stateDict = parseSvDict(tuple[1]);
            var jobDict   = parseSvDict(tuple[2]);

            var job = {
                state: state,
                creationMetadata: jobDict,
                stateMetadata: {},
                metadata: {},
                current: 0,
                total: 0,
                stageName: "",
                promptText: "",
                finishedValue: null
            };

            // Extract state-specific fields (same logic as parseSignalLine).
            if (state === "progress") {
                job.current = stateDict.current !== undefined ? parseInt(stateDict.current) : 0;
                job.total   = stateDict.total   !== undefined ? parseInt(stateDict.total)   : 0;
            } else if (state === "stage") {
                job.stageName = stateDict.name || "";
            } else if (state === "prompt") {
                job.promptText = stateDict.question || "";
                showPromptOverlay(parseInt(jobId), job.promptText);
            } else if (state === "finished") {
                job.finishedValue = stateDict.status !== undefined ? stateDict.status : null;
            }

            job.stateMetadata = extractStateMetadata(state, stateDict);
            job.metadata = Object.assign({}, job.creationMetadata, job.stateMetadata);

            merged[jobId] = job;
        }
        jobs = merged;
    }

    Process {
        id: getJobsProcess
        stdout: SplitParser {
            onRead: line => root.parseGetJobsResponse(line)
        }
    }

    Component.onCompleted: { fetchDaemonColors(); fetchJobs(); }

    // ── busctl monitor process ────────────────────────────────────────────────

    Process {
        id: monitorProcess
        command: ["busctl", "--user", "--json=short", "monitor", "zsa.oryx.Jobs"]
        running: true

        onRunningChanged: {
            if (!running)
                reconnectTimer.restart();
        }

        stdout: SplitParser {
            onRead: line => root.parseSignalLine(line)
        }
    }

    Timer {
        id: reconnectTimer
        interval: 2000
        repeat: false
        onTriggered: {
            monitorProcess.running = true;
            root.fetchDaemonColors();
            root.fetchJobs();
        }
    }

    // ── One-shot call process ─────────────────────────────────────────────────

    Process { id: callProcess }

    // ── Public API ────────────────────────────────────────────────────────────

    function promptResolve(jobId, accepted) {
        callProcess.exec([
            "busctl", "--user", "call",
            "zsa.oryx.Jobs", "/zsa/oryx/Jobs", "zsa.oryx.Jobs",
            "PromptResolve", "uba{sv}",
            jobId.toString(), accepted ? "true" : "false",
            "0"
        ]);
    }

    function clearJob(jobId) {
        callProcess.exec([
            "busctl", "--user", "call",
            "zsa.oryx.Jobs", "/zsa/oryx/Jobs", "zsa.oryx.Jobs",
            "Clear", "u",
            jobId.toString()
        ]);
    }

}
