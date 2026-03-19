import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

ColumnLayout {
    id: root

    property var pluginApi: null

    spacing: Style.marginL

    // ── Main reference for daemon defaults ────────────────────────────────────

    readonly property var mainRef: pluginApi?.mainInstance ?? null

    // ── Default resolution ────────────────────────────────────────────────────

    /// Return the default spec for a settings key.
    /// Falls back: daemon colors -> manifest defaults -> hardcoded.
    function def(key) {
        if (mainRef) {
            var spec = mainRef.colorSpec(key);
            if (spec) return spec;
        }
        var ms = pluginApi?.manifest?.metadata?.defaultSettings?.[key];
        if (ms) {
            if (typeof ms === "string")
                return { type: "static", color: ms };
            if (typeof ms === "object" && ms.type)
                return ms;
        }
        return { type: "static", color: "#888888" };
    }

    // ── Editable spec properties ──────────────────────────────────────────────
    // Each setting is a full spec: { type, color, periodMs }

    function loadSpec(key) {
        var saved = pluginApi?.pluginSettings?.[key];
        if (saved && typeof saved === "object" && saved.type)
            return saved;
        if (saved && typeof saved === "string")
            return { type: "static", color: saved };
        return def(key);
    }

    property var editIdle:             loadSpec("colorIdle")
    property var editStarted:          loadSpec("colorStarted")
    property var editProgressStart:    loadSpec("colorProgressStart")
    property var editProgressEnd:      loadSpec("colorProgressEnd")
    property var editFinishedDefault:  loadSpec("colorFinishedDefault")
    property var editStageDefault:     loadSpec("colorStageDefault")
    property var editPromptWaiting:    loadSpec("colorPromptWaiting")
    property var editPromptAccept:     loadSpec("colorPromptAccept")
    property var editPromptReject:     loadSpec("colorPromptReject")

    // ── Save ──────────────────────────────────────────────────────────────────

    function saveSettings() {
        if (!pluginApi) return;
        pluginApi.pluginSettings.colorIdle            = root.editIdle;
        pluginApi.pluginSettings.colorStarted         = root.editStarted;
        pluginApi.pluginSettings.colorProgressStart   = root.editProgressStart;
        pluginApi.pluginSettings.colorProgressEnd     = root.editProgressEnd;
        pluginApi.pluginSettings.colorFinishedDefault = root.editFinishedDefault;
        pluginApi.pluginSettings.colorStageDefault    = root.editStageDefault;
        pluginApi.pluginSettings.colorPromptWaiting   = root.editPromptWaiting;
        pluginApi.pluginSettings.colorPromptAccept    = root.editPromptAccept;
        pluginApi.pluginSettings.colorPromptReject    = root.editPromptReject;
        pluginApi.saveSettings();
    }

    // ── Animation type model ──────────────────────────────────────────────────

    readonly property var animTypeModel: [
        { "key": "static",  "name": "Static" },
        { "key": "breathe", "name": "Breathe" },
        { "key": "bounce",  "name": "Bounce" }
    ]

    // ── Job States ────────────────────────────────────────────────────────

    NText {
        text: "Job States"
        font.bold: true
        pointSize: Style.fontSizeL
    }

    ColorSpecRow {
        label: "Idle"
        spec: root.editIdle
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editIdle = newSpec
    }

    ColorSpecRow {
        label: "Started"
        spec: root.editStarted
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editStarted = newSpec
    }

    ColorSpecRow {
        label: "Stage"
        spec: root.editStageDefault
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editStageDefault = newSpec
    }

    ColorSpecRow {
        label: "Finished"
        spec: root.editFinishedDefault
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editFinishedDefault = newSpec
    }

    NDivider { Layout.fillWidth: true }

    // ── Progress ──────────────────────────────────────────────────────────

    NText {
        text: "Progress Gradient"
        font.bold: true
        pointSize: Style.fontSizeL
    }

    ColorSpecRow {
        label: "Start (0%)"
        spec: root.editProgressStart
        allowAnimation: false
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editProgressStart = newSpec
    }

    ColorSpecRow {
        label: "End (100%)"
        spec: root.editProgressEnd
        allowAnimation: false
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editProgressEnd = newSpec
    }

    NDivider { Layout.fillWidth: true }

    // ── Prompt ────────────────────────────────────────────────────────────

    NText {
        text: "Prompt"
        font.bold: true
        pointSize: Style.fontSizeL
    }

    ColorSpecRow {
        label: "Waiting"
        spec: root.editPromptWaiting
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editPromptWaiting = newSpec
    }

    ColorSpecRow {
        label: "Accept"
        spec: root.editPromptAccept
        allowAnimation: false
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editPromptAccept = newSpec
    }

    ColorSpecRow {
        label: "Reject"
        spec: root.editPromptReject
        allowAnimation: false
        animTypeModel: root.animTypeModel
        onSpecEdited: newSpec => root.editPromptReject = newSpec
    }

    NDivider { Layout.fillWidth: true }

    // ── Reset ─────────────────────────────────────────────────────────────

    NButton {
        text: "Reset to Defaults"
        icon: "refresh"
        outlined: true
        onClicked: {
            root.editIdle            = def("colorIdle");
            root.editStarted         = def("colorStarted");
            root.editProgressStart   = def("colorProgressStart");
            root.editProgressEnd     = def("colorProgressEnd");
            root.editFinishedDefault = def("colorFinishedDefault");
            root.editStageDefault    = def("colorStageDefault");
            root.editPromptWaiting   = def("colorPromptWaiting");
            root.editPromptAccept    = def("colorPromptAccept");
            root.editPromptReject    = def("colorPromptReject");
        }
    }

    Item { Layout.fillHeight: true }
}
