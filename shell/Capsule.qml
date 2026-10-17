import QtQuick
import "Model.js" as Model

// The bar indicator: one quiet pill. Five bars are your last few mic
// levels, newest on the right, in the theme's hues; the dot is the camera's
// tally light. Grey means off, green that the camera is capturing, red that
// a call is hearing or seeing you.
Item {
  id: root
  property var levels: []
  property var stops: ["#7dcfff", "#7aa2f7", "#bb9af7", "#f7768e"]
  property color ink: "white"
  property color red: "#f7768e"
  property color green: "#9ece6a"
  // "level", "muted", "waiting" or "off".
  property string mic: "off"
  property bool micOnAir: false
  // "off", "capturing" or "air".
  property string camera: "off"
  property bool showMic: true
  property bool showCamera: true
  readonly property int bars: 5
  readonly property real unit: height / 13
  implicitHeight: 13
  implicitWidth: (showMic ? 6 + bars * 3.6 : 4) * unit + (showCamera ? 10 * unit : 3 * unit)

  // The pill.
  Rectangle {
    anchors.fill: parent
    radius: height / 2
    color: Qt.rgba(0, 0, 0, 0.16)
    border.width: 1
    border.color: root.micOnAir ? Qt.rgba(root.red.r, root.red.g, root.red.b, 0.7)
                                : Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.24)
    Behavior on border.color { ColorAnimation { duration: 200 } }
  }

  Repeater {
    model: root.showMic ? root.bars : 0
    Rectangle {
      required property int index
      readonly property real value: {
        if (root.mic !== "level") return 0
        var i = root.levels.length - root.bars + index
        return i >= 0 ? root.levels[i] : 0
      }
      x: (6 + index * 3.6) * root.unit - width / 2
      width: 2 * root.unit
      height: Math.max(1.5 * root.unit, value * 9 * root.unit)
      y: (root.height - height) / 2
      radius: width / 2
      antialiasing: true
      color: root.mic === "muted" ? root.red
        : root.mic === "waiting" ? Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.5)
        : root.mic === "level" ? Model.gradient(root.stops, index / (root.bars - 1))
        : Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.3)
      Behavior on height { NumberAnimation { duration: 70 } }
    }
  }

  // The tally light.
  Rectangle {
    visible: root.showCamera
    width: 6 * root.unit; height: width; radius: width / 2
    anchors.verticalCenter: parent.verticalCenter
    anchors.right: parent.right; anchors.rightMargin: 3.5 * root.unit
    antialiasing: true
    color: root.camera === "air" ? root.red
      : root.camera === "capturing" ? root.green
      : Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.28)
    Behavior on color { ColorAnimation { duration: 200 } }
  }
}
