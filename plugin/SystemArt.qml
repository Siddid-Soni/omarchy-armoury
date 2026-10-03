import QtQuick
import qs.Commons

// System tile drawing: the panel with its refresh rate for the current power source,
// and the AC / battery rules side by side (the current source highlighted).
Item {
  id: root
  property var sys: null           // config.toml [system]
  property var snap: null
  property color fg: Color.foreground
  property color accent: Color.accent
  property string fontFamily: Style.font.family

  readonly property bool onAc: !snap || !snap.lighting || snap.lighting.on_ac !== false
  function modeLabel(p) { return p === "quiet" ? "Silent" : p === "balanced" ? "Balanced" : p === "performance" ? "Turbo" : p === "manual" ? "Manual" : "—" }
  function hz(v) { return v ? Math.round(v) + " Hz" : "—" }

  // the panel
  Rectangle {
    id: panel
    anchors.horizontalCenter: parent.horizontalCenter
    y: 0
    width: Math.min(parent.width * 0.62, (parent.height - rules.height - Style.space(14)) * 1.6)
    height: width / 1.6
    radius: Style.cornerRadius
    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.05)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
    border.width: 2
    visible: height > Style.space(30)
    Column {
      anchors.centerIn: parent
      spacing: Style.space(2)
      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: root.hz(root.sys ? (root.onAc ? root.sys.refresh_ac : root.sys.refresh_battery) : null)
        color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.title; font.bold: true
      }
      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: (root.snap && root.snap.system && root.snap.system.panel_od ? "Overdrive" : "No overdrive")
        color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
      }
      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: (root.snap && root.snap.system ? (root.snap.system.mem_sleep || "—") : "—") + " sleep"
        color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
      }
    }
    // stand
    Rectangle { anchors.top: parent.bottom; anchors.horizontalCenter: parent.horizontalCenter; width: parent.width * 1.12; height: 3; radius: Math.min(2, Style.cornerRadius); color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.3) }
  }

  // AC and battery rules
  Row {
    id: rules
    anchors.bottom: parent.bottom
    width: parent.width
    spacing: Style.space(10)
    Repeater {
      model: [{ src: "ac", icon: "󰚥", label: "AC" }, { src: "battery", icon: "󰁹", label: "Battery" }]
      Rectangle {
        required property var modelData
        readonly property bool current: (modelData.src === "ac") === root.onAc
        readonly property bool hasOd: !!root.sys && root.sys["panel_od_" + modelData.src] !== undefined && root.sys["panel_od_" + modelData.src] !== null
        width: (rules.width - rules.spacing) / 2
        height: Style.space(hasOd ? 64 : 52)
        radius: Style.cornerRadius
        color: current ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.14) : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.04)
        border.color: current ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.5) : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)
        Row {
          anchors.centerIn: parent
          spacing: Style.space(8)
          Text { text: modelData.icon; color: current ? root.accent : root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.title; anchors.verticalCenter: parent.verticalCenter }
          Column {
            anchors.verticalCenter: parent.verticalCenter
            Text { text: modelData.label; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.caption; font.bold: true }
            Text {
              text: root.sys ? root.modeLabel(root.sys["profile_" + modelData.src]) + " · " + root.hz(root.sys["refresh_" + modelData.src]) : "—"
              color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.caption
            }
            Text {
              visible: hasOd
              text: "Overdrive " + (root.sys && root.sys["panel_od_" + modelData.src] ? "on" : "off")
              color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.caption
            }
          }
        }
      }
    }
  }
}
