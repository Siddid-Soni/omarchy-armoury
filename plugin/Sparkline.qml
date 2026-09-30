import QtQuick
import qs.Commons

// Label · current value, over a small line graph of recent values.
// `series`: [{ values: [...], color }]; the first series' last value is `value` unless set.
Column {
  id: root
  property string label: ""
  property string value: ""
  property var series: []
  property real minimum: NaN      // NaN = fit the data
  property real maximum: NaN
  // when fitting the data, show at least this range, so small wobbles stay small
  property real minSpan: 0
  // keep the fitted range inside these
  property real floor: -Infinity
  property real ceiling: Infinity
  property real graphHeight: Style.space(44)
  property color fg: Color.foreground
  property string fontFamily: Style.font.family
  // set from the window's open state: a hidden canvas doesn't paint, so repaint when shown
  property bool shown: true
  onShownChanged: if (shown) Qt.callLater(canvas.requestPaint)

  width: parent ? parent.width : 0
  spacing: Style.space(4)

  Item {
    width: parent.width
    height: labelText.implicitHeight
    Text { id: labelText; text: root.label; color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption }
    Text { anchors.right: parent.right; text: root.value; color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.caption; font.bold: true }
  }

  Canvas {
    id: canvas
    width: parent.width
    height: root.graphHeight
    onPaint: {
      var ctx = getContext("2d")
      ctx.reset()
      var lo = root.minimum, hi = root.maximum
      if (isNaN(lo) || isNaN(hi)) {
        var all = []
        for (var s = 0; s < root.series.length; s++) all = all.concat(root.series[s].values || [])
        if (all.length === 0) return
        var mn = Math.min.apply(null, all), mx = Math.max.apply(null, all)
        var pad = Math.max((mx - mn) * 0.15, 1)
        var grow = Math.max(0, root.minSpan - (mx - mn + 2 * pad)) / 2
        if (isNaN(lo)) lo = Math.max(root.floor, mn - pad - grow)
        if (isNaN(hi)) hi = Math.min(root.ceiling, mx + pad + grow)
      }
      var span = Math.max(hi - lo, 1e-6)
      for (var i = 0; i < root.series.length; i++) {
        var v = root.series[i].values || []
        if (v.length === 1) v = [v[0], v[0]]   // one reading so far: a flat line, not a blank graph
        if (v.length < 2) continue
        var c = root.series[i].color
        var step = width / (v.length - 1)
        var y = function(x) { return height - 1 - (Math.min(Math.max(x, lo), hi) - lo) / span * (height - 2) }
        ctx.beginPath()
        ctx.moveTo(0, y(v[0]))
        for (var k = 1; k < v.length; k++) ctx.lineTo(k * step, y(v[k]))
        ctx.strokeStyle = c
        ctx.lineWidth = 1.5
        ctx.stroke()
        if (i === 0) {
          // soft fill under the first series
          ctx.lineTo(width, height)
          ctx.lineTo(0, height)
          ctx.closePath()
          ctx.fillStyle = Qt.rgba(c.r, c.g, c.b, 0.12)
          ctx.fill()
        }
      }
    }
    Connections { target: root; function onSeriesChanged() { canvas.requestPaint() } }
    onWidthChanged: requestPaint()
  }
}
