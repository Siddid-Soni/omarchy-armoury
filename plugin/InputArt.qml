import QtQuick
import qs.Commons

// Input tile drawing: the three bindable keys with what they do, and the touchpad
// (NumberPad grid lit while it is on, crossed out while the touchpad is off).
Item {
  id: root
  property var keys: null          // config.toml [keys]
  property bool touchpadOn: true
  property string numpad: "unavailable"
  property color fg: Color.foreground
  property color accent: Color.accent
  property string fontFamily: Style.font.family

  function actionLabel(a) {
    return ({ none: "—", open_window: "Armoury", cycle_mode: "Mode", cycle_brightness: "Brightness", cycle_effect: "Effect",
              toggle_numpad: "NumberPad", toggle_music: "Music", command: "Command" })[a] || "—"
  }

  Row {
    id: keyRow
    width: parent.width
    spacing: Style.space(10)
    Repeater {
      model: [{ cap: "ROG", key: "rog" }, { cap: "Fn+F4", key: "aura" }, { cap: "Fn+F5", key: "fan" }]
      Column {
        required property var modelData
        width: (keyRow.width - keyRow.spacing * 2) / 3
        spacing: Style.space(6)
        Rectangle {
          width: parent.width
          height: Style.space(34)
          radius: Style.space(6)
          color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.07)
          border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
          Text { anchors.centerIn: parent; text: modelData.cap; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true }
        }
        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          text: root.keys ? root.actionLabel(root.keys[modelData.key]) : "—"
          color: root.accent; font.family: root.fontFamily; font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }
    }
  }

  // touchpad
  Rectangle {
    id: pad
    anchors.horizontalCenter: parent.horizontalCenter
    anchors.top: keyRow.bottom
    anchors.topMargin: Style.space(16)
    anchors.bottom: parent.bottom
    width: Math.min(parent.width * 0.7, height * 1.75)
    radius: Style.space(8)
    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, root.touchpadOn ? 0.06 : 0.02)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, root.touchpadOn ? 0.25 : 0.1)
    visible: height > Style.space(40)

    Grid {
      anchors.fill: parent
      anchors.margins: Style.space(10)
      anchors.bottomMargin: padLabel.height + Style.space(12)   // room for the label under the grid
      columns: 5
      rows: 4
      spacing: Style.space(4)
      opacity: root.numpad === "on" ? 1 : 0
      Behavior on opacity { NumberAnimation { duration: 200 } }
      Repeater {
        model: 20
        Rectangle {
          width: (parent.width - parent.spacing * 4) / 5
          height: (parent.height - parent.spacing * 3) / 4
          radius: 3
          color: Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.35)
        }
      }
    }
    // touchpad off: struck through
    Rectangle {
      visible: !root.touchpadOn
      anchors.centerIn: parent
      width: Math.sqrt(parent.width * parent.width + parent.height * parent.height) * 0.8
      height: 2
      rotation: -Math.atan2(parent.height, parent.width) * 180 / Math.PI
      color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.35)
    }
    Text {
      id: padLabel
      anchors.bottom: parent.bottom
      anchors.horizontalCenter: parent.horizontalCenter
      anchors.bottomMargin: Style.space(6)
      text: !root.touchpadOn ? "Touchpad off" : root.numpad === "on" ? "NumberPad on" : "Touchpad"
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
  }
}
