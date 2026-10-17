import QtQuick
import Quickshell
import "plugin" as Omalive

// Drives the widget against a real `omalive --demo` on a private socket:
// on air, muted, the quick panel, settings, vertical layout and offline.
// With OMALIVE_SMOKE_OUTPUT set it also saves PNG renders of each state.
ShellRoot {
  id: root
  property int step: 0
  property int ticks: 0
  property int pending: 0
  property string picturesSeen: ""
  readonly property string output: Quickshell.env("OMALIVE_SMOKE_OUTPUT")

  Omalive.Service { id: service }
  QtObject {
    id: shellApi
    function serviceFor(id) { return service }
    function summon(id, payload) { widget.open() }
    function hide(id) { widget.close() }
    function toggle(id, payload) { widget.toggle() }
  }
  QtObject {
    id: barApi
    property var shell: shellApi
    property color foreground: "#c0caf5"
    property color barForeground: foreground
    property color background: "#1a1b26"
    property color urgent: "#f7768e"
    property string fontFamily: "monospace"
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
  // Visible only for offscreen renders; the Wayland smoke run stays hidden.
  FloatingWindow {
    visible: !!root.output
    implicitWidth: 460; implicitHeight: 720
    color: "#101315"
    Rectangle {
      id: barStrip
      width: parent.width; height: 26; color: "#1a1b26"
      Omalive.BarWidget { id: widget; bar: barApi; x: 440 - width; height: 26 }
    }
  }

  function fail(message) { console.error("FAIL: " + message); Qt.quit() }
  function card() {
    var item = widget.panelContents
    while (item && item.objectName !== "panelShim") item = item.parent
    return item
  }
  function grab(item, name) {
    if (!output) return
    pending++
    item.grabToImage(function(result) {
      result.saveToFile(output + "/" + name + ".png")
      pending--
    })
  }

  Timer {
    interval: 250; repeat: true; running: true
    onTriggered: {
      root.ticks++
      if (root.pending) return
      var status = service.status
      if (root.ticks > 200) return root.fail("timed out at step " + root.step + " " + JSON.stringify({online: service.online, voice: status.voice, camera: status.camera, levels: service.levels.length, picture: service.picture, watchers: service.watchers}))
      switch (root.step) {
      case 0:
        // The demo talks, and its pretend call listens and watches.
        if (!service.online || status.voice !== "live" || service.levels.length < 10) return
        if (widget.waveMode !== "air") return root.fail("wave mode " + widget.waveMode)
        if (status.theme.magenta[0] !== "#") return root.fail("no theme in status")
        if (service.picture) return root.fail("picture sent with the panel closed")
        break
      case 1: root.grab(barStrip, "bar-on-air"); break
      case 2:
        widget.open()
        break
      case 3:
        // The open panel watches: a real picture arrives and loads.
        if (service.watchers !== 1) return root.fail("panel did not watch: " + service.watchers)
        if (!service.picture.startsWith("file://")) return
        if (!root.output) break
        root.picturesSeen = root.picturesSeen || service.picture
        if (service.picture === root.picturesSeen) return
        root.grab(root.card(), "panel")
        break
      case 4:
        widget.close()
        if (service.watchers !== 0 || service.picture) return root.fail("closing the panel did not stop the picture")
        service.toggleMute()
        break
      case 5:
        if (status.voice !== "muted") return
        if (widget.waveMode !== "muted") return root.fail("muted wave mode " + widget.waveMode)
        break
      case 6: root.grab(barStrip, "bar-muted"); break
      case 7:
        service.toggleMute()
        service.toggleCamera()
        break
      case 8:
        if (status.camera !== "off" || service.picture) return
        break
      case 9: root.grab(barStrip, "bar-camera-off"); break
      case 10:
        widget.settings = {showMic: true, showCamera: false}
        if (widget.showCamera) return root.fail("settings did not apply")
        widget.settings = {}
        barApi.vertical = true
        break
      case 11:
        barApi.vertical = false
        service.quit()
        break
      case 12:
        if (service.online) return
        if (!widget.children.length) return root.fail("widget vanished offline")
        break
      case 13: root.grab(barStrip, "bar-offline"); break
      case 14:
        console.log("PASS: on air, muted, quick panel, camera off, settings, vertical and offline")
        Qt.quit()
      }
      root.step++
    }
  }
}
