# Changelog

All notable changes to omalive are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-17

### Added

- Voice chain in PipeWire's filter-chain: high-pass filter, RNNoise noise
  suppression, a four-band EQ, an SC4 compressor and a −1 dBFS limiter,
  published as the **Omalive Mic** microphone.
- Live slider changes on the running chain, mute, and Natural, Podcast and
  Calls presets.
- A 10-second voice test that records the raw and processed microphone,
  measures peak, loudness and background noise, and plays both back.
- Camera grading in one GPU pass: exposure, contrast, highlights, shadows,
  warmth, tint, vibrance and saturation, with a CPU fallback.
- Eight grade presets, nine built-in looks, and import of `.cube` LUTs and
  Hald CLUT PNGs, with a looks carousel of live swatches.
- **Omalive Camera** as a PipeWire camera and, when v4l2loopback is loaded,
  a V4L2 device.
- Raw NV12 and YUYV capture, and MJPEG for capture cards and USB cameras.
- On-demand capture, so the camera sleeps until an app opens it.
- Background running, start at login, and resuming voice and camera on
  the next start.
- An Omarchy bar widget with a mic level, a camera tally light and a quick
  panel, connected over a local socket.
- Demo mode with a simulated microphone and a test-pattern camera.
- `make install`, an Arch PKGBUILD for the app, and `make film-looks` to
  fetch RawTherapee's film looks.

[0.1.0]: https://github.com/kierangraham/omalive/releases/tag/v0.1.0
