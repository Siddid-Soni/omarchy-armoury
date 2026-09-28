import QtQuick
import qs.Commons
import qs.Ui

// Label · slider · value, for integer settings. Emits `committed` on release.
Column {
  id: root
  property string label: ""
  property string unit: ""
  property real value: 0
  property real minimum: 0
  property real maximum: 100
  property real step: 1
  property bool usable: true
  property bool unset: false          // shows "default" until moved
  property color fg: Color.foreground
  property string fontFamily: Style.font.family
  signal committed(real value)

  width: parent ? parent.width : 0
  spacing: Style.space(4)

  Row {
    width: parent.width
    Text {
      text: root.label
      color: root.fg
      opacity: 0.7
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      width: parent.width - valueText.width
    }
    Text {
      id: valueText
      text: root.unset && !slider.dragging ? "default" : Math.round(slider.liveValue) + (root.unit ? " " + root.unit : "")
      color: root.fg
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
    }
  }

  PanelSlider {
    id: slider
    width: parent.width
    value: root.value
    minimum: root.minimum
    maximum: root.maximum
    step: root.step
    integer: true
    enabled: root.usable
    opacity: root.usable ? 1 : 0.5
    trackColor: Style.selectedFillFor(root.fg, Color.accent)
    fillColor: root.fg
    knobColor: root.fg
    onReleased: function(v) { root.unset = false; root.committed(Math.round(v)) }
  }
}
