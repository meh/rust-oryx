import QtQuick
import qs.Commons

/// A circle-fill progress indicator: a dot in the center with an arc outline
/// that fills clockwise as `progress` goes from 0.0 to 1.0.
/// Sized to match NIcon's default dimensions (font-based, ~Style.fontSizeL).
Canvas {
    id: root

    property real progress: 0.0   // 0.0 .. 1.0
    property color color: Color.mOnSurface

    // Match NIcon's implicit size: fontSizeL * uiScaleRatio, roughly 17-18px.
    readonly property real iconSize: Style.fontSizeL * Style.uiScaleRatio * 1.35
    implicitWidth: iconSize
    implicitHeight: iconSize

    onProgressChanged: requestPaint()
    onColorChanged: requestPaint()

    onPaint: {
        var ctx = getContext("2d");
        ctx.reset();

        var w = width;
        var h = height;
        var cx = w / 2;
        var cy = h / 2;
        var r = Math.min(cx, cy) - 1;
        var lineW = Math.max(1.5, r * 0.18);

        ctx.strokeStyle = root.color;
        ctx.fillStyle = root.color;
        ctx.lineWidth = lineW;

        // Center dot (radius ~20% of the circle)
        ctx.beginPath();
        ctx.arc(cx, cy, r * 0.2, 0, 2 * Math.PI);
        ctx.fill();

        // Track: faint full circle
        ctx.globalAlpha = 0.2;
        ctx.beginPath();
        ctx.arc(cx, cy, r - lineW / 2, 0, 2 * Math.PI);
        ctx.stroke();

        // Progress arc: starts at top (-PI/2), sweeps clockwise
        ctx.globalAlpha = 1.0;
        if (root.progress > 0) {
            var startAngle = -Math.PI / 2;
            var endAngle = startAngle + 2 * Math.PI * Math.min(1.0, root.progress);
            ctx.beginPath();
            ctx.arc(cx, cy, r - lineW / 2, startAngle, endAngle);
            ctx.stroke();
        }
    }
}
