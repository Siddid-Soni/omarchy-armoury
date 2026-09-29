import QtQuick
import Quickshell
import Quickshell.Wayland
import qs.Commons
import qs.Ui

// Armoury window (panel kind): header strip, dashboard tiles, detail pages.
// Summoned by the popup's "Open Armoury ›" and the ROG key.
Item {
  id: root

  property var shell: null
  property var manifest: null
  property bool opened: false
  property string page: ""

  readonly property var snap: armoury.snap
  readonly property color fg: Color.foreground
  readonly property color dim: Qt.darker(Color.foreground, 1.5)
  readonly property color surface: Color.popups.background
  readonly property color border: Color.popups.border
  readonly property string fontFamily: Style.font.family

  function open(payloadJson) {
    var p = {}
    try { p = JSON.parse(payloadJson || "{}") || {} } catch (e) {}
    var pg = p.page ? String(p.page) : ""
    root.page = pageTitle(pg) !== "" ? pg : ""
    root.opened = true
  }
  function close() { root.opened = false; root.page = "" }
  // armouryd asks this so the ROG key can toggle the window
  function isOpen() { return root.opened ? "open" : "closed" }
  function dismiss() {
    if (root.shell && typeof root.shell.hide === "function") root.shell.hide((root.manifest && root.manifest.id) || "asus.armoury")
    else close()
  }

  function modeLabel(p) { return p === "quiet" ? "Silent" : p === "balanced" ? "Balanced" : p === "performance" ? "Turbo" : "—" }
  function gpuLabel(m) { return m === "AsusMuxDgpu" ? "Ultimate" : (m || "—") }

  ArmouryClient { id: armoury }

  PanelWindow {
    visible: root.opened
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    exclusionMode: ExclusionMode.Ignore
    WlrLayershell.namespace: "asus-armoury"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive

    // No dim behind the card; a click outside it still closes the window.
    MouseArea { anchors.fill: parent; onClicked: root.dismiss() }

    Item {
      id: keys
      anchors.fill: parent
      focus: true
      Keys.onEscapePressed: root.page !== "" ? root.page = "" : root.dismiss()

      Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - Style.space(48), Style.space(1040))
        height: Math.min(parent.height - Style.space(48), Style.space(720))
        radius: Style.space(10)
        color: Qt.rgba(root.surface.r, root.surface.g, root.surface.b, 1)   // opaque: nothing dims or shows through
        border.color: root.border
        border.width: 1

        MouseArea { anchors.fill: parent; onClicked: {} }   // keep clicks off the scrim

        Column {
          anchors.fill: parent
          anchors.margins: Style.space(20)
          spacing: Style.space(16)

          // ---------- Header strip ----------
          Row {
            id: header
            width: parent.width
            spacing: Style.space(14)

            Text {
              text: root.page === "" ? "󰢮" : "‹"
              color: root.fg
              font.family: root.fontFamily
              font.pixelSize: Style.font.display
              anchors.verticalCenter: parent.verticalCenter
              MouseArea { anchors.fill: parent; enabled: root.page !== ""; onClicked: root.page = "" }
            }

            Column {
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(2)
              Text {
                text: root.page === "" ? (root.snap && root.snap.model ? root.snap.model.split("_")[0] : "Armoury") : root.pageTitle(root.page)
                color: root.fg
                font.family: root.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }
              Text {
                text: (!armoury.online ? "armouryd not running"
                  : root.modeLabel(root.snap.perf.profile) + " · GPU " + root.gpuLabel(root.snap.gpu.mode)
                    + (root.snap.gpu.pending ? " → " + root.gpuLabel(root.snap.gpu.pending) + " after reboot" : "")
                    + (root.snap.keystone ? " · Keystone" : "")
                    + (armoury.active ? "" : " · watching (G-Helper in control)")).toUpperCase()
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                font.bold: true
                font.letterSpacing: 1.2
              }
            }

            Item { width: Math.max(0, header.width - header.children[0].width - header.children[1].width - controlBtn.width - closeBtn.width - header.spacing * 4); height: 1 }

            Button {
              id: controlBtn
              visible: armoury.online
              anchors.verticalCenter: parent.verticalCenter
              text: armoury.active ? "Hand back to G-Helper" : "Take over"
              fontSize: Style.font.bodySmall
              foreground: root.fg
              fontFamily: root.fontFamily
              bordered: true
              onClicked: armoury.run({ cmd: armoury.active ? "handback" : "takeover" })
            }
            Button {
              id: closeBtn
              anchors.verticalCenter: parent.verticalCenter
              text: "✕"
              foreground: root.fg
              fontFamily: root.fontFamily
              onClicked: root.dismiss()
            }
          }

          Text {
            visible: armoury.lastError !== ""
            width: parent.width
            wrapMode: Text.WordWrap
            text: armoury.lastError
            color: Color.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }

          PanelSeparator { foreground: root.fg }

          // ---------- Body ----------
          Item {
            width: parent.width
            height: card.height - header.height - Style.space(20) * 2 - Style.space(16) * 3 - (armoury.lastError !== "" ? Style.space(24) : 0)

            // Dashboard tiles
            Grid {
              visible: root.page === ""
              anchors.fill: parent
              columns: 3
              spacing: Style.space(12)
              readonly property real tileW: (width - spacing * 2) / 3
              readonly property real tileH: (height - spacing) / 2

              Tile {
                pageId: "fans"; icon: "󰈐"; title: "Fans"
                lines: root.snap && root.snap.perf ? [
                  "CPU " + (root.snap.perf.cpu_fan_rpm || 0) + " rpm · GPU " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm",
                  root.snap.perf.cpu_temp_c ? "CPU " + Math.round(root.snap.perf.cpu_temp_c) + "°C" : "",
                  "Curves per mode"
                ] : []
              }
              Tile {
                pageId: "cpugpu"; icon: "󰍛"; title: "CPU / GPU"
                lines: root.snap ? [
                  root.modeLabel(root.snap.perf.profile) + " mode",
                  "dGPU " + (root.snap.gpu.dgpu_active === true ? "awake" : root.snap.gpu.dgpu_active === false ? "asleep" : "—"),
                  root.snap.perf.undervolt ? (root.snap.perf.undervolt.unlocked ? "Undervolt available" : "Undervolt locked by BIOS") : "Power limits, EPP, boost"
                ] : []
              }
              Tile {
                pageId: "lighting"; icon: "󰌌"; title: "Lighting"
                lines: root.snap && root.snap.lighting ? [
                  "Keyboard " + ["off", "low", "medium", "high"][root.snap.lighting.brightness || 0],
                  "Effects, colours, zones",
                  root.snap.lighting.on_ac === false ? "On battery" : "On AC"
                ] : []
              }
              Tile {
                pageId: "battery"; icon: "󰁹"; title: "Battery"
                lines: root.snap && root.snap.battery_info ? [
                  (root.snap.battery_info.capacity || 0) + "% · limit " + (root.snap.battery_info.charge_limit || "—") + "%",
                  root.snap.battery_info.health_pct ? "Health " + Math.round(root.snap.battery_info.health_pct) + "%" : "",
                  root.snap.battery_info.status || ""
                ] : []
              }
              Tile {
                pageId: "input"; icon: "󰘳"; title: "Input"
                lines: root.snap ? [
                  "ROG key · Fn+F4 · Fn+F5",
                  "Touchpad " + (root.snap.system && root.snap.system.touchpad === false ? "off" : "on"),
                  "Keyboard idle dim"
                ] : []
              }
              Tile {
                pageId: "system"; icon: "󰒓"; title: "System"
                lines: root.snap && root.snap.system ? [
                  "Sleep " + (root.snap.system.mem_sleep || "—"),
                  "Overdrive " + (root.snap.system.panel_od ? "on" : "off"),
                  "Refresh, lid, auto-switch"
                ] : []
              }
            }

            // Detail pages
            Loader {
              anchors.fill: parent
              active: root.page !== ""
              sourceComponent: root.page === "fans" ? fansPage
                : root.page === "cpugpu" ? cpuGpuPage
                : root.page === "lighting" ? lightingPage
                : root.page === "battery" ? batteryPage
                : root.page === "input" ? inputPage
                : root.page === "system" ? systemPage : null
            }
          }
        }
      }
    }
  }

  function pageTitle(p) {
    return ({ fans: "Fans", cpugpu: "CPU / GPU", lighting: "Lighting", battery: "Battery", input: "Input", system: "System" })[p] || ""
  }

  Component { id: fansPage; FansPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: cpuGpuPage; CpuGpuPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: lightingPage; LightingPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: batteryPage; BatteryPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: inputPage; InputPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: systemPage; SystemPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }

  component Tile: Rectangle {
    property string pageId: ""
    property string icon: ""
    property string title: ""
    property var lines: []
    width: parent.tileW
    height: parent.tileH
    radius: Style.space(8)
    color: hover.containsMouse ? Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.10) : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.05)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)

    Column {
      anchors.fill: parent
      anchors.margins: Style.space(18)
      spacing: Style.space(8)
      Row {
        spacing: Style.space(10)
        Text { text: icon; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.display }
        Text { text: title; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.title; font.bold: true; anchors.verticalCenter: parent.verticalCenter }
      }
      Repeater {
        model: lines
        Text {
          required property var modelData
          visible: String(modelData) !== ""
          text: String(modelData)
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
          elide: Text.ElideRight
          width: parent.width
        }
      }
    }
    Text {
      anchors.right: parent.right
      anchors.bottom: parent.bottom
      anchors.margins: Style.space(14)
      text: "›"
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.title
    }
    MouseArea {
      id: hover
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: root.page = pageId
    }
  }
}
