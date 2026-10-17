#!/usr/bin/env python3
"""Cut the promo from the recordings in artifacts/promo.

Each scene is rendered on its own (footage in a rounded card, titles that
fade and rise in), then the scenes are cross-faded into omalive-promo.mp4.
Colours are Tokyo Night's theme slots, the theme the footage was shot in.
"""
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get("PROMO_OUT", ROOT / "artifacts" / "promo"))
WORK = OUT / "work"
FONTS = Path.home() / ".local/share/fonts/Iosevka"
HEAVY = FONTS / "IosevkaNerdFontMono-Heavy.ttf"
BOLD = FONTS / "IosevkaNerdFontMono-Bold.ttf"
REGULAR = FONTS / "IosevkaNerdFontMono-Regular.ttf"
W, H, FPS = 1920, 1080, 30
FADE = 0.6

BG = "#13141c"
FG = "#c0caf5"
DIM = "#7c84a8"
ACCENT = "#7aa2f7"
HUES = ["#449dab", "#7aa2f7", "#ad8ee6", "#f7768e", "#eb927b", "#e0af68"]


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def magick(*args):
    run("magick", *args)


def assets():
    WORK.mkdir(parents=True, exist_ok=True)
    # Background: the theme's darkest slot, two soft glows and a dot grid,
    # the app's own visual language.
    magick("-size", "480x270", f"xc:{BG}",
           "-fill", "#7aa2f740", "-draw", "circle 70,40 70,190",
           "-fill", "#ad8ee630", "-draw", "circle 430,250 430,390",
           "-blur", "0x70", "-resize", f"{W}x{H}!",
           "(", "-size", "24x24", "xc:none", "-fill", "#414868", "-draw",
           "circle 12,12 12,13.3", "-write", "mpr:dot", "+delete", ")",
           "(", "-size", f"{W}x{H}", "tile:mpr:dot", "-channel", "A",
           "-evaluate", "multiply", "0.35", "+channel", ")", "-composite",
           WORK / "bg.png")
    # A row of dots in the theme's hues: the spectrum, as a signature.
    dots = ["-size", "430x24", "xc:none"]
    for i in range(18):
        color = HUES[i * len(HUES) // 18]
        x = 12 + i * 24
        dots += ["-fill", color, "-draw", f"circle {x},12 {x},{12 + 8 - abs(i - 8.5) * 0.45}"]
    magick(*dots, WORK / "dots.png")


def card_assets(name, w, h, radius=18):
    mask = WORK / f"{name}-mask.png"
    shadow = WORK / f"{name}-shadow.png"
    magick("-size", f"{w}x{h}", "xc:black", "-fill", "white", "-draw",
           f"roundrectangle 0,0 {w - 1},{h - 1} {radius},{radius}", mask)
    pad = 60
    magick("-size", f"{w + pad * 2}x{h + pad * 2}", "xc:none", "-fill", "#000000b0", "-draw",
           f"roundrectangle {pad},{pad + 14} {w + pad - 1},{h + pad + 13} {radius},{radius}",
           "-blur", "0x22", shadow)
    return mask, shadow, pad


def text(content, x, y, size, color, font, start, rise=18, fade=0.55, alpha=1.0):
    """A drawtext filter that fades in and rises into place at `start`."""
    content = content.replace("'", "’")
    a = f"if(lt(t,{start}),0,if(lt(t,{start + fade}),(t-{start})/{fade},1))*{alpha}"
    ease = f"(1-pow(1-min(max((t-{start})/{fade},0),1),3))"
    if isinstance(x, str) and x == "center":
        x = "(w-text_w)/2"
    return (f"drawtext=fontfile='{font}':text='{content}':fontsize={size}:fontcolor={color}"
            f":x='{x}':y='{y}+{rise}*(1-{ease})':alpha='{a}'")


def block(lines, start):
    """Kicker, headline lines and body lines, left-aligned at x=96."""
    kicker, headline, body = lines
    filters = [text(kicker, 96, 300, 24, ACCENT, BOLD, start)]
    y = 346
    for i, line in enumerate(headline):
        filters.append(text(line, 96, y, 58, FG, HEAVY, start + 0.15 + i * 0.1))
        y += 70
    y += 26
    for i, line in enumerate(body):
        filters.append(text(line, 96, y, 27, DIM, REGULAR, start + 0.45 + i * 0.08))
        y += 40
    return filters


def render(name, duration, inputs, graph):
    """Render one scene: inputs after the background, graph ends at [out]."""
    args = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
            "-loop", "1", "-framerate", str(FPS), "-t", str(duration), "-i", WORK / "bg.png"]
    for item in inputs:
        args += item
    args += ["-filter_complex", graph, "-map", "[out]", "-t", str(duration), "-r", str(FPS),
             "-c:v", "libx264", "-preset", "medium", "-crf", "14", "-pix_fmt", "yuv420p",
             WORK / f"{name}.mp4"]
    run(*args)
    return WORK / f"{name}.mp4"


def still(path):
    return ["-loop", "1", "-framerate", str(FPS), "-i", path]


def clip(path, start, duration):
    return ["-ss", str(start), "-t", str(duration), "-i", path]


def footage_card(index, crop, size, at, name, duration, extra_inputs=0):
    """Graph fragment: crop, scale and round input `index`, with a shadow,
    composited on [base] at `at`, gliding up slightly as it fades in."""
    w, h = size
    mask, shadow, pad = card_assets(name, w, h)
    m, s = index + 1, index + 2
    x, y = at
    glide = f"{y}+24*(1-(1-pow(1-min(t/0.8,1),3)))"
    return ([still(mask), still(shadow)],
            f"[{index}:v]{crop}scale={w}:{h}:flags=lanczos,format=rgba,setsar=1[f{index}];"
            f"[{m}:v]format=gray,setsar=1[m{index}];"
            f"[f{index}][m{index}]alphamerge,fade=in:st=0.1:d=0.6:alpha=1[c{index}];"
            f"[{s}:v]format=rgba,fade=in:st=0.1:d=0.6:alpha=1[s{index}];"
            f"[base][s{index}]overlay=x={x - pad}:y='{glide}-{pad}':shortest=1[b{index}];"
            f"[b{index}][c{index}]overlay=x={x}:y='{glide}':shortest=1[base]")


