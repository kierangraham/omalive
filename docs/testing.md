# Testing

omalive has three kinds of check: automated tests that need no devices,
live probes against your own PipeWire session, and manual checks on real
hardware.

## Automated tests

These need no microphone, camera or display session.

| Command | What it checks |
| --- | --- |
| `make test` | Unit tests for all three crates, plus GPUI interaction tests that drive the real views with a recording engine |
| `make shell-test` | The bar widget: its model in node, a round trip through `omalive --demo`, then the QML in a hidden Quickshell |
| `make lint` | `rustfmt` and `clippy -D warnings` |
| `make check` | `cargo check` of every target |
| `omarchy plugin validate .` | The plugin manifest, as `omarchy plugin add` checks it |

The GPUI interaction tests cover the cases that broke during development:

- page keys and shortcuts after a page switch;
- start blocked until a microphone is chosen;
- slider changes reaching a running voice chain live;
- structural changes waiting for **Apply**;
- a missing effect plugin being explained;
- camera start, grade and compare;
- a 480×480 window at 180% zoom staying in bounds.

`make shell-test` drives the widget through these states: on air, muted,
quick panel open, camera off, settings, a vertical bar and omalive offline.
Run `shell/tests/qml-smoke --render` to save a PNG of each state to
`shell/tests/output/`.

## Live probes

**`make probe` uses your first microphone and camera for a few seconds.**

1. It starts the real voice chain, changes live controls and mutes it.
2. It runs the camera, with a PipeWire consumer reading "Omalive Camera".

The probe prints frame rate, dropped frames, grading time and latency. To
probe a capture card instead of the built-in camera:

```sh
PROBE_CAMERA=/dev/video1 PROBE_SIZE=1920x1080 target/release/examples/probe camera 10
```

## Verified on hardware

These checks ran during development of 0.1.0, on an Apple M2 Max running
Omarchy on Asahi Linux with PipeWire 1.6.

| Area | Result |
| --- | --- |
| Voice chain with RNNoise and SC4 | Starts in about 15 ms. Live controls and mute apply without a restart |
| Voice DSP load | 10 to 63 µs per 10 ms cycle |
| Built-in camera, 1280×720 | 30 fps, no dropped frames, about 1 ms of grading per frame, 2% of one core |
| USB HDMI capture card, 1920×1080 MJPEG | 30 fps, about 2 ms of grading per frame after a 4 ms decode |
| Bar widget | Checked on the real bar at 1.6× scale |
| Background mode | No CPU use while idle with the bar connected |

[Performance](performance.md) has the full measurements.

## Not yet verified

- **The v4l2loopback output**, and native apps that read it, such as Hopp,
  Zoom and Discord.
- **Browsers through the camera portal.**
- **x86_64 machines and USB webcams.** YUYV and MJPEG input have unit
  tests, but only the capture card has been tried.
- **The CPU grading fallback** on a machine without a GPU.

**Known issue:** the capture card drops about half its frames for the
first 4 seconds after starting. The cause is not known yet.
