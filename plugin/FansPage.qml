import QtQuick
import qs.Commons
import qs.Ui

Item {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property bool usable: client && client.active
  property string profile: snap && snap.perf ? (snap.perf.profile || "balanced") : "balanced"
  property var curves: []       // [{fan, temps, percent, enabled}]
  property bool dirty: false
  property string error: ""
  readonly property real graphHeight: Math.max(Style.space(170), (height - top.height - Style.space(90)) / Math.max(1, curves.length) - Style.space(30))

  function load() {
    dirty = false
    client.call({ cmd: "fan_curves", profile: profile }, function(r) {
      if (r.ok) { root.curves = r.data; root.error = "" }
      else root.error = root.usable ? (r.error || "Could not read fan curves") : "Fan curves are read from asusd — press Take over to edit them."
    })
  }
  Component.onCompleted: load()
  onProfileChanged: load()

  // Each graph keeps its own edits (reassigning `curves` would rebuild the graphs mid-drag).
  function applyAll() {
    var list = []
    for (var k = 0; k < fanRepeater.count; k++) {
      var d = fanRepeater.itemAt(k)
      list.push({ fan: d.modelData.fan, temps: d.graph.temps, percent: d.graph.percent, enabled: d.customOn })
    }
    var next = function(i) {
      if (i >= list.length) { root.load(); return }
      root.client.run({ cmd: "set_fan_curve", profile: root.profile, curve: list[i] }, function(r) { if (r.ok) next(i + 1) })
    }
    next(0)
  }

  Column {
    anchors.fill: parent
    spacing: Style.space(12)

    Row {
      id: top
      width: parent.width
      spacing: Style.space(12)
      ChoiceRow {
        width: Style.space(420)
        fg: root.fg
        options: [{ label: "Silent", value: "quiet" }, { label: "Balanced", value: "balanced" }, { label: "Turbo", value: "performance" }]
        value: root.profile
        onChosen: function(v) { root.profile = v }
      }
      Item { width: Math.max(0, top.width - Style.space(420) - applyBtn.width - resetBtn.width - top.spacing * 3); height: 1 }
      Button { id: applyBtn; anchors.bottom: parent.bottom; text: root.dirty ? "Apply" : "Saved"; bordered: true; foreground: root.fg; enabled: root.usable && root.dirty; onClicked: root.applyAll() }
      Button {
        id: resetBtn; anchors.bottom: parent.bottom; text: "Reset to default"; foreground: root.fg; enabled: root.usable
        onClicked: root.client.run({ cmd: "reset_fan_curves", profile: root.profile }, function() { root.load() })
      }
    }

    Text {
      visible: root.error !== ""
      width: parent.width
      wrapMode: Text.WordWrap
      text: root.error
      color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }

    Repeater {
      id: fanRepeater
      model: root.curves
      Column {
        id: fanCol
        required property var modelData
        required property int index
        property bool customOn: modelData.enabled
        property alias graph: graph
        width: parent.width
        spacing: Style.space(4)
        Row {
          spacing: Style.space(10)
          Text {
            text: (fanCol.modelData.fan === "gpu" ? "GPU fan" : fanCol.modelData.fan === "mid" ? "Mid fan" : "CPU fan")
              + (fanCol.index === 0 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.cpu_fan_rpm || 0) + " rpm" : "")
              + (fanCol.index === 1 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm" : "")
            color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true
            anchors.verticalCenter: parent.verticalCenter
          }
          Button {
            text: fanCol.customOn ? "Custom curve on" : "Firmware auto"
            fontSize: Style.font.caption
            bordered: true; foreground: root.fg
            active: fanCol.customOn
            enabled: root.usable
            onClicked: { fanCol.customOn = !fanCol.customOn; root.dirty = true }
          }
        }
        FanGraph {
          id: graph
          width: parent.width
          height: root.graphHeight
          temps: fanCol.modelData.temps
          percent: fanCol.modelData.percent
          lineColor: fanCol.index === 0 ? Color.accent : Qt.lighter(Color.urgent, 1.1)
          fg: root.fg
          usable: root.usable
          onEdited: { fanCol.customOn = true; root.dirty = true }
        }
      }
    }
    Text {
      text: "Drag points to edit · Shift-drag moves the whole curve · Apply saves it for this mode"
      color: root.fg; opacity: 0.55; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
  }
}
