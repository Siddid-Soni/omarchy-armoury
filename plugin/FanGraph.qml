import QtQuick
import qs.Commons

// 8-point fan curve editor: x = temperature (20–100 °C), y = fan speed (0–100 %).
// Drag a point to move it; points stay in order (temperatures and speeds never decrease).
// Shift-drag moves the whole curve up or down.
Item {
  id: root
  property string title: ""
  property var temps: [30, 40, 50, 60, 70, 80, 90, 100]
  property var percent: [0, 10, 20, 30, 40, 60, 80, 100]
  property color lineColor: Color.accent
  property color fg: Color.foreground
  property string fontFamily: Style.font.family
  property bool usable: true
  signal edited()

  readonly property real tMin: 20
  readonly property real tMax: 100
  readonly property real pad: Style.space(28)
  property int dragIndex: -1
  property real dragStartY: 0
  property var dragStartPercent: []

  function px(t) { return pad + (t - tMin) / (tMax - tMin) * (width - pad * 1.5) }
  function py(p) { return height - pad - p / 100 * (height - pad * 1.8) }
  function toT(x) { return tMin + (x - pad) / (width - pad * 1.5) * (tMax - tMin) }
  function toP(y) { return (height - pad - y) / (height - pad * 1.8) * 100 }
  function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)) }

  onTempsChanged: canvas.requestPaint()
  onPercentChanged: canvas.requestPaint()
  onWidthChanged: canvas.requestPaint()
  onHeightChanged: canvas.requestPaint()

  Text {
    x: root.pad
    y: Style.space(2)
    text: root.title
    color: root.fg
    font.family: root.fontFamily
    font.pixelSize: Style.font.bodySmall
    font.bold: true
  }

  Canvas {
    id: canvas
    anchors.fill: parent
    onPaint: {
      var ctx = getContext("2d")
      ctx.reset()
      var grid = Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)
      ctx.strokeStyle = grid
      ctx.lineWidth = 1
      ctx.fillStyle = Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.55)
      ctx.font = Math.round(Style.font.caption) + "px " + root.fontFamily
      for (var p = 0; p <= 100; p += 20) {
        var y = root.py(p)
        ctx.beginPath(); ctx.moveTo(root.pad, y); ctx.lineTo(root.width - root.pad / 2, y); ctx.stroke()
        ctx.fillText(p + "%", 0, y + 4)
      }
      for (var t = 20; t <= 100; t += 20) {
        var x = root.px(t)
        ctx.beginPath(); ctx.moveTo(x, root.py(0)); ctx.lineTo(x, root.py(100)); ctx.stroke()
        ctx.fillText(t + "°", x - 8, root.height - 6)
      }
      ctx.strokeStyle = root.lineColor
      ctx.lineWidth = 2
      ctx.beginPath()
      for (var i = 0; i < 8; i++) {
        var px = root.px(root.temps[i]), py = root.py(root.percent[i])
        if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py)
      }
      ctx.stroke()
      ctx.fillStyle = root.lineColor
      for (var j = 0; j < 8; j++) {
        ctx.beginPath()
        ctx.arc(root.px(root.temps[j]), root.py(root.percent[j]), j === root.dragIndex ? 6 : 4.5, 0, Math.PI * 2)
        ctx.fill()
      }
    }
  }

  MouseArea {
    anchors.fill: parent
    enabled: root.usable
    onPressed: function(mouse) {
      var best = -1, bestD = 18 * 18
      for (var i = 0; i < 8; i++) {
        var dx = root.px(root.temps[i]) - mouse.x, dy = root.py(root.percent[i]) - mouse.y
        if (dx * dx + dy * dy < bestD) { bestD = dx * dx + dy * dy; best = i }
      }
      root.dragIndex = best
      root.dragStartY = mouse.y
      root.dragStartPercent = root.percent.slice()
      canvas.requestPaint()
    }
    onPositionChanged: function(mouse) {
      var i = root.dragIndex
      if (i < 0) return
      var p = root.percent.slice(), t = root.temps.slice()
      if (mouse.modifiers & Qt.ShiftModifier) {
        var delta = root.toP(mouse.y) - root.toP(root.dragStartY)
        for (var k = 0; k < 8; k++) p[k] = Math.round(root.clamp(root.dragStartPercent[k] + delta, 0, 100))
      } else {
        var lo = i > 0 ? t[i - 1] : root.tMin, hi = i < 7 ? t[i + 1] : root.tMax
        t[i] = Math.round(root.clamp(root.toT(mouse.x), lo, hi))
        var plo = i > 0 ? p[i - 1] : 0, phi = i < 7 ? p[i + 1] : 100
        p[i] = Math.round(root.clamp(root.toP(mouse.y), plo, phi))
      }
      root.temps = t
      root.percent = p
      root.edited()
    }
    onReleased: { root.dragIndex = -1; canvas.requestPaint() }
  }
}
