import QtQuick
import qs.Commons
import qs.Ui

// Manual mode: saved profiles, each a base mode + fan curves + CPU/GPU tuning.
// Fan graphs keep their own edits; `curveModel` is set only when a profile is
// selected, so changing a slider never rebuilds the graphs mid-edit.
Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property bool usable: !!(client && client.active)
  readonly property bool manualOn: !!(snap && snap.perf && snap.perf.mode === "manual")
  property var view: ({ enabled: false, active: null, profiles: [] })
  property var bounds: ({})
  property string selected: ""
  property string base: "performance"
  property var settings: ({})
  property var curveModel: []
  property bool dirty: false
  property bool renaming: false
  property var detail: null
  property alias fans: fanRepeater   // for the runtime probe (tests/)

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to sliders and graphs; WheelScroll scrolls
  WheelScroll { flick: root }

  function copy(o) { return JSON.parse(JSON.stringify(o)) }
  function find(name) {
    for (var i = 0; i < view.profiles.length; i++) if (view.profiles[i].name === name) return view.profiles[i]
    return null
  }
  function select(name) {
    var p = find(name)
    if (!p) return
    selected = name; base = p.base; settings = copy(p.settings || {}); curveModel = copy(p.curves || [])
    dirty = false; renaming = false
  }
  function load(keep) {
    client.call({ cmd: "manual_profiles" }, function(r) {
      if (!r.ok) return
      root.view = r.data
      var want = keep && root.find(keep) ? keep : (r.data.active || (r.data.profiles.length ? r.data.profiles[0].name : ""))
      if (want) root.select(want)
    })
    client.call({ cmd: "limit_bounds" }, function(r) { if (r.ok) root.bounds = r.data })
  }
  Component.onCompleted: load("")

  function current(name) {
    var curves = []
    for (var k = 0; k < fanRepeater.count; k++) {
      var g = fanRepeater.itemAt(k).graph
      curves.push({ fan: curveModel[k].fan, temps: g.temps.slice(), percent: g.percent.slice() })
    }
    return { name: name, base: base, curves: curves, settings: settings }
  }
  function saveAs(name, original, then) {
    client.run({ cmd: "save_manual_profile", profile: current(name), original_name: original }, function(r) {
      if (!r.ok) return
      root.load(name)
      if (then) then()
    })
  }
  function uniqueName() {
    for (var n = 1; ; n++) if (!find("Manual " + n)) return "Manual " + n
  }
  function val(key, fallback) { return settings[key] !== undefined ? settings[key] : fallback }
  function set(key, v) { var s = copy(settings); s[key] = v; settings = s; dirty = true }
  function range(key, lo, hi) { return bounds[key] || [lo, hi] }
  readonly property bool isActive: view.active === selected && manualOn

  // NVIDIA details only while the page is open, every 5 s: reading them keeps the dGPU awake.
  Timer {
    interval: 5000
    running: !!(root.visible && root.snap && root.snap.gpu && root.snap.gpu.dgpu_active === true)
    repeat: true
    triggeredOnStart: true
    onTriggered: root.client.call({ cmd: "status" }, function(r) { if (r.ok) root.detail = r.data.gpu })
  }

  Column {
    id: col
    width: Math.min(root.width, Style.space(680))
    spacing: Style.space(14)

    Text {
      visible: !root.manualOn
      width: parent.width
      wrapMode: Text.WordWrap
      text: "Fans are on firmware auto in Silent, Balanced and Turbo. Activate a profile to use it."
      color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }

    // ---------- profile picker ----------
    Row {
      width: parent.width
      spacing: Style.space(8)
      Dropdown {
        id: picker
        width: parent.width - newBtn.width - renameBtn.width - deleteBtn.width - parent.spacing * 3
        label: "Profile"
        options: root.view.profiles.map(function(p) {
          return { label: p.name + (p.name === root.view.active ? (root.manualOn ? "  · in use" : "  · active") : ""), value: p.name }
        })
        value: root.selected
        onChanged: function(v) { root.select(v) }
      }
      Button {
        id: newBtn; anchors.bottom: parent.bottom; text: "New"; bordered: true; foreground: root.fg; enabled: root.usable
        onClicked: root.saveAs(root.uniqueName(), null)
      }
      Button {
        id: renameBtn; anchors.bottom: parent.bottom; text: "Rename"; foreground: root.fg; enabled: root.usable && root.selected !== ""
        onClicked: { nameField.text = root.selected; root.renaming = true }
      }
      Button {
        id: deleteBtn; anchors.bottom: parent.bottom; text: "Delete"; foreground: root.fg
        enabled: root.usable && root.view.profiles.length > 1
        onClicked: root.client.run({ cmd: "delete_manual_profile", name: root.selected }, function(r) { if (r.ok) root.load("") })
      }
    }
    Row {
      visible: root.renaming
      width: parent.width
      spacing: Style.space(8)
      TextField { id: nameField; width: parent.width - renameSave.width - parent.spacing; foreground: root.fg }
      Button {
        id: renameSave; text: "Save name"; bordered: true; foreground: root.fg; enabled: root.usable && nameField.text.trim() !== ""
        onClicked: root.saveAs(nameField.text.trim(), root.selected)
      }
    }

    ChoiceRow {
      fg: root.fg
      label: "Runs on"
      usable: root.usable
      options: [{ label: "Silent", value: "quiet" }, { label: "Balanced", value: "balanced" }, { label: "Turbo", value: "performance" }]
      value: root.base
      onChosen: function(v) { root.base = v; root.dirty = true }
    }

    // ---------- fans ----------
    Section { text: "FANS"; fg: root.fg }
    Repeater {
      id: fanRepeater
      model: root.curveModel
      Column {
        id: fanCol
        required property var modelData
        required property int index
        property alias graph: graph
        width: col.width
        spacing: Style.space(4)
        Text {
          text: (fanCol.modelData.fan === "gpu" ? "GPU fan" : fanCol.modelData.fan === "mid" ? "Mid fan" : "CPU fan")
            + (fanCol.index === 0 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.cpu_fan_rpm || 0) + " rpm" : "")
            + (fanCol.index === 1 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm" : "")
          color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true
        }
        FanGraph {
          id: graph
          width: parent.width
          height: Style.space(190)
          temps: fanCol.modelData.temps
          percent: fanCol.modelData.percent
          lineColor: fanCol.index === 0 ? Color.accent : Qt.lighter(Color.urgent, 1.1)
          fg: root.fg
          usable: root.usable
          onEdited: root.dirty = true
        }
      }
    }
    Row {
      spacing: Style.space(10)
      Text {
        anchors.verticalCenter: parent.verticalCenter
        text: "Drag points to edit · Shift-drag moves the whole curve"
        color: root.fg; opacity: 0.55; font.family: root.fontFamily; font.pixelSize: Style.font.caption
      }
      Button {
        text: "Firmware curves"; fontSize: Style.font.caption; foreground: root.fg; enabled: root.usable
        onClicked: root.client.run({ cmd: "default_curves", base: root.base }, function(r) {
          if (r.ok) { root.curveModel = r.data; root.dirty = true }
        })
      }
    }

    // ---------- CPU ----------
    Section { text: "CPU"; fg: root.fg }
    ValueSlider {
      fg: root.fg; label: "PL1 (sustained)"; unit: "W"
      minimum: root.range("pl1", 5, 150)[0]; maximum: root.range("pl1", 5, 150)[1]
      value: root.val("pl1", 45); unset: root.val("pl1", undefined) === undefined
      usable: root.usable
      // PL2 can never sit below PL1: it slides up with it
      onLive: function(v) { if (v > root.val("pl2", 65)) root.set("pl2", v) }
      onCommitted: function(v) { root.set("pl1", v); if (v > root.val("pl2", 65)) root.set("pl2", v) }
    }
    ValueSlider {
      fg: root.fg; label: "PL2 (boost)"; unit: "W"
      minimum: root.range("pl2", 5, 150)[0]; maximum: root.range("pl2", 5, 150)[1]
      value: root.val("pl2", 65); unset: root.val("pl2", undefined) === undefined
      usable: root.usable
      // PL1 can never sit above PL2: it slides down with it
      onLive: function(v) { if (v < root.val("pl1", 45)) root.set("pl1", v) }
      onCommitted: function(v) { root.set("pl2", v); if (v < root.val("pl1", 45)) root.set("pl1", v) }
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
      visible: !!(root.snap && root.snap.perf && root.snap.perf.undervolt && root.snap.perf.undervolt.unlocked)
      fg: root.fg; label: "Undervolt (core + cache)"; unit: "mV"
      minimum: -150; maximum: 0
      value: root.val("uv_mv", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("uv_mv", v) }
    }

    // ---------- NVIDIA ----------
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

    // ---------- save / activate ----------
    Row {
      spacing: Style.space(10)
      Button {
        text: root.dirty ? (root.isActive ? "Save and apply" : "Save") : "Saved"
        bordered: true; foreground: root.fg
        enabled: root.usable && root.dirty && root.selected !== ""
        onClicked: root.saveAs(root.selected, root.selected)
      }
      Button {
        visible: !root.isActive && root.selected !== ""
        text: root.dirty ? "Save and activate" : "Activate"
        bordered: true; foreground: root.fg; enabled: root.usable
        onClicked: {
          var name = root.selected
          var go = function() { root.client.run({ cmd: "activate_manual_profile", name: name }, function(r) { if (r.ok) root.load(name) }) }
          if (root.dirty) root.saveAs(name, name, go); else go()
        }
      }
      Button { text: "Discard"; foreground: root.fg; visible: root.dirty; onClicked: root.select(root.selected) }
    }

    // ---------- GPU status ----------
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
      InfoRow {
        fg: root.fg; label: "Keeping it awake"
        value: root.detail && root.detail.users && root.detail.users.length ? root.detail.users.map(function(u) { return u.name }).join(", ") : "—"
      }
    }
  }
}