def main():
    if not (OUT / "app.mp4").exists():
        sys.exit("Record first: promo/record-app.sh, then promo/record-bar.sh for both layouts")
    assets()
    scenes = []

    # 1. Title.
    d = 4.6
    title = [text("omalive", "center", 360, 190, FG, HEAVY, 0.25, rise=26, fade=0.8),
             text("Studio voice and a graded camera for every call.", "center", 600, 42, FG, REGULAR, 1.15),
             text("Native to Omarchy  ·  Rust  ·  PipeWire  ·  wgpu", "center", 668, 26, DIM, REGULAR, 1.7)]
    scenes.append((render("1-title", d, [still(WORK / "dots.png")],
        "[1:v]format=rgba,fade=in:st=0.9:d=0.6:alpha=1[dots];"
        "[0:v][dots]overlay=(W-w)/2:572[base];"
        f"[base]{','.join(title)}[out]"), d))

    # 2. Voice: the app's voice page, the chain walk and a mute.
    d = 11
    inputs, graph = footage_card(1, "", (1240, 671), (612, 204), "voice", d)
    words = block(("01  VOICE", ["Studio sound,", "any microphone."],
                   ["Noise suppression, EQ, compressor", "and limiter, running natively in", "PipeWire. Changes apply live,", "with no dropouts."]), 0.5)
    scenes.append((render("2-voice", d, [clip(OUT / "app.mp4", 0.2, d)] + inputs,
        f"[0:v]null[base];{graph};[base]{','.join(words)}[out]"), d))

    # 3. Camera: looks change on the graded side of the split preview.
    d = 15
    inputs, graph = footage_card(1, "", (1240, 671), (612, 204), "camera", d)
    words = block(("02  CAMERA", ["Your camera,", "colour-graded."],
                   ["Exposure, white balance, tone and", ".cube LUTs, baked into one GPU", "pass, about a millisecond a frame.", "Compare with the original any time."]), 0.5)
    scenes.append((render("3-camera", d, [clip(OUT / "app.mp4", 13.2, d)] + inputs,
        f"[0:v]null[base];{graph};[base]{','.join(words)}[out]"), d))

    # 4. The bar, magnified: waveform, halftone, a new look, a mute.
    d = 9
    inputs, graph = footage_card(1, "crop=1440:212:228:407,", (1440, 212), (240, 440), "bar", d)
    words = [text("03  IN YOUR BAR", "center", 236, 24, ACCENT, BOLD, 0.4),
             text("On air, at a glance.", "center", 282, 64, FG, HEAVY, 0.55),
             text("Your mic level and a live halftone of your camera, beside CPU and memory.", "center", 720, 28, DIM, REGULAR, 0.9),
             text("A label turns red while a call is hearing or seeing you.", "center", 762, 28, DIM, REGULAR, 0.98)]
    scenes.append((render("4-bar", d, [clip(OUT / "bar-zoom.mp4", 3.2, d)] + inputs,
        f"[0:v]null[base];{graph};[base]{','.join(words)}[out]"), d))

    # 5. The quick panel.
    d = 8.5
    inputs, graph = footage_card(1, "crop=712:1000:538:0,", (686, 963), (1150, 58), "panel", d)
    words = block(("04  QUICK CONTROLS", ["Everything,", "one click away."],
                   ["Mute, switch looks, turn the mic", "or camera on and off, without", "opening a window. omalive keeps", "running in the background."]), 0.5)
    scenes.append((render("5-panel", d, [clip(OUT / "bar-panel.mp4", 4.6, d)] + inputs,
        f"[0:v]null[base];{graph};[base]{','.join(words)}[out]"), d))

    # 6. Close.
    d = 5.2
    close = [text("omalive", "center", 330, 150, FG, HEAVY, 0.3, rise=22, fade=0.8),
             text("Pick  Omalive Mic  and  Omalive Camera  in any call app.", "center", 560, 36, FG, REGULAR, 1.0),
             text("Browsers · OBS · native apps via v4l2loopback", "center", 618, 26, DIM, REGULAR, 1.4),
             text("make install", "center", 720, 30, ACCENT, BOLD, 1.9)]
    scenes.append((render("6-close", d, [still(WORK / "dots.png")],
        "[1:v]format=rgba,fade=in:st=0.8:d=0.6:alpha=1[dots];"
        "[0:v][dots]overlay=(W-w)/2:508[base];"
        f"[base]{','.join(close)},fade=out:st={d - 0.8}:d=0.8[out]"), d))

    # Cross-fade the scenes together.
    args = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y"]
    for path, _ in scenes:
        args += ["-i", path]
    graph, last, offset = [], "0:v", 0.0
    for i, (_, duration) in enumerate(scenes[:-1]):
        offset += duration - FADE
        out = f"x{i}"
        graph.append(f"[{last}][{i + 1}:v]xfade=transition=fade:duration={FADE}:offset={offset:.3f}[{out}]")
        last = out
    final = OUT / "omalive-promo.mp4"
    args += ["-filter_complex", ";".join(graph), "-map", f"[{last}]", "-r", str(FPS),
             "-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p",
             "-movflags", "+faststart", final]
    run(*args)
    print(final)


if __name__ == "__main__":
    main()
