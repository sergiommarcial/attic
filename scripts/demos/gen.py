#!/usr/bin/env python3
"""Draws the README demos: attic's screens as animated terminal SVGs.

The data (paths, sizes, ages) is made up; the text the app draws is not.
check.py fails when UI_TEXT stops matching the app's source.

usage: gen.py OUTDIR [SCENE FRAME]   (SCENE FRAME writes one static frame)
"""
import copy
import sys
from xml.sax.saxutils import escape

COLS, ROWS = 106, 22
CW, LH, FS = 7.2, 17.0, 12
PAD = 18
BAR = 30
CAP = 46
W = COLS * CW + PAD * 2
H = BAR + PAD + ROWS * LH + PAD + CAP

BG, FG, DIM = "#16181d", "#d7dae0", "#7b8190"
ACCENT, WARN, OK = "#7aa2f7", "#e0af68", "#9ece6a"
FONT = "ui-monospace, 'SF Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace"
SANS = "-apple-system, 'SF Pro Text', 'Helvetica Neue', Arial, sans-serif"


def X(c):
    return PAD + c * CW


def Y(r):
    return BAR + PAD + r * LH


def text(col, row, s, fill=FG, bold=False):
    if not s:
        return ""
    w = "font-weight:700;" if bold else ""
    return (
        f'<text x="{X(col):.1f}" y="{Y(row) + 12.5:.1f}" fill="{fill}" '
        f'style="{w}" textLength="{len(s) * CW:.1f}" lengthAdjust="spacing" '
        f'xml:space="preserve">{escape(s)}</text>'
    )


def rect(x, y, w, h, fill="none", stroke=None, rx=0, op=None):
    s = f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{rx}" fill="{fill}"'
    if stroke:
        s += f' stroke="{stroke}" stroke-width="1"'
    if op is not None:
        s += f' opacity="{op}"'
    return s + "/>"


def box(c0, r0, c1, r1, title, fill=None):
    out = []
    if fill:
        out.append(rect(X(c0), Y(r0), (c1 - c0 + 1) * CW, (r1 - r0 + 1) * LH, fill))
    out.append(
        rect(X(c0) + CW / 2, Y(r0) + LH / 2, (c1 - c0) * CW, (r1 - r0) * LH, "none", DIM, 2)
    )
    if title:
        out.append(rect(X(c0 + 1), Y(r0), (len(title) + 0) * CW, LH, BG))
        out.append(text(c0 + 1, r0, title, FG))
    return "".join(out)


def human_cells(s):
    return s


def row_text(r, width, focus):
    indent = "  " * r.get("depth", 0)
    marker = "▸ " if focus == "S" else ""
    tags = f"{'rebuildable' if r.get('rebuildable') else '':<11} {'!' if r.get('warned') else ' '}"
    used = (UI_TEXT["age prefix"] + r["age"]) if focus == "S" else r["age"]
    kind = "/" if r.get("dir", True) else ""
    tags = f"  {tags}" if focus == "S" else ""
    right = f"{r['size']:>8}  {used:>18}{tags}"
    left = f"{indent}{marker}{r['path']}{kind}"
    room = width - len(right) - 1
    pad = max(0, room - len(left))
    return f"{left}{' ' * pad} {right}"


def draw_list(rows, c0, r0, c1, r1, title, sel):
    out = [box(c0, r0, c1, r1, title)]
    width = (c1 - c0 + 1) - 2 - 2
    for i, r in enumerate(rows[: r1 - r0 - 1]):
        row = r0 + 1 + i
        line = row_text(r, width + 2, r["_focus"])
        if sel == i:
            out.append(rect(X(c0 + 1), Y(row), (c1 - c0 - 1) * CW, LH, FG))
            out.append(text(c0 + 1, row, line, BG))
        else:
            out.append(text(c0 + 1, row, line))
        if r.get("warned") and sel != i:
            pass
    return "".join(out)


