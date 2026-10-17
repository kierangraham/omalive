import QtQuick
import QtQuick.Controls as QQC
import Quickshell
import qs.Ui
import qs.Commons
import "Model.js" as Model

// omalive in the bar: one quiet pill. Five bars are your mic's last few
// levels, the dot is the camera's tally light; both turn red while a call
// is hearing or seeing you. Left-click opens quick controls, middle-click
// mutes, right-click opens the omalive window.
Panel {
  id: root
  moduleName: "kierangraham.omalive"
  manageIpc: false
  readonly property var service: bar && bar.shell ? bar.shell.serviceFor(moduleName) : null
  readonly property var status: service ? service.status : Model.emptyStatus()
  readonly property bool online: !!service && service.online
  readonly property var theme: status.theme
  readonly property var waveStops: [theme.cyan, theme.blue, theme.magenta, theme.red]
  readonly property color statusInk: bar ? bar.barForeground : Color.foreground
  readonly property color ink: Color.popups.text
  readonly property color subdued: Qt.rgba(ink.r, ink.g, ink.b, 0.6)
  readonly property bool showMic: setting("showMic", true)
  readonly property bool showCamera: setting("showCamera", true)
  readonly property bool micOnAir: !!service && service.micOnAir
  readonly property bool cameraOnAir: !!service && service.cameraOnAir
  // The panel's waveform.
  readonly property string waveMode: !online ? "off"
    : status.voice === "muted" ? "muted"
    : status.voice === "no-input" ? "waiting"
    : status.voice === "live" ? "air"
    : "off"
  // Exported for the smoke test.
  readonly property string levelMode: !online ? "off"
    : status.voice === "muted" ? "muted"
    : status.voice === "no-input" ? "waiting"
    : status.voice === "live" ? "level"
    : "off"
  readonly property string cameraMode: !online ? "off"
    : status.camera === "live" ? "air"
    : status.camera === "ready" ? "capturing"
    : "off"
  readonly property Item popupAnchor: button
  // Exposed for the smoke test's renders.
  readonly property Item panelContents: contents

  // The panel's camera picture flows only while the panel is open.
  property bool watching: false
  function syncWatching() {
    var want = opened && !!service
    if (want === watching) return
    watching = want
    service.watch(want)
  }
  onOpenedChanged: syncWatching()
  onServiceChanged: syncWatching()
  Component.onDestruction: if (watching && service) service.watch(false)

  implicitWidth: button.implicitWidth
  implicitHeight: bar ? bar.barSize : Style.bar.sizeHorizontal

  WidgetButton {
    id: button
    anchors.centerIn: parent
    bar: root.bar
    labelVisible: false
    hasVisualContent: true
    dimmed: !root.online
    fixedWidth: capsule.width + Style.space(10)
    tooltipText: Model.tooltip(root.status, root.online)
    onPressed: function(mouseButton) {
      if (!root.service) return
      if (mouseButton === Qt.MiddleButton) { root.service.toggleMute(); return }
      if (mouseButton === Qt.RightButton) { root.service.openWindow(); return }
      if (!root.online) root.service.launch()
      root.toggle()
    }
    Capsule {
      id: capsule
      anchors.centerIn: parent
      height: Style.space(14)
      width: implicitWidth
      levels: root.service ? root.service.levels : []
      stops: root.waveStops
      ink: root.statusInk
      red: root.theme.red
      green: root.theme.green
      mic: root.levelMode
      micOnAir: root.micOnAir
      camera: root.cameraMode
      showMic: root.showMic || !root.showCamera
      showCamera: root.showCamera
    }
  }

  component Caption: Text {
    textFormat: Text.PlainText
    color: root.subdued
    font.family: root.bar ? root.bar.fontFamily : Style.font.family
    font.pixelSize: Style.font.caption
    renderType: Text.NativeRendering
  }
  component Heading: Text {
    textFormat: Text.PlainText
    color: root.subdued
    font.family: root.bar ? root.bar.fontFamily : Style.font.family
    font.pixelSize: Style.font.caption
    font.bold: true
    font.letterSpacing: Style.spaceReal(1.2)
    font.capitalization: Font.AllUppercase
    renderType: Text.NativeRendering
  }
  component Choice: Button {
    foreground: root.ink
    fontFamily: root.bar ? root.bar.fontFamily : Style.font.family
    fontSize: Style.font.caption
    bordered: true
  }

  KeyboardPanel {
    id: popup
    anchorItem: root.popupAnchor
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keys
    contentWidth: popup.fittedContentWidth(Style.space(360))
    contentHeight: popup.fittedContentHeight(contents.implicitHeight, Style.space(640))

    PanelKeyCatcher {
      id: keys
      anchors.fill: parent
      onCloseRequested: root.close()
      onTextKey: function(text) {
        if (!root.service || !root.online) return
        switch (text) {
        case "m": root.service.toggleMute(); break
        case "v": root.service.toggleVoice(); break
        case "c": root.service.toggleCamera(); break
        case "o": root.service.openWindow(); root.close(); break
        }
      }

      Column {
        id: contents
        width: parent.width
        spacing: Style.space(14)

        Row {
          width: parent.width
          Column {
            width: parent.width - liveLabel.implicitWidth
            spacing: Style.space(3)
            Text {
              text: "omalive"
              color: root.ink
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.title; font.bold: true
              renderType: Text.NativeRendering
            }
            Caption {
              text: root.online ? "Voice and camera for calls" : "Not running"
            }
          }
          Text {
            id: liveLabel
            readonly property bool onAir: root.service && (root.service.micOnAir || root.service.cameraOnAir)
            text: !root.online ? "● OFFLINE" : onAir ? "● ON AIR" : "● READY"
            color: !root.online ? Color.urgent : onAir ? root.theme.red : root.theme.green
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption; font.bold: true
            anchors.verticalCenter: parent.verticalCenter
            renderType: Text.NativeRendering
          }
        }

        Column {
          visible: !root.online
          width: parent.width
          spacing: Style.space(10)
          Caption {
            width: parent.width; wrapMode: Text.WordWrap
            text: root.service && root.service.error ? root.service.error
              : "omalive is starting in the background…"
          }
          Choice { text: "Start omalive"; onClicked: root.service.launch() }
        }

        Column {
          visible: root.online
          width: parent.width
          spacing: Style.space(14)

          // ---- mic
          Column {
            width: parent.width
            spacing: Style.space(6)
            Row {
              width: parent.width
              Heading { text: "Mic"; width: parent.width - micSwitch.width; anchors.verticalCenter: parent.verticalCenter }
              ToggleSwitch {
                id: micSwitch
                checked: root.status.voice !== "off"
                foreground: root.ink
                accent: root.theme.accent
                onToggled: root.service.toggleVoice()
              }
            }
            Rectangle {
              width: parent.width; height: Style.space(44)
              radius: Style.space(3)
              color: Qt.rgba(0, 0, 0, 0.14)
              border.width: 1; border.color: Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.18)
              DotWave {
                anchors.fill: parent; anchors.margins: Style.space(6)
                columns: Math.max(Model.WAVE_COLUMNS, Math.min(Model.HISTORY, Math.floor(width / (height / Model.ROWS))))
                // Only while open: a closed panel's dots should cost nothing.
                levels: root.opened && root.service ? root.service.levels : []
                stops: root.waveStops
                ink: root.ink
                mode: root.waveMode
                mutedColor: root.theme.red
                waitingColor: root.theme.yellow
              }
            }
            Row {
              width: parent.width
              Caption { text: Model.micText(root.status); width: parent.width - muteButton.width; anchors.verticalCenter: parent.verticalCenter; elide: Text.ElideRight }
              Choice {
                id: muteButton
                text: root.status.voice === "muted" ? "Unmute · m" : "Mute · m"
                visible: root.status.voice === "live" || root.status.voice === "muted"
                active: root.status.voice === "muted"
                onClicked: root.service.toggleMute()
              }
            }
          }

          PanelSeparator { width: parent.width; foreground: root.ink }

          // ---- camera
          Column {
            width: parent.width
            spacing: Style.space(6)
            Row {
              width: parent.width
              Heading { text: "Camera"; width: parent.width - cameraSwitch.width; anchors.verticalCenter: parent.verticalCenter }
              ToggleSwitch {
                id: cameraSwitch
                checked: root.status.camera !== "off" && root.status.camera !== "failed"
                foreground: root.ink
                accent: root.theme.accent
                onToggled: root.service.toggleCamera()
              }
            }
            Rectangle {
              width: parent.width; height: width * 9 / 16
              radius: Style.space(3)
              color: Qt.rgba(0, 0, 0, 0.14)
              border.width: 1; border.color: Qt.rgba(root.ink.r, root.ink.g, root.ink.b, 0.18)
              clip: true
              Image {
                id: picture
                anchors.fill: parent; anchors.margins: 1
                visible: !!root.service && root.service.cameraFlowing && status === Image.Ready
                source: root.service ? root.service.picture : ""
                // One small file rewritten in place: read it fresh each time,
                // synchronously so frames swap without a blank between.
                cache: false
                asynchronous: false
                smooth: true
                fillMode: Image.PreserveAspectCrop
              }
              Caption {
                visible: !picture.visible
                anchors.centerIn: parent
                text: root.status.camera === "off" ? "Camera off"
                  : root.status.camera === "failed" ? "Camera failed · open omalive for details"
                  : root.service && root.service.cameraFlowing ? "Starting the picture…"
                  : "Asleep until an app or the preview uses it"
              }
            }
            Caption { text: Model.cameraText(root.status); width: parent.width; elide: Text.ElideRight }
          }

          PanelSeparator { width: parent.width; foreground: root.ink }

          Row {
            spacing: Style.space(6)
            Choice { text: "Open omalive · o"; onClicked: { root.service.openWindow(); root.close() } }
            Choice { text: "Quit"; onClicked: { root.service.quit(); root.close() } }
          }
        }
      }
    }
  }
}
