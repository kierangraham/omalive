import QtQuick
import Quickshell
import qs.Commons
import "plugin" as Omalive

// The promo's bar shots, fed by an `omalive --demo` instance through the
// real service. PROMO_LAYOUT=zoom shows a stretch of bar holding the
// widget (record-bar.sh renders it at QT_SCALE_FACTOR=7.5, so text stays
// sharp); PROMO_LAYOUT=panel shows the widget with its quick panel open.
ShellRoot {
  readonly property bool zoomed: Quickshell.env("PROMO_LAYOUT") !== "panel"
  Omalive.Service { id: service }
  QtObject {
    id: shellApi
    function serviceFor(id) { return service }
    function summon(id, payload) {}
    function hide(id) {}
    function toggle(id, payload) {}
  }
  QtObject {
    id: barApi
    property var shell: shellApi
    property color foreground: Color.foreground
    property color barForeground: Color.foreground
    property color background: Color.background
    property color urgent: Color.urgent
    property string fontFamily: "Iosevka NFM"
    property string position: "top"
    property bool vertical: false
    property int barSize: 26
    property bool foregroundAnimationEnabled: false
    property var activePopout: null
    property var clickTargets: []
    function registerClickTarget(target) {}
    function unregisterClickTarget(target) {}
    function hideTooltip(target) {}
    function showTooltip(target, text) {}
    function requestPopout(owner) {}
    function releasePopout(owner) {}
  }

  FloatingWindow {
    implicitWidth: 1840; implicitHeight: 1000
    color: Qt.rgba(Color.background.r, Color.background.g, Color.background.b, 1)

    // A stretch of bar.
    Item {
      id: zoom
      visible: zoomed
      width: 190; height: 26
      anchors.centerIn: parent
      Rectangle {
        anchors.fill: parent
        radius: 4
        color: Qt.darker(Color.background, 1.25)
        border.width: 0.5; border.color: Qt.rgba(1, 1, 1, 0.06)
      }
      Text {
        anchors.verticalCenter: parent.verticalCenter
        x: 12
        text: "17:45"
        color: Qt.rgba(Color.foreground.r, Color.foreground.g, Color.foreground.b, 0.55)
        font.family: "Iosevka NFM"; font.pixelSize: 11; font.weight: Font.DemiBold
      }
      Omalive.BarWidget {
        bar: barApi
        x: 190 - width - 6
        height: 26
      }
    }

    // The bar widget as it sits in the bar, with its panel open.
    Item {
      visible: !zoomed
      x: 1840 / 2 + 330; y: 4
      scale: 1.36
      transformOrigin: Item.TopRight
      Omalive.BarWidget {
        id: panelled
        bar: barApi
        x: -width
        height: 26
        Component.onCompleted: open()
      }
    }
  }
}
