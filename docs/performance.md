# Performance

Latency and CPU/GPU cost are omalive's main constraints: it runs for the
whole length of a call, alongside the call app. This page records what was
measured, what was changed because of it, and what was considered and
rejected. All numbers are from an Apple M2 Max (Asahi, Honeykrisp Vulkan,
PipeWire 1.6.9), the FaceTime HD camera at 1280×720 NV12 30 fps, and release
builds. Reproduce them with the examples in `crates/omalive-media/examples`.

## Voice

The DSP runs inside PipeWire's filter-chain, in a `pipewire -c` child. It is
native C, processes on the graph's realtime thread, and never passes
samples through omalive. Reimplementing it in Rust would gain nothing.

| Measurement (`pw-top`, B = busy per cycle) | Result |
| --- | --- |
| Full chain (HPF, RNNoise, 3 EQ bands, SC4, trim, clamp), 256 quantum | 80 µs per 5.3 ms ≈ 1.5% of a core, 187 wakeups/s |
| Same chain at a 480 quantum | 10–63 µs per 10 ms ≤ 0.6% of a core, 100 wakeups/s |
| Without RNNoise, 256 quantum | 37.5 µs per 5.3 ms ≈ 0.7% |
| No call app listening, meter hidden | Chain and microphone **suspended**, 0% |

The chain measured here had three EQ bands. The fourth band adds one more
biquad; together the biquads cost about 1 µs per cycle.

Decisions:

- **Quantum follows the effects.** RNNoise works on 480-sample (10 ms)
  frames. A smaller quantum only adds wakeups and re-buffering, so the chain
  requests `480/48000` with suppression on and `256/48000` without
  (`omalive_core::voice::latency`).
- **Passive capture.** The capture side is `node.passive`, so with no
  consumer of "Omalive Mic" the whole chain and the microphone suspend.
- **The meter only exists while visible.** It is a capture stream, so it
  would keep the chain awake. It is created when the Voice page is shown and
  destroyed when it is hidden. Analysis runs in the RT callback into atomics,
  with no allocation or locking.
- **Slider changes are live.** `Props` params are set on the running chain
  instead of restarting it. Only structural changes (input, RNNoise on/off,
  compressor on/off) restart it, and only when the user presses Apply.
- **Event-driven graph.** PipeWire registry events keep omalive's model of
  the graph current, so nothing polls `pw-dump` or streams the full state to
  the UI.

Considered and rejected:

- *Merging the four biquads into one `param_eq` node.* At about 1 µs per
  cycle for the biquads, the saving is below measurement noise.
- *Replacing SC4.* It is the costliest stage after RNNoise, but at ≈0.5%
  of a core there is no builtin compressor in filter-chain to replace it
  with that would justify the change.

## Camera

Per frame, on one thread: V4L2 dequeue → upload the Y and CbCr planes → one
compute dispatch (Y'CbCr → R'G'B' → one 3D-LUT sample → re-encode) →
readback of only the formats someone wants → hand off.

| Measurement | Result |
| --- | --- |
| Frame rate, dropped frames | 30.0 fps, 0 dropped |
| omalive process CPU with a PipeWire consumer and the preview | **2% of one core** |
| Capture timestamp → frame handed to outputs | 0.9–1.1 ms median, ≤2.1 ms p95 |
| Upload + encode / submit + GPU wait / readback | 86 µs / 813 µs / 96 µs (medians) |
| Grade bake (33³ lattice, with a creative LUT) | ≈1 ms, on the camera thread |

For comparison, an FFmpeg filter graph doing the same grade (`eq`,
`colortemperature`, `colorbalance`, `vibrance`, `lut3d`) used 1.5 cores at
720p and 3 at 1080p. With cheaper filters it was 0.36 core without a LUT,
and 0.76 with one.

Decisions:

- **Bake, don't compute.** Every control is a per-pixel colour function, so
  the whole grade plus the creative LUT at its strength is baked into one
  33³ lattice. The shader's cost is constant regardless of which controls
  are active. Bakes tabulate the per-channel curves once per axis (99
  transfer-function evaluations instead of 107,811) and run on the camera
  thread, newest look only, so dragging a slider never queues work or
  touches the UI thread.
