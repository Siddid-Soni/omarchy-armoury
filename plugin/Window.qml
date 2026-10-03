import QtQuick
import Quickshell
import Quickshell.Io
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
    if (pg === "fans" || pg === "cpugpu") pg = "manual"   // pages merged into Manual
    root.page = pageTitle(pg) !== "" ? pg : ""
    root.opened = true
  }
  function close() { root.opened = false; root.page = "" }
  // armouryd asks this so the ROG key can toggle the window
  function isOpen() { return root.opened ? "open" : "closed" }
  function dismiss() {
    if (root.shell && typeof root.shell.hide === "function") root.shell.hide((root.manifest && root.manifest.id) || "io.github.siddid-soni.armoury")
    else close()
  }

  function modeLabel(p) { return p === "quiet" ? "Silent" : p === "balanced" ? "Balanced" : p === "performance" ? "Turbo" : p === "manual" ? "Manual" : "—" }
  function gpuLabel(m) { return m === "AsusMuxDgpu" ? "Ultimate" : (m || "—") }
  function effectLabel(m) { return String(m).split("_").map(function(w) { return w.charAt(0).toUpperCase() + w.slice(1) }).join(" ") }
  function ksSummary(a) {
    if (!a) return "nothing"
    var parts = []
    if (a.mode) parts.push(modeLabel(a.mode))
    if (a.light && a.light !== "unchanged") parts.push(a.light === "previous" ? "previous lighting" : a.light === "music" ? "Music" : effectLabel(a.light))
    if (a.command) parts.push("command")
    if (a.lock) parts.push("lock")
    return parts.length ? parts.join(", ") : "nothing"
  }

  // config.toml (music colours for the lighting preview, Keystone summary)
  property var cfg: ({})
  function reloadCfg() { armoury.call({ cmd: "config" }, function(r) { if (r.ok) root.cfg = r.data }) }
  onOpenedChanged: if (opened) { reloadCfg(); borderProc.running = true; scaleProc.running = true }
  onPageChanged: if (page === "") reloadCfg()
  readonly property color accent: Color.accent

  ArmouryClient { id: armoury }

  // Hyprland's general:border_size, so the card's frame matches the tiled windows'
  property int hyprBorder: 2
  Process {
    id: borderProc
    command: ["hyprctl", "-j", "getoption", "general:border_size"]
    stdout: StdioCollector {
      onStreamFinished: { try { var n = Number(JSON.parse(text).int); if (isFinite(n) && n >= 0) root.hyprBorder = n } catch (e) {} }
    }
  }
  // The focused monitor's real scale. Qt rounds a fractional scale up (1.6 -> 2) and
  // Hyprland shrinks the buffer, so Screen.devicePixelRatio is not the physical grid.
  property real monitorScale: 1
  Process {
    id: scaleProc
    command: ["hyprctl", "-j", "monitors"]
    stdout: StdioCollector {
      onStreamFinished: {
        try {
          var ms = JSON.parse(text), m = ms.filter(function(x) { return x.focused })[0] || ms[0]
          var n = Number(m.scale); if (isFinite(n) && n > 0) root.monitorScale = n
        } catch (e) {}
      }
    }
  }

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
        // Geometry and border snapped to whole monitor pixels: at a fractional scale
        // (1.6) a centred card lands between pixels and each side rounds differently.
        readonly property real dpr: root.monitorScale
        function snap(v) { return Math.round(v * dpr) / dpr }
        width: snap(Math.min(parent.width - Style.space(48), Style.space(1040)))
        height: snap(Math.min(parent.height - Style.space(48), Style.space(720)))
        x: snap((parent.width - width) / 2)
        y: snap((parent.height - height) / 2)
        radius: Style.cornerRadius
        color: Qt.rgba(root.surface.r, root.surface.g, root.surface.b, 1)   // opaque: nothing dims or shows through
        border.color: root.border
        border.width: root.hyprBorder > 0 ? Math.max(1, Math.round(root.hyprBorder * dpr)) / dpr : 0

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

            // one child (the spacer below sizes itself from children[0] and [1]):
            // the logo on the dashboard, a back arrow on pages
            Item {
              anchors.verticalCenter: parent.verticalCenter
              width: root.page === "" ? logo.width : back.width
              height: Math.max(logo.height, back.height)

              RogLogo {
                id: logo
                visible: root.page === ""
                color: root.fg
                implicitHeight: Style.font.display * 0.9
                anchors.verticalCenter: parent.verticalCenter
              }

              Text {
                id: back
                visible: root.page !== ""
                text: "‹"
                color: root.fg
                font.family: root.fontFamily
                font.pixelSize: Style.font.display
                anchors.verticalCenter: parent.verticalCenter
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.page = "" }
              }
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
                  : root.modeLabel(root.snap.perf.mode || root.snap.perf.profile) + " · GPU " + root.gpuLabel(root.snap.gpu.mode)
                    + (root.snap.gpu.pending ? " → " + root.gpuLabel(root.snap.gpu.pending) + " after reboot" : "")
                    + (root.snap.keystone ? " · Keystone" : "")
                    + (armoury.active ? "" : " · read-only")).toUpperCase()
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                font.bold: true
                font.letterSpacing: 1.2
              }
            }

            Item {
              width: Math.max(0, header.width - header.children[0].width - header.children[1].width - closeBtn.width - header.spacing * 3)
              height: 1
              anchors.verticalCenter: parent.verticalCenter
              Text {
                visible: !!(root.manifest && root.manifest.version)
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: "v" + (root.manifest ? root.manifest.version : "")
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
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
                pageId: "manual"; icon: "󰈐"; title: "Manual"
                lines: root.snap && root.snap.perf ? [
                  root.snap.perf.mode === "manual" ? "In use: " + (root.snap.perf.manual_profile || "—") : "Profile: " + (root.snap.perf.manual_profile || "none yet"),
                  "Fan curves, power limits, GPU"
                ] : []
                Column {
                  id: perfGraphs
                  width: parent.width
                  spacing: Style.space(10)
                  // the graphs share the space under the text (power only when the firmware reports it)
                  readonly property bool hasPower: armoury.hist.power.some(function(v) { return v > 0 })
                  readonly property int count: hasPower ? 3 : 2
                  readonly property real graphH: Math.max(Style.space(14), (parent.height - spacing * (count - 1)) / count - Style.space(20))
                  Sparkline {
                    shown: root.opened
                    graphHeight: perfGraphs.graphH
                    label: "CPU temperature"; fg: root.fg; fontFamily: root.fontFamily
                    value: root.snap && root.snap.perf && root.snap.perf.cpu_temp_c ? Math.round(root.snap.perf.cpu_temp_c) + " °C" : "—"
                    series: [{ values: armoury.hist.cpuTemp, color: root.accent }]
                    minSpan: 15; floor: 0
                  }
                  Sparkline {
                    shown: root.opened
                    graphHeight: perfGraphs.graphH
                    label: "Fans (CPU · GPU)"; fg: root.fg; fontFamily: root.fontFamily
                    value: root.snap && root.snap.perf ? (root.snap.perf.cpu_fan_rpm || 0) + " · " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm" : "—"
                    series: [{ values: armoury.hist.cpuFan, color: root.accent }, { values: armoury.hist.gpuFan, color: root.dim }]
                    minSpan: 1500; floor: 0
                  }
                  Sparkline {
                    shown: root.opened
                    visible: perfGraphs.hasPower
                    graphHeight: perfGraphs.graphH
                    label: "Power draw"; fg: root.fg; fontFamily: root.fontFamily
                    value: root.snap && root.snap.perf && root.snap.perf.power_draw_w ? root.snap.perf.power_draw_w.toFixed(1) + " W" : "—"
                    series: [{ values: armoury.hist.power, color: root.accent }]
                    minSpan: 10; floor: 0
                  }
                }
              }
              Tile {
                pageId: "lighting"; icon: "󰌌"; title: "Lighting"
                readonly property var l: root.snap ? (root.snap.lighting || {}) : {}
                lines: root.snap ? [
                  (l.music === "on" ? "Music" : l.effect ? root.effectLabel(l.effect.mode) : "Effect —")
                    + " · keyboard " + ["off", "low", "medium", "high"][l.brightness || 0],
                  l.on_ac === false ? "On battery" : "On AC"
                ] : []
                KeyboardPreview {
                  // the whole space under the text: the drawing keeps its proportions and centres itself
                  anchors.fill: parent
                  fg: root.fg
                  effect: parent.parent.l.effect || null
                  brightness: parent.parent.l.brightness || 0
                  musicOn: parent.parent.l.music === "on"
                  bands: armoury.bands
                  music: root.cfg.music || null
                }
              }
              Tile {
                pageId: "battery"; icon: "󰁹"; title: "Battery"
                readonly property var b: root.snap ? (root.snap.battery_info || {}) : {}
                lines: root.snap ? [
                  (b.status || "") + (b.draw_w && b.draw_w > 0.5 ? " · " + b.draw_w.toFixed(1) + " W" : "")
                    + (b.time_left_min ? " · " + Math.floor(b.time_left_min / 60) + " h " + (b.time_left_min % 60) + " min left" : ""),
                  b.health_pct ? "Health " + Math.round(b.health_pct) + "%" : ""
                ] : []
                Column {
                  width: parent.width
                  spacing: Style.space(12)
                  Item {
                    width: parent.width
                    height: Style.space(40)
                    readonly property var b: parent.parent.parent.b
                    Rectangle {
                      id: battBar
                      anchors.fill: parent
                      radius: Style.cornerRadius
                      color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.06)
                      border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.15)
                      Rectangle {
                        width: parent.width * Math.min(1, (parent.parent.b.capacity || 0) / 100)
                        height: parent.height
                        radius: parent.radius
                        color: Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.55)
                      }
                      // the charge limit
                      Rectangle {
                        visible: !!parent.parent.b.charge_limit && parent.parent.b.charge_limit < 100
                        x: parent.width * (parent.parent.b.charge_limit || 100) / 100 - width / 2
                        width: 2; height: parent.height
                        color: root.fg
                      }
                      Text {
                        anchors.centerIn: parent
                        text: (parent.parent.b.capacity || 0) + "%" + (parent.parent.b.charge_limit ? "  ·  limit " + parent.parent.b.charge_limit + "%" : "")
                        color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true
                      }
                    }
                  }
                  Sparkline {
                    shown: root.opened
                    label: "Charge, last 30 min"; fg: root.fg; fontFamily: root.fontFamily
                    value: ""
                    series: [{ values: armoury.batteryHist, color: root.accent }]
                    minSpan: 10; floor: 0; ceiling: 100
                  }
                }
              }
              Tile {
                pageId: "input"; icon: "󰘳"; title: "Input"
                lines: ["Keys, touchpad, NumberPad"]
                InputArt {
                  anchors.fill: parent
                  fg: root.fg; accent: root.accent; fontFamily: root.fontFamily
                  keys: root.cfg.keys || null
                  touchpadOn: !(root.snap && root.snap.system && root.snap.system.touchpad === false)
                  numpad: root.snap && root.snap.system ? (root.snap.system.numpad || "unavailable") : "unavailable"
                }
              }
              Tile {
                pageId: "system"; icon: "󰒓"; title: "System"
                lines: ["Display, sleep, power source"]
                SystemArt {
                  anchors.fill: parent
                  fg: root.fg; accent: root.accent; fontFamily: root.fontFamily
                  sys: root.cfg.system || null
                  snap: root.snap
                }
              }
              Tile {
                pageId: "keystone"; icon: "󰌆"; title: "Keystone"
                readonly property var k: root.cfg.keystone || null
                lines: root.snap ? [
                  root.snap.keystone === true ? "Inserted" : root.snap.keystone === false ? "Not inserted" : "—",
                  !k ? "" : k.enabled === false ? "Actions off" : "Actions on" + (k.animation ? " · animation" : ""),
                  k && k.enabled !== false ? "Insert: " + root.ksSummary(k.insert) : "",
                  k && k.enabled !== false ? "Remove: " + root.ksSummary(k.remove) : ""
                ] : []
                KeystoneArt {
                  anchors.fill: parent
                  fg: root.fg; fontFamily: root.fontFamily
                  inserted: !!root.snap && root.snap.keystone === true
                  enabled_: !parent.parent.k || parent.parent.k.enabled !== false
                }
              }
            }

            // Detail pages
            Loader {
              anchors.fill: parent
              active: root.page !== ""
              sourceComponent: root.page === "manual" ? manualPage
                : root.page === "lighting" ? lightingPage
                : root.page === "battery" ? batteryPage
                : root.page === "input" ? inputPage
                : root.page === "system" ? systemPage
                : root.page === "keystone" ? keystonePage : null
            }
          }
        }
      }
    }
  }

  function pageTitle(p) {
    return ({ manual: "Manual", lighting: "Lighting", battery: "Battery", input: "Input", system: "System", keystone: "Keystone" })[p] || ""
  }

  Component { id: manualPage; ManualPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: lightingPage; LightingPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: batteryPage; BatteryPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: inputPage; InputPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: systemPage; SystemPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }
  Component { id: keystonePage; KeystonePage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }

  component Tile: Rectangle {
    property string pageId: ""
    property string icon: ""
    property string title: ""
    property var lines: []
    // extra content (graphs, preview) in the space under the text
    default property alias content: area.data
    width: parent.tileW
    height: parent.tileH
    radius: Style.cornerRadius
    color: hover.containsMouse ? Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.10) : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.05)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)

    Column {
      id: textCol
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.top: parent.top
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
    Item {
      id: area
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.top: textCol.bottom
      anchors.bottom: parent.bottom
      anchors.leftMargin: Style.space(18)
      anchors.rightMargin: Style.space(18)
      anchors.topMargin: Style.space(14)
      anchors.bottomMargin: Style.space(34)
      clip: true
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
