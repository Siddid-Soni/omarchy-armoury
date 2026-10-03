import QtQuick
import qs.Commons
import qs.Ui

// Keystone: what happens when it goes in or out (armouryd, active mode).
Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property bool usable: client && client.active
  property var cfg: ({})

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to controls; WheelScroll scrolls
  WheelScroll { flick: root }

  function reload() { client.call({ cmd: "config" }, function(r) { if (r.ok) root.cfg = r.data }) }
  Component.onCompleted: reload()

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
      label: "Keystone animation on insert"
      usable: root.usable
      options: root.onOff()
      value: root.ks ? root.ks.animation : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_keystone_animation", on: v }, function() { root.reload() }) }
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
