# Architecture

Three crates, each depending only on the ones below it. The workspace
forbids unsafe code; `pipewire`, `v4l` and `wgpu` encapsulate the FFI.

| Crate | Owns | Tested by |
| --- | --- | --- |
| `omalive-core` | Settings and validation, voice presets, the filter-chain description, live-control names, the EQ response, the colour grade and its lattice bake, `.cube` parsing and the LUT library, Y'CbCr matrices, and a plain model of the PipeWire graph. No threads and no devices. | Unit tests |
| `omalive-media` | The PipeWire thread and the camera thread. | Unit tests for every pure part; `examples/probe.rs` and `examples/consume.rs` against the live session |
| `omalive-app` | GPUI views, a GPUI-free view-model, and the engine adapters. | Model unit tests and GPUI interaction tests with a recording engine |

## Threads

**UI (GPUI).** `ui::Omalive` holds a `model::Model` and a `Box<dyn
Engine>`. User actions call model methods, which update state and return
`Effect`s (start voice, send live controls, apply a look, and so on). The
view runs them on the engine. Engine events arrive over an `async_channel`
and update the model. The meter refresh (30 Hz) and preview frames exist
only while they are on screen.

The view entity belongs to the app (`session`), not to a window, and the
app quits only when asked (`QuitMode::Explicit`). Closing the window calls
`Omalive::hidden`, which turns off the meter taps, the preview and
animation; voice and camera keep running. Opening the window again roots
it on the same entity, so nothing restarts. `--background` starts with no
window at all.

**Control socket** (`control`). `$XDG_RUNTIME_DIR/omalive.sock`, created
only while holding the instance lock, speaks JSON lines:

- To clients: the full `Status` on connect and whenever it changes (the
  view publishes after every event and effect; unchanged status is not
  resent). While voice runs unmuted, `level` lines at 12 Hz (the meter's
  held peak since the last read). While a client has asked for the
  picture and the camera captures, a `picture` line for each one the
  camera renders (up to 30 a second), naming a 640×360 binary PPM beside
  the socket (`omalive.ppm`, renamed into place so readers never see a
  partial file). Levels and pictures each have a thread that sleeps on a
  condition variable when nobody is connected or nothing flows; the
  picture thread wakes as each picture lands, and sends it only to the
  clients watching.
- From clients: `picture {on}` (handled by the server, per client: the
  camera only samples pictures while some client watches), and `open`,
  `toggle-voice`, `toggle-mute`, `toggle-camera`, `bypass`, `look`, `quit`,
  forwarded to the GPUI thread and run through
  the same model methods as the window's controls. Unknown looks are
  ignored rather than treated as paths.
- Connecting clients set the engine's bar feed (`Engine::set_bar`).

`omalive status --follow` bridges the socket to stdio and reconnects while
omalive isn't running; it is the bar widget's backend, and a second
`omalive` launch uses the socket to open the window.

**Bar widget** (`shell/`). An Omarchy shell plugin, one `Service` per
shell running the bridge, and a `BarWidget` per bar. Its pure model
(`Model.js`) is shared with node tests; `shell/tests/qml-smoke` drives the
real QML in a hidden Quickshell against `omalive --demo` on a private
socket.

**PipeWire** (`media::pw`). One `MainLoop` owns every PipeWire object:

- `registry`: globals become `core::Graph` nodes, ports and links.
  Microphone lists, hotplug and "our nodes vanished" all come from registry
  events, with no polling.
- `chain`: the voice filter-chain runs as a `pipewire -c` child under
  `setpriv --pdeathsig`, so a crash cannot orphan the virtual microphone.
  Its config is rendered indented, because PipeWire rejects long one-line
  module arrays while staying alive.
- Linking uses the `link-factory`; it relinks automatically when the
  microphone returns.
- Live controls are `Props { params = [...] }` sent to the capture node, and
  mute is `Props { mute }` on the source node (`pods`).
