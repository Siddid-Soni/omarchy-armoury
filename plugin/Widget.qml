import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import qs.Commons
import qs.Ui

// Bar icon + quick popup for armouryd (ASUS ROG control).
// Everything goes through ArmouryClient; no hardware access here.
Panel {
  id: root
  moduleName: "io.github.siddid-soni.armoury"
  ipcTarget: "io.github.siddid-soni.armoury"
  manageIpc: false

  // The bar sizes widgets from their implicit size (as omarchy.power does).
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  ArmouryClient { id: client }

  readonly property var snap: client.snap
  readonly property bool online: client.online
  readonly property bool active: client.active
  readonly property color fg: bar ? bar.foreground : Color.foreground
  readonly property color dim: Qt.darker(fg, 1.5)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  readonly property var modes: [
    { id: "quiet", label: "Silent", icon: "󰾆" },
    { id: "balanced", label: "Balanced", icon: "󰾅" },
    { id: "performance", label: "Turbo", icon: "󰓅" },
    { id: "manual", label: "Manual", icon: "󰈐" }
  ]
  readonly property var gpuModes: [
    { id: "Integrated", label: "Integrated" },
    { id: "Hybrid", label: "Hybrid" },
    { id: "AsusMuxDgpu", label: "Ultimate" }
  ]

  readonly property string profile: snap && snap.perf ? (snap.perf.mode || snap.perf.profile || "") : ""
  readonly property var modeInfo: {
    for (var i = 0; i < modes.length; i++) if (modes[i].id === profile) return modes[i]
    return { id: "", label: "—", icon: "󰢮" }
  }
  readonly property int cpuTemp: snap && snap.perf && snap.perf.cpu_temp_c ? Math.round(snap.perf.cpu_temp_c) : -1
  property string gpuConfirm: ""   // GPU mode awaiting a second click
  property string gpuMessage: ""

  function setting(name, fallback) {
    var s = root.settings || {}
    return s[name] !== undefined ? s[name] : fallback
  }

  function openWindow() {
    root.close()
    Quickshell.execDetached(["omarchy-shell", "shell", "summon", "io.github.siddid-soni.armoury", "{}"])
  }

  function setMode(id) { if (root.active) client.run({ cmd: "set_profile", profile: id }) }

  function gpuClick(id) {
    if (!root.active || !snap || !snap.gpu || snap.gpu.mode === id) return
    if (root.gpuConfirm !== id) {
      client.call({ cmd: "plan_gpu_mode", mode: id }, function(r) {
        if (r.ok) { root.gpuConfirm = id; root.gpuMessage = "Click again to switch (needs a reboot)" }
        else { root.gpuConfirm = ""; root.gpuMessage = r.error || "Not possible now" }
      })
      return
    }
    root.gpuConfirm = ""
    client.call({ cmd: "set_gpu_mode", mode: id }, function(r) { root.gpuMessage = r.ok ? r.data.message : (r.error || "Failed") })
  }

  function setToggle(which, on) { client.run({ cmd: "set_toggle", toggle: which, on: on }) }

  // ---------- keyboard idle dim (Omarchy has no hypridle; its shell uses IdleMonitor too) ----------
  readonly property int idleSeconds: Number(setting("kbdIdleSeconds", 60))
  IdleMonitor {
    id: idle
    enabled: root.active && root.idleSeconds > 0
    timeout: Math.max(1, root.idleSeconds)
    respectInhibitors: true
    onIsIdleChanged: client.run({ cmd: isIdle ? "kbd_idle" : "kbd_resume" })
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    readonly property bool showTemp: String(root.setting("showTemperature", true)) === "true" && root.cpuTemp >= 0 && !vertical
    text: !root.online ? "󰢮" : (showTemp ? root.modeInfo.icon + " " + root.cpuTemp + "°" : root.modeInfo.icon)
    slotSize: Style.bar.iconSlot * (showTemp ? 2 : 1)
    tooltipText: root.online ? "Armoury: " + root.modeInfo.label + " mode" : "Armoury: armouryd not running"
    onPressed: function(b) { root.toggle() }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(14)

        // ---------- Hero ----------
        Item {
          width: parent.width
          implicitHeight: Math.max(heroIcon.implicitHeight, heroText.implicitHeight)

          Text {
            id: heroIcon
            text: "󰢮"
            color: root.fg
            font.family: root.fontFamily
            font.pixelSize: Style.font.display
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
          }

          Column {
            id: heroText
            anchors.left: heroIcon.right
            anchors.leftMargin: Style.space(14)
            anchors.right: keystone.left
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(2)

            Text {
              text: root.snap && root.snap.model ? root.snap.model.split("_")[0] : "Armoury"
              color: root.fg
              font.family: root.fontFamily
              font.pixelSize: Style.font.title
              font.bold: true
              elide: Text.ElideRight
              width: parent.width
            }
            Text {
              text: (!root.online ? "armouryd not running"
                : root.modeInfo.label + " mode" + (root.cpuTemp >= 0 ? " · CPU " + root.cpuTemp + "°C" : "")
                  + (root.snap.perf.cpu_fan_rpm ? " · " + root.snap.perf.cpu_fan_rpm + " rpm" : "")).toUpperCase()
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              font.bold: true
              font.letterSpacing: 1.2
              elide: Text.ElideRight
              width: parent.width
            }
          }

          Text {
            id: keystone
            visible: root.online && root.snap.keystone === true
            text: "⬢ Keystone"
            color: root.fg
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
          }
        }

        // ---------- Offline banner ----------
        Rectangle {
          visible: !root.online
          width: parent.width
          implicitHeight: bannerRow.implicitHeight + Style.space(16)
          radius: Style.space(6)
          color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)

          Row {
            id: bannerRow
            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: Style.space(10)
            spacing: Style.space(10)

            Text {
              width: parent.width
              anchors.verticalCenter: parent.verticalCenter
              wrapMode: Text.WordWrap
              text: "armouryd is not running. First time? Run install.sh in the plugin folder (see the README). Otherwise: systemctl --user start armouryd"
              color: root.fg
              font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall
            }
          }
        }

        // ---------- Mode ----------
        Column {
          visible: root.online
          width: parent.width
          spacing: Style.space(8)
          PanelSectionHeader { text: "MODE"; foreground: root.fg; fontFamily: root.fontFamily }
          Row {
            id: modeRow
            width: parent.width
            spacing: Style.space(6)
            Repeater {
              model: root.modes
              Button {
                required property var modelData
                width: (modeRow.width - modeRow.spacing * (root.modes.length - 1)) / root.modes.length
                iconText: modelData.icon
                iconSize: Style.font.title
                text: modelData.label
                fontSize: Style.font.bodySmall
                foreground: root.fg
                fontFamily: root.fontFamily
                bordered: true
                enabled: root.active
                opacity: root.active ? 1 : 0.5
                active: root.profile === modelData.id
                onClicked: root.setMode(modelData.id)
              }
            }
          }
        }

        // ---------- GPU ----------
        Column {
          visible: root.online && root.snap.gpu && root.snap.gpu.mode
          width: parent.width
          spacing: Style.space(8)
          PanelSectionHeader {
            text: "GPU" + (root.snap && root.snap.gpu && root.snap.gpu.pending ? "  ·  " + root.snap.gpu.pending + " AFTER REBOOT" : "")
            foreground: root.fg
            fontFamily: root.fontFamily
          }
          Row {
            id: gpuRow
            width: parent.width
            spacing: Style.space(6)
            Repeater {
              model: root.gpuModes
              Button {
                required property var modelData
                visible: root.snap && root.snap.gpu && (root.snap.gpu.supported || []).indexOf(modelData.id) >= 0
                width: (gpuRow.width - gpuRow.spacing * 2) / 3
                text: root.gpuConfirm === modelData.id ? "Confirm?" : modelData.label
                fontSize: Style.font.bodySmall
                foreground: root.fg
                fontFamily: root.fontFamily
                bordered: true
                enabled: root.active
                opacity: root.active ? 1 : 0.5
                active: root.snap && root.snap.gpu && root.snap.gpu.mode === modelData.id
                onClicked: root.gpuClick(modelData.id)
              }
            }
          }
          Text {
            visible: root.gpuMessage !== ""
            width: parent.width
            wrapMode: Text.WordWrap
            text: root.gpuMessage
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        // ---------- Quick toggles ----------
        Column {
          visible: root.online
          width: parent.width
          spacing: Style.space(8)
          PanelSectionHeader { text: "QUICK TOGGLES"; foreground: root.fg; fontFamily: root.fontFamily }
          Grid {
            id: grid
            columns: 3
            width: parent.width
            spacing: Style.space(6)
            readonly property real cell: (width - spacing * 2) / 3
            readonly property var sys: root.snap ? (root.snap.system || {}) : {}
            readonly property int kbd: root.snap && root.snap.lighting ? (root.snap.lighting.brightness || 0) : 0

            Toggle {
              icon: "󰌌"; label: "Keyboard " + ["off", "low", "med", "high"][grid.kbd]
              on: grid.kbd > 0
              onClicked: client.run({ cmd: "set_brightness", level: (grid.kbd + 1) % 4 })
            }
            Toggle {
              icon: "󰟸"; label: "Touchpad"
              on: grid.sys.touchpad !== false
              onClicked: root.setToggle("touchpad", !on)
            }
            Toggle {
              icon: "󰎠"; label: "NumberPad"
              visible: grid.sys.numpad !== undefined
              on: grid.sys.numpad === "on"
              usable: root.active && grid.sys.numpad !== "unavailable"
              onClicked: client.run({ cmd: "set_numpad", on: !on })
            }
            Toggle {
              icon: "󰌢"; label: "Lid awake (AC)"
              on: grid.sys.clamshell === true
              onClicked: root.setToggle("clamshell", !on)
            }
            Toggle {
              icon: "󰍹"; label: "Overdrive"
              visible: grid.sys.panel_od !== undefined && grid.sys.panel_od !== null
              on: grid.sys.panel_od === true
              onClicked: root.setToggle("panel_od", !on)
            }
            Toggle {
              icon: "󰕾"; label: "Boot sound"
              visible: grid.sys.boot_sound !== undefined && grid.sys.boot_sound !== null
              on: grid.sys.boot_sound === true
              onClicked: root.setToggle("boot_sound", !on)
            }
            Toggle {
              icon: "󰝚"; label: "Music"
              readonly property string st: root.snap && root.snap.lighting && root.snap.lighting.music ? root.snap.lighting.music : "unavailable"
              on: st === "on"
              usable: root.active && st !== "unavailable"
              onClicked: client.run({ cmd: "set_music", on: !on })
            }
          }
        }

        // ---------- Status ----------
        Column {
          visible: root.online
          width: parent.width
          spacing: Style.spacing.labelGap
          InfoPair {
            label: "Battery"
            value: {
              var b = root.snap ? (root.snap.battery_info || {}) : {}
              return (b.capacity !== undefined && b.capacity !== null ? b.capacity + "%" : "—")
                + (b.charge_limit ? " · limit " + b.charge_limit + "%" : "")
                + (b.draw_w && b.draw_w > 0.5 ? " · " + b.draw_w.toFixed(1) + " W" : "")
            }
          }
          InfoPair {
            label: "Fans"
            value: root.snap && root.snap.perf && root.snap.perf.cpu_fan_rpm !== undefined
              ? "CPU " + (root.snap.perf.cpu_fan_rpm || 0) + " · GPU " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm" : "—"
          }
          InfoPair {
            label: "dGPU"
            value: root.snap && root.snap.gpu ? (root.snap.gpu.dgpu_active === true ? "awake" : (root.snap.gpu.dgpu_active === false ? "asleep" : "—")) : "—"
          }
        }

        Text {
          visible: client.lastError !== ""
          width: parent.width
          wrapMode: Text.WordWrap
          text: client.lastError
          color: Color.urgent
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
        }

        PanelSeparator { foreground: root.fg }

        Row {
          width: parent.width
          spacing: Style.space(8)
          Item { width: parent.width - openBtn.width - parent.spacing; height: 1 }
          Button {
            id: openBtn
            text: "Open Armoury ›"
            fontSize: Style.font.bodySmall
            foreground: root.fg
            fontFamily: root.fontFamily
            bordered: true
            onClicked: root.openWindow()
          }
        }
      }
    }
  }

  component Toggle: Button {
    property string icon: ""
    property string label: ""
    property bool on: false
    property bool usable: true
    width: grid.cell
    iconText: icon
    iconSize: Style.font.title
    text: label
    fontSize: Style.font.caption
    foreground: root.fg
    fontFamily: root.fontFamily
    bordered: true
    active: on
    enabled: usable && root.active
    opacity: enabled ? 1 : 0.45
  }

  component InfoPair: Row {
    property string label: ""
    property string value: ""
    width: parent.width
    spacing: Style.space(8)
    Text { text: label; color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
    Item { width: Math.max(0, parent.width - parent.children[0].implicitWidth - parent.children[2].implicitWidth - parent.spacing * 2); height: 1 }
    Text { text: value; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
  }
}
