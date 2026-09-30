import QtQuick
import qs.Commons

// Keystone tile drawing: the slot, with the Keystone seated (and glowing red, like its
// light) or lifted out of it.
Item {
  id: root
  property bool inserted: false
  property bool enabled_: true
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property real slotW: Math.min(width * 0.5, height * 0.9)
  readonly property color red: "#ff3040"

  // the slot in the chassis
  Rectangle {
    id: slot
    anchors.horizontalCenter: parent.horizontalCenter
    anchors.bottom: parent.bottom
    anchors.bottomMargin: Style.space(8)
    width: root.slotW
    height: Style.space(14)
    radius: height / 2
    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
  }

  // the Keystone: a key-shaped tag
  Item {
    id: key
    width: root.slotW * 0.8
    height: width * 0.42
    anchors.horizontalCenter: parent.horizontalCenter
    y: root.inserted ? slot.y - height + slot.height * 0.6 : slot.y - height - Style.space(28)
    opacity: root.inserted ? 1 : 0.45
    Behavior on y { NumberAnimation { duration: 350; easing.type: Easing.OutCubic } }
    Behavior on opacity { NumberAnimation { duration: 350 } }

    // glow while seated (the Keystone light)
    Rectangle {
      anchors.centerIn: body
      width: body.width + Style.space(18)
      height: body.height + Style.space(18)
      radius: height / 2
      color: Qt.rgba(1, 0.19, 0.25, root.inserted && root.enabled_ ? 0.18 : 0)
      Behavior on color { ColorAnimation { duration: 350 } }
    }
    Rectangle {
      id: body
      anchors.fill: parent
      radius: height * 0.3
      color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)
      border.color: root.inserted ? root.red : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.4)
      border.width: 2
      // ring hole
      Rectangle {
        anchors.verticalCenter: parent.verticalCenter
        x: parent.height * 0.3
        width: parent.height * 0.4; height: width; radius: width / 2
        color: "transparent"
        border.color: root.inserted ? root.red : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.4)
        border.width: 2
      }
      Text {
        anchors.verticalCenter: parent.verticalCenter
        anchors.right: parent.right
        anchors.rightMargin: parent.height * 0.35
        text: "ROG"
        color: root.inserted ? root.red : root.fg
        opacity: root.inserted ? 1 : 0.6
        font.family: root.fontFamily; font.pixelSize: Math.max(10, parent.height * 0.32); font.bold: true
      }
    }
  }
}
