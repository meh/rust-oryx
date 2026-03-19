import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

ColumnLayout {
    id: specRow

    property string label: ""
    property var spec: ({ type: "static", color: "#888888" })
    /// Whether this field supports animation (false = static only).
    property bool allowAnimation: true
    /// Model for the animation type combo box.
    property var animTypeModel: []

    signal specEdited(var newSpec)

    spacing: Style.marginS / 2

    RowLayout {
        Layout.fillWidth: true
        spacing: Style.marginM

        NText {
            text: specRow.label
            Layout.preferredWidth: 120
        }

        NColorPicker {
            selectedColor: specRow.spec.color || "#888888"
            onColorSelected: c => {
                var s = Object.assign({}, specRow.spec);
                s.color = c.toString();
                specRow.specEdited(s);
            }
        }
    }

    RowLayout {
        visible: specRow.allowAnimation
        Layout.fillWidth: true
        spacing: Style.marginM

        Item { Layout.preferredWidth: 120 }

        NComboBox {
            label: ""
            model: specRow.animTypeModel
            currentKey: specRow.spec.type || "static"
            minimumWidth: 120
            onSelected: key => {
                var s = Object.assign({}, specRow.spec);
                s.type = key;
                if (key !== "static" && !s.periodMs)
                    s.periodMs = key === "breathe" ? 1500 : 2000;
                specRow.specEdited(s);
            }
        }

        NText {
            visible: specRow.spec.type !== "static"
            text: "Period"
            opacity: 0.7
        }
        NTextInput {
            visible: specRow.spec.type !== "static"
            placeholderText: specRow.spec.type === "breathe"
                ? "1500" : "2000"
            text: specRow.spec.periodMs
                ? String(Math.round(specRow.spec.periodMs))
                : ""
            Layout.preferredWidth: 80
            onEditingFinished: {
                var v = parseInt(text);
                if (v > 0) {
                    var s = Object.assign({}, specRow.spec);
                    s.periodMs = v;
                    specRow.specEdited(s);
                }
            }
        }
        NText {
            visible: specRow.spec.type !== "static"
            text: "ms"
            opacity: 0.5
        }
    }
}
