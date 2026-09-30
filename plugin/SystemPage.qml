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
  function onOff(v) { return [{ label: "On", value: true }, { label: "Off", value: false }] }

  // Keystone actions (config.toml [keystone]); a change replaces that event's whole action
  readonly property var ks: cfg.keystone || null
  readonly property var effects: ["static", "breathe", "rainbow_cycle", "rainbow_wave", "star", "rain", "highlight", "laser", "ripple", "pulse", "comet", "flash"]
  function effectLabel(m) { return String(m).split("_").map(function(w) { return w.charAt(0).toUpperCase() + w.slice(1) }).join(" ") }
  function lightOptions(ev) {
    var out = [{ label: "Unchanged", value: "unchanged" }, { label: "Music", value: "music" }]
    if (ev === "remove") out.push({ label: "Back to before insert", value: "previous" })
    return out.concat(effects.map(function(m) { return { label: effectLabel(m), value: m } }))
  }
  function setKs(ev, key, v) {
    var a = Object.assign({}, ks[ev])
    a[key] = v
    client.run({ cmd: "set_keystone_action", event: ev, action: a }, function() { root.reload() })
  }

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

    Section { visible: !!root.ks; text: "KEYSTONE"; fg: root.fg }
    Text {
      visible: !!root.ks
      width: parent.width
      wrapMode: Text.WordWrap
      text: (root.snap && root.snap.keystone === true ? "Inserted." : root.snap && root.snap.keystone === false ? "Not inserted." : "")
        + (root.ks && root.ks.enabled === false ? " Keystone actions are off: nothing happens on insert or remove."
          : " Actions run within 2 s of inserting or removing it, while Armoury is in control.")
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
    ChoiceRow {
      visible: !!root.ks
      fg: root.fg
      label: "Keystone actions"
      usable: root.usable
      options: root.onOff()
      value: root.ks ? root.ks.enabled !== false : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_keystone_enabled", on: v }, function() { root.reload() }) }
    }
    ChoiceRow {
      visible: !!root.ks && root.ks.enabled !== false
      fg: root.fg
      label: "Flash the Keystone light on insert"
      usable: root.usable
      options: root.onOff()
      value: root.ks ? root.ks.flash : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_keystone_flash", on: v }, function() { root.reload() }) }
    }
    Repeater {
      model: root.ks && root.ks.enabled !== false ? [{ ev: "insert", title: "When inserted" }, { ev: "remove", title: "When removed" }] : []
      Column {
        id: ksCol
        required property var modelData
        readonly property var act: root.ks[modelData.ev] || {}
        width: col.width
        spacing: Style.space(10)
        Text { text: ksCol.modelData.title; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true }
        Dropdown {
          width: Math.min(parent.width, Style.space(320))
          label: "Mode"
          options: [{ label: "Unchanged", value: "" }].concat(root.modeOptions)
          value: ksCol.act.mode || ""
          enabled: root.usable
          onChanged: function(v) { root.setKs(ksCol.modelData.ev, "mode", v === "" ? null : v) }
        }
        Dropdown {
          width: Math.min(parent.width, Style.space(320))
          label: "Lighting"
          options: root.lightOptions(ksCol.modelData.ev)
          value: ksCol.act.light || "unchanged"
          enabled: root.usable
          onChanged: function(v) { root.setKs(ksCol.modelData.ev, "light", v) }
        }
        Text { text: "Command (optional)"; color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
        Row {
          width: parent.width
          spacing: Style.space(8)
          TextField { id: ksCmd; width: parent.width - ksSave.width - parent.spacing; text: ksCol.act.command || ""; foreground: root.fg }
          Button {
            id: ksSave; text: "Save"; bordered: true; foreground: root.fg; enabled: root.usable
            onClicked: root.setKs(ksCol.modelData.ev, "command", ksCmd.text.trim() === "" ? null : ksCmd.text)
          }
        }
        ChoiceRow {
          visible: ksCol.modelData.ev === "remove"
          fg: root.fg
          label: "Lock the screen"
          usable: root.usable
          options: root.onOff()
          value: ksCol.act.lock === true
          onChosen: function(v) { root.setKs("remove", "lock", v) }
        }
      }
    }
  }
}
