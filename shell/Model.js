// Pure helpers for the omalive bar widget, shared with the node tests.

var WAVE_COLUMNS = 14
// Levels kept for the panel's wide waveform (10 s at 12 Hz).
var HISTORY = 120
var ROWS = 9
// Quietest level the waveform shows, in dBFS.
var FLOOR_DB = -60

function emptyStatus() {
  return {
    voice: "off", listeners: 0, camera: "off", format: "", look: "",
    bypass: false, window: false,
    theme: {
      fg: "#c0caf5", muted: "#565f89", accent: "#7aa2f7", cyan: "#7dcfff", blue: "#7aa2f7",
      magenta: "#bb9af7", red: "#f7768e", orange: "#ff9e64", yellow: "#e0af68", green: "#9ece6a"
    }
  }
}

// One line from `omalive status --follow`: {type: "status" | "level" | "picture" | "offline", ...}.
function decode(line) {
  var message = JSON.parse(line)
  if (!message || typeof message.type !== "string") throw new Error("not an omalive message")
  if (message.type === "status") {
    var status = Object.assign(emptyStatus(), message)
    status.theme = Object.assign(emptyStatus().theme, message.theme || {})
    return status
  }
  return message
}

// A level message's peak (dBFS) as a waveform height, 0…1.
function levelHeight(peakDb) {
  if (typeof peakDb !== "number" || !isFinite(peakDb)) return 0
  return Math.max(0, Math.min(1, (peakDb - FLOOR_DB) / -FLOOR_DB))
}

// Newest value last, at most `limit` values.
function push(history, value, limit) {
  var next = history.slice(Math.max(0, history.length - limit + 1))
  next.push(value)
  return next
}

// Whether a waveform dot `row` (0 = top of `rows`, ROWS by default) is lit
// for a column of height `value`: mirrored around the centre row.
function waveLit(value, row, rows) {
  var centre = ((rows || ROWS) - 1) / 2
  var distance = Math.abs(row - centre)
  if (value <= 0.02) return false
  return distance <= Math.round(value * centre)
}

// A picture message as an Image source. The app rewrites one file in
// place; the sequence in the query makes each a new source to load.
function pictureSource(message) {
  if (!message || typeof message.path !== "string" || !message.path) return ""
  return "file://" + message.path + "?" + message.sequence
}

// Colour along evenly spaced "#rrggbb" stops at t (0…1), as "#rrggbb".
function gradient(stops, t) {
  t = Math.max(0, Math.min(1, t))
  if (stops.length < 2) return stops[0] || "#ffffff"
  var position = t * (stops.length - 1)
  var i = Math.min(stops.length - 2, Math.floor(position))
  var f = position - i
  var a = parseInt(stops[i].substr(1), 16), b = parseInt(stops[i + 1].substr(1), 16)
  var mix = function(shift) {
    var x = (a >> shift) & 255, y = (b >> shift) & 255
    return Math.round(x + (y - x) * f)
  }
  return "#" + ((1 << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)).toString(16).slice(1)
}

// "live" means a call is hearing you; the bar treats that as on air.
function micOnAir(status) {
  return status.voice === "live" && status.listeners > 0
}

function micText(status) {
  switch (status.voice) {
  case "live":
    if (status.listeners === 0) return "Mic on · no app listening"
    return "Mic on air · " + status.listeners + (status.listeners === 1 ? " app listening" : " apps listening")
  case "muted": return "Mic muted"
  case "starting": return "Mic starting"
  case "no-input": return "Mic waiting for its input"
  default: return "Mic off"
  }
}

function cameraText(status) {
  var look = status.bypass ? "original" : (status.look || "no look")
  switch (status.camera) {
  case "live": return "Camera on air · " + status.format + " · " + look
  case "ready": return "Camera on · " + look
  case "standby": return "Camera standby · wakes for a call"
  case "starting": return "Camera starting"
  case "failed": return "Camera failed"
  default: return "Camera off"
  }
}

function tooltip(status, online) {
  if (!online) return "omalive is not running · click to start it"
  return micText(status) + "\n" + cameraText(status) + "\nClick for controls · middle-click mutes · right-click opens omalive"
}

if (typeof module !== "undefined") module.exports = {
  WAVE_COLUMNS: WAVE_COLUMNS, HISTORY: HISTORY, ROWS: ROWS,
  emptyStatus: emptyStatus, decode: decode, levelHeight: levelHeight, push: push,
  waveLit: waveLit, pictureSource: pictureSource, gradient: gradient, micOnAir: micOnAir,
  micText: micText, cameraText: cameraText, tooltip: tooltip
}
