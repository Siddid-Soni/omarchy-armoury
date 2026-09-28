import QtQuick
import qs.Commons

// "Label ........ value" line.
Row {
  id: root
  property string label: ""
  property string value: ""
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  width: parent ? parent.width : 0
  spacing: Style.space(8)

  Text { id: l; text: root.label; color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
  Item { width: Math.max(0, root.width - l.implicitWidth - v.implicitWidth - root.spacing * 2); height: 1 }
  Text { id: v; text: root.value; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
}