# What the app says. Every entry must appear in the app's source (check.py).
UI_TEXT = {
    "largest list title": "Largest folders",
    "suggestions list title": "Could free ~",
    "age prefix": "untouched ",
    "confirm title": "Move to the Trash?",
    "confirm prompt": "y: move it to the Trash · any other key: cancel",
    "second confirm prompt": "Press y again to move it to the Trash. Any other key cancels.",
    "message footer": "Press any key.",
    "moved message start": "Moved to the Trash: ",
    "moved message end": "It frees space once the Trash is emptied.",
    "git warning": "Holds a git repository with uncommitted, stashed or unpushed work.",
    "footer note": "Moving to the Trash frees space once the Trash is emptied.",
    "key hints": "↑↓ move · Tab switch list · Enter open · Backspace back · s sort · c clean · u unreadable · q quit",
    "notification title": "attic",
    "notification body": "could be freed. Run attic to review.",
}
KEYS = UI_TEXT["key hints"]
NOTE = UI_TEXT["footer note"]


def disk(used, total, pct):
    out = [text(0, 0, "Disk  ")]
    filled = round(pct / 100 * 20)
    for i in range(20):
        out.append(
            rect(X(6 + i) + 0.5, Y(0) + 3, CW - 1, LH - 7, ACCENT if i < filled else "#2c3039")
        )
    out.append(text(28, 0, f"{used} used of {total} ({pct}%)"))
    return "".join(out)


def keypill(label):
    if not label:
        return ""
    w = (len(label) + 2) * CW
    x = X(COLS) - w
    return rect(x, Y(0) - 1, w, LH + 2, "#2a2f3a", "#4a5163", 4) + text(
        COLS - len(label) - 1, 0, label, WARN, True
    )


def wrap(s, width):
    lines, cur = [], ""
    for word in s.split(" "):
        if cur and len(cur) + 1 + len(word) > width:
            lines.append(cur)
            cur = word
        else:
            cur = f"{cur} {word}".strip()
    return lines + [cur]


def dialog(lines):
    width = min(COLS - 8, 80)
    height = len(lines) + 2
    c0 = (COLS - width) // 2
    r0 = (ROWS - height) // 2
    out = [box(c0, r0, c0 + width - 1, r0 + height - 1, None, BG)]
    for i, (s, bold, fill) in enumerate(lines):
        out.append(text(c0 + 1, r0 + 1 + i, s, fill, bold))
    return "".join(out)


def app(st):
    """One screen of the app for state `st`."""
    out = [disk(*st["disk"])]
    L = [dict(r, _focus="L") for r in st["largest"]]
    S = [dict(r, _focus="S") for r in st["sugg"]]
    if st.get("browse"):
        b = st["browse"]
        rows = [dict(r, _focus="L") for r in b["rows"]]
        out.append(draw_list(rows, 0, 2, COLS - 1, 18, b["title"], st.get("sel")))
        if b.get("other"):
            out.append(text(2, 19, b["other"]))
    else:
        top_sel = st["sel"] if st["focus"] == "L" else None
        bot_sel = st["sel"] if st["focus"] == "S" else None
        out.append(draw_list(L, 0, 2, COLS - 1, 9, UI_TEXT["largest list title"], top_sel))
        out.append(draw_list(S, 0, 10, COLS - 1, 19, f"{UI_TEXT['suggestions list title']}{st['total']}", bot_sel))
    out.append(text(0, 20, NOTE))
    out.append(text(0, 21, KEYS))
    out.append(keypill(st.get("key")))
    d = st.get("dialog")
    if d:
        out.append(dialog(d))
    return "".join(out)


def shell(typed, cursor=True, extra=None):
    out = [text(0, 0, "~ % ", OK), text(4, 0, typed)]
    if cursor:
        out.append(rect(X(4 + len(typed)), Y(0) + 2, CW, LH - 4, FG, op=0.8))
    for i, (s, fill) in enumerate(extra or []):
        out.append(text(0, 1 + i, s, fill))
    return "".join(out)


