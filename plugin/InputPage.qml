import QtQuick
import qs.Commons
import qs.Ui

Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var sys: client && client.snap ? (client.snap.system || {}) : {}
  readonly property bool usable: client && client.active
  property var cfg: ({})
  property string rogCommand: ""
  property string fanCommand: ""
  property string auraCommand: ""

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to sliders; WheelScroll scrolls
  WheelScroll { flick: root }

  function reload() {
    client.call({ cmd: "config" }, function(r) {
      if (!r.ok) return
      root.cfg = r.data
      root.rogCommand = r.data.keys.rog_command || ""
      root.fanCommand = r.data.keys.fan_command || ""
      root.auraCommand = r.data.keys.aura_command || ""
    })
  }
  Component.onCompleted: reload()

  readonly property var actions: [
    { label: "Nothing", value: "none" },
    { label: "Open Armoury", value: "open_window" },
    { label: "Cycle mode", value: "cycle_mode" },
    { label: "Keyboard brightness", value: "cycle_brightness" },
    { label: "Lighting effect", value: "cycle_effect" },
    { label: "Toggle NumberPad", value: "toggle_numpad" },
    { label: "Toggle music lighting", value: "toggle_music" },
    { label: "Run command", value: "command" }
  ]

  readonly property var np: cfg.numpad || ({ start_brightness: 8, allow_when_touchpad_off: false, idle_dim_secs: 60, hold_ms: 1000, key_repeat: true, repeat_delay_ms: 0, repeat_rate_hz: 0 })
  function setNp(key, v) { var r = { cmd: "set_numpad_config" }; r[key] = v; client.run(r, function() { root.reload() }) }

  function bindKey(key, action, command) {
    var req = { cmd: "set_key_binding", key: key, action: action }
    if (command) req.command = command
    client.run(req, function() { root.reload() })
  }

  Column {
    id: col
    width: Math.min(root.width, Style.space(640))
    spacing: Style.space(16)

    Section { text: "KEYS"; fg: root.fg }
    Dropdown {
      width: parent.width
      label: "ROG key"
      options: root.actions
      value: root.cfg.keys ? root.cfg.keys.rog : ""
      enabled: root.usable
      onChanged: function(v) { if (v !== "command") root.bindKey("rog", v, "") }
    }
    Row {
      visible: !!root.cfg.keys && (root.cfg.keys.rog === "command" || rogField.activeFocus)
      width: parent.width
      spacing: Style.space(8)
      TextField { id: rogField; width: parent.width - rogSave.width - parent.spacing; text: root.rogCommand; foreground: root.fg }
      Button { id: rogSave; text: "Save"; bordered: true; foreground: root.fg; enabled: root.usable; onClicked: root.bindKey("rog", "command", rogField.text) }
    }
    Dropdown {
      width: parent.width
      label: "Fn+F5"
      options: root.actions
      value: root.cfg.keys ? root.cfg.keys.fan : ""
      enabled: root.usable
      onChanged: function(v) { if (v !== "command") root.bindKey("fan", v, "") }
    }
    Row {
      visible: !!root.cfg.keys && root.cfg.keys.fan === "command"
      width: parent.width
      spacing: Style.space(8)
      TextField { id: fanField; width: parent.width - fanSave.width - parent.spacing; text: root.fanCommand; foreground: root.fg }
      Button { id: fanSave; text: "Save"; bordered: true; foreground: root.fg; enabled: root.usable; onClicked: root.bindKey("fan", "command", fanField.text) }
    }
    Dropdown {
      width: parent.width
      label: "Fn+F4 (Aura)"
      options: root.actions
      value: root.cfg.keys ? root.cfg.keys.aura : ""
      enabled: root.usable
      onChanged: function(v) { if (v !== "command") root.bindKey("aura", v, "") }
    }
    Row {
      visible: !!root.cfg.keys && root.cfg.keys.aura === "command"
      width: parent.width
      spacing: Style.space(8)
      TextField { id: auraField; width: parent.width - auraSave.width - parent.spacing; text: root.auraCommand; foreground: root.fg }
      Button { id: auraSave; text: "Save"; bordered: true; foreground: root.fg; enabled: root.usable; onClicked: root.bindKey("aura", "command", auraField.text) }
    }
    Text {
      width: parent.width
      wrapMode: Text.WordWrap
      text: "To run a command from a key, enter it and press Save. Volume and mic keys are handled by Omarchy."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }

    Section { text: "TOUCHPAD"; fg: root.fg }
    ChoiceRow {
      fg: root.fg
      usable: root.usable
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.sys.touchpad !== false
      onChosen: function(v) { root.client.run({ cmd: "set_toggle", toggle: "touchpad", on: v }) }
    }

    Section { text: "NUMBERPAD"; fg: root.fg }
    Text {
      visible: root.sys.numpad === "unavailable" || root.sys.numpad === undefined
      width: parent.width
      wrapMode: Text.WordWrap
      text: "No NumberPad touchpad found."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
    ChoiceRow {
      fg: root.fg
      label: "NumberPad"
      usable: root.usable && root.sys.numpad !== "unavailable"
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.sys.numpad === "on"
      onChosen: function(v) { root.client.run({ cmd: "set_numpad", on: v }) }
    }
    ValueSlider {
      fg: root.fg; label: "Brightness when it turns on"; unit: ""
      minimum: 1; maximum: 8
      value: root.np.start_brightness
      usable: root.usable
      onCommitted: function(v) { root.setNp("start_brightness", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Dark after no touch (0 = never)"; unit: "s"
      minimum: 0; maximum: 600; step: 10
      value: root.np.idle_dim_secs
      usable: root.usable
      onCommitted: function(v) { root.setNp("idle_dim_secs", v) }
    }
    ChoiceRow {
      fg: root.fg
      label: "Key repeat (finger resting on a key)"
      usable: root.usable
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.np.key_repeat !== false
      onChosen: function(v) { root.setNp("key_repeat", v) }
    }
    ValueSlider {
      visible: root.np.key_repeat !== false
      fg: root.fg; label: "Key repeat delay (0 = same as the keyboard)"; unit: "ms"
      minimum: 0; maximum: 2000; step: 50
      value: root.np.repeat_delay_ms
      usable: root.usable
      // 1–99 isn't a valid delay: snap it to "same as the keyboard"
      onCommitted: function(v) { root.setNp("repeat_delay_ms", v > 0 && v < 100 ? 0 : v) }
    }
    ValueSlider {
      visible: root.np.key_repeat !== false
      fg: root.fg; label: "Key repeat rate (0 = same as the keyboard)"; unit: "/s"
      minimum: 0; maximum: 100; step: 1
      value: root.np.repeat_rate_hz
      usable: root.usable
      onCommitted: function(v) { root.setNp("repeat_rate_hz", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Hold time to toggle"; unit: "ms"
      minimum: 300; maximum: 3000; step: 100
      value: root.np.hold_ms
      usable: root.usable
      onCommitted: function(v) { root.setNp("hold_ms", v) }
    }
    ChoiceRow {
      fg: root.fg
      label: "While the touchpad is off"
      usable: root.usable
      options: [{ label: "NumberPad off too", value: false }, { label: "Allow NumberPad", value: true }]
      value: root.np.allow_when_touchpad_off === true
      onChosen: function(v) { root.setNp("allow_when_touchpad_off", v) }
    }
  }
}
