import QtQuick
import qs.Commons
import qs.Ui

Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property var sys: snap ? (snap.system || {}) : {}
  readonly property bool usable: client && client.active
  property var cfg: ({})
  property var panel: null      // built-in display from a detail Status (Hyprland)
  property int gamma: 100

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to sliders; WheelScroll scrolls
  WheelScroll { flick: root }

  function reload() {
    client.call({ cmd: "config" }, function(r) { if (r.ok) root.cfg = r.data })
    client.call({ cmd: "status" }, function(r) {
      if (!r.ok) return
      var d = r.data.display || []
      root.panel = null
      for (var i = 0; i < d.length; i++) if (String(d[i].output).indexOf("eDP") === 0) root.panel = d[i]
    })
  }
  Component.onCompleted: reload()

  readonly property var rateOptions: {
    var out = []
    var rs = root.panel ? (root.panel.rates || []) : []
    for (var i = 0; i < rs.length; i++) out.push({ label: Math.round(rs[i]) + " Hz", value: Math.round(rs[i]) })
    return out
  }
  readonly property var modeOptions: [{ label: "Silent", value: "quiet" }, { label: "Balanced", value: "balanced" }, { label: "Turbo", value: "performance" }, { label: "Manual", value: "manual" }]
  readonly property var odOptions: [{ label: "Leave", value: "leave" }, { label: "On", value: "on" }, { label: "Off", value: "off" }]
  function onOff(v) { return [{ label: "On", value: true }, { label: "Off", value: false }] }


  Column {
    id: col
    width: Math.min(root.width, Style.space(640))
    spacing: Style.space(16)

    Section { text: "DISPLAY"; fg: root.fg }
    Text {
      visible: !root.panel
      text: "Built-in panel not reachable (lid closed or Hyprland not running)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }
    ChoiceRow {
      visible: !!root.panel
      fg: root.fg
      label: "Refresh rate now"
      usable: root.usable
      options: root.rateOptions
      value: root.panel ? Math.round(root.panel.refresh_hz) : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_refresh", hz: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      visible: !!root.panel
      fg: root.fg
      label: "On AC"
      usable: root.usable
      options: root.rateOptions
      value: root.cfg.system && root.cfg.system.refresh_ac ? Math.round(root.cfg.system.refresh_ac) : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_source_refresh", ac: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      visible: !!root.panel
      fg: root.fg
      label: "On battery"
      usable: root.usable
      options: root.rateOptions
      value: root.cfg.system && root.cfg.system.refresh_battery ? Math.round(root.cfg.system.refresh_battery) : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_source_refresh", battery: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Panel Overdrive"
      visible: root.sys.panel_od !== undefined && root.sys.panel_od !== null
      usable: root.usable
      options: root.onOff()
      value: root.sys.panel_od
      onChosen: function(v) { root.client.run({ cmd: "set_toggle", toggle: "panel_od", on: v }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Overdrive on AC"
      visible: root.sys.panel_od !== undefined && root.sys.panel_od !== null
      usable: root.usable
      options: root.odOptions
      value: root.cfg.system && root.cfg.system.panel_od_ac === true ? "on" : (root.cfg.system && root.cfg.system.panel_od_ac === false ? "off" : "leave")
      onChosen: function(v) { root.client.run({ cmd: "set_source_overdrive", ac: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Overdrive on battery"
      visible: root.sys.panel_od !== undefined && root.sys.panel_od !== null
      usable: root.usable
      options: root.odOptions
      value: root.cfg.system && root.cfg.system.panel_od_battery === true ? "on" : (root.cfg.system && root.cfg.system.panel_od_battery === false ? "off" : "leave")
      onChosen: function(v) { root.client.run({ cmd: "set_source_overdrive", battery: v }, function() { root.reload() }) }
    }
    ValueSlider {
      fg: root.fg
      label: "Gamma (needs Omarchy night light / hyprsunset)"
      unit: "%"
      minimum: 20
      maximum: 100
      value: root.gamma
      usable: root.usable
      onCommitted: function(v) { root.gamma = v; root.client.run({ cmd: "set_gamma", percent: v }) }
    }

    Section { text: "POWER SOURCE"; fg: root.fg }
    Text {
      width: parent.width
      wrapMode: Text.WordWrap
      text: "Switched automatically when you plug in or unplug. Manual uses the active manual profile."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
    ChoiceRow {
      fg: root.fg
      label: "Mode on AC"
      usable: root.usable
      options: root.modeOptions
      value: root.cfg.system ? root.cfg.system.profile_ac : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_source_profile", ac: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Mode on battery"
      usable: root.usable
      options: root.modeOptions
      value: root.cfg.system ? root.cfg.system.profile_battery : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_source_profile", battery: v }, function() { root.reload() }) }
    }

    Section { text: "SYSTEM"; fg: root.fg }
    ChoiceRow {
      fg: root.fg
      label: "Sleep mode"
      usable: root.usable
      options: (root.sys.sleep_modes || []).map(function(m) { return { label: m === "s2idle" ? "s2idle (modern standby)" : "deep (S3)", value: m } })
      value: root.sys.mem_sleep
      onChosen: function(v) { root.client.run({ cmd: "set_sleep_mode", mode: v }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Lid closed stays awake on AC"
      usable: root.usable
      options: root.onOff()
      value: root.sys.clamshell === true
      onChosen: function(v) { root.client.run({ cmd: "set_toggle", toggle: "clamshell", on: v }) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Boot sound"
      visible: root.sys.boot_sound !== undefined && root.sys.boot_sound !== null
      usable: root.usable
      options: root.onOff()
      value: root.sys.boot_sound
      onChosen: function(v) { root.client.run({ cmd: "set_toggle", toggle: "boot_sound", on: v }) }
    }

  }
}
