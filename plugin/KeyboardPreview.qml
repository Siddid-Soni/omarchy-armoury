import QtQuick
import qs.Commons

// A small drawing of the G533's lights in the current lighting: the keys, the logo, the
// display bar (two LEDs, drawn above the keys) and the front light bar (six LEDs: a bar up
// each side, an L round each bottom corner, two along the bottom). Effects colour every light by its position (a hint
// of the animation for Breathe and the rainbows); music draws live bars on the keys and pulses
// the other lights with loudness, as armouryd does.
Item {
  id: root
  property var effect: null        // snapshot lighting.effect
  property int brightness: 3       // 0..3
  property bool musicOn: false
  property var bands: []           // music band levels, bass → treble
  property var music: null         // config.toml [music]
  property color fg: Color.foreground

  // The G533's lights on a key-unit grid (1 = one key pitch), copied from ASUS's top view:
  // a small media row over F1–F5, a short F-row, five rows of square keys with the side
  // column of media keys, half-height arrows dropping below the bottom row, the logo and the
  // display bar (two LEDs) above, and the front bar (six LEDs) below the keys and up both sides. `row` counts key rows up from the bottom.
  readonly property int rowCount: 7
  readonly property var keys: {
    var k = [], g = 0.14
    function key(x, y, w, h, row) { k.push({ x: x, y: y, w: w - g, h: h, row: row }) }
    function line(x, y, widths, row) { for (var i = 0; i < widths.length; i++) { if (widths[i] > 0) key(x, y, widths[i], 1 - g, row); x += Math.abs(widths[i]) } }
    for (var m = 0; m < 5; m++) key(1.4 + m, 0, 1, 0.42, 6)                       // Vol−, Vol+, mic, fan, Armoury Crate
    key(0, 0.55, 1, 0.48, 5)                                                       // Esc
    for (var f = 0; f < 12; f++) key(1.4 + f + Math.floor(f / 4) * 0.25, 0.55, 1, 0.48, 5)  // F1–F12
    key(14, 0.55, 1, 0.48, 5)                                                      // Delete
    key(15.35, 0.55, 1, 0.48, 5)                                                   // side column: Home
    var y = 1.2
    line(0, y, [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2], 4)                      // ` 1–0 - = Backspace
    line(0, y + 1, [1.5, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1.5], 3)              // Tab Q–] \
    line(0, y + 2, [1.75, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2.25], 2)               // Caps A–' Enter
    line(0, y + 3, [2.25, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2.75], 1)                  // Shift Z–/ Shift
    line(0, y + 4, [1.25, 1, 1, 1.25, 5.5, 1, 1], 0)                               // Ctrl Fn Win Alt Space Alt Ctrl
    for (var c = 0; c < 5; c++) key(15.35, y + c, 1, 1 - g, 4 - c)                // side column: play, stop, prev, next, PrtSc
    key(13, y + 4.18, 1, 0.44, 0)                                                  // Up
    for (var a = 0; a < 3; a++) key(12 + a, y + 4.66, 1, 0.44, 0)                 // Left Down Right
    return k
  }
  readonly property real gridW: 16.35          // keys: Esc to the side column
  // the whole drawing, keys plus lights, in grid units
  readonly property real minX: -0.75
  readonly property real maxX: 17.1
  readonly property real minY: -1.75
  readonly property real maxY: 7.75
  readonly property real u: Math.min(width / (maxX - minX), height / (maxY - minY))
  readonly property real ox: (width - (maxX - minX) * u) / 2 - minX * u
  readonly property real oy: (height - (maxY - minY) * u) / 2 - minY * u
  readonly property real bar: 0.3              // light bar thickness, a third of a key
  function gx(x) { return ox + x * u }
  function gy(y) { return oy + y * u }
  readonly property real keyRadius: Math.min(3, u * 0.15, Style.cornerRadius)
  readonly property real barRadius: Style.cornerRadius > 0 ? bar * u / 2 : 0

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
  function mix(a, b, t) { return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, a.a + (b.a - a.a) * t) }
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
    return { x: rand(0.3, 0.7, cycle), h: Math.floor(rand(0.9, 0.1, cycle) * root.rowCount), age: frac(phase / 1.2) }
  }

  // colour of the key centred at x (0..1), `h` rows up from the bottom row
  function keyColor(x, h, phase, bands) {
    if (root.level === 0) return root.off
    if (root.musicOn && root.music) {
      var c1 = rgb(root.music.colour1), c2 = rgb(root.music.colour2)
      var band = Math.min(17, Math.floor(x * 18))
      var colour = root.music.scheme === "rainbow" ? hue(band / 17 * 0.8)
        : root.music.scheme === "single" ? c1 : mix(c1, c2, h / (root.rowCount - 1))
      if (root.music.style === "pulse") return dimmed(root.music.scheme === "gradient" ? mix(c1, c2, root.loudness) : colour, root.loudness)
      var v = bands && bands.length ? bands[band] || 0 : 0
      return dimmed(colour, Math.max(0, Math.min(1, v * root.rowCount - h)))
    }
    if (!root.effect) return root.off
    var a = rgb(root.effect.colour1), b = rgb(root.effect.colour2)
    var rowsN = root.rowCount
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

  // colour of a light that isn't a key, at x (0..1) and `h` rows up from the bottom row:
  // the effect there, or music's loudness pulse
  function lightColor(x, h, phase, bands) {
    if (root.musicOn && root.music) {
      if (root.level === 0) return root.off
      var c1 = rgb(root.music.colour1), c2 = rgb(root.music.colour2)
      return dimmed(root.music.scheme === "gradient" ? mix(c1, c2, root.loudness)
        : root.music.scheme === "rainbow" ? hue(root.loudness * 0.8) : c1, root.loudness)
    }
    return keyColor(x, h, phase, bands)
  }

  // logo (top left) and display bar (two long segments over the right half) above the keys
  RogLogo {
    x: root.gx(-0.5)
    y: root.gy(-1.75)
    height: 1.45 * root.u
    width: height * 8662 / 5083
    color: root.lightColor(0.02, root.rowCount, root.phase, root.bands)
  }
  Repeater {
    model: [[5.9, 10.9], [11.2, 16.35]]
    Rectangle {
      required property var modelData
      x: root.gx(modelData[0])
      y: root.gy(-1.025 - root.bar / 2)
      width: (modelData[1] - modelData[0]) * root.u
      height: root.bar * root.u
      radius: root.barRadius
      color: root.lightColor((modelData[0] + modelData[1]) / 2 / root.gridW, root.rowCount, root.phase, root.bands)
    }
  }

  Repeater {
    model: root.keys
    Rectangle {
      required property var modelData
      x: root.gx(modelData.x)
      y: root.gy(modelData.y)
      width: modelData.w * root.u
      height: modelData.h * root.u
      radius: root.keyRadius
      color: root.keyColor(Math.min(1, (modelData.x + modelData.w / 2) / root.gridW), modelData.row, root.phase, root.bands)
    }
  }

  // front light bar: six LEDs, left → right: a bar up the left side, an L round the bottom-left
  // corner, two along the bottom, an L round the bottom-right corner, a bar up the right side
  readonly property real riseTop: 2.6
  readonly property real riseSplit: 5.0        // where the side bar ends and the L begins
  readonly property real armW: 2.4             // the L's foot along the bottom
  Repeater {
    model: 6
    Item {
      id: led
      required property int index
      readonly property bool isLeft: index < 3
      readonly property real sideX: isLeft ? root.minX : root.maxX - root.bar
      readonly property real span: root.maxX - root.minX - 2 * (root.armW + 0.25)   // between the L feet
      readonly property real segW: (span - 0.25) / 2
      readonly property color c: root.lightColor(index / 5, -1, root.phase, root.bands)
      // side bar (LEDs 0 and 5)
      Rectangle {
        visible: led.index === 0 || led.index === 5
        x: root.gx(led.sideX)
        y: root.gy(root.riseTop)
        width: root.bar * root.u
        height: (root.riseSplit - 0.25 - root.riseTop) * root.u
        radius: root.barRadius
        color: led.c
      }
      // the L (LEDs 1 and 4): up the side from the corner, and along the bottom
      Rectangle {
        visible: led.index === 1 || led.index === 4
        x: root.gx(led.sideX)
        y: root.gy(root.riseSplit)
        width: root.bar * root.u
        height: (root.maxY - root.bar - root.riseSplit) * root.u   // meets the foot: no overlap
        radius: root.barRadius
        color: led.c
      }
      Rectangle {
        visible: led.index === 1 || led.index === 4
        x: led.isLeft ? root.gx(root.minX) : root.gx(root.maxX - root.armW)
        y: root.gy(root.maxY - root.bar)
        width: root.armW * root.u
        height: root.bar * root.u
        radius: root.barRadius
        color: led.c
      }
      // along the bottom (LEDs 2 and 3)
      Rectangle {
        visible: led.index === 2 || led.index === 3
        x: root.gx(root.minX + root.armW + 0.25 + (led.index - 2) * (led.segW + 0.25))
        y: root.gy(root.maxY - root.bar)
        width: led.segW * root.u
        height: root.bar * root.u
        radius: root.barRadius
        color: led.c
      }
    }
  }
}
