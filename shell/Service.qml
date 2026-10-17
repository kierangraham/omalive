import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import "Model.js" as Model

// One bridge to the running omalive for every bar on every monitor:
// `omalive status --follow` streams state as JSON lines and takes commands
// on stdin. It reconnects by itself while omalive is not running.
Item {
  id: root
  property var shell: null
  property var status: Model.emptyStatus()
  property bool online: false
  // Mic peaks, 0…1, newest last, while the voice runs unmuted. Enough for
  // the panel's wide waveform; the bar shows the last few seconds.
  property var levels: []
  // The camera picture as an Image source, while a panel watches.
  property string picture: ""
  // Open panels that show the picture; the app only sends it while > 0.
  property int watchers: 0
  property string error: ""
  readonly property string binary: Quickshell.env("OMALIVE_BIN") || Quickshell.env("HOME") + "/.local/bin/omalive"
  readonly property bool micOnAir: online && Model.micOnAir(status)
  readonly property bool cameraOnAir: online && status.camera === "live"
  readonly property bool micFlowing: online && status.voice === "live"
  readonly property bool cameraFlowing: online && (status.camera === "live" || status.camera === "ready")

  function ingest(line) {
    var message
    try { message = Model.decode(line) } catch (e) { return }
    switch (message.type) {
    case "status":
      status = message
      if (!online && watchers > 0) send({cmd: "picture", on: true})
      online = true
      error = ""
      if (message.voice !== "live") levels = []
      if (message.camera !== "live" && message.camera !== "ready") picture = ""
      break
    case "level":
      if (status.voice === "live") levels = Model.push(levels, Model.levelHeight(message.peak), Model.HISTORY)
      break
    case "picture":
      if (watchers > 0 && (status.camera === "live" || status.camera === "ready")) picture = Model.pictureSource(message)
      break
    case "offline":
      online = false
      levels = []
      picture = ""
      break
    }
  }

  function send(command) {
    if (bridge.running) bridge.write(JSON.stringify(command) + "\n")
  }
  function toggleVoice() { send({cmd: "toggle-voice"}) }
  function toggleMute() { send({cmd: "toggle-mute"}) }
  function toggleCamera() { send({cmd: "toggle-camera"}) }
  function bypass() { send({cmd: "bypass"}) }
  function look(file) { send({cmd: "look", file: file}) }
  function quit() { send({cmd: "quit"}) }
  // A panel opened or closed.
  function watch(on) {
    var before = watchers
    watchers = Math.max(0, watchers + (on ? 1 : -1))
    if ((before > 0) !== (watchers > 0)) send({cmd: "picture", on: watchers > 0})
    if (watchers === 0) picture = ""
  }
  function openWindow() {
    if (online) send({cmd: "open"})
    else Util.execArgv(["setsid", "uwsm-app", "--", root.binary])
  }
  // Start omalive without a window; the bar fills in as it comes up.
  function launch() {
    Util.execArgv(["setsid", "uwsm-app", "--", root.binary, "--background"])
  }

  Timer { id: retry; interval: 5000; onTriggered: bridge.running = true }
  Process {
    id: bridge
    command: [root.binary, "status", "--follow"]
    running: true
    stdinEnabled: true
    stdout: SplitParser { onRead: function(data) { root.ingest(data) } }
    stderr: SplitParser { onRead: function(data) { root.error = String(data).slice(0, 240) } }
    onExited: function(code, status) {
      root.online = false
      if (!root.error) root.error = "omalive is not installed at " + root.binary
      retry.restart()
    }
  }
  IpcHandler {
    target: "omalive"
    function status(): string { return JSON.stringify({online: root.online, status: root.status}) }
    function open(): void { if (root.shell) root.shell.summon("kierangraham.omalive", "{}") }
    function close(): void { if (root.shell) root.shell.hide("kierangraham.omalive") }
    function toggle(): void { if (root.shell) root.shell.toggle("kierangraham.omalive", "{}") }
    function mute(): void { root.toggleMute() }
  }
}
