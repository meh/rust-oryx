import QtQuick
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Services.UI
import qs.Widgets

Item {
    id: root

    property var pluginApi: null
    property ShellScreen screen
    property string widgetId: ""
    property string section: ""

    // Main.qml exposes these on the pluginApi.mainInstance
    readonly property var mainRef: pluginApi?.mainInstance ?? null
    readonly property var jobsList: mainRef?.jobsList ?? []
    readonly property int jobsCount: mainRef?.jobsCount ?? 0

    readonly property bool hovered: mouseArea.containsMouse
    readonly property string screenName: screen ? screen.name : ""
    readonly property real capsuleH: Style.getCapsuleHeightForScreen(screenName)
    readonly property real contentWidth: jobsCount <= 1
        ? capsuleH
        : row.implicitWidth + Style.marginS * 2

    anchors.centerIn: parent
    implicitWidth: contentWidth
    implicitHeight: capsuleH

    Rectangle {
        id: visualCapsule
        x: Style.pixelAlignCenter(parent.width, width)
        y: Style.pixelAlignCenter(parent.height, height)
        width: root.contentWidth
        height: root.capsuleH
        radius: Style.radiusL
        color: mouseArea.containsMouse ? Color.mHover : Style.capsuleColor
        border.color: Style.capsuleBorderColor
        border.width: Style.capsuleBorderWidth

        Behavior on color {
            enabled: !Color.isTransitioning
            ColorAnimation {
                duration: Style.animationFast
                easing.type: Easing.InOutQuad
            }
        }

        RowLayout {
            id: row
            anchors.centerIn: parent
            spacing: Style.marginS

            // No active jobs: show empty circle icon
            Loader {
                active: root.jobsCount === 0
                visible: active
                sourceComponent: NIcon {
                    icon: "circle"
                    color: root.hovered
                        ? mainRef ? mainRef.ensureContrast(Color.mOnSurface, Color.mHover)
                                  : Color.mOnSurface
                        : Color.mOnSurface
                }
            }

            // One icon per active job
            Repeater {
                model: root.jobsList

                delegate: Item {
                    required property var modelData

                    readonly property string jobState: modelData.state || ""
                    readonly property string jobColor: mainRef
                        ? mainRef.resolveColor(modelData)
                        : Color.mOnSurface
                    readonly property bool isProgress: jobState === "progress"
                    readonly property real jobProgress: {
                        var t = modelData.total || 0;
                        if (t <= 0) return 0;
                        return Math.max(0, Math.min(1, (modelData.current || 0) / t));
                    }

                    implicitWidth: isProgress ? progressIcon.implicitWidth : normalIcon.implicitWidth
                    implicitHeight: isProgress ? progressIcon.implicitHeight : normalIcon.implicitHeight

                    NIcon {
                        id: normalIcon
                        visible: !parent.isProgress
                        anchors.centerIn: parent
                        icon: mainRef ? mainRef.iconForState(parent.jobState) : "circle"
                        color: root.hovered && mainRef
                            ? mainRef.ensureContrast(parent.jobColor, Color.mHover)
                            : parent.jobColor
                        opacity: mainRef ? mainRef.animOpacity(parent.jobState) : 1.0
                    }

                    ProgressIcon {
                        id: progressIcon
                        visible: parent.isProgress
                        anchors.centerIn: parent
                        progress: parent.jobProgress
                        color: root.hovered && mainRef
                            ? mainRef.ensureContrast(parent.jobColor, Color.mHover)
                            : parent.jobColor
                    }
                }
            }
        }

        MouseArea {
            id: mouseArea
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            cursorShape: Qt.PointingHandCursor

            onClicked: mouse => {
                if (mouse.button === Qt.RightButton) {
                    PanelService.showContextMenu(contextMenu, root, root.screen);
                } else {
                    if (mainRef) mainRef.hidePromptOverlay();
                    if (pluginApi?.openPanel)
                        pluginApi.openPanel(root.screen, root);
                }
            }
        }
    }

    // ── Prompt popup (anchored below bar widget) ────────────────────────────

    PopupWindow {
        id: promptPopup
        visible: mainRef?.promptOverlayVisible ?? false
        color: "transparent"
        anchor.item: visualCapsule
        anchor.rect.x: -(implicitWidth - visualCapsule.width) / 2
        anchor.rect.y: visualCapsule.height + Style.marginS

        implicitWidth: promptCard.width
        implicitHeight: promptCard.height

        Rectangle {
            id: promptCard
            width: promptLayout.implicitWidth + Style.marginM * 2
            height: promptLayout.implicitHeight + Style.marginM * 2
            radius: Style.radiusM
            color: Color.mSurface
            border.color: Style.capsuleBorderColor
            border.width: Style.capsuleBorderWidth

            ColumnLayout {
                id: promptLayout
                anchors.fill: parent
                anchors.margins: Style.marginM
                spacing: Style.marginS

                RowLayout {
                    spacing: Style.marginS
                    NIcon {
                        icon: "question-mark"
                        color: mainRef ? mainRef.colorSpec("colorPromptWaiting").color
                                       : "#C800FF"
                    }
                    NText {
                        text: "Job #" + (mainRef?.activePromptJobId ?? "")
                        color: Color.mOnSurface
                        pointSize: Style.fontSizeM
                        font.weight: Font.Bold
                    }
                }

                NText {
                    text: mainRef?.activePromptText ?? ""
                    color: Color.mOnSurface
                    pointSize: Style.fontSizeS
                    Layout.maximumWidth: 280 * Style.uiScaleRatio
                    wrapMode: Text.WordWrap
                }

                RowLayout {
                    spacing: Style.marginS
                    Layout.topMargin: Style.marginS

                    NButton {
                        id: rejectBtn
                        readonly property string rawColor: mainRef
                            ? mainRef.ensureContrast(
                                mainRef.colorSpec("colorPromptReject").color,
                                Color.mSurface)
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
                            if (mainRef) {
                                mainRef.promptResolve(mainRef.activePromptJobId, false);
                                mainRef.hidePromptOverlay();
                            }
                        }
                    }

                    NButton {
                        id: acceptBtn
                        readonly property string rawColor: mainRef
                            ? mainRef.ensureContrast(
                                mainRef.colorSpec("colorPromptAccept").color,
                                Color.mSurface)
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
                            if (mainRef) {
                                mainRef.promptResolve(mainRef.activePromptJobId, true);
                                mainRef.hidePromptOverlay();
                            }
                        }
                    }
                }
            }
        }
    }

    NPopupContextMenu {
        id: contextMenu

        model: [
            {
                "label": I18n.tr("actions.widget-settings"),
                "action": "widget-settings",
                "icon": "settings"
            }
        ]

        onTriggered: action => {
            contextMenu.close();
            PanelService.closeContextMenu(root.screen);

            if (action === "widget-settings") {
                BarService.openPluginSettings(root.screen, pluginApi.manifest);
            }
        }
    }
}
