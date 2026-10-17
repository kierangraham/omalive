import QtQuick
import qs.Commons

// Render-test stand-in for Ui/KeyboardPanel, which is a layer-shell window
// and cannot load offscreen. Draws the same card inside the test window.
Item {
  id: root
  objectName: "panelShim"
  required property Item anchorItem
  required property QtObject bar
  property var owner: null
  property int padding: Style.spacing.popupPadding
  property int contentWidth: Style.space(280)
  property int contentHeight: Style.space(200)
  property bool open: false
  property Item focusTarget: null
  default property alias contentItem: holder.children
  visible: open
  z: 100
  width: contentWidth + padding * 2
  height: contentHeight + padding * 2
  x: anchorItem ? anchorItem.mapToItem(parent, anchorItem.width, 0).x - width : 0
  y: (anchorItem ? anchorItem.height : 0) + Style.space(8)
  onOpenChanged: if (open && focusTarget) focusTarget.forceActiveFocus()
  function fittedContentWidth(width, cap) { return Math.round(cap > 0 ? Math.min(width, cap) : width) }
  function fittedContentHeight(height, cap) {
    var desired = height + padding * 2
    return Math.round(cap > 0 ? Math.min(desired, cap) : desired) - padding * 2
  }
  function close() {}
  Rectangle { anchors.fill: parent; color: Color.popups.background; border.color: Color.popups.border; border.width: Math.max(1, Style.space(2)) }
  Item { id: holder; x: root.padding; y: root.padding; width: root.contentWidth; height: root.contentHeight }
}
