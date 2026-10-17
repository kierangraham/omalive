import QtQuick
import "Model.js" as Model

// The mic as a dot waveform: columns mirrored around the centre row,
// newest on the right, coloured by loudness through the theme's hues.
// Quiet states draw only the centre row: faint when nobody is listening,
// red when muted.
Item {
  id: root
  property var levels: []
  property var stops: ["#7dcfff", "#7aa2f7", "#bb9af7", "#f7768e"]
  property color ink: "white"
  // "air" (waveform), "idle", "muted", "waiting" or "off".
  property string mode: "off"
  property color mutedColor: "#f7768e"
  property color waitingColor: "#e0af68"
  // The bar shows the last few levels; the panel fills its width.
  property int columns: Model.WAVE_COLUMNS
  // Odd, so there is a centre row. The bar uses fewer, larger dots.
  property int rows: Model.ROWS
  readonly property real pitchX: width / columns
  readonly property real pitchY: height / rows
  readonly property real dotSize: Math.min(pitchX, pitchY) * 0.7
  readonly property int centre: (rows - 1) / 2

  Repeater {
    model: root.columns
    Item {
      id: column
      required property int index
      // Right-aligned: the newest level is always the last column.
      readonly property real value: {
        var i = index - (root.columns - root.levels.length)
        return i >= 0 ? root.levels[i] : 0
      }
      x: (index + 0.5) * root.pitchX
      height: root.height
      Repeater {
        model: root.rows
        Rectangle {
          required property int index
          readonly property bool centreRow: index === root.centre
          readonly property bool lit: root.mode === "air" && Model.waveLit(column.value, index, root.rows)
          x: -width / 2
          y: (index + 0.5) * root.pitchY - height / 2
          width: root.dotSize; height: width; radius: width / 2
          antialiasing: true
          color: lit ? Model.gradient(root.stops, column.value)
            : centreRow && root.mode === "muted" ? root.mutedColor
            : centreRow && root.mode === "waiting" ? root.waitingColor
            : root.ink
          opacity: lit ? 0.6 + 0.4 * column.index / (root.columns - 1)
            : centreRow && (root.mode === "muted" || root.mode === "waiting") ? 0.85
            : centreRow && (root.mode === "idle" || root.mode === "air") ? 0.32
            : 0.06
        }
      }
    }
  }
}
