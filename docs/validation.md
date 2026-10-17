# Validation

## 2026-10-07 — Rust rewrite

Machine: Apple M2 Max on Asahi (a custom 7.1.12 kernel), Hyprland 0.56.2,
PipeWire 1.6.9, Honeykrisp Vulkan, FaceTime HD camera (apple-isp, NV12).
`noise-suppression-for-voice` and `swh-plugins` installed; v4l2loopback not
loaded.

### Gates

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | 72 passed: core 29, media 21, app 22 (14 model, 8 GPUI interaction) |
| `cargo build --release -p omalive-app` | Pass (1 min 35 s clean dependency build) |
| `desktop-file-validate packaging/omalive.desktop` | Pass |

The GPUI interaction tests drive the real views with a recording engine.
They cover:

- page keys and keeping work to what is visible (meter and preview start
  and stop)
- start blocked until a microphone is chosen
- keyboard slider changes reaching a running chain live
- structural changes waiting for Apply
- a missing plugin being explained
- mute only while running
- camera start, grade and compare
- a 480×480 window at 180% zoom in the light theme staying in bounds

They found and fixed three defects: shortcuts stopped working after a page
switch removed the focused control; clicking a slider did not focus it for
arrow keys; and the header overflowed at 480 px and 180% zoom.

### Live checks (`make probe`)

Voice, on the built-in headset input:

- The chain started with RNNoise and SC4 in 15 ms.
- `low:Gain` and the compressor threshold changed on the running chain;
  read back with `pw-dump`.
- `Props.mute` was set on the source node.
- The meter received 111 buffers (the input was silent).
- Stopping left no `omalive_*` nodes.
- Killing the app cannot orphan the chain: it runs under
  `setpriv --pdeathsig TERM`.

Voice DSP load, with `pw-top`:

| Condition | DSP time |
| --- | --- |
| Full chain, 480 quantum (RNNoise on) | 10–63 µs per 10 ms cycle |
| Full chain, previous 256 quantum | 80 µs per 5.3 ms cycle |
| No consumer and meter hidden | Nodes suspended |

Camera, 1280×720 NV12 graded on the GPU, with a pipewire-rs consumer
(`examples/consume.rs`) connected to "Omalive Camera":

- The consumer negotiated NV12 1280×720 @ 30/1 and received 30.0–30.2 fps
  of complete 1,382,400-byte frames with plausible content.
- The engine reported 30.0 fps and 0 dropped frames.
- Grading took 0.9–1.4 ms per frame.
- Capture → hand-off latency was 0.9–1.3 ms median and 1.5–4.1 ms p95.
- The whole omalive process used 2% of one core.

A preview frame converted to PNG showed a correct decode: no colour cast,
correct orientation, and the test grade visible. The driver reports
Rec. 709 limited-range colorimetry, which the decoder uses.

### Not yet verified

- **Screenshots of the running app.** `scripts/screenshot.sh` opens the app
  on a temporary headless Hyprland output. During this session that output
  showed a password prompt drawn over every output, so no app image was
  captured. A nested headless Hyprland (`HYPRLAND_HEADLESS_ONLY`) failed to
  create its backend.
- **v4l2loopback output and Hopp.** The module was built against the `-53`
  kernel tree but is not loaded until reboot.
- **Call apps.** Browsers through the camera portal, Zoom and Discord.
- **On-demand standby wake-up timing**, measured with a real call app.
- **USB cameras.** YUYV input is implemented and unit-tested through the
  CPU path. MJPEG decoding is unit-tested with a frame stripped of its
  Huffman tables the way UVC cameras send it, and verified on a capture
  card (below).
- **The CPU grading fallback** on a machine without a hardware GPU.

## 2026-10-08 — MJPEG capture card

A MacroSilicon USB HDMI capture card (`534d:2109`, uvcvideo) offers raw
YUYV at 5 fps at 1080p and MJPEG at up to 60 fps. Before this change
omalive only read raw formats, so starting it at 1080p failed. Now it
picks MJPEG at 30 fps. Run with
`PROBE_CAMERA=/dev/video1 PROBE_SIZE=1920x1080 probe camera 10` and
`consume 5`:

