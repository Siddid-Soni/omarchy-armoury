import QtQuick

// Wheel / touchpad scrolling for a page whose Flickable has `interactive: false`
// (declared inside it, so it lands on the content item: give the page contentWidth: width),
// so dragging a slider or a fan point never scrolls the page instead.
WheelHandler {
  id: handler
  required property Flickable flick
  acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
  onWheel: function(e) {
    var dy = e.pixelDelta.y !== 0 ? e.pixelDelta.y : e.angleDelta.y / 2
    var max = Math.max(0, flick.contentHeight - flick.height)
    flick.contentY = Math.max(0, Math.min(max, flick.contentY - dy))
  }
}
