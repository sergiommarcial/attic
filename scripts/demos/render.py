#!/usr/bin/env python3
"""Renders the README demos: an animated SVG and a GIF per scene.

usage: render.py OUTDIR [SCENE_PREFIX] [--mp4]

SCENE_PREFIX picks scenes whose name starts with it ("3" for one scene).
--mp4 also writes an MP4, for posting somewhere GIFs look poor; it is not
meant to be committed. macOS only: needs qlmanage, ImageMagick (magick) and
ffmpeg.
"""
import pathlib
import re
import subprocess
import sys
import tempfile

import gen

SIZE = 1600
HOLD_LAST = 1.5


def render_scene(name, frames, out, work, mp4):
    (out / f"{name}.svg").write_text(gen.build(name, frames))
    svgs = []
    for i in range(len(frames)):
        svg = gen.build(name, frames, i)
        # A square viewBox keeps qlmanage from cropping the thumbnail.
        svg = re.sub(
            r'viewBox="0 0 (\d+) (\d+)" width="\d+" height="\d+"',
            r'viewBox="0 0 \1 \1" width="\1" height="\1"',
            svg,
        )
        path = work / f"{name}-{i:02d}.svg"
        path.write_text(svg)
        svgs.append(str(path))
    subprocess.run(
        ["qlmanage", "-t", "-s", str(SIZE), "-o", str(work), *svgs],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=True,
    )
    height = round(gen.H * SIZE / gen.W)
    height -= height % 2
    lines = []
    for i, (duration, *_rest) in enumerate(frames):
        thumb = work / f"{name}-{i:02d}.svg.png"
        crop = work / f"{name}-{i:02d}.png"
        subprocess.run(
            ["magick", str(thumb), "-crop", f"{SIZE}x{height}+0+0", "+repage", str(crop)],
            check=True,
        )
        if i == len(frames) - 1:
            duration += HOLD_LAST
        lines.append(f"file '{crop.name}'\nduration {duration}\n")
    lines.append(f"file '{work}/{name}-{len(frames) - 1:02d}.png'\n")
    listing = work / f"{name}.txt"
    listing.write_text("".join(lines))

    def ffmpeg(*args):
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
             "-i", str(listing), *args],
            check=True,
        )

    ffmpeg(
        "-vf",
        "fps=15,scale=1000:-2:flags=lanczos,split[a][b];"
        "[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none",
        "-loop", "0", str(out / f"{name}.gif"),
    )
    if mp4:
        ffmpeg(
            "-vf", "fps=30,scale=1600:-2:flags=lanczos,format=yuv420p",
            "-c:v", "libx264", "-crf", "20", "-movflags", "+faststart",
            str(out / f"{name}.mp4"),
        )


def main():
    args = [a for a in sys.argv[1:] if a != "--mp4"]
    if not args:
        sys.exit(__doc__)
    out = pathlib.Path(args[0])
    prefix = args[1] if len(args) > 1 else ""
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        for name, scene in gen.SCENES.items():
            if name.startswith(prefix):
                render_scene(name, scene(), out, work, "--mp4" in sys.argv)
                print(name, "ok")


if __name__ == "__main__":
    main()