- **Work only for present consumers.** NV12 is produced only while a
  PipeWire consumer streams, YUYV only for v4l2loopback or a YUY2
  consumer, and the preview only once the UI has drawn the previous one. A
  hidden window draws nothing, so its preview costs nothing.
- **On-demand capture.** In on-demand mode the V4L2 stream (and the camera
  light, ISP and GPU) stops 3 s after the last consumer leaves, and resumes
  when a call app starts streaming "Omalive Camera" or the preview is
  shown. v4l2loopback cannot report readers without unsafe ioctls, so while
  it is present the camera stays on once started.
- **Latency over throughput.** Each frame is submitted and waited on before
  the next is dequeued. Pipelining would hide the 0.8 ms GPU wake-up but
  add a frame (33 ms) of latency. Two V4L2 buffers are used: one filling,
  one being graded.
- **Latest-wins handoff.** Every slot holds one frame; a slow reader gets
  the newest frame, never a queue. Buffers are swapped rather than copied
  between the grader and the slots. The remaining full-frame copies are
  the upload, the readback and the copy into the consumer's PipeWire
  buffer.
- **Driver-mode source.** "Omalive Camera" drives its own graph cycle and
  is triggered once per graded frame, so a frame leaves as soon as it
  exists rather than waiting for the next cycle of another driver.

Considered and rejected for now:

- *Zero-copy DMA-BUF import of V4L2 buffers into Vulkan.* This would remove
  the upload copy (≈86 µs). wgpu exposes external memory only through
  unsafe HAL calls, and the workspace forbids unsafe code.
- *Mappable storage buffers* (`MAPPABLE_PRIMARY_BUFFERS`) to skip the
  GPU-side copy before readback. That copy is a few µs on unified memory.
- *Realtime scheduling for the camera thread.* Measured p95 stays far
  inside one frame; RTKit adds a D-Bus dependency for no observed gain.

## Interface

Measured in demo mode on a headless 1180×820 output at 2× scale, main-thread
CPU per frame (the M2's frequency scaling moves absolute figures by up to
2×, so changes were compared within one process, alternating every 3 s).

| Change | Saving per frame |
| --- | --- |
| Spectrum: one bar per spectrum point (192) under a cached dot mask, instead of a quad per dot column (≈360) per series plus a mask quad per row | ≈2.5 → 1.4 ms |
| Waveform: one quad per column coloured by amplitude, cached mask | ≈1.4 → 0.2 ms |
| Balance pad: static dot field as a cached image, glow painted live | ≈1.7 → ≈1.2 ms |

Decisions:

- **Cap animation at 30 fps.** Frames were requested every display refresh;
  on this 120 Hz panel that is four times the work for no visible gain in
  meters and spectra. A timer now schedules the next frame, and only while
  a spring or the live voice moves.
- **Readouts at 5 Hz.** Peak, RMS, reduction, loudness and noise figures
  refresh five times a second, like a hardware meter: readable, and fewer
  new strings to shape.
- **GPUI's cost is per primitive.** Each painted quad costs about 2 µs of
  main-thread time, so graphs are drawn with as few primitives as their
  look allows rather than one per dot.
- **Hidden windows draw nothing.** GPUI waits for the compositor's frame
  callback after each present, and Hyprland withholds it for hidden
  surfaces, so the frame timer stops with the window.

Considered and rejected for now:

- *Cached GPUI views for static sections.* A cached view needs its size up
  front, and notifying any view re-renders its ancestors, so the animated
  graphs would still rebuild the page. The static sections cost about
  0.5 ms per frame together.

### MJPEG cameras

Cameras that only reach full rate in MJPEG (capture cards, many USB
webcams) cost a JPEG decode per frame on the camera thread. For a 1080p
4:2:2 frame on the M2 Max:

| Decoder | Per frame |
| --- | --- |
| zune-jpeg 0.5, to interleaved YCbCr, then packed to YUYV | 7.4 ms |
| libjpeg-turbo, straight to YUV planes, then packed to YUYV | **4.3 ms** |

libjpeg-turbo skips the colour conversion and chroma upsampling, and its
packing step is a plain interleave. At 30 fps that is about 13% of one core
and 4 ms of added latency. Capture stays at 30 fps even when a camera offers
60: calls use 30, and every extra frame is another decode.