def notification(title, body, y_off=0):
    """A macOS banner as `osascript` shows it: dark, translucent, Script
    Editor's icon (drawn in outline), a bold title and a wrapped body."""
    k = 1.15  # points to SVG units
    w, h = 344 * k, 74 * k
    x = W - w - 20
    y = 8 + y_off
    gradient = (
        '<defs><linearGradient id="scroll" x1="0" y1="0" x2="1" y2="1">'
        '<stop offset="0" stop-color="#f4f4f6"/><stop offset="1" stop-color="#85858b"/>'
        "</linearGradient></defs>"
    )
    ix, iy, isz = x + 12.5 * k, y + 21 * k, 32 * k
    cx, cy = ix + isz / 2, iy + isz / 2
    scroll = (
        f'<g transform="rotate(-38 {cx:.1f} {cy:.1f})">'
        f'<rect x="{cx - isz * .17:.1f}" y="{cy - isz * .36:.1f}" width="{isz * .34:.1f}" '
        f'height="{isz * .72:.1f}" rx="{isz * .13:.1f}" fill="url(#scroll)"/>'
        f'<ellipse cx="{cx:.1f}" cy="{cy - isz * .33:.1f}" rx="{isz * .17:.1f}" ry="{isz * .07:.1f}" '
        'fill="#c9c9ce" stroke="#6f6f75" stroke-width="0.8"/>'
        f'<ellipse cx="{cx:.1f}" cy="{cy + isz * .33:.1f}" rx="{isz * .17:.1f}" ry="{isz * .07:.1f}" '
        'fill="#8d8d93" stroke="#55555a" stroke-width="0.8"/></g>'
    )
    out = [
        "<g>",
        gradient,
        rect(x, y, w, h, "#47474c", "#6b6b72", 16 * k, 0.94),
        rect(ix, iy, isz, isz, "#1c1c1e", "#3a3a3d", 8 * k),
        scroll,
        f'<text x="{x + 58 * k:.1f}" y="{y + 25 * k:.1f}" font-size="{13 * k:.1f}" fill="#ffffff" '
        f'style="font-family:{SANS};font-weight:600">{escape(title)}</text>',
    ]
    for i, line in enumerate(wrap(body, 38)):
        out.append(
            f'<text x="{x + 58 * k:.1f}" y="{y + (41 + 16 * i) * k:.1f}" font-size="{13 * k:.1f}" '
            f'fill="#e6e6e8" style="font-family:{SANS}">{escape(line)}</text>'
        )
    out.append("</g>")
    return "".join(out)


# ---------- data ----------

def R(path, size, age, **kw):
    return dict(path=path, size=size, age=age, **kw)


LARGEST = [
    R("~/Library", "71 GB", "today"),
    R("~/Library/Developer", "38 GB", "2 days"),
    R("~/code", "33 GB", "today"),
    R("~/Movies", "29 GB", "4 months"),
    R("~/Downloads", "22 GB", "3 weeks"),
    R("~/Library/Caches", "14 GB", "today"),
]
SUGG = [
    R("~/Movies/Raw footage 2023", "18 GB", "14 months"),
    R("~/Downloads/macOS Installer.dmg", "12 GB", "8 months", dir=False),
    R("~/code/old-experiments", "4.1 GB", "11 months", warned=True),
    R("~/Library/Developer/Xcode/DerivedData", "9.8 GB", "6 weeks", rebuildable=True),
    R("~/code/webapp/node_modules", "3.4 GB", "5 months", rebuildable=True),
]
BASE = dict(
    disk=("412 GB", "500 GB", 82),
    largest=LARGEST,
    sugg=SUGG,
    total="47 GB",
    focus="L",
    sel=0,
)


def st(**kw):
    s = copy.deepcopy(BASE)
    s.update(kw)
    return s


def confirm(path, size, warnings=(), second=False):
    lines = [(UI_TEXT["confirm title"], True, FG), ("", False, FG), (path, False, FG), (size, False, FG)]
    for w in warnings:
        lines.append((f"! {w}", True, WARN))
    lines.append(("", False, FG))
    lines.append(
        (
            UI_TEXT["second confirm prompt"]
            if second
            else UI_TEXT["confirm prompt"],
            False,
            FG,
        )
    )
    return lines


def message(text_):
    return [(l, False, FG) for l in wrap(text_, 78)] + [("", False, FG), (UI_TEXT["message footer"], False, DIM)]


# ---------- scenes ----------

def typing(cmd, caption, first=0.7):
    frames = [(first, shell(""), caption)]
    for i in range(1, len(cmd) + 1):
        frames.append((0.11, shell(cmd[:i]), caption))
    frames.append((0.35, shell(cmd, cursor=False), caption))
    return frames


