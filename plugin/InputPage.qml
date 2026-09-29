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
    { label: "Run command", value: "command" }
  ]

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

    Section { text: "KEYBOARD BACKLIGHT WHEN IDLE"; fg: root.fg }
    ChoiceRow {
      fg: root.fg
      usable: root.usable
      options: [{ label: "Dim when idle", value: false }, { label: "Keep on", value: true }]
      value: root.cfg.lighting ? root.cfg.lighting.keep_on === true : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_keep_on", on: v }, function() { root.reload() }) }
    }
    Text {
      width: parent.width
      wrapMode: Text.WordWrap
      text: "The idle timeout is a bar widget setting (Omarchy Setup › Bar › Armoury)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
  }
}