- Running at 1920×1080 30 fps, graded on the GPU.
- A PipeWire consumer negotiated NV12 1920×1080 at 30/1 and received
  3,110,400-byte frames.
- Steady state: 29.8–30.1 fps, 0 dropped frames, 1.7–2.2 ms per frame on
  the camera thread for grading (the MJPEG decode, about 4.3 ms, comes
  before that).
- During the first 4 s about half the frames failed to decode and were
  dropped (about 15 fps delivered). The cause is not yet known.
- Capture timestamp to output: 42–52 ms median, ≤56 ms p95, against about
  1 ms for the built-in camera. UVC stamps a frame when its first packet
  arrives. The rest is the USB transfer of the whole compressed frame, the
  decode, and the card's own latency.
- Not measured: process CPU. `pidstat` is not installed.

## 2026-10-08 — Background running and the bar widget

Gates: 104 Rust tests, clippy `-D warnings` clean, the widget's node tests
and `shell/tests/qml-smoke` (hidden Quickshell against `omalive --demo` on a
private socket: on air, muted, quick panel, camera off, settings, vertical
bar and offline; `--render` saves PNGs).

Live, against the real session (debug build, a temporary config):

- **Idle in the background:** `omalive --background` with a bar client
  connected used 0 CPU ticks over 10 s (`/proc/<pid>/stat`).
- **Mic listeners:** voice running on the Scarlett with nobody listening
  had no meter node. `pw-record --target omalive_voice` raised
  `listeners` to 1, `omalive_meter` appeared and levels arrived at 12 Hz
  (60 in 5 s, 84 ms apart). When the recorder stopped, `listeners`
  returned to 0, the meter node went away and levels stopped, so the bar
  never keeps the chain awake by itself.
- **Camera picture:** against the demo, `picture` lines arrived only
  between `picture on` and `picture off` (43 in 1.5 s, about 30 a
  second), each naming a valid PPM; the widget's open panel loaded them,
  and closing the panel stopped them (`shell/tests`). One-pixel stripes
  render as an even grey on GPU and CPU, and the two agree within 6/255
  (`grader::tests::the_bar_picture_averages_fine_detail_on_cpu_and_gpu`).
  A hidden Quickshell loading 30 pictures a second used 8% of a core with
  the software renderer; the demo's omalive went from 9% to 14% while
  watched.
- **Look swatches:** GPU and CPU swatches of three different looks agree
  within 4/255 (`grader::tests::swatches_render_each_look`). On the
  Camera page with 29 looks, interleaved against the previous build on a
  loaded machine: whole process 25–33% of a core versus 32–36%, UI thread
  16–21% versus 22–26%, and no more 7–36 ms main-thread stalls every two
  seconds. omalive's own per-frame UI work there is about 1 ms; the rest
  is GPUI painting the window at 30 fps, as on the Voice page.
- **Window lifecycle** (on a headless Hyprland output): closing the window
  left the process running and the socket answering; launching `omalive`
  again reopened the window on the same state; `quit` over the socket
  stopped voice, removed the socket and exited.
- **Commands from one-shot clients:** a client that sends `quit` and hangs
  up at once used to be dropped before its command was read (the status
  write failed first). Fixed and covered.

Not yet verified: the widget's look on the real bar at 1.6× (the offscreen
renders use Qt's software renderer, which snaps dots to pixels), and the
widget during a real call.

### Follow-up: the widget on the real bar

The first widget only drew the mic while a call was listening, so with
voice on and no call it looked dead. Now it meters whenever voice runs and
the widget is connected (levels at 12 Hz confirmed with no listener), and
it is drawn like ostat: stacked MIC and CAM labels beside braille-style dot
frames, with a label turning red while on air. Checked on the live bar at
1.6× after `omarchy restart shell`, which the shell needs to drop its
cached copy of the widget. A property named `baseline` broke loading in the
real shell (it overrides `Item.baseline`); renamed, and the smoke test now
runs after every widget change.