def scene_find():
    f = typing("attic", "Open the app. It reads what the background service already found, so it starts instantly.")
    cap1 = "Largest folders on top, sorted by size. Move down to look around."
    f.append((1.8, app(st()), cap1))
    f.append((0.9, app(st(sel=1, key="↓")), cap1))
    cap2 = "Enter opens a folder. Here, what is inside ~/Library/Developer."
    f.append((0.6, app(st(sel=1, key="Enter")), cap2))
    dev = dict(
        title="~/Library/Developer",
        rows=[
            R("~/Library/Developer/Xcode", "33 GB", "6 weeks"),
            R("~/Library/Developer/CoreSimulator", "4.6 GB", "3 months"),
        ],
        other="2 other files, 12 MB",
    )
    f.append((1.6, app(st(browse=dev, sel=0)), cap2))
    xc = dict(
        title="~/Library/Developer/Xcode",
        rows=[
            R("~/Library/Developer/Xcode/Archives", "14 GB", "9 months"),
            R("~/Library/Developer/Xcode/DerivedData", "9.8 GB", "6 weeks"),
            R("~/Library/Developer/Xcode/iOS DeviceSupport", "9.2 GB", "3 months"),
        ],
    )
    f.append((0.5, app(st(browse=dev, sel=0, key="Enter")), cap2))
    f.append((2.2, app(st(browse=xc, sel=1)), "Down to the folder that holds the space."))
    f.append((0.5, app(st(browse=xc, sel=1, key="Backspace")), "Backspace goes back up."))
    f.append((0.9, app(st(sel=1)), "Backspace goes back up."))
    cap3 = "Tab jumps to what attic suggests cleaning: big and long untouched comes first."
    f.append((0.5, app(st(focus="S", sel=0, key="Tab")), cap3))
    f.append((3.2, app(st(focus="S", sel=0)), cap3))
    return f


def scene_clean():
    cap = "Pick a suggestion. 'rebuildable' means the tool that made it can make it again."
    s0 = st(focus="S", sel=4)
    f = [(1.4, app(s0), cap)]
    f.append((0.5, app(dict(s0, key="c")), "c asks before anything moves."))
    path = "~/code/webapp/node_modules"
    f.append((2.2, app(dict(s0, dialog=confirm(path, "3.4 GB"))), "One confirmation. Nothing happens until you press y."))
    f.append((0.5, app(dict(s0, dialog=confirm(path, "3.4 GB"), key="y")), "Moved through Finder, so Put Back works."))
    msg = f"{UI_TEXT['moved message start']}{path} (3.4 GB). {UI_TEXT['moved message end']}"
    f.append((2.6, app(dict(s0, dialog=message(msg))), "Moved through Finder, so Put Back works."))
    after = dict(
        sugg=SUGG[:4],
        total="44 GB",
        largest=[dict(r, size="30 GB") if r["path"] == "~/code" else r for r in LARGEST],
        sel=3,
        key=None,
    )
    s1 = st(focus="S", **after)
    f.append((3.2, app(s1), "Gone from the list. Could free drops from 47 GB to 44 GB. Empty the Trash to get the space back."))
    return f


def scene_safe():
    cap = "Some suggestions carry a ! : work that exists nowhere else."
    s0 = st(focus="S", sel=2)
    f = [(2.0, app(s0), cap)]
    f.append((0.5, app(dict(s0, key="c")), "Clean it anyway? attic asks twice."))
    path = "~/code/old-experiments"
    warn = [UI_TEXT["git warning"]]
    f.append((2.4, app(dict(s0, dialog=confirm(path, "4.1 GB", warn))), "First confirmation, with the warning spelled out."))
    f.append((0.5, app(dict(s0, dialog=confirm(path, "4.1 GB", warn), key="y")), "First confirmation, with the warning spelled out."))
    f.append((2.6, app(dict(s0, dialog=confirm(path, "4.1 GB", warn, True))), "Second confirmation. Any key but y cancels."))
    f.append((0.5, app(dict(s0, dialog=confirm(path, "4.1 GB", warn, True), key="n")), "Any key but y cancels."))
    f.append((2.8, app(s0), "Nothing moved. Your home folder and ~/Documents, ~/Library and the like are never offered whole."))
    return f


