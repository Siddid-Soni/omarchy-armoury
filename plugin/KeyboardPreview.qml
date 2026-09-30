import QtQuick
import qs.Commons

// A small drawing of the keyboard in the current lighting: the effect's colours (a hint of
// its animation for Breathe and the rainbows), or live bars while music lighting runs.
Item {
  id: root
  property var effect: null        // snapshot lighting.effect
  property int brightness: 3       // 0..3
  property bool musicOn: false
  property var bands: []           // music band levels, bass → treble
  property var music: null         // config.toml [music]
  property color fg: Color.foreground

  // simplified 15" layout: key widths per row, top to bottom
  readonly property var rows: [
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 1],
    [1.5, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1.5, 1],
    [1.75, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2.25, 1],
    [2.25, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1.75, 1, 1],
    [1.25, 1, 1, 1.25, 6, 1, 1, 1, 1, 1, 1]
  ]
  readonly property real gap: Math.max(2, width / 200)
  readonly property real rowH: (height - gap * (rows.length + 1)) / (rows.length + 0.6)
  readonly property bool animated: musicOn || (!!effect && effect.mode !== "static")
  // the effect's speed: phase advances this much per tick
  readonly property real speed: !effect ? 1 : effect.speed === "low" ? 0.5 : effect.speed === "high" ? 2 : 1
  property real phase: 0

  Timer {
    interval: 50
    running: root.visible && root.animated && !root.musicOn
    repeat: true
    onTriggered: root.phase += 0.05 * root.speed
  }

  function rgb(c) { return c ? Qt.rgba(c[0] / 255, c[1] / 255, c[2] / 255, 1) : root.fg }
  function mix(a, b, t) { return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1) }
  function hue(h) { return Qt.hsva(((h % 1) + 1) % 1, 1, 1, 1) }
  readonly property color off: Qt.rgba(fg.r, fg.g, fg.b, 0.07)
  readonly property real level: [0, 0.45, 0.7, 1][Math.max(0, Math.min(3, brightness))]
  function dimmed(c, k) { var t = Math.max(0, Math.min(1, k * root.level)); return mix(root.off, c, t) }
  readonly property real loudness: {
    if (!bands || bands.length === 0) return 0
    var s = 0
    for (var i = 0; i < bands.length; i++) s += bands[i]
    return Math.min(1, 1.6 * s / bands.length)
  }

  // deterministic 0..1 noise per key / cycle
  function rand(x, h, n) { var v = Math.sin(x * 127.1 + h * 311.7 + n * 74.7) * 43758.5453; return v - Math.floor(v) }
  function frac(v) { return v - Math.floor(v) }
  // a simulated key press for the reactive effects: every ~1.2 s at a random key
  function press(phase) {
    var cycle = Math.floor(phase / 1.2)
    return { x: rand(0.3, 0.7, cycle), h: Math.floor(rand(0.9, 0.1, cycle) * root.rows.length), age: frac(phase / 1.2) }
  }

  // colour of the key centred at x (0..1), `h` rows up from the bottom row
  function keyColor(x, h, phase, bands) {
    if (root.level === 0) return root.off
    if (root.musicOn && root.music) {
      var c1 = rgb(root.music.colour1), c2 = rgb(root.music.colour2)
      var band = Math.min(17, Math.floor(x * 18))
      var colour = root.music.scheme === "rainbow" ? hue(band / 17 * 0.8)
        : root.music.scheme === "single" ? c1 : mix(c1, c2, h / (root.rows.length - 1))
      if (root.music.style === "pulse") return dimmed(root.music.scheme === "gradient" ? mix(c1, c2, root.loudness) : colour, root.loudness)
      var v = bands && bands.length ? bands[band] || 0 : 0
      return dimmed(colour, Math.max(0, Math.min(1, v * root.rows.length - h)))
    }
    if (!root.effect) return root.off
    var a = rgb(root.effect.colour1), b = rgb(root.effect.colour2)
    var rowsN = root.rows.length
    var top = rowsN - 1 - h
    var dir = root.effect.direction
    switch (root.effect.mode) {
    case "breathe": {
      var t = (Math.sin(phase * 2) + 1) / 2
      return dimmed(mix(a, b, Math.floor(phase / Math.PI) % 2), 0.15 + 0.85 * t)
    }
    case "rainbow_cycle": return dimmed(hue(phase * 0.08), 1)
    case "rainbow_wave": {
      var pos = dir === "left" ? -x : dir === "up" ? h / rowsN : dir === "down" ? -h / rowsN : x
      return dimmed(hue(pos - phase * 0.08), 1)
    }
    case "pulse": return dimmed(a, Math.abs(Math.sin(phase * 1.5)))
    case "flash": return dimmed(a, frac(phase * 0.6) < 0.25 ? 1 : 0)
    case "star": {
      // keys twinkle on their own clocks, in either colour
      var st = phase * 0.5 + rand(x, h, 0) * 10
      var cyc = Math.floor(st)
      var on = rand(x, h, cyc) > 0.8
      return on ? dimmed(rand(x, h, cyc + 0.5) > 0.5 ? a : b, Math.sin(Math.PI * frac(st))) : dimmed(b, 0.12)
    }
    case "rain": {
      // drops fall down each column
      var col = Math.floor(x * 18)
      var drop = frac(phase * 0.35 + rand(col, 0.5, 1)) * (rowsN + 3) - 1
      var d = top - drop
      return d <= 0 && d > -2.5 ? dimmed(a, 1 + d / 2.5) : dimmed(b, 0.1)
    }
    case "comet": {
      // a streak sweeping across, with a fading tail
      var head = frac(phase * 0.25) * 1.4 - 0.2
      var behind = dir === "left" ? x - (1 - head) : head - x
      return behind >= 0 && behind < 0.35 ? dimmed(a, 1 - behind / 0.35) : root.off
    }
    case "highlight": case "laser": case "ripple": {
      // reactive effects: shown answering a simulated key press
      var p = press(phase)
      var fade = 1 - p.age
      if (root.effect.mode === "highlight")
        return Math.abs(x - p.x) < 0.04 && h === p.h ? dimmed(a, fade) : root.off
      if (root.effect.mode === "laser") {
        var beam = Math.abs(x - p.x) - p.age * 1.2
        return h === p.h && beam < 0 && beam > -0.25 ? dimmed(a, fade) : root.off
      }
      var dist = Math.sqrt(Math.pow((x - p.x) * 18, 2) + Math.pow(h - p.h, 2))
      var ring = Math.abs(dist - p.age * 12)
      return ring < 1.5 ? dimmed(a, fade * (1 - ring / 1.5)) : root.off
    }
    default: return dimmed(a, 1)
    }
  }

  Column {
    anchors.fill: parent
    anchors.margins: root.gap
    spacing: root.gap
    Repeater {
      model: root.rows
      Row {
        id: keyRow
        required property var modelData
        required property int index
        readonly property real units: modelData.reduce(function(s, w) { return s + w }, 0)
        readonly property real unitW: (root.width - root.gap * 2 - root.gap * (modelData.length - 1)) / units
        spacing: root.gap
        Repeater {
          model: keyRow.modelData
          Rectangle {
            required property var modelData
            required property int index
            readonly property real before: keyRow.modelData.slice(0, index).reduce(function(s, w) { return s + w }, 0)
            width: keyRow.unitW * modelData + root.gap * (modelData - 1)
            height: root.rowH
            radius: Math.min(3, height / 5)
            color: root.keyColor((before + modelData / 2) / keyRow.units, root.rows.length - 1 - keyRow.index, root.phase, root.bands)
          }
        }
      }
    }
    // lightbar
    Rectangle {
      width: parent.width
      height: root.rowH * 0.6
      radius: height / 2
      color: root.musicOn && root.music
        ? root.dimmed(root.music.scheme === "gradient" ? root.mix(root.rgb(root.music.colour1), root.rgb(root.music.colour2), root.loudness)
            : root.music.scheme === "rainbow" ? root.hue(root.loudness * 0.8) : root.rgb(root.music.colour1), root.loudness)
        : root.keyColor(0.5, 0, root.phase, root.bands)
    }
  }
}
