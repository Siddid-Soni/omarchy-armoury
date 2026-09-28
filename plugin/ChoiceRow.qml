import QtQuick
import qs.Commons
import qs.Ui

// A labelled row of mutually exclusive buttons. options: [{label, value}].
Column {
  id: root
  property string label: ""
  property var options: []
  property var value: undefined
  property bool usable: true
  property color fg: Color.foreground
  property string fontFamily: Style.font.family
  signal chosen(var value)

  width: parent ? parent.width : 0
  spacing: Style.space(6)

  Text {
    visible: root.label !== ""
    text: root.label
    color: root.fg
    opacity: 0.7
    font.family: root.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  Row {
    id: row
    width: parent.width
    spacing: Style.space(6)
    Repeater {
      model: root.options
      Button {
        required property var modelData
        width: Math.min(Style.space(160), (row.width - row.spacing * Math.max(0, root.options.length - 1)) / Math.max(1, root.options.length))
        text: String(modelData.label)
        fontSize: Style.font.bodySmall
        foreground: root.fg
        fontFamily: root.fontFamily
        bordered: true
        enabled: root.usable
        opacity: root.usable ? 1 : 0.5
        active: root.value !== undefined && String(root.value) === String(modelData.value)
        onClicked: root.chosen(modelData.value)
      }
    }
  }
}
