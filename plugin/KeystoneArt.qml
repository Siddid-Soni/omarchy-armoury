import QtQuick
import QtQuick.Shapes
import qs.Commons

// Keystone tile drawing: the slot, with the Keystone seated (and glowing red, like its
// light) or lifted out of it. The Keystone is the real part's outline: a translucent red
// head that narrows downwards, with the ROG logo, over a tab with the dark contact pad
// that goes into the slot. Front view, drawn in a 100×58 box.
Item {
  id: root
  property bool inserted: false
  property bool enabled_: true
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property color red: "#ff3040"
  readonly property real gap: Style.space(10)   // between the lifted tab and the slot
  // key width: the lifted key, the gap and the slot fit the tile
  readonly property real kw: Math.min(width * 0.55, (height - Style.space(8 + 14) - gap) * 100 / 58)
  readonly property real u: kw / 100
  // the tab spans x 20–80 of the box
  readonly property real tabX: 20 * u
  readonly property real tabW: 60 * u

  // the slot in the chassis, under the tab
  Rectangle {
    id: slot
    x: key.x + root.tabX - Style.space(6)
    anchors.bottom: parent.bottom
    anchors.bottomMargin: Style.space(8)
    width: root.tabW + Style.space(12)
    height: Style.space(14)
    radius: Style.cornerRadius > 0 ? height / 2 : 0
    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
    border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
  }

  // glow while seated (the Keystone light): a radial fade squashed to an ellipse, so it
  // never reads as a box whatever the corner style; outside the clip below so it isn't cut
  Shape {
    id: glow
    readonly property real w: key.width + Style.space(32)
    readonly property real h: 29 * root.u + Style.space(28)
    x: key.x - Style.space(16); y: key.y - Style.space(14)
    width: w; height: w
    transform: Scale { yScale: glow.h / glow.w }
    preferredRendererType: Shape.CurveRenderer
    opacity: root.inserted && root.enabled_ ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: 250 } }
    ShapePath {
      strokeWidth: -1
      fillGradient: RadialGradient {
        centerX: glow.w / 2; centerY: glow.w / 2; centerRadius: glow.w / 2
        focalX: glow.w / 2; focalY: glow.w / 2
        GradientStop { position: 0; color: Qt.rgba(1, 0.19, 0.25, 0.30) }
        GradientStop { position: 0.55; color: Qt.rgba(1, 0.19, 0.25, 0.12) }
        GradientStop { position: 1; color: Qt.rgba(1, 0.19, 0.25, 0) }
      }
      startX: 0; startY: 0
      PathLine { x: glow.w; y: 0 }
      PathLine { x: glow.w; y: glow.w }
      PathLine { x: 0; y: glow.w }
      PathLine { x: 0; y: 0 }
    }
  }

  // everything above the slot's middle: the seated tab disappears into it
  Item {
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.bottom: slot.verticalCenter
    clip: true

    Item {
      id: key
      width: root.kw
      height: 58 * root.u
      x: (root.width - width) / 2
      // seated: the head's bottom edge sits on the slot; lifted: the tab clears it
      y: root.inserted ? slot.y + slot.height / 2 - 32 * root.u : slot.y - root.gap - 57 * root.u
      opacity: root.inserted ? 1 : 0.45
      Behavior on y { NumberAnimation { duration: 250; easing.type: Easing.OutCubic } }
      Behavior on opacity { NumberAnimation { duration: 250 } }

      Shape {
        width: 100
        height: 58
        preferredRendererType: Shape.CurveRenderer
        transform: Scale { xScale: root.u; yScale: root.u }

        // tab (behind the head)
        ShapePath {
          strokeColor: root.red; strokeWidth: 1.4
          fillColor: Qt.rgba(1, 0.19, 0.25, 0.30)
          joinStyle: ShapePath.RoundJoin
          startX: 20; startY: 28
          PathLine { x: 80; y: 28 }
          PathLine { x: 80; y: 57 }
          PathLine { x: 20; y: 57 }
          PathLine { x: 20; y: 28 }
        }
        // contact pad
        ShapePath {
          strokeWidth: -1
          fillColor: "#2a2a2c"
          startX: 25; startY: 33
          PathLine { x: 75; y: 33 }
          PathLine { x: 75; y: 52 }
          PathLine { x: 25; y: 52 }
          PathLine { x: 25; y: 33 }
        }
        // head: wide top, narrowing to the tab
        ShapePath {
          strokeColor: root.red; strokeWidth: 1.4
          fillColor: Qt.rgba(1, 0.19, 0.25, 0.55)
          joinStyle: ShapePath.RoundJoin
          startX: 1; startY: 1
          PathLine { x: 99; y: 1 }
          PathLine { x: 80; y: 29 }
          PathLine { x: 20; y: 29 }
          PathLine { x: 1; y: 1 }
        }
        // highlight along the top edge
        ShapePath {
          strokeColor: Qt.rgba(1, 0.75, 0.78, 0.6); strokeWidth: 1
          fillColor: "transparent"
          startX: 6; startY: 4
          PathLine { x: 94; y: 4 }
        }
      }

      // the embossed logo
      RogLogo {
        color: Qt.rgba(1, 0.8, 0.82, 0.75)
        implicitHeight: 12 * root.u
        anchors.horizontalCenter: parent.horizontalCenter
        y: 10 * root.u
      }
    }
  }
}
