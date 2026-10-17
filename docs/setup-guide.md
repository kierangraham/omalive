# Recommended setup

These starting points come from manufacturers' specs and common podcast
practice ([sources](#sources)). Make small changes of 2–4 dB, judge them on
headphones, and check with the voice test (**Test 10 s** on the Voice page).

## Voice: a dynamic mic such as the Shure SM58 into a Focusrite Scarlett

- **Gain:** the SM58 is quiet (−54.5 dBV/Pa) and needs about 50–60 dB of
  gain, so the knob sits around three o'clock or higher. That is normal.
  Set it so speech peaks around −12 to −10 dBFS. A 4th-gen Solo has 69 dB
  and needs no booster. A 3rd-gen Solo has 56 dB, which is fine 5–10 cm
  from the mic but marginal further away; an inline booster
  (Cloudlifter-style) helps there.
- **Air:** the SM58 is dark, so the Scarlett's Air (Presence) suits it.
  With Air on, take about 1 dB off omalive's Air band.
- **Technique:** 5–15 cm from the grille, slightly off-axis (15–30°) to
  avoid plosives, at a steady distance. Closer than a few centimetres the
  bass rises 6–10 dB (proximity effect). Point the back of the mic at
  noise.
- **omalive:** choose the **Podcast** preset, the default. It sets:
  - an 80 Hz low cut;
  - EQ: Body 0 dB, Mud −3 dB at 300 Hz, Presence +1.5 dB at 3 kHz,
    Air +1 dB at 10 kHz;
  - compression at 3:1 from −18 dB, 15 ms attack, 150 ms release and
    +5 dB makeup, so loud phrases get 3–6 dB of reduction;
  - noise suppression with an 85% voice gate. Lower the gate to 70–80% if
    word endings get clipped.
  - **Calls** cuts more low end, compresses faster and gates harder for
    noisy rooms.
- **Calls:** in Zoom, turn off "Automatically adjust microphone volume" so
  it doesn't fight the compressor.

## Camera: a Sony a6000 over HDMI into a USB capture card

- **Clean output:** Setup → HDMI Settings: **HDMI Resolution 1080p** (not
  Auto or 1080i, which looks combed when captured), **HDMI Info Display
  Off** (check it after restarts), **CTRL FOR HDMI Off**. Put the mode
  dial on Movie.
- **Power:** use an NP-FW50 dummy battery. The 30-minute limit only
  applies to recording to the card; the camera turning off is its
  power-save timer, which **Remote Ctrl On** keeps from firing. It runs
  warm, so flip the screen out.
- **Exposure:** manual, with the shutter at **1/50** in the UK and other
  50 Hz countries, or **1/60** on 60 Hz mains, to avoid light flicker.
  Use f/2–2.8 on a fast prime (f/1.4 is too shallow if you move), or
  f/3.5 at 16 mm on the kit lens. Keep ISO 100–800, with Auto ISO capped
  around 1600–3200.
- **Colour:** set white balance in Kelvin (about 5600 K for daylight LEDs,
  3200 K for tungsten) or from a grey card. Creative Style Neutral or
  Portrait with contrast −1 to −3, saturation −1 to 0 and sharpness −1
  to −2; lower sharpness also suits MJPEG. DRO off, so exposure stays put.
- **Focus:** AF-C with face detection. If the face box shows in the
  output, turn the mode dial away and back.
- **Capture:** the MS2109 card captures 1080p at 30 fps in MJPEG, which
  omalive picks automatically. Then grade gently in omalive: the flatter
  Neutral style leaves room for a little contrast and vibrance, or a look.
- **Range:** if colours look washed out or blacks look crushed, the card
  and camera disagree about full and limited range; compare with a test
  chart.

## Sources

- Shure SM58 user guide (sensitivity, proximity effect):
  https://www.fullcompass.com/common/files/1625-ShureSM58UserGuide.pdf
- Focusrite Scarlett Solo 3rd gen specifications:
  https://userguides.focusrite.com/hc/en-gb/articles/23031457381138-Solo-3rd-Gen-specifications
- Scarlett Solo 4th gen gain and Air: https://hifirated.com/reviews/focusrite-scarlett-solo-4th-gen-review
- Apple Podcasts audio requirements (−16 LKFS, −1 dBFS true peak):
  https://podcasters.apple.com/support/893-audio-requirements
- Podcast EQ and compression practice: https://podrewind.com/blog/podcast-eq-settings-guide,
  https://podrewind.com/blog/podcast-compression-best-practices
- Presence EQ for voice: https://randycoppinger.com/2013/03/19/eq-for-voice/
- RNNoise voice gate guidance: https://github.com/werman/noise-suppression-for-voice
- Zoom audio settings: https://online.berklee.edu/help/en_US/zoom/1793280-how-to-configure-zoom-audio-settings-for-music-performance
- a6000 as a webcam (clean HDMI): https://medium.com/@liketortilla/using-a-sony-a6000-digital-camera-as-a-webcam-5b7b7fad5a2c
- a6000 HDMI output format: https://foxtailwhipz.com/blogs/news/does-the-sony-a6000-have-clean-hdmi-output-a6000-a6100-a6300-a6400-a6500-a6600
- a6000 dummy battery and power-off: https://www.dpreview.com/forums/threads/using-a-dummy-battery-timeout-problems.4631472/
- a6000 video settings: https://arronprescott.com/2021/08/28/sony-a6000-settings-for-photos-and-video/
- MS2109 capture card review: https://www.naut.ca/blog/2020/07/09/cheap-hdmi-capture-card-review/
