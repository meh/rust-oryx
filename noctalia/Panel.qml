import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

Item {
    id: root

    property var pluginApi: null

    // SmartPanel properties (required for panel behavior)
    readonly property var geometryPlaceholder: panelContainer
    readonly property bool allowAttach: true

    // Content-driven sizing: width fixed, height tracks content (capped).
    property real contentPreferredWidth: 400 * Style.uiScaleRatio
    property real contentPreferredHeight: Math.min(
        500 * Style.uiScaleRatio,
        contentColumn.implicitHeight + Style.margin2L
    )

    readonly property var mainRef: pluginApi?.mainInstance ?? null
    readonly property var jobsList: mainRef?.jobsList ?? []
    readonly property int jobsCount: mainRef?.jobsCount ?? 0

    Rectangle {
        id: panelContainer
        anchors.fill: parent
        color: "transparent"

        Flickable {
            anchors.fill: parent
            anchors.margins: Style.marginM
            contentHeight: contentColumn.implicitHeight
            clip: contentHeight > height
            boundsBehavior: Flickable.StopAtBounds

            ColumnLayout {
                id: contentColumn
                width: parent.width
                spacing: Style.marginM

                // ── Empty state ───────────────────────────────────────
                NText {
                    visible: root.jobsCount === 0
                    text: "No active jobs"
                    color: Color.mOnSurface
                    opacity: 0.5
                    pointSize: Style.fontSizeM
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: Style.marginL
                }

                // ── Job cards ─────────────────────────────────────────
                Repeater {
                    model: root.jobsList

                    delegate: Rectangle {
                        id: jobCard

                        required property var modelData
                        readonly property int jobId: modelData.jobId || 0
                        readonly property string jobState: modelData.state || ""
                        readonly property string jobName: {
                            var meta = modelData.metadata || {};
                            return meta.name || ("Job #" + jobId);
                        }
                        readonly property string jobColor: mainRef
                            ? mainRef.resolveColor(modelData)
                            : "#888888"

                        Layout.fillWidth: true
                        implicitHeight: cardContent.implicitHeight + Style.marginM * 2
                        radius: Style.radiusM
                        color: Style.capsuleColor

                        ColumnLayout {
                            id: cardContent
                            anchors.fill: parent
                            anchors.margins: Style.marginM
                            spacing: Style.marginS

                            // ── Header: icon + name + state [+ stage] ─
                            RowLayout {
                                spacing: Style.marginS

                                Item {
                                    readonly property bool isProgress: jobCard.jobState === "progress"
                                    readonly property real jobProgress: {
                                        var t = jobCard.modelData.total || 0;
                                        if (t <= 0) return 0;
                                        return Math.max(0, Math.min(1, (jobCard.modelData.current || 0) / t));
                                    }

                                    implicitWidth: isProgress ? pIcon.implicitWidth : hIcon.implicitWidth
                                    implicitHeight: isProgress ? pIcon.implicitHeight : hIcon.implicitHeight

                                    NIcon {
                                        id: hIcon
                                        visible: !parent.isProgress
                                        anchors.centerIn: parent
                                        icon: mainRef
                                            ? mainRef.iconForState(jobCard.jobState)
                                            : "circle"
                                        color: jobCard.jobColor
                                        opacity: mainRef
                                            ? mainRef.animOpacity(jobCard.jobState)
                                            : 1.0
                                    }

                                    ProgressIcon {
                                        id: pIcon
                                        visible: parent.isProgress
                                        anchors.centerIn: parent
                                        progress: parent.jobProgress
                                        color: jobCard.jobColor
                                    }
                                }

                                NText {
                                    text: jobCard.jobName
                                    color: Color.mOnSurface
                                    pointSize: Style.fontSizeM
                                    font.weight: Font.Bold
                                }

                                Item { Layout.fillWidth: true }

                                NText {
                                    visible: jobCard.jobState === "stage"
                                    text: jobCard.modelData.stageName || ""
                                    color: jobCard.jobColor
                                    pointSize: Style.fontSizeS
                                    font.weight: Font.Bold
                                }
                            }

                            // ── Metadata (from job creation, excluding name) ──
                            Repeater {
                                model: {
                                    var meta = jobCard.modelData.metadata || {};
                                    var keys = Object.keys(meta);
                                    var arr = [];
                                    for (var i = 0; i < keys.length; i++) {
                                        if (keys[i] === "name") continue;
                                        arr.push({ key: keys[i], val: String(meta[keys[i]]) });
                                    }
                                    return arr;
                                }

                                delegate: RowLayout {
                                    required property var modelData
                                    spacing: Style.marginS

                                    NText {
                                        text: modelData.key + ":"
                                        color: Color.mOnSurface
                                        opacity: 0.6
                                        pointSize: Style.fontSizeS
                                    }
                                    NText {
                                        text: modelData.val
                                        color: Color.mOnSurface
                                        pointSize: Style.fontSizeS
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                    }
                                }
                            }

                            // ── Progress info ─────────────────────────
                            Loader {
                                active: jobCard.jobState === "progress"
                                Layout.fillWidth: true
                                sourceComponent: ColumnLayout {
                                    spacing: Style.marginS / 2

                                    RowLayout {
                                        NText {
                                            text: (jobCard.modelData.current || 0)
                                                  + " / "
                                                  + (jobCard.modelData.total || 0)
                                            color: jobCard.jobColor
                                            pointSize: Style.fontSizeS
                                        }
                                        Item { Layout.fillWidth: true }
                                        NText {
                                            readonly property real pct: {
                                                var t = jobCard.modelData.total || 0;
                                                if (t <= 0) return 0;
                                                return Math.round(
                                                    (jobCard.modelData.current || 0) / t * 100);
                                            }
                                            text: pct + "%"
                                            color: jobCard.jobColor
                                            pointSize: Style.fontSizeS
                                        }
                                    }

                                    // Progress bar
                                    Rectangle {
                                        Layout.fillWidth: true
                                        height: 4 * Style.uiScaleRatio
                                        radius: height / 2
                                        color: Color.mOnSurface
                                        opacity: 0.15

                                        Rectangle {
                                            width: {
                                                var t = jobCard.modelData.total || 0;
                                                if (t <= 0) return 0;
                                                var frac = Math.max(0, Math.min(1,
                                                    (jobCard.modelData.current || 0) / t));
                                                return parent.width * frac;
                                            }
                                            height: parent.height
                                            radius: parent.radius
                                            color: jobCard.jobColor

                                            Behavior on width {
                                                NumberAnimation { duration: 200 }
                                            }
                                        }
                                    }
                                }
                            }

                            // ── Prompt: text + accept/reject buttons ──
                            Loader {
                                active: jobCard.jobState === "prompt"
                                Layout.fillWidth: true
                                sourceComponent: ColumnLayout {
                                    spacing: Style.marginS

                                    NText {
                                        text: jobCard.modelData.promptText || ""
                                        color: Color.mOnSurface
                                        pointSize: Style.fontSizeS
                                        Layout.fillWidth: true
                                        wrapMode: Text.WordWrap
                                    }

                                    RowLayout {
                                        spacing: Style.marginS

                                        NButton {
                                            readonly property string rawColor: mainRef
                                                ? mainRef.ensureContrast(
                                                    mainRef.colorSpec("colorPromptReject").color,
                                                    Style.capsuleColor)
                                                : "#CC0000"
                                            text: "Reject"
                                            fontSize: Style.fontSizeS
                                            backgroundColor: rawColor
                                            textColor: mainRef
                                                ? mainRef.contrastTextColor(rawColor)
                                                : "#FFFFFF"
                                            hoverColor: mainRef
                                                ? mainRef.hoverVariant(rawColor)
                                                : Qt.darker(rawColor, 1.3)
                                            textHoverColor: mainRef
                                                ? mainRef.contrastTextColor(mainRef.hoverVariant(rawColor))
                                                : "#FFFFFF"
                                            implicitHeight: 28 * Style.uiScaleRatio
                                            onClicked: {
                                                if (mainRef)
                                                    mainRef.promptResolve(jobCard.jobId, false);
                                            }
                                        }

                                        NButton {
                                            readonly property string rawColor: mainRef
                                                ? mainRef.ensureContrast(
                                                    mainRef.colorSpec("colorPromptAccept").color,
                                                    Style.capsuleColor)
                                                : "#007A00"
                                            text: "Accept"
                                            fontSize: Style.fontSizeS
                                            backgroundColor: rawColor
                                            textColor: mainRef
                                                ? mainRef.contrastTextColor(rawColor)
                                                : "#FFFFFF"
                                            hoverColor: mainRef
                                                ? mainRef.hoverVariant(rawColor)
                                                : Qt.darker(rawColor, 1.3)
                                            textHoverColor: mainRef
                                                ? mainRef.contrastTextColor(mainRef.hoverVariant(rawColor))
                                                : "#FFFFFF"
                                            implicitHeight: 28 * Style.uiScaleRatio
                                            onClicked: {
                                                if (mainRef)
                                                    mainRef.promptResolve(jobCard.jobId, true);
                                            }
                                        }
                                    }
                                }
                            }

                            // ── Finished: value + clear button ────────
                            Loader {
                                active: jobCard.jobState === "finished"
                                Layout.fillWidth: true
                                sourceComponent: RowLayout {
                                    spacing: Style.marginS

                                    NText {
                                        visible: jobCard.modelData.finishedValue !== null
                                        text: "Result: " + String(
                                            jobCard.modelData.finishedValue ?? "")
                                        color: Color.mOnSurface
                                        opacity: 0.7
                                        pointSize: Style.fontSizeS
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                    }

                                    Item { Layout.fillWidth: true }

                                    NButton {
                                        text: "Clear"
                                        fontSize: Style.fontSizeS
                                        backgroundColor: Color.mOnSurface
                                        textColor: Color.mSurface
                                        implicitHeight: 28 * Style.uiScaleRatio
                                        onClicked: {
                                            if (mainRef)
                                                mainRef.clearJob(jobCard.jobId);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