def scene_notify():
    body = f"About 28 GB {UI_TEXT['notification body']}"
    f = [(1.2, shell("", cursor=True), "A background service watches the disk. Nothing to do until it finds something.")]
    f.append((0.15, shell("") + notification(UI_TEXT["notification title"], body, -30), "Once there is enough to free, it sends one notification."))
    f.append((3.0, shell("") + notification(UI_TEXT["notification title"], body), "At most one a day. Nothing leaves your machine."))
    f += [(fr[0], fr[1], "Run attic to review.") for fr in typing("attic", "")][1:]
    big = st(
        focus="S",
        sel=0,
        disk=("447 GB", "500 GB", 89),
        total="28 GB",
        largest=[R("~/Library", "71 GB", "today"), R("~/Movies", "29 GB", "4 months"), R("~/code", "33 GB", "today")],
        sugg=[
            R("~/Movies/Raw footage 2023", "18 GB", "14 months"),
            R("~/Library/Developer/Xcode/DerivedData", "9.8 GB", "6 weeks", rebuildable=True),
        ],
    )
    f.append((3.5, app(big), "The same list, ready to review."))
    return f


SCENES = {
    "1-find-what-takes-space": scene_find,
    "2-clean-up-safely": scene_clean,
    "3-warns-before-losing-work": scene_safe,
    "4-notices-in-the-background": scene_notify,
}


def chrome(title):
    return (
        rect(0.5, 0.5, W - 1, H - 1, "#0f1115", "#2a2f3a", 12)
        + rect(0.5, 0.5, W - 1, BAR, "#1c1f26", None, 12)
        + rect(0.5, BAR - 12, W - 1, 12, "#1c1f26")
        + '<circle cx="20" cy="15" r="6" fill="#ff5f57"/><circle cx="40" cy="15" r="6" fill="#febc2e"/><circle cx="60" cy="15" r="6" fill="#28c840"/>'
        + f'<text x="{W / 2:.1f}" y="19" text-anchor="middle" font-size="12" fill="{DIM}" style="font-family:{SANS}">{escape(title)}</text>'
        + rect(PAD - 6, BAR + PAD - 8, COLS * CW + 12, ROWS * LH + 16, BG, None, 6)
    )


def build(name, frames, only=None):
    total = sum(d for d, *_ in frames)
    css = [
        f"svg{{font-family:{FONT};font-size:{FS}px}}",
        ".f{opacity:0;animation-timing-function:step-end;animation-iteration-count:infinite;"
        f"animation-duration:{total + 1.5:.2f}s}}",
        "@media (prefers-reduced-motion:reduce){.f{animation:none}.last{opacity:1}}",
    ]
    body = []
    t = 0.0
    span = total + 1.5
    for i, (d, shown, cap) in enumerate(frames):
        s, e = t / span * 100, (t + d) / span * 100
        t += d
        if i == len(frames) - 1:
            e = 100
        k = f"0%{{opacity:{1 if s == 0 else 0}}}"
        if s > 0:
            k += f"{s:.3f}%{{opacity:1}}"
        if e < 100:
            k += f"{e:.3f}%{{opacity:0}}"
        css.append(f"@keyframes k{i}{{{k}}}")
        css.append(f".f{i}{{animation-name:k{i}}}")
        cls = f"f f{i}" + (" last" if i == len(frames) - 1 else "")
        capsvg = (
            f'<text x="{W / 2:.1f}" y="{H - 18:.1f}" text-anchor="middle" font-size="14" '
            f'fill="#c3c8d4" style="font-family:{SANS}">{escape(cap)}</text>'
            if cap
            else ""
        )
        if only is not None:
            if i != only:
                continue
            cls = "x"
            css.append(".x{opacity:1}")
        body.append(f'<g class="{cls}">{shown}{capsvg}</g>')
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W:.0f} {H:.0f}" '
        f'width="{W:.0f}" height="{H:.0f}" role="img" aria-label="{escape(name)}">'
        f"<style>{''.join(css)}</style>{chrome('attic')}{''.join(body)}</svg>"
    )


if __name__ == "__main__":
    out = sys.argv[1]
    if len(sys.argv) == 4:
        name = [n for n in SCENES if n.startswith(sys.argv[2])][0]
        svg = build(name, SCENES[name](), int(sys.argv[3]))
        open(f"{out}/{name}.frame.svg", "w").write(svg)
    else:
        for name, fn in SCENES.items():
            open(f"{out}/{name}.svg", "w").write(build(name, fn()))
            print(name, len(fn()), "frames")
