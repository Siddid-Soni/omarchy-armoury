import QtQuick
import qs.Commons
import qs.Ui

Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var b: client && client.snap ? (client.snap.battery_info || {}) : {}
  readonly property bool usable: client && client.active
  property int custom: 80

  contentHeight: col.implicitHeight
  clip: true

  function fmt(v, digits, unit) { return v === undefined || v === null ? "—" : Number(v).toFixed(digits) + (unit ? " " + unit : "") }

  Column {
    id: col
    width: Math.min(root.width, Style.space(560))
    spacing: Style.space(16)

    Section { text: "BATTERY"; fg: root.fg }
    Column {
      width: parent.width
      spacing: Style.spacing.labelGap
      InfoRow { fg: root.fg; label: "Charge"; value: (root.b.capacity !== undefined ? root.b.capacity + "%" : "—") + (root.b.status ? " · " + root.b.status : "") }
      InfoRow { fg: root.fg; label: "Health"; value: root.b.health_pct ? Math.round(root.b.health_pct) + "% (" + root.fmt(root.b.full_wh, 1, "") + " of " + root.fmt(root.b.design_wh, 1, "Wh") + ")" : "—" }
      InfoRow { fg: root.fg; label: "Charge cycles"; value: root.b.cycles ? String(root.b.cycles) : "not reported" }
      InfoRow { fg: root.fg; label: "Voltage"; value: root.fmt(root.b.voltage_v, 2, "V") }
      InfoRow { fg: root.fg; label: "Draw"; value: root.b.draw_w && root.b.draw_w > 0.5 ? root.fmt(root.b.draw_w, 1, "W") : "—" }
      InfoRow { fg: root.fg; label: "Time left"; value: root.b.time_left_min ? Math.floor(root.b.time_left_min / 60) + "h " + (root.b.time_left_min % 60) + "m" : "—" }
    }

    Section { text: "CHARGE LIMIT"; fg: root.fg }
    ChoiceRow {
      fg: root.fg
      usable: root.usable
      options: [{ label: "60%", value: 60 }, { label: "80%", value: 80 }, { label: "100%", value: 100 }]
      value: root.b.charge_limit
      onChosen: function(v) { root.client.run({ cmd: "set_charge_limit", percent: v }) }
    }
    Row {
      spacing: Style.space(10)
      NumberField {
        label: "Custom"
        value: root.b.charge_limit || 80
        from: 20
        to: 100
        foreground: root.fg
        onModified: function(v) { root.custom = v }
      }
      Button {
        anchors.bottom: parent.bottom
        text: "Set"
        bordered: true
        foreground: root.fg
        enabled: root.usable
        onClicked: root.client.run({ cmd: "set_charge_limit", percent: root.custom })
      }
    }
    Button {
      text: "Charge to 100% once"
      bordered: true
      foreground: root.fg
      enabled: root.usable
      onClicked: root.client.run({ cmd: "one_shot_charge" })
    }
  }
}