- `streams`: the level meter, and the "Omalive Camera" `Video/Source` in
  driver mode, triggered once per graded frame. The meter keeps the chain
  processing, so it runs while the window shows it, or for the bar only
  while an app is already listening (`Graph::listeners`, which ignores
  omalive's own meter and taps).

**Camera** (`media::camera`). A `Source` (V4L2 or a synthetic pattern) and
a `Pipeline`:

- `watch` follows `/dev` with inotify. When `video*` nodes come, go or
  change permissions, it waits 500 ms for the burst to settle, lists the
  cameras again and emits `Event::Cameras` if the list changed.
  `Model::set_cameras` moves a switched-on camera with the list: onto
  another camera when the one in use leaves, back to the chosen camera
  when it returns, and out of a failure when a camera appears.

- For each size the V4L2 source picks the format and rate nearest 30 fps
  without going over, preferring raw NV12 or YUYV to MJPEG at equal rates.
  Capture cards and many USB cameras only reach full rate in MJPEG; those
  frames are decoded by libjpeg-turbo straight to YUV planes and
  interleaved into YUYV, so the grader is unchanged.
- The pipeline asks which outlets want frames: PipeWire (NV12 or YUY2, as
  negotiated), v4l2loopback (YUYV), and the preview (BGRA, only once the
  UI has drawn the last one).
- It grades once (`grader`: wgpu compute, or a multi-threaded CPU
  fallback) and swaps buffers into latest-wins `frames::Slot`s.
- Looks (grade, LUT, strength, compare) are baked on this thread, newest
  look only.
- In on-demand mode the V4L2 stream stops 3 s after the last consumer
  leaves and resumes when one appears.
- While the bar's panel watches, the grader renders a 640×360 RGBA bar
  picture in the same GPU submission as frames graded anyway (NV12, YUYV
  or the preview), up to 30 a second. Four filtered taps per pixel average
  its footprint, so a 1080p frame shrinks without shimmer. Wanting it
  never wakes the camera.
- Look swatches: the UI names the LUTs in the carousel's view; the camera
  thread bakes each with the current grade into a 17³ lattice, and the
  GPU stacks them in one 3D texture and renders 192×108 swatches in the
  preview's dispatch, appended to the preview readback. Nothing is
  rendered for looks out of view, or without the preview.

## The grade

`core::grade` defines every control as a pure colour function.

- **Per-channel stage:** sRGB decode, white balance and exposure gains, a
  soft shoulder that fades in only when gain can exceed white, re-encode,
  then a monotonic tone curve (contrast, shadows, highlights).
- **Cross-channel stage:** vibrance and saturation around Rec. 709 luma.

The bake tabulates the per-channel stage per axis, applies the
cross-channel stage and the creative LUT (mixed at its strength) at each of
33³ nodes, and uploads the result as an `Rgba16Float` 3D texture. The
shader decodes the camera's Y'CbCr using the matrix and range the driver
reports, samples the lattice with hardware trilinear filtering, and encodes
BT.601 limited range for outputs, which is what webcam consumers expect.

## The UI

Omarchy's panel style on GPUI Kit and `gpui-omarchy`, drawn by
`ui/kit.rs`: ruled sections with letter-spaced uppercase headings, square
switches, thin round sliders, bordered choices and neutral list selection,
with btop-style dot graphs.

- **Theme.** Every colour comes from the live Omarchy theme.
  `gpui-omarchy` carries accent, red, yellow and green; `ThemeSlots` reads
  blue, magenta, cyan, orange, muted and the dim foreground from the
  theme's `colors.toml` (falling back to ANSI keys, then to colours derived
  from the theme) and reloads whenever gpui-omarchy reloads the theme.
- **Letter spacing.** GPUI has none, so `tracked` shapes the label with
  every glyph forced to the mono advance plus 0.16 em: one element and one
  cached shaped line per label.
- **Graphs.** Bars are one gradient quad per data point, spanning as many
  dot columns as their share of the width. A cached image, a
  background-coloured grid with transparent holes, splits everything under
  it into dots. Static dot fields (the balance pad) are cached images too;
  only what moves is painted per frame.
- **Animation.** Springs step by elapsed time; frames are scheduled by a
  timer at 30 fps at most, and only while something moves and the window
  is presented.
- **Controls.** Sliders take drag, click, scroll, arrows or `h`/`l`
  (shift ×10), `Home`/`End`, and double-click to reset; bipolar sliders
  fill from the neutral point. Shortcuts are scoped
  `omalive && !Input && !Popover`, and focus returns to the window when the
  focused control disappears.
- **Layout.** Sizes are rem-based and follow `Ctrl+=`/`-`/`0` zoom. Two
  columns from 760 logical pixels (times the zoom); below 520 the header
  puts the tabs on their own row and the chain wraps.
