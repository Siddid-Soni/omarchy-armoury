import QtQuick
import qs.Commons
import qs.Ui

// Section header used by the window's detail pages.
PanelSectionHeader {
  property color fg: Color.foreground
  foreground: fg
  fontFamily: Style.font.family
  width: parent ? parent.width : 0
}
