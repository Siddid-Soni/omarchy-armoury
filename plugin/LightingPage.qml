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
  property var info: null
  property string error: ""
  property var music: null   // config.toml [music]
  readonly property string musicState: snap && snap.lighting && snap.lighting.music ? snap.lighting.music : "unavailable"

  // effect being edited
  property string mode: "static"
  property string colour1: "ff0000"
  property string colour2: "000000"
  property string speed: "med"
  property string direction: "right"

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to sliders; WheelScroll scrolls
  WheelScroll { flick: root }

  function hex(c) { return c ? c.map(function(v) { return ("0" + v.toString(16)).slice(-2) }).join("") : "000000" }
  function rgb(h) { return [parseInt(h.substr(0, 2), 16), parseInt(h.substr(2, 2), 16), parseInt(h.substr(4, 2), 16)] }
  function valid(h) { return /^[0-9a-fA-F]{6}$/.test(h) }

  function reload() {
    client.call({ cmd: "lighting" }, function(r) {
      if (!r.ok) { root.error = r.error || ""; return }
      root.error = ""
      root.info = r.data
      if (r.data.effect) {
        root.mode = r.data.effect.mode
        root.colour1 = root.hex(r.data.effect.colour1)
        root.colour2 = root.hex(r.data.effect.colour2)
        root.speed = r.data.effect.speed
        root.direction = r.data.effect.direction
      }
    })
  }
  function reloadMusic() {
    client.call({ cmd: "config" }, function(r) { if (r.ok) root.music = r.data.music })
  }
  function setMusic(key, v) { var r = { cmd: "set_music_config" }; r[key] = v; client.run(r, function() { root.reloadMusic() }) }
  Component.onCompleted: { reload(); reloadMusic() }

  function applyEffect() {
    if (!valid(colour1) || !valid(colour2)) { root.error = "Colours must be RRGGBB"; return }
    client.run({ cmd: "set_effect", effect: { mode: mode, colour1: rgb(colour1), colour2: rgb(colour2), speed: speed, direction: direction } }, function() { root.reload() })
  }

  function modeLabel(m) { return String(m).split("_").map(function(w) { return w.charAt(0).toUpperCase() + w.slice(1) }).join(" ") }
  // Which settings each effect actually uses (asusd / rog-aura semantics).
  readonly property var effectParams: ({
    static: ["colour1"], breathe: ["colour1", "colour2", "speed"], rainbow_cycle: ["speed"],
    rainbow_wave: ["speed", "direction"], star: ["colour1", "colour2", "speed"], rain: ["speed"],
    highlight: ["colour1", "speed"], laser: ["colour1", "speed"], ripple: ["colour1", "speed"],
    pulse: ["colour1"], comet: ["colour1"], flash: ["colour1"]
  })
  function uses(p) { var l = effectParams[mode]; return !l || l.indexOf(p) >= 0 }

  readonly property var presets: ["ff0000", "ff6a00", "ffd000", "00ff40", "00e5ff", "0050ff", "a000ff", "ff00a0", "ffffff"]

  Column {
    id: col
    width: Math.min(root.width, Style.space(680))
    spacing: Style.space(16)

    Text {
      visible: root.error !== ""
      width: parent.width
      wrapMode: Text.WordWrap
      text: root.usable ? root.error : "Effects and zones are set through asusd — press Take over to edit them."
      color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }

    Section { text: "BRIGHTNESS"; fg: root.fg }
    ChoiceRow {
      fg: root.fg
      usable: root.usable
      options: [{ label: "Off", value: 0 }, { label: "Low", value: 1 }, { label: "Medium", value: 2 }, { label: "High", value: 3 }]
      value: root.snap && root.snap.lighting ? root.snap.lighting.brightness : undefined
      onChosen: function(v) { root.client.run({ cmd: "set_brightness", level: v }) }
    }
    Text {
      width: parent.width
      wrapMode: Text.WordWrap
      text: "Remembered separately on AC and on battery (Fn keys too)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }

    Section { visible: !!root.info; text: "EFFECT"; fg: root.fg }
    Dropdown {
      visible: !!root.info
      width: Math.min(parent.width, Style.space(320))
      label: "Effect"
      options: root.info ? root.info.modes.map(function(m) { return { label: root.modeLabel(m), value: m } }) : []
      value: root.mode
      enabled: root.usable
      onChanged: function(v) { root.mode = v }
    }
    Repeater {
      model: root.info ? [{ key: "colour1", label: "Colour" }, { key: "colour2", label: "Second colour" }] : []
      Column {
        required property var modelData
        visible: root.uses(modelData.key)
        width: col.width
        spacing: Style.space(6)
        Text { text: modelData.label; color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
        Row {
          spacing: Style.space(6)
          Rectangle {
            width: Style.space(34); height: Style.space(34); radius: Style.space(6)
            color: root.valid(root[modelData.key]) ? "#" + root[modelData.key] : "transparent"
            border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.3)
          }
          TextField {
            width: Style.space(110)
            text: root[modelData.key]
            foreground: root.fg
            onTextChanged: root[modelData.key] = text.replace("#", "")
          }
          Repeater {
            model: root.presets
            Rectangle {
              required property var modelData
              width: Style.space(26); height: Style.space(26); radius: width / 2
              anchors.verticalCenter: parent.verticalCenter
              color: "#" + modelData
              border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.3)
              MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root[parent.parent.parent.modelData.key] = parent.modelData }
            }
          }
        }
      }
    }
    ChoiceRow {
      visible: !!root.info && root.uses("speed")
      fg: root.fg
      label: "Speed"
      usable: root.usable
      options: [{ label: "Slow", value: "low" }, { label: "Medium", value: "med" }, { label: "Fast", value: "high" }]
      value: root.speed
      onChosen: function(v) { root.speed = v }
    }
    ChoiceRow {
      visible: !!root.info && root.uses("direction")
      fg: root.fg
      label: "Direction"
      usable: root.usable
      options: [{ label: "Right", value: "right" }, { label: "Left", value: "left" }, { label: "Up", value: "up" }, { label: "Down", value: "down" }]
      value: root.direction
      onChosen: function(v) { root.direction = v }
    }
    Button {
      visible: !!root.info
      text: "Apply effect"
      bordered: true
      foreground: root.fg
      enabled: root.usable
      onClicked: root.applyEffect()
    }

    Section { visible: !!root.music; text: "MUSIC"; fg: root.fg }
    Column {
      visible: !!root.music
      width: parent.width
      spacing: Style.space(12)
      Row {
        spacing: Style.space(10)
        Button {
          text: root.musicState === "on" ? "Music lighting: on" : "Music lighting: off"
          bordered: true
          foreground: root.fg
          active: root.musicState === "on"
          enabled: root.usable && root.musicState !== "unavailable"
          onClicked: root.client.run({ cmd: "set_music", on: root.musicState !== "on" }, function() { root.reloadMusic() })
        }
      }
      Text {
        width: parent.width
        wrapMode: Text.WordWrap
        text: root.musicState === "failed" ? "Stopped: " + (root.snap.lighting.music_error || "repeated failures") + " — turn it on to retry."
          : root.musicState === "unavailable" ? (root.usable ? "Needs a per-key keyboard." : "Press Take over to use music lighting.")
          : "Lights the keyboard to whatever is playing. Setting an effect turns it off."
        color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
      }
      ChoiceRow {
        fg: root.fg
        label: "Style"
        usable: root.usable
        options: [{ label: "Spectrum", value: "spectrum" }, { label: "Pulse", value: "pulse" }]
        value: root.music ? root.music.style : undefined
        onChosen: function(v) { root.setMusic("style", v) }
      }
      ChoiceRow {
        fg: root.fg
        label: "Colours"
        usable: root.usable
        options: [{ label: "Gradient", value: "gradient" }, { label: "Rainbow", value: "rainbow" }, { label: "Single", value: "single" }]
        value: root.music ? root.music.scheme : undefined
        onChosen: function(v) { root.setMusic("scheme", v) }
      }
      Repeater {
        model: root.music && root.music.scheme !== "rainbow"
          ? (root.music.scheme === "single" ? [{ key: "colour1", label: "Colour" }] : [{ key: "colour1", label: "Low / quiet" }, { key: "colour2", label: "High / loud" }])
          : []
        Column {
          id: mc
          required property var modelData
          readonly property string current: root.music ? root.hex(root.music[modelData.key]) : "000000"
          width: col.width
          spacing: Style.space(6)
          Text { text: mc.modelData.label; color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall }
          Row {
            spacing: Style.space(6)
            Rectangle {
              width: Style.space(34); height: Style.space(34); radius: Style.space(6)
              color: "#" + mc.current
              border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.3)
            }
            TextField {
              width: Style.space(110)
              text: mc.current
              foreground: root.fg
              enabled: root.usable
              onEditingFinished: {
                var h = text.replace("#", "")
                if (root.valid(h) && h.toLowerCase() !== mc.current) root.setMusic(mc.modelData.key, root.rgb(h))
              }
            }
            Repeater {
              model: root.presets
              Rectangle {
                required property var modelData
                width: Style.space(26); height: Style.space(26); radius: width / 2
                anchors.verticalCenter: parent.verticalCenter
                color: "#" + modelData
                border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.3)
                MouseArea {
                  anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                  enabled: root.usable
                  onClicked: root.setMusic(mc.modelData.key, root.rgb(parent.modelData))
                }
              }
            }
          }
        }
      }
      ValueSlider {
        fg: root.fg; label: "Sensitivity"; unit: ""
        minimum: 1; maximum: 10
        value: root.music ? root.music.sensitivity : 5
        usable: root.usable
        onCommitted: function(v) { root.setMusic("sensitivity", v) }
      }
    }

    Section { visible: !!root.info; text: "ZONES"; fg: root.fg }
    Repeater {
      model: root.info ? root.info.zones : []
      Column {
        required property var modelData
        width: col.width
        spacing: Style.space(6)
        Text { text: root.modeLabel(modelData.zone); color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true }
        Row {
          spacing: Style.space(6)
          Repeater {
            model: ["boot", "awake", "sleep", "shutdown"]
            Button {
              required property var modelData
              readonly property var zone: parent.parent.modelData
              text: modelData.charAt(0).toUpperCase() + modelData.slice(1)
              fontSize: Style.font.bodySmall
              foreground: root.fg
              bordered: true
              enabled: root.usable
              active: zone[modelData] === true
              onClicked: {
                var z = Object.assign({}, zone)
                z[modelData] = !zone[modelData]
                root.client.run({ cmd: "set_zone_power", zone: z }, function() { root.reload() })
              }
            }
          }
        }
      }
    }
  }
}
