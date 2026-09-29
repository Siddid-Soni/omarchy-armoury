import QtQuick
import qs.Commons
import qs.Ui

Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property bool usable: client && client.active
  property string profile: snap && snap.perf ? (snap.perf.profile || "balanced") : "balanced"
  property var stored: ({})     // saved settings for `profile`
  property var bounds: ({})
  property var edit: ({})       // changes not yet applied
  property var detail: null    // detail Status (NVIDIA) while this page is open

  contentHeight: col.implicitHeight
  clip: true

  function load() {
    edit = ({})
    client.call({ cmd: "mode_settings", profile: profile }, function(r) {
      if (r.ok) { root.stored = r.data.settings || {}; root.bounds = r.data.bounds || {} }
    })
  }
  Component.onCompleted: load()
  onProfileChanged: load()

  function val(key, fallback) { return edit[key] !== undefined ? edit[key] : (stored[key] !== undefined ? stored[key] : fallback) }
  function set(key, v) { var e = Object.assign({}, edit); e[key] = v; edit = e }
  function range(key, lo, hi) { var b = bounds[key]; return b ? b : [lo, hi] }
  function apply() {
    client.run({ cmd: "set_mode_settings", profile: profile, settings: edit }, function(r) { if (r.ok) root.load() })
  }

  // NVIDIA details only while the page is open, every 5 s: reading them keeps the dGPU awake.
  Timer {
    interval: 5000
    running: root.visible && root.snap && root.snap.gpu && root.snap.gpu.dgpu_active === true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.client.call({ cmd: "status" }, function(r) { if (r.ok) root.detail = r.data.gpu })
  }

  Column {
    id: col
    width: Math.min(root.width, Style.space(640))
    spacing: Style.space(14)

    ChoiceRow {
      fg: root.fg
      label: "Settings for mode"
      options: [{ label: "Silent", value: "quiet" }, { label: "Balanced", value: "balanced" }, { label: "Turbo", value: "performance" }]
      value: root.profile
      onChosen: function(v) { root.profile = v }
    }

    Section { text: "CPU"; fg: root.fg }
    ValueSlider {
      fg: root.fg; label: "PL1 (sustained)"; unit: "W"
      minimum: root.range("pl1", 5, 150)[0]; maximum: root.range("pl1", 5, 150)[1]
      value: root.val("pl1", 45); unset: root.val("pl1", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("pl1", v) }
    }
    ValueSlider {
      fg: root.fg; label: "PL2 (boost)"; unit: "W"
      minimum: root.range("pl2", 5, 150)[0]; maximum: root.range("pl2", 5, 150)[1]
      value: root.val("pl2", 65); unset: root.val("pl2", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("pl2", v) }
    }
    ChoiceRow {
      fg: root.fg; label: "CPU boost"; usable: root.usable
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.val("cpu_boost", true)
      onChosen: function(v) { root.set("cpu_boost", v) }
    }
    Dropdown {
      width: Math.min(parent.width, Style.space(320))
      label: "Energy preference (EPP)"
      options: [
        { label: "Default", value: "default" }, { label: "Performance", value: "performance" },
        { label: "Balance performance", value: "balance_performance" }, { label: "Balance power", value: "balance_power" },
        { label: "Power saving", value: "power" }
      ]
      value: root.val("epp", "default")
      enabled: root.usable
      onChanged: function(v) { root.set("epp", v) }
    }
    ValueSlider {
      visible: root.snap && root.snap.perf && root.snap.perf.undervolt && root.snap.perf.undervolt.unlocked
      fg: root.fg; label: "Undervolt (core + cache)"; unit: "mV"
      minimum: -150; maximum: 0
      value: root.val("uv_mv", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("uv_mv", v) }
    }
    Text {
      visible: root.snap && root.snap.perf && root.snap.perf.undervolt && !root.snap.perf.undervolt.unlocked
      text: "Undervolt is locked by the BIOS on this laptop."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }

    Section { text: "NVIDIA"; fg: root.fg }
    ValueSlider {
      fg: root.fg; label: "Dynamic Boost"; unit: "W"
      minimum: root.range("nv_boost", 5, 25)[0]; maximum: root.range("nv_boost", 5, 25)[1]
      value: root.val("nv_boost", 25); unset: root.val("nv_boost", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("nv_boost", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Temperature target"; unit: "°C"
      minimum: root.range("nv_temp", 75, 87)[0]; maximum: root.range("nv_temp", 75, 87)[1]
      value: root.val("nv_temp", 87); unset: root.val("nv_temp", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("nv_temp", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Core clock offset"; unit: "MHz"
      minimum: -300; maximum: 300; step: 5
      value: root.val("gpu_core_offset", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("gpu_core_offset", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Memory clock offset"; unit: "MHz"
      minimum: -500; maximum: 1500; step: 10
      value: root.val("gpu_mem_offset", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("gpu_mem_offset", v) }
    }
    Text {
      width: parent.width
      wrapMode: Text.WordWrap
      text: "Offsets and clock locks are applied whenever the dGPU is awake. Raise them in small steps."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }

    Row {
      spacing: Style.space(10)
      Button {
        text: Object.keys(root.edit).length ? "Apply to " + (root.profile === "quiet" ? "Silent" : root.profile === "performance" ? "Turbo" : "Balanced") : "No changes"
        bordered: true; foreground: root.fg
        enabled: root.usable && Object.keys(root.edit).length > 0
        onClicked: root.apply()
      }
      Button { text: "Discard"; foreground: root.fg; visible: Object.keys(root.edit).length > 0; onClicked: root.load() }
    }

    Section { text: "GPU STATUS"; fg: root.fg }
    Text {
      visible: !(root.snap && root.snap.gpu && root.snap.gpu.dgpu_active === true)
      text: "dGPU is asleep (saving power)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }
    Column {
      visible: !!root.detail && !!root.detail.nvidia
      width: parent.width
      spacing: Style.spacing.labelGap
      readonly property var n: root.detail && root.detail.nvidia ? root.detail.nvidia : ({})
      InfoRow { fg: root.fg; label: "Clocks"; value: (parent.n.core_mhz || 0) + " / " + (parent.n.mem_mhz || 0) + " MHz" }
      InfoRow { fg: root.fg; label: "Temperature · power"; value: (parent.n.temp_c || 0) + "°C · " + Number(parent.n.power_w || 0).toFixed(1) + " W" }
      InfoRow { fg: root.fg; label: "Load · VRAM"; value: (parent.n.util_pct || 0) + "% · " + (parent.n.vram_used_mb || 0) + " / " + (parent.n.vram_total_mb || 0) + " MB" }
      InfoRow { fg: root.fg; label: "Offsets"; value: (parent.n.core_offset || 0) + " / " + (parent.n.mem_offset || 0) + " MHz" }
      InfoRow {
        fg: root.fg; label: "Keeping it awake"
        value: root.detail && root.detail.users && root.detail.users.length ? root.detail.users.map(function(u) { return u.name }).join(", ") : "—"
      }
    }
  }
}
