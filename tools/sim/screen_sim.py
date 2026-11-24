#!/usr/bin/env python3
"""screen_sim.py -- offline screen simulator for the `dst` Rust TUI.

Spawns the dst binary inside a pseudo-terminal (pty), feeds its ANSI output into
a pyte screen model, and renders deterministic snapshots of the resulting
framebuffer as:

  * plain text files (one file per snapshot, plus `final.txt`)
  * PNG images that model the device panel: a 320x170 RGB framebuffer with a
    53x12 grid of 6x14 px cells (6x13 px glyph box + 1 px line gap, 1 px black
    margin), rendered 1:1 from the public-domain X11 6x13 bitmap font (with RGB
    subpixel antialiasing for CJK/vector fallbacks) and then integer-upscaled
    for review, plus `final.png`

It supports scripted input: text sends, named keys, waits, quiet waits,
snapshots and a watch mode.  Scenarios can be given on the command line
(order matters) or loaded from a JSON file.

Why a pty and not pipes: dst calls `stty`/reads raw bytes and draws a
full-screen framebuffer; a pty is the only faithful terminal.  The slave side
is pre-configured to raw mode by this script because dst's own stty calls use
GNU-style `-F /dev/tty`, which macOS's BSD stty does not understand (it wants
`-f`).  See README.md "Limitations".

Examples
--------
    # splash screen only, text + PNG
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --snap splash

    # send a line, wait for a streamed reply, snapshot twice
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --send "hello" --key Enter --wait-ms 700 --snap thinking \
        --wait-quiet 800:12000 --snap reply

    # JSON scenario
    python3 screen_sim.py --home /tmp/home --scenario sample_scenario.json

    # watch the animated splash for 3 s at 250 ms cadence
    python3 screen_sim.py --home /tmp/home --out /tmp/shots \
        --watch 3 --interval-ms 250 --watch-prefix anim

The final text screen is printed to stdout; diagnostics go to stderr.
Exit status: 0 on success, 2 on usage/setup error, 3 on snapshot/render error.
"""

from __future__ import annotations

import argparse
import codecs
import fcntl
import glob
import gzip
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time
import tty
import unicodedata

try:
    import pyte
except ImportError:  # pragma: no cover
    sys.stderr.write("screen_sim: missing dependency: pyte (python3 -m pip install pyte)\n")
    raise SystemExit(2)

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover
    Image = None  # text-only mode still works


# --------------------------------------------------------------------------- #
# defaults / constants
# --------------------------------------------------------------------------- #

REPO = os.environ.get("DST_REPO") or os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
DEFAULT_BIN = os.environ.get("DST_BIN", os.path.join(REPO, "target", "debug", "dst"))

# Device panel model -- the PNG renderer is built on these exact pixel numbers.
#   320x170 RGB panel, 53x12 terminal grid, 6 px x 14 px cell pitch (a 6x13 px
#   glyph box at the top of each cell + a 1 px line gap).  The text area is
#   53*6 x 12*14 = 318x168 px, centred with a 1 px black margin all around.
#   The panel framebuffer is rendered 1:1 and only then integer-upscaled.
PANEL_W = 320              # device framebuffer width in RGB pixels
PANEL_H = 170              # device framebuffer height in RGB pixels
GRID_COLS = 53             # device terminal columns
GRID_ROWS = 12             # device terminal rows
CELL_W = 6                 # cell pitch, px
CELL_H = 14                # cell pitch, px (13 px glyph box + 1 px line gap)
GLYPH_W = 6                # glyph box inside one cell, px
GLYPH_H = 13
GLYPH_ASCENT = 11          # baseline row of the 6x13 bitmap font (descent 2)
SUBPIXEL = 3               # LCD RGB stripes: 3 subpixels per device pixel
PANEL_BORDER = 1           # black margin / text-area origin (native px)
TEXT_W = GRID_COLS * CELL_W   # 318 px
TEXT_H = GRID_ROWS * CELL_H   # 168 px

DEFAULT_COLS = GRID_COLS   # device: 320 px / 6 px per cell = 53 columns
DEFAULT_ROWS = GRID_ROWS   # device: 12 rows of 14 px = 168 px in a 170 px panel
DEFAULT_SCALE = 3          # output PNG: panel * scale => 320*3 x 170*3 px
DEFAULT_OUT = "snapshots"
DEFAULT_STARTUP_MS = 650   # let dst paint the splash before the first snapshot
DEFAULT_AFTER_MS = 100     # delay inserted after every send/key step
DEFAULT_QUIT_TIMEOUT_MS = 2000

KEYMAP = {
    "Enter": b"\r",
    "Return": b"\r",
    "Esc": b"\x1b",
    "Escape": b"\x1b",
    "Tab": b"\t",
    "Space": b" ",
    "Backspace": b"\x7f",
    "Delete": b"\x1b[3~",
    "Up": b"\x1b[A",
    "Down": b"\x1b[B",
    "Right": b"\x1b[C",
    "Left": b"\x1b[D",
    "Home": b"\x1b[H",
    "End": b"\x1b[F",
    "PgUp": b"\x1b[5~",
    "PgDn": b"\x1b[6~",
    "ShiftEnter": b"\x1b[13;2u",
    "AltEnter": b"\x1b\r",
    "CtrlA": b"\x01",
    "CtrlC": b"\x03",
    "CtrlD": b"\x04",
    "CtrlE": b"\x05",
    "CtrlL": b"\x0c",
    "CtrlO": b"\x0f",
    "CtrlQ": b"\x11",
    "CtrlU": b"\x15",
    "CtrlW": b"\x17",
}

# Font discovery -------------------------------------------------------------
# The bitmap font is vendored in tools/sim/fonts (public-domain X11 "Fixed");
# the vector fonts below are host fallbacks for CJK and for --font-mode ttf.
FONTS_DIR = os.environ.get("DST_SIM_FONTS") or os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "fonts")
BDF_REGULAR = "6x13.bdf.gz"       # X11 6x13 regular (4121 glyphs, no CJK)
BDF_BOLD = "6x13B.bdf.gz"         # X11 6x13 bold (1282 glyphs)
MISSING_PROBE_CHAR = "\U0010fffd"  # unmapped private-use char: .notdef probe

MONO_FONT_CANDIDATES = [
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Supplemental/Menlo.ttc",
    "/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "/System/Library/Fonts/Supplemental/Courier New.ttf",
]
CJK_FONT_CANDIDATES = [
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Supplemental/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
]
SYMBOL_FONT_CANDIDATES = [
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
]
FONT_SEARCH_DIRS = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"]

# Terminal palette (default: a dark terminal; matches what dst assumes).
DEFAULT_FG = (216, 222, 233)
DEFAULT_BG = (14, 16, 20)
CURSOR_COLOR = (120, 200, 255)

ANSI_BASE_COLORS = {
    "black": (28, 28, 28),
    "red": (204, 85, 85),
    "green": (85, 170, 85),
    "brown": (212, 160, 23),       # ANSI 33 "yellow"/brown
    "yellow": (212, 160, 23),
    "blue": (85, 85, 204),
    "magenta": (204, 85, 204),
    "cyan": (85, 204, 204),
    "white": (204, 204, 204),
}
ANSI_BRIGHT_COLORS = {
    "brightblack": (128, 128, 128),
    "brightred": (255, 85, 85),
    "brightgreen": (85, 255, 85),
    "brightbrown": (255, 215, 95),
    "brightyellow": (255, 215, 95),
    "brightblue": (85, 85, 255),
    "brightmagenta": (255, 85, 255),
    "brightcyan": (85, 255, 255),
    "brightwhite": (255, 255, 255),
}


# --------------------------------------------------------------------------- #
# small helpers
# --------------------------------------------------------------------------- #

def log(msg: str) -> None:
    sys.stderr.write(f"[sim] {msg}\n")
    sys.stderr.flush()


def safe_name(name: str) -> str:
    s = re.sub(r"[^A-Za-z0-9_.-]+", "_", name.strip())
    return s.strip("_") or "snap"


def is_wide(ch: str) -> bool:
    """East-Asian wide/fullwidth, matching what terminals draw in two cells."""
    if not ch:
        return False
    return unicodedata.east_asian_width(ch) in ("W", "F")


def xterm_256_color(n: int):
    """Map an xterm-256 palette index to RGB."""
    if 0 <= n < 16:
        names = [
            "black", "red", "green", "brown", "blue", "magenta", "cyan", "white",
        ]
        base = [ANSI_BASE_COLORS[k] for k in names]
        bright = [
            ANSI_BRIGHT_COLORS[k]
            for k in [
                "brightblack", "brightred", "brightgreen", "brightyellow",
                "brightblue", "brightmagenta", "brightcyan", "brightwhite",
            ]
        ]
        return (base + bright)[n]
    if 16 <= n <= 231:
        n -= 16
        r, g, b = n // 36, (n % 36) // 6, n % 6
        steps = [0, 95, 135, 175, 215, 255]
        return (steps[r], steps[g], steps[b])
    if 232 <= n <= 255:
        v = 8 + (n - 232) * 10
        return (v, v, v)
    return DEFAULT_FG


def resolve_color(value, default, bold=False, brighten=True):
    """Resolve a pyte color (None/'default'/name/int) to an RGB tuple."""
    if value is None or value == "default":
        rgb = default
        return rgb
    if isinstance(value, int):
        return xterm_256_color(value)
    name = str(value).lower()
    if name.startswith("bright") and name in ANSI_BRIGHT_COLORS:
        return ANSI_BRIGHT_COLORS[name]
    if name in ANSI_BASE_COLORS:
        base = ANSI_BASE_COLORS[name]
        if bold and brighten:
            bright_name = "bright" + name
            if bright_name in ANSI_BRIGHT_COLORS:
                return ANSI_BRIGHT_COLORS[bright_name]
        return base
    return default


def find_font(candidates, dirs, patterns):
    for c in candidates:
        if os.path.exists(c):
            return c
    for d in dirs:
        if not os.path.isdir(d):
            continue
        for pat in patterns:
            hits = sorted(glob.glob(os.path.join(d, pat)))
            if hits:
                return hits[0]
    return None


# --------------------------------------------------------------------------- #
# bitmap font parser (vendored X11 "Fixed" 6x13, public domain)
# --------------------------------------------------------------------------- #

_BDF_CACHE = {}


def _normalise_bdf_rows(rows, bbx, fbb):
    """Map a BDF glyph bitmap onto the device 6x13 glyph box.

    A BDF glyph is a ``BBX w h xoff yoff`` bitmap whose rows run top-down and
    whose coordinates are font units (baseline at y=0, +y up).  The device
    glyph box corresponds to the font's FONTBOUNDINGBOX cell (top row = index
    0), so the bitmap's first row lands at
    ``(fby + fbh) - (by + bh)``.  Pixels outside the 6x13 box are dropped.
    """
    bw, bh, bx, by = bbx
    _fbw, fbh, _fbx, fby = fbb
    top = (fby + fbh) - (by + bh)
    out = [0] * GLYPH_H
    for k, (value, bits) in enumerate(rows):
        row = top + k
        if row < 0 or row >= GLYPH_H:
            continue
        mask = 0
        # BDF rows are hex bytes padded on the *right*: the leftmost glyph
        # pixel is the most significant bit of the padded row, so a 6 px wide
        # glyph in 2 hex digits uses bits 7..2, not bits 5..0.
        for px in range(min(bw, bits)):
            if value & (1 << (bits - 1 - px)):
                x = bx + px
                if 0 <= x < GLYPH_W:
                    mask |= 1 << (GLYPH_W - 1 - x)
        out[row] = mask
    return tuple(out)


def load_bdf(path):
    """Parse an X11 BDF (optionally gzip-compressed) into ``{cp: 13-row mask}``.

    Row masks are 6-bit ints, MSB = leftmost glyph pixel, index 0 = top of the
    glyph box.  The result is cached at module level (a renderer is created once
    per process but a manifest run can build several).  Any parse failure
    returns ``{}``: a missing font only degrades the PNG, never the text capture.
    """
    cached = _BDF_CACHE.get(path)
    if cached is not None:
        return cached
    glyphs = {}
    try:
        opener = gzip.open if path.endswith(".gz") else open
        with opener(path, "rb") as fh:
            text = fh.read().decode("latin-1")
    except Exception as exc:
        log(f"warning: cannot read bitmap font {path}: {exc!r}")
        _BDF_CACHE[path] = glyphs
        return glyphs
    fbb = (GLYPH_W, GLYPH_H, 0, -(GLYPH_H - GLYPH_ASCENT))
    lines = text.splitlines()
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        if line.startswith("FONTBOUNDINGBOX"):
            try:
                fbb = tuple(int(p) for p in line.split()[1:5])
            except ValueError:
                pass
        elif line.startswith("STARTCHAR"):
            encoding, bbx, rows = -1, None, []
            i += 1
            while i < n and not lines[i].startswith("ENDCHAR"):
                item = lines[i]
                if item.startswith("ENCODING"):
                    try:
                        encoding = int(item.split()[1])
                    except (ValueError, IndexError):
                        encoding = -1
                elif item.startswith("BBX"):
                    try:
                        bbx = tuple(int(p) for p in item.split()[1:5])
                    except ValueError:
                        bbx = None
                elif item.startswith("BITMAP") and bbx is not None:
                    for k in range(bbx[1]):
                        j = i + 1 + k
                        if j >= n:
                            break
                        hexrow = lines[j].strip()
                        try:
                            value = int(hexrow or "0", 16)
                        except ValueError:
                            value = 0
                        rows.append((value, 4 * len(hexrow) if hexrow else bbx[0]))
                    i += bbx[1]
                i += 1
            if encoding >= 0 and bbx is not None:
                glyphs.setdefault(encoding, _normalise_bdf_rows(rows, bbx, fbb))
        i += 1
    _BDF_CACHE[path] = glyphs
    log(f"bitmap font: {os.path.basename(path)} ({len(glyphs)} glyphs)")
    return glyphs


# --------------------------------------------------------------------------- #
# PNG renderer: faithful model of the Cardputer Zero 320x170 panel
# --------------------------------------------------------------------------- #

class ScreenRenderer:
    """Render a pyte.Screen to a PNG of the device panel.

    Geometry (all native device pixels): the panel is 320x170 RGB; the terminal
    grid is 53x12 cells with a 6x14 px pitch -- a 6x13 px glyph box at the top
    of every cell plus a 1 px line gap -- so the 318x168 px text area is
    centred with a 1 px black margin on every side.  Glyphs are rasterised at
    that native resolution; the output PNG is the panel integer-upscaled by
    ``--scale`` with nearest-neighbour sampling (320*N x 170*N), so no font is
    re-rasterised at the larger size and an upscaled PNG shows exactly the
    device pixels.  With ``--native`` the 1:1 320x170 image is saved instead.

    Font handling: the vendored X11 6x13 bitmap font (``6x13.bdf.gz`` and, for
    bold, ``6x13B.bdf.gz``) draws every glyph it covers with exact 6x13 cell
    geometry (``--font-mode auto`` / ``bitmap``); characters it does not cover
    (CJK, emoji, odd symbols) and everything in ``--font-mode ttf`` are
    rasterised from a host vector font at the device box size and composited by
    the RGB-stripe subpixel filter in :meth:`_subpixel_coverage`.  Every font
    lookup is guarded: a missing font only degrades the PNG.
    """

    def __init__(self, cols, rows, scale=DEFAULT_SCALE,
                 mono_path=None, cjk_path=None, symbol_path=None,
                 font_mode="auto", subpixel=True, native=False,
                 bdf_dir=None):
        self.cols = max(1, int(cols))
        self.rows = max(1, int(rows))
        self.native = bool(native)
        self.scale = 1 if self.native else max(1, int(scale))
        mode = str(font_mode or "auto").lower()
        self.font_mode = mode if mode in ("auto", "bitmap", "ttf") else "auto"
        self.subpixel = bool(subpixel)

        # -- device geometry (native px) ----------------------------------- #
        self.text_w = self.cols * CELL_W
        self.text_h = self.rows * CELL_H
        self.panel_w = max(PANEL_W, self.text_w + 2 * PANEL_BORDER)
        self.panel_h = max(PANEL_H, self.text_h + 2 * PANEL_BORDER)
        self.text_x = (self.panel_w - self.text_w) // 2
        self.text_y = (self.panel_h - self.text_h) // 2
        # Legacy attributes: output pixels per cell at the requested scale.
        self.cell_w = CELL_W * self.scale
        self.cell_h = CELL_H * self.scale

        # -- bitmap fonts (the device-faithful default) -------------------- #
        self.bdf_dir = bdf_dir or FONTS_DIR
        self.bdf_path = os.path.join(self.bdf_dir, BDF_REGULAR)
        self.bdf_bold_path = os.path.join(self.bdf_dir, BDF_BOLD)
        self.bdf = (load_bdf(self.bdf_path) if os.path.exists(self.bdf_path)
                    else {})
        self.bdf_bold = (load_bdf(self.bdf_bold_path)
                         if os.path.exists(self.bdf_bold_path) else {})
        if not self.bdf:
            log(f"warning: bitmap font missing ({self.bdf_path}); "
                "every glyph will be vector-rendered")
        elif not self.bdf_bold:
            log(f"warning: bold bitmap font missing ({self.bdf_bold_path}); "
                "bold cells will use the regular bitmap font")

        # -- vector fonts (subpixel fallback and --font-mode ttf) ---------- #
        self.mono_path = mono_path or find_font(
            MONO_FONT_CANDIDATES, FONT_SEARCH_DIRS, ["Menlo*", "Monaco*", "Courier*"])
        self.cjk_path = cjk_path or find_font(
            CJK_FONT_CANDIDATES, FONT_SEARCH_DIRS,
            ["PingFang*", "Hiragino*", "STHeiti*", "Songti*", "*Unicode*"])
        self.symbol_path = symbol_path or find_font(
            SYMBOL_FONT_CANDIDATES, FONT_SEARCH_DIRS, ["Apple Symbols*", "Symbol*"])
        self.faces = {}
        for kind, path in (("mono", self.mono_path), ("cjk", self.cjk_path),
                           ("symbol", self.symbol_path)):
            self._register_face(kind, path)
        # Compatibility attributes: the face descriptor actually loaded (or None).
        self.mono = self.faces.get("mono")
        self.cjk = self.faces.get("cjk")
        self.symbol = self.faces.get("symbol")
        self._font_cache = {}
        self._glyph_cache = {}
        self._coverage_cache = {}
        self._size_cache = {}
        self.fonts_used = set()

    # -- font loading ------------------------------------------------------ #
    def _try_font(self, path, size, index=0):
        if Image is None or not path or not os.path.exists(path):
            return None
        try:
            return ImageFont.truetype(path, size, index=index)
        except Exception:
            return None

    def _pick_face(self, path, bold):
        """Pick a face index inside a font file/collection.

        Returns ``(index, label)``.  For ``bold`` it looks for a bold/weighted
        face (``Menlo Bold``, ``Hiragino Sans GB W6``, ...) and falls back to
        the first face when the file only ships one weight.
        """
        first = None
        for idx in range(0, 6):
            font = self._try_font(path, 13, idx)
            if font is None:
                continue
            try:
                name = " ".join(str(p) for p in font.getname()).strip()
            except Exception:
                name = ""
            label = name or f"{os.path.basename(path)}#{idx}"
            if first is None:
                first = (idx, label)
            if bold and any(tok in name.lower() for tok in (
                    "bold", "w6", "heavy", "black", "semibold", "medium")):
                return idx, label
        return first

    def _register_face(self, kind, path):
        if Image is None or not path or not os.path.exists(path):
            return
        regular = self._pick_face(path, bold=False)
        if regular is None:
            return
        self.faces[kind] = {"key": kind, "path": path,
                            "index": regular[0], "label": regular[1]}
        if kind in ("mono", "cjk"):
            bold = self._pick_face(path, bold=True)
            if bold is not None and bold[0] != regular[0]:
                self.faces[kind + "_bold"] = {
                    "key": kind + "_bold", "path": path,
                    "index": bold[0], "label": bold[1]}

    def _ttf(self, face, size):
        key = (face["path"], face["index"], int(size))
        font = self._font_cache.get(key)
        if font is None:
            font = self._try_font(face["path"], int(size), face["index"])
            self._font_cache[key] = font
        return font

    def _has_glyph(self, face, ch):
        """True when the face has a real glyph for ``ch`` (not .notdef)."""
        if face is None:
            return False
        key = ("glyph", face["path"], face["index"], ch)
        hit = self._glyph_cache.get(key)
        if hit is not None:
            return hit
        font = self._ttf(face, 13)
        has = False
        if font is not None:
            try:
                probe_key = ("notdef", face["path"], face["index"])
                probe = self._glyph_cache.get(probe_key)
                if probe is None:
                    probe = font.getmask(MISSING_PROBE_CHAR)
                    self._glyph_cache[probe_key] = probe
                has = bytes(font.getmask(ch)) != bytes(probe)
            except Exception:
                has = True
        self._glyph_cache[key] = has
        return has

    def _face_candidates(self, wide, bold):
        order = ["cjk", "symbol", "mono"] if wide else ["mono", "symbol", "cjk"]
        out = []
        for kind in order:
            if bold and (kind + "_bold") in self.faces:
                out.append(kind + "_bold")
            if kind in self.faces:
                out.append(kind)
        return out

    def _ttf_face_for(self, ch, wide, bold):
        for kind in self._face_candidates(wide, bold):
            face = self.faces.get(kind)
            if face is not None and self._has_glyph(face, ch):
                return face
        return None

    # -- glyph source selection -------------------------------------------- #
    def _bitmap_glyph(self, ch, bold):
        """Return ``(13-row masks, font label)`` from the bitmap font."""
        if not self.bdf:
            return None, None
        cp = ord(ch)
        if bold and self.bdf_bold:
            rows = self.bdf_bold.get(cp)
            if rows is not None:
                return rows, "6x13B.bdf"
        rows = self.bdf.get(cp)
        if rows is not None:
            return rows, "6x13.bdf"
        return None, None

    def _pick_source(self, ch, wide, bold):
        """Choose ``(kind, payload, label)`` for one character.

        ``auto``/``bitmap`` prefer the exact-shape 6x13 bitmap font and fall
        back to the vector font only for characters it does not cover; ``ttf``
        forces the vector font (the bitmap stays as a last resort).
        """
        if self.font_mode == "ttf":
            face = self._ttf_face_for(ch, wide, bold)
            if face is not None:
                return "ttf", face, face["label"]
            if not wide and self.bdf:
                rows, label = self._bitmap_glyph(ch, bold)
                if rows is not None:
                    return "bitmap", rows, label
            return None, None, None
        if not wide and self.bdf:
            rows, label = self._bitmap_glyph(ch, bold)
            if rows is not None:
                return "bitmap", rows, label
        face = self._ttf_face_for(ch, wide, bold)
        if face is not None:
            return "ttf", face, face["label"]
        return None, None, None

    # -- bitmap glyphs ----------------------------------------------------- #
    def _draw_bitmap(self, px, rows, x0, y0, fg):
        for gy in range(GLYPH_H):
            mask = rows[gy]
            if not mask:
                continue
            y = y0 + gy
            if y < 0 or y >= self.panel_h:
                continue
            for gx in range(GLYPH_W):
                if mask & (1 << (GLYPH_W - 1 - gx)):
                    x = x0 + gx
                    if 0 <= x < self.panel_w:
                        px[x, y] = fg

    # -- subpixel/grayscale antialiased vector glyphs ---------------------- #
    def _fit_ttf_size(self, face, ch, box_w, box_h):
        """Pick the high-res TTF pixel size for one glyph and device box.

        The panel cell is 6x14 px, so the font is sized for the *cell*: the
        advance is fitted to the device box width (6 px for a narrow glyph,
        12 px for a wide one) and the ink bounding box is then clamped into the
        6x13 / 12x13 glyph box.  At the 3x supersampling of the subpixel
        pipeline this is a 30 px em for a 6 px-wide cell and a 36 px em for a
        12 px-wide (CJK) cell; both are reported in the README.
        """
        key = ("size", face["path"], face["index"], ch, box_w)
        cached = self._size_cache.get(key)
        if cached is not None:
            return cached
        target_adv = box_w * SUBPIXEL
        ref_size = 39  # 13 device px at 3x: the reference to measure advances
        ref_font = self._ttf(face, ref_size)
        size = ref_size
        if ref_font is not None:
            try:
                adv = float(ref_font.getlength(ch))
            except Exception:
                adv = 0.0
            if adv > 0:
                size = int(round(ref_size * target_adv / adv))
            size = max(4, min(96, size))
        best = None
        for cand in range(max(4, size - 2), size + 3):
            font = self._ttf(face, cand)
            if font is None:
                continue
            try:
                adv = float(font.getlength(ch))
            except Exception:
                adv = float(target_adv)
            score = abs(adv - target_adv)
            if best is None or score < best[0]:
                best = (score, cand)
        if best is not None:
            size = best[1]
        # Shrink until the ink fits the glyph box (never clip a glyph).
        hr_w, hr_h = box_w * SUBPIXEL, GLYPH_H * SUBPIXEL
        for _ in range(10):
            font = self._ttf(face, size)
            if font is None:
                break
            try:
                bbox = font.getbbox(ch, anchor="ls")
            except Exception:
                bbox = None
            if not bbox:
                break
            left, top, right, bottom = bbox
            if (right - left) <= hr_w and (bottom - top) <= hr_h:
                break
            size = max(4, size - 1)
        self._size_cache[key] = size
        return size

    def _subpixel_coverage(self, face, ch, box_w, box_h):
        """Rasterise a vector glyph and return RGB subpixel coverages.

        Model (an approximation of an LCD, documented in README):
          1. render the glyph into an 8-bit "L" mask at ``3*box_w x 3*box_h``
             (3x horizontal for the RGB stripes, 3x vertical for the filter);
          2. per device pixel average the mask over each subpixel *column* and
             box-filter the 3 subpixel rows -> c_r, c_g, c_b in 0..1;
          3. apply FreeType's default [1,2,1]/4 horizontal FIR across the
             subpixel samples (RGB order) per device row;
          4. the caller composites ``bg*(1-c) + fg*c`` per channel.
        With ``--subpixel off`` the three coverages are replaced by their
        average (plain grayscale AA), i.e. no coloured fringes.
        """
        key = ("cov", face["path"], face["index"], ch, box_w, box_h,
               self.subpixel)
        cached = self._coverage_cache.get(key)
        if cached is not None:
            return cached
        size = self._fit_ttf_size(face, ch, box_w, box_h)
        font = self._ttf(face, size)
        if font is None:
            return None
        hr_w, hr_h = box_w * SUBPIXEL, GLYPH_H * SUBPIXEL
        mask = Image.new("L", (hr_w, hr_h), 0)
        draw = ImageDraw.Draw(mask)
        # Same baseline as the bitmap font: ascent 11 device px = 33 subpixels.
        anchor_x, anchor_y = 0, GLYPH_ASCENT * SUBPIXEL
        try:
            bbox = font.getbbox(ch, anchor="ls")
        except Exception:
            bbox = None
        if bbox:
            left, top, right, bottom = bbox
            if anchor_x + left < 0:
                anchor_x -= anchor_x + left
            if anchor_x + right > hr_w:
                anchor_x -= anchor_x + right - hr_w
            if anchor_y + top < 0:
                anchor_y -= anchor_y + top
            if anchor_y + bottom > hr_h:
                anchor_y -= anchor_y + bottom - hr_h
        try:
            draw.text((anchor_x, anchor_y), ch, font=font, fill=255, anchor="ls")
        except Exception:
            try:
                draw.text((0, 0), ch, font=font, fill=255)
            except Exception:
                return None
        data = mask.tobytes()
        rows = []
        n_sub = box_w * SUBPIXEL
        for gy in range(box_h):
            raw = [0.0] * n_sub
            for sx in range(n_sub):
                total = 0
                for k in range(SUBPIXEL):
                    total += data[(gy * SUBPIXEL + k) * hr_w + sx]
                raw[sx] = total / (SUBPIXEL * 255.0)
            if self.subpixel:
                filtered = [0.0] * n_sub
                for sx in range(n_sub):
                    a = raw[sx - 1] if sx > 0 else raw[0]
                    b = raw[sx]
                    c = raw[sx + 1] if sx < n_sub - 1 else raw[n_sub - 1]
                    filtered[sx] = (a + 2.0 * b + c) / 4.0
            else:
                filtered = [0.0] * n_sub
                for gx in range(box_w):
                    cover = (raw[3 * gx] + raw[3 * gx + 1] + raw[3 * gx + 2]) / 3.0
                    filtered[3 * gx] = cover
                    filtered[3 * gx + 1] = cover
                    filtered[3 * gx + 2] = cover
            rows.append([0.0 if v <= 0.0 else (1.0 if v >= 1.0 else v)
                         for v in filtered])
        self._coverage_cache[key] = rows
        return rows

    def _draw_subpixel(self, px, coverage, x0, y0, fg, box_w):
        fr, fgc, fb = fg
        for gy, row in enumerate(coverage):
            y = y0 + gy
            if y < 0 or y >= self.panel_h:
                continue
            for gx in range(box_w):
                x = x0 + gx
                if x < 0 or x >= self.panel_w:
                    continue
                cr, cg, cb = row[3 * gx], row[3 * gx + 1], row[3 * gx + 2]
                if cr <= 0.0 and cg <= 0.0 and cb <= 0.0:
                    continue
                pr, pg, pb = px[x, y]
                nr = pr + (fr - pr) * cr
                ng = pg + (fgc - pg) * cg
                nb = pb + (fb - pb) * cb
                px[x, y] = (min(255, max(0, int(nr + 0.5))),
                            min(255, max(0, int(ng + 0.5))),
                            min(255, max(0, int(nb + 0.5))))

    # -- colours ----------------------------------------------------------- #
    @staticmethod
    def _colours(cell):
        fg = resolve_color(cell.fg, DEFAULT_FG, bold=cell.bold)
        bg = resolve_color(cell.bg, DEFAULT_BG)
        if cell.reverse:
            # Swap foreground/background. With default colours that means a
            # light fill with dark text, not dark-on-dark.
            if bg == DEFAULT_BG:
                fg, bg = DEFAULT_BG, DEFAULT_FG
            elif fg == DEFAULT_BG:
                fg, bg = DEFAULT_FG, bg
            else:
                fg, bg = bg, fg
        return fg, bg

    # -- glyph drawing ----------------------------------------------------- #
    def _draw_glyph(self, px, cell, ch, x0, y0, wide):
        bold = bool(cell.bold)
        fg, _bg = self._colours(cell)
        kind, payload, label = self._pick_source(ch, wide, bold)
        if kind == "bitmap" and payload is not None:
            self._draw_bitmap(px, payload, x0, y0, fg)
            self.fonts_used.add(label)
        elif kind == "ttf" and payload is not None:
            box_w = 2 * CELL_W if wide else CELL_W
            coverage = self._subpixel_coverage(payload, ch, box_w, GLYPH_H)
            if coverage is not None:
                self._draw_subpixel(px, coverage, x0, y0, fg, box_w)
                self.fonts_used.add(label)
        # Combining marks (rare in this UI) are overlaid in the same cell box.
        extras = cell.data[1:] if len(cell.data) > 1 else ""
        for extra in extras:
            face = self._ttf_face_for(extra, False, bold)
            if face is None:
                continue
            coverage = self._subpixel_coverage(face, extra, CELL_W, GLYPH_H)
            if coverage is not None:
                self._draw_subpixel(px, coverage, x0, y0, fg, CELL_W)
                self.fonts_used.add(face["label"])

    # -- text -------------------------------------------------------------- #
    def text(self, screen) -> str:
        lines = []
        for r in range(self.rows):
            row = screen.buffer[r]
            chars = []
            for c in range(self.cols):
                data = row[c].data
                chars.append(data if data else "")
            lines.append("".join(chars).rstrip())
        return "\n".join(lines)

    # -- png --------------------------------------------------------------- #
    def png(self, screen, path: str) -> None:
        if Image is None:
            raise RuntimeError("Pillow is not installed; cannot render PNG")
        # The panel framebuffer is native: black outside the text area.
        img = Image.new("RGB", (self.panel_w, self.panel_h), (0, 0, 0))
        draw = ImageDraw.Draw(img)

        # Pass 1: paint every cell background first (the full 6x14 px cell,
        # line gap included). A wide character's continuation cell inherits its
        # leading cell's colours, so reverse-video CJK rows are never clipped.
        for r in range(self.rows):
            row = screen.buffer[r]
            for c in range(self.cols):
                cell = row[c]
                if cell.data == "":
                    prev = row[c - 1] if c > 0 else None
                    if prev is not None and prev.data and is_wide(prev.data[0]):
                        _fg, bg = self._colours(prev)
                    else:
                        _fg, bg = self._colours(cell)
                else:
                    _fg, bg = self._colours(cell)
                x0 = self.text_x + c * CELL_W
                y0 = self.text_y + r * CELL_H
                draw.rectangle([x0, y0, x0 + CELL_W - 1, y0 + CELL_H - 1],
                               fill=bg)

        # Pass 2: glyphs on top (bitmap = exact device pixels, vector = RGB
        # subpixel antialiasing). Glyph pixels never cross their cell box.
        px = img.load()
        for r in range(self.rows):
            row = screen.buffer[r]
            for c in range(self.cols):
                cell = row[c]
                data = cell.data
                if not data or not data.strip():
                    continue  # empty cell or the continuation of a wide glyph
                x0 = self.text_x + c * CELL_W
                y0 = self.text_y + r * CELL_H
                try:
                    self._draw_glyph(px, cell, data[0], x0, y0,
                                     is_wide(data[0]))
                except Exception as exc:  # one bad glyph must not kill the PNG
                    log(f"warning: glyph {data[0]!r} not rendered: {exc!r}")

        # Cursor: a 1 px rectangle inside the tracked cell (native pixels).
        try:
            cur = screen.cursor
            if not cur.hidden and 0 <= cur.x < self.cols and 0 <= cur.y < self.rows:
                x0 = self.text_x + cur.x * CELL_W
                y0 = self.text_y + cur.y * CELL_H
                draw.rectangle([x0, y0, x0 + CELL_W - 1, y0 + CELL_H - 1],
                               outline=CURSOR_COLOR, width=1)
        except Exception:
            pass

        # Production pet pixels. dst writes a 320×170 PPM when DST_PET_FB_OUT
        # is set; magenta is the transparent key, same as the framebuffer blit.
        overlay = getattr(self, "pet_fb_out", None)
        if overlay and os.path.isfile(overlay):
            try:
                pet = Image.open(overlay).convert("RGB")
                if pet.size == (self.panel_w, self.panel_h):
                    pp = pet.load()
                    for y in range(self.panel_h):
                        for x in range(self.panel_w):
                            pix = pp[x, y]
                            if pix != (255, 0, 255):
                                px[x, y] = pix
            except Exception as exc:
                log(f"warning: pet overlay {overlay} not composited: {exc!r}")

        if self.scale == 1:
            img.save(path, "PNG")
        else:
            big = img.resize((self.panel_w * self.scale,
                              self.panel_h * self.scale), Image.NEAREST)
            big.save(path, "PNG")

    # -- diagnostics ------------------------------------------------------- #
    @property
    def output_size(self):
        return (self.panel_w * self.scale, self.panel_h * self.scale)

    def describe(self) -> dict:
        return {
            "panel": f"{PANEL_W}x{PANEL_H} RGB device panel",
            "native_size": [self.panel_w, self.panel_h],
            "output_size": list(self.output_size),
            "scale": self.scale,
            "native": self.native,
            "grid": [self.cols, self.rows],
            "cell_pitch": [CELL_W, CELL_H],
            "glyph_box": [GLYPH_W, GLYPH_H],
            "text_area": [self.text_w, self.text_h],
            "text_origin": [self.text_x, self.text_y],
            "cell_px": [CELL_W * self.scale, CELL_H * self.scale],
            "font_mode": self.font_mode,
            "subpixel": self.subpixel,
            "bitmap_font": self.bdf_path if self.bdf else None,
            "bitmap_bold_font": self.bdf_bold_path if self.bdf_bold else None,
            "mono_font": self.mono_path,
            "cjk_font": self.cjk_path,
            "symbol_font": self.symbol_path,
            "fonts_used": sorted(self.fonts_used),
        }


# --------------------------------------------------------------------------- #
# pty session
# --------------------------------------------------------------------------- #

class PtySim:
    def __init__(self, binary, cols=DEFAULT_COLS, rows=DEFAULT_ROWS,
                 home=None, cwd=None, env=None, vtime=3, startup_ms=DEFAULT_STARTUP_MS,
                 after_ms=DEFAULT_AFTER_MS, raw_log=None, out_dir=DEFAULT_OUT,
                 renderer=None, text_only=False, startup_timeout_ms=20000,
                 pid_file=None, record_dir=None, record_interval_ms=120):
        self.binary = binary
        self.cols = int(cols)
        self.rows = int(rows)
        self.home = home
        self.cwd = cwd or os.getcwd()
        self.extra_env = dict(env or {})
        self.vtime = int(vtime)
        self.startup_ms = int(startup_ms)
        self.after_ms = int(after_ms)
        self.startup_timeout_ms = int(startup_timeout_ms)
        self.raw_log = raw_log
        self.out_dir = out_dir
        self.text_only = text_only
        self.renderer = renderer

        self.master = None
        self.slave = None
        self.pid = None
        self.returncode = None
        self.exited = False
        self.total_bytes = 0
        self.last_output = time.time()
        self.screen = pyte.Screen(self.cols, self.rows)
        self.stream = pyte.Stream(self.screen)
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self._raw_fh = None
        self._start_time = 0.0
        self.pid_file = pid_file
        self._snapshots = []
        self._snap_idx = 0
        self._input_log = []
        # Frame recording: a real screen recording of the run (PNG per tick),
        # used by the animated demo instead of a slideshow of snapshots.
        self.record_dir = record_dir
        self.record_interval_ms = int(record_interval_ms)
        self._record_frames = []
        self._record_idx = 0
        self._last_record = 0.0

    # -- lifecycle --------------------------------------------------------- #
    def start(self):
        if not os.path.exists(self.binary):
            raise SystemExit(
                f"screen_sim: dst binary not found: {self.binary}\n"
                f"  build it with: cd {REPO} && cargo build")
        os.makedirs(self.out_dir, exist_ok=True)
        if self.record_dir:
            os.makedirs(self.record_dir, exist_ok=True)
        if self.raw_log:
            os.makedirs(os.path.dirname(os.path.abspath(self.raw_log)), exist_ok=True)
            self._raw_fh = open(self.raw_log, "wb")

        master, slave = pty.openpty()
        self.master, self.slave = master, slave
        # Window size first: dst probes `stty size` at startup and clamps to 53x13.
        fcntl.ioctl(slave, termios.TIOCSWINSZ,
                    struct.pack("HHHH", self.rows, self.cols, 0, 0))
        # Put the slave in raw mode ourselves (dst's `stty -F /dev/tty` is
        # GNU-syntax and silently fails on macOS).  VMIN=0/VTIME=vtime makes
        # reads behave like dst's Linux poll mode (0.1s ticks by default).
        tty.setraw(slave)
        attrs = termios.tcgetattr(slave)
        attrs[6][termios.VMIN] = 0
        attrs[6][termios.VTIME] = self.vtime
        termios.tcsetattr(slave, termios.TCSANOW, attrs)

        env = dict(os.environ)
        env.update({
            "TERM": "xterm-256color",
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "COLORTERM": "truecolor",
        })
        env.pop("TMUX", None)  # dst hides its header row when TMUX is set
        if self.home:
            env["HOME"] = os.path.abspath(self.home)
        env.update(self.extra_env)
        cwd = os.path.abspath(self.cwd)

        pid = os.fork()
        if pid == 0:  # child
            try:
                os.close(master)
                os.setsid()
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
                os.dup2(slave, 0)
                os.dup2(slave, 1)
                os.dup2(slave, 2)
                if slave > 2:
                    os.close(slave)
                try:
                    os.chdir(cwd)
                except OSError:
                    os._exit(126)
                os.execve(self.binary, [self.binary] + list(self.child_args), env)
            except BaseException:
                os._exit(127)
        # parent
        self.pid = pid
        if self.pid_file:
            try:
                with open(self.pid_file, "w", encoding="utf-8") as fh:
                    fh.write(str(pid))
            except OSError:
                pass
        os.close(slave)
        self.slave = None
        os.set_blocking(master, False)
        self._start_time = time.time()

        # Wait for the first paint, then let the splash settle a moment.
        if not self._wait_for_first_paint(self.startup_timeout_ms):
            self._reap_nonblocking()
            raise SystemExit(
                "screen_sim: dst produced no output in "
                f"{self.startup_timeout_ms} ms (exit={self.returncode}); "
                "is the binary runnable?")
        self.pump(self.startup_ms)
        if self.exited:
            raise SystemExit(
                f"screen_sim: dst exited during startup (code={self.returncode})")

    # Extra argv for the child; subclasses/tests may override.
    child_args = ()

    def _wait_for_first_paint(self, timeout_ms):
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(50)
            if self.total_bytes > 0:
                return True
            if self.exited:
                return False
        return False

    # -- io ---------------------------------------------------------------- #
    def pump(self, ms):
        """Drain pty output for up to `ms` milliseconds."""
        deadline = time.time() + ms / 1000.0
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                self._record_frame()
                return
            if self.exited:
                time.sleep(min(remaining, 0.01))
                return
            try:
                r, _, _ = select.select([self.master], [], [], min(remaining, 0.05))
            except (OSError, ValueError):
                self._mark_exited()
                return
            if not r:
                self._reap_nonblocking()
                self._record_frame()
                continue
            try:
                data = os.read(self.master, 65536)
            except OSError:
                self._mark_exited()
                return
            if not data:
                self._mark_exited()
                return
            self._feed(data)
            self._record_frame()

    def _record_frame(self, force: bool = False):
        """Save the current screen as a recording frame (rate-limited)."""
        if not self.record_dir or self.renderer is None:
            return
        now = time.time()
        if not force and (now - self._last_record) * 1000.0 < self.record_interval_ms:
            return
        self._last_record = now
        path = os.path.join(self.record_dir, f"frame_{self._record_idx:05d}.png")
        try:
            self.renderer.png(self.screen, path)
        except Exception as exc:  # a bad frame must not lose the recording
            log(f"record frame render failed (skipped): {exc!r}")
            return
        self._record_frames.append({
            "t_ms": int((now - self._start_time) * 1000),
            "path": path,
        })
        self._record_idx += 1

    def flush_recording(self):
        if not self.record_dir:
            return
        self._record_frame(force=True)
        index = {
            "frames": self._record_frames,
            "cols": self.cols,
            "rows": self.rows,
            "duration_ms": self._record_frames[-1]["t_ms"] if self._record_frames else 0,
        }
        with open(os.path.join(self.record_dir, "frames.json"), "w",
                  encoding="utf-8") as fh:
            json.dump(index, fh)
        log(f"recorded {len(self._record_frames)} frames "
            f"({index['duration_ms'] / 1000.0:.1f}s) -> {self.record_dir}")

    def type_text(self, text: str, per_key_ms: int = 60):
        """Type like a (fast) human: one character per write, pumping between
        characters so the recording shows the prompt filling in."""
        for ch in text:
            self.write(ch.encode("utf-8"))
            self.pump(max(10, per_key_ms))

    def _feed(self, data: bytes):
        self.total_bytes += len(data)
        self.last_output = time.time()
        if self._raw_fh is not None:
            try:
                self._raw_fh.write(data)
                self._raw_fh.flush()
            except OSError:
                pass
        try:
            self.stream.feed(self._decoder.decode(data))
        except Exception as exc:  # pragma: no cover - pyte bug guard
            log(f"pyte feed error (ignored): {exc!r}")

    def pump_until_quiet(self, quiet_ms, timeout_ms):
        """Drain until no output has arrived for quiet_ms.

        NOTE: dst repaints its idle screen every ~300 ms (its edit loop redraws
        after each VTIME tick), so on a live dst this normally never goes quiet.
        Prefer `pump_until_stable` to detect "the turn finished"; this one is
        useful for a process that has genuinely stopped drawing (e.g. during a
        blocking tool call, or after exit)."""
        quiet = quiet_ms / 1000.0
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(40)
            if self.exited:
                return True
            if time.time() - self.last_output >= quiet:
                return True
        return False

    def screen_text(self) -> str:
        """Current framebuffer as plain text (the same view snapshots save)."""
        lines = []
        for r in range(self.rows):
            row = self.screen.buffer[r]
            lines.append("".join(row[c].data for c in range(self.cols)).rstrip())
        return "\n".join(lines)

    def pump_until_stable(self, stable_ms, timeout_ms):
        """Drain until the *rendered screen* has not changed for stable_ms.

        This is the reliable "turn is over" detector for dst: while a reply
        streams (or the splash animates) the screen text changes; once the turn
        settles, the periodic idle repaint produces identical frames."""
        stable = stable_ms / 1000.0
        deadline = time.time() + timeout_ms / 1000.0
        last = None
        last_change = time.time()
        while time.time() < deadline:
            self.pump(50)
            cur = self.screen_text()
            if cur != last:
                last = cur
                last_change = time.time()
            if self.exited:
                return True
            if time.time() - last_change >= stable:
                return True
        return False

    def pump_until_text(self, needle, timeout_ms):
        """Drain until `needle` appears in the rendered screen text."""
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            self.pump(50)
            if needle in self.screen_text():
                return True
            if self.exited:
                break
        return False

    def write(self, data: bytes):
        if isinstance(data, str):
            data = data.encode("utf-8")
        self._input_log.append(
            {"t": round(time.time() - self._start_time, 3), "bytes": data.hex(),
             "text": data.decode("utf-8", "replace")})
        if self.exited or self.master is None:
            raise SystemExit("screen_sim: cannot send input, dst has exited")
        view = memoryview(data)
        while view:
            try:
                n = os.write(self.master, view)
            except OSError as exc:
                raise SystemExit(f"screen_sim: write to pty failed: {exc}")
            view = view[n:]

    def send_text(self, text: str):
        self.write(text.encode("utf-8"))

    def send_key(self, name: str):
        key = name.strip()
        if key not in KEYMAP:
            raise SystemExit(
                f"screen_sim: unknown key {name!r}; known: {', '.join(sorted(KEYMAP))}")
        self.write(KEYMAP[key])

    # -- snapshots --------------------------------------------------------- #
    def capture(self, name="snap", final=False) -> str:
        """Write a text and (unless text-only) PNG snapshot.  Returns text."""
        self.pump(max(40, self.after_ms // 2))  # let pending bytes land
        text = self.screen_text()
        self._snap_idx += 1
        os.makedirs(self.out_dir, exist_ok=True)
        prefix = f"{self._snap_idx:02d}_"
        base = os.path.join(self.out_dir, prefix + safe_name(name))
        with open(base + ".txt", "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
        png_path = None
        if not self.text_only:
            png_path = base + ".png"
            try:
                self.renderer.png(self.screen, png_path)
            except Exception as exc:  # a broken PNG must never lose the text
                log(f"PNG render failed for {name!r} (text snapshot kept): {exc!r}")
                png_path = None
        entry = {
            "index": self._snap_idx,
            "name": name,
            "text": base + ".txt",
            "png": png_path,
            "elapsed_ms": int((time.time() - self._start_time) * 1000),
            "bytes_seen": self.total_bytes,
        }
        self._snapshots.append(entry)
        if final:
            with open(os.path.join(self.out_dir, "final.txt"), "w", encoding="utf-8") as fh:
                fh.write(text + "\n")
            if not self.text_only:
                try:
                    self.renderer.png(self.screen,
                                      os.path.join(self.out_dir, "final.png"))
                except Exception as exc:
                    log(f"PNG render failed for final screen (text kept): {exc!r}")
            entry["final"] = True
        png_note = f" / {png_path}" if png_path else ""
        log(f"snapshot {self._snap_idx:02d} {name!r} -> {base}.txt{png_note}")
        return text

    # -- shutdown ---------------------------------------------------------- #
    def shutdown(self, timeout_ms=DEFAULT_QUIT_TIMEOUT_MS):
        if self.master is None:
            return
        if not self.exited:
            try:
                # Ctrl-C is a normal key in dst's raw mode and exits cleanly.
                self.write(b"\x03")
            except SystemExit:
                pass
            deadline = time.time() + timeout_ms / 1000.0
            while time.time() < deadline and not self.exited:
                self.pump(50)
        if not self.exited:
            self._reap_nonblocking()
        if not self.exited and self.pid:
            log("dst did not exit after Ctrl-C; sending SIGTERM")
            try:
                os.kill(self.pid, signal.SIGTERM)
            except OSError:
                pass
            deadline = time.time() + 1.0
            while time.time() < deadline and not self.exited:
                self.pump(50)
        if not self.exited and self.pid:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except OSError:
                pass
        self._reap_blocking()
        try:
            os.close(self.master)
        except OSError:
            pass
        self.master = None
        if self._raw_fh is not None:
            self._raw_fh.close()
            self._raw_fh = None

    def _record_status(self, status):
        self.exited = True
        if os.WIFEXITED(status):
            self.returncode = os.WEXITSTATUS(status)
        elif os.WIFSIGNALED(status):
            self.returncode = -os.WTERMSIG(status)

    def _reap_blocking(self, timeout_ms=1000):
        """Reap the child for real so manifest exit codes are accurate."""
        if not self.pid:
            return
        deadline = time.time() + timeout_ms / 1000.0
        while time.time() < deadline:
            try:
                pid, status = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError:
                self.exited = True
                return
            except OSError:
                return
            if pid == self.pid:
                self._record_status(status)
                return
            time.sleep(0.02)
        try:
            os.kill(self.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            _, status = os.waitpid(self.pid, 0)
            self._record_status(status)
        except (ChildProcessError, OSError):
            self.exited = True

    def _reap_nonblocking(self, force=False):
        if self.exited and not force:
            return
        try:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            self.exited = True
            return
        except OSError:
            return
        if pid == self.pid:
            self._record_status(status)

    def _mark_exited(self):
        self.exited = True
        self._reap_nonblocking(force=True)

    # -- manifest ---------------------------------------------------------- #
    def write_manifest(self, path=None):
        path = path or os.path.join(self.out_dir, "manifest.json")
        doc = {
            "binary": self.binary,
            "cols": self.cols,
            "rows": self.rows,
            "home": self.home,
            "cwd": self.cwd,
            "env_overrides": self.extra_env,
            "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "duration_ms": int((time.time() - self._start_time) * 1000) if self._start_time else 0,
            "bytes_seen": self.total_bytes,
            "exit_code": self.returncode,
            "renderer": self.renderer.describe() if self.renderer else None,
            "snapshots": self._snapshots,
            "input": self._input_log,
            "final_text": os.path.join(self.out_dir, "final.txt"),
            "final_png": None if self.text_only else os.path.join(self.out_dir, "final.png"),
        }
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(doc, fh, indent=2, ensure_ascii=False)
            fh.write("\n")


# --------------------------------------------------------------------------- #
# scenario ops
# --------------------------------------------------------------------------- #

def normalize_steps(steps):
    """Validate/normalize JSON scenario steps into an op list."""
    ops = []
    if not isinstance(steps, list):
        raise SystemExit("scenario: 'steps' must be a list")
    for i, step in enumerate(steps):
        if not isinstance(step, dict):
            raise SystemExit(f"scenario step {i}: must be an object")
        produced = False
        if "send" in step:
            ops.append({"op": "send", "text": str(step["send"])})
            produced = True
        if "send_line" in step:
            ops.append({"op": "send_line", "text": str(step["send_line"])})
            produced = True
        if "type" in step or "type_line" in step:
            key = "type" if "type" in step else "type_line"
            ops.append({
                "op": "type",
                "text": str(step[key]),
                "per_key_ms": int(step.get("per_key_ms", 55)),
                "enter": key == "type_line",
            })
            produced = True
        if "key" in step:
            keys = step["key"]
            if isinstance(keys, list):
                for k in keys:
                    ops.append({"op": "key", "key": str(k)})
            else:
                ops.append({"op": "key", "key": str(keys)})
            produced = True
        if "wait_ms" in step:
            ops.append({"op": "wait", "ms": int(step["wait_ms"])})
            produced = True
        if "delay_ms" in step:
            ops.append({"op": "wait", "ms": int(step["delay_ms"])})
            produced = True
        if "wait_quiet" in step:
            wq = step["wait_quiet"]
            if isinstance(wq, dict):
                ops.append({
                    "op": "wait_quiet",
                    "quiet_ms": int(wq.get("quiet_ms", 700)),
                    "timeout_ms": int(wq.get("timeout_ms", 15000)),
                })
            else:
                ops.append({"op": "wait_quiet", "quiet_ms": int(wq), "timeout_ms": 15000})
            produced = True
        if "wait_quiet_ms" in step:
            ops.append({
                "op": "wait_quiet",
                "quiet_ms": int(step["wait_quiet_ms"]),
                "timeout_ms": int(step.get("timeout_ms", 15000)),
            })
            produced = True
        if "wait_stable" in step:
            ws = step["wait_stable"]
            if isinstance(ws, dict):
                ops.append({
                    "op": "wait_stable",
                    "stable_ms": int(ws.get("stable_ms", 800)),
                    "timeout_ms": int(ws.get("timeout_ms", 15000)),
                })
            else:
                ops.append({"op": "wait_stable", "stable_ms": int(ws),
                            "timeout_ms": int(step.get("timeout_ms", 15000))})
            produced = True
        if "wait_stable_ms" in step:
            ops.append({
                "op": "wait_stable",
                "stable_ms": int(step["wait_stable_ms"]),
                "timeout_ms": int(step.get("timeout_ms", 15000)),
            })
            produced = True
        if "wait_for" in step:
            wf = step["wait_for"]
            if isinstance(wf, dict):
                ops.append({"op": "wait_for", "text": str(wf.get("text", "")),
                            "timeout_ms": int(wf.get("timeout_ms", 15000))})
            else:
                ops.append({"op": "wait_for", "text": str(wf),
                            "timeout_ms": int(step.get("timeout_ms", 15000))})
            produced = True
        snaps = []
        if "snap" in step:
            snaps.append(str(step["snap"]))
        if "snaps" in step:
            snaps.extend(str(s) for s in step["snaps"])
        for s in snaps:
            ops.append({"op": "snap", "name": s})
            produced = True
        if "watch" in step:
            w = step["watch"]
            ops.append({
                "op": "watch",
                "seconds": float(w.get("seconds", 3.0)),
                "interval_ms": int(w.get("interval_ms", 300)),
                "prefix": str(w.get("prefix", "watch")),
            })
            produced = True
        if not produced:
            raise SystemExit(f"scenario step {i}: empty/unknown step {step!r}")
    return ops


def load_scenario(path):
    if path == "-":
        doc = json.load(sys.stdin)
    else:
        with open(path, "r", encoding="utf-8") as fh:
            doc = json.load(fh)
    if isinstance(doc, list):
        return normalize_steps(doc)
    if not isinstance(doc, dict) or "steps" not in doc:
        raise SystemExit(f"scenario {path}: expected {{'steps': [...]}} or a list")
    return normalize_steps(doc["steps"])


def run_ops(sim: PtySim, ops):
    for op in ops:
        kind = op["op"]
        if kind == "send":
            sim.send_text(op["text"])
            sim.pump(sim.after_ms)
        elif kind == "send_line":
            sim.send_text(op["text"])
            sim.pump(max(30, sim.after_ms // 2))
            sim.write(b"\r")
            sim.pump(sim.after_ms)
        elif kind == "type":
            sim.type_text(op["text"], op.get("per_key_ms", 55))
            if op.get("enter"):
                sim.write(b"\r")
                sim.pump(sim.after_ms)
        elif kind == "key":
            sim.send_key(op["key"])
            sim.pump(sim.after_ms)
        elif kind == "wait":
            sim.pump(op["ms"])
        elif kind == "wait_quiet":
            ok = sim.pump_until_quiet(op["quiet_ms"], op["timeout_ms"])
            if not ok:
                log(f"wait_quiet timed out after {op['timeout_ms']} ms; snapshotting anyway")
        elif kind == "wait_stable":
            ok = sim.pump_until_stable(op["stable_ms"], op["timeout_ms"])
            if not ok:
                log(f"wait_stable timed out after {op['timeout_ms']} ms; "
                    "the screen never settled (still animating/streaming?)")
        elif kind == "wait_for":
            ok = sim.pump_until_text(op["text"], op["timeout_ms"])
            if not ok:
                log(f"wait_for {op['text']!r} not found within {op['timeout_ms']} ms")
        elif kind == "snap":
            sim.capture(op["name"])
        elif kind == "watch":
            run_watch(sim, op["seconds"], op["interval_ms"], op["prefix"])
        else:  # pragma: no cover
            raise SystemExit(f"internal: unknown op {kind}")


def run_watch(sim: PtySim, seconds: float, interval_ms: int, prefix: str):
    log(f"watch: {seconds:.1f}s at {interval_ms} ms")
    start = time.time()
    n = 0
    while time.time() - start < seconds:
        sim.pump(interval_ms)
        sim.capture(f"{prefix}_{n:03d}")
        n += 1
    return n


# --------------------------------------------------------------------------- #
# command line
# --------------------------------------------------------------------------- #

STEP_OPTS = {
    "--send", "--send-line", "--key", "--wait-ms", "--delay-ms",
    "--snap", "--wait-quiet", "--wait-stable", "--wait-for",
}


def build_parser():
    p = argparse.ArgumentParser(
        prog="screen_sim.py",
        description="Run the dst TUI in a pty and capture its screen as text/PNG.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Scenario steps are executed in command-line order (interleaving is\n"
            "preserved).  JSON scenario files use {\"steps\": [...]} with keys\n"
            "send/send_line/key/wait_ms/wait_quiet/snap/watch.\n\n"
            "Examples:\n"
            "  %(prog)s --home /tmp/dsthome --snap splash\n"
            "  %(prog)s --home /tmp/dsthome --send hi --key Enter --wait-ms 700\n"
            "           --snap thinking --wait-quiet 800:12000 --snap reply\n"
            "  %(prog)s --scenario sample_scenario.json --out screenshots\n"
            "  %(prog)s --watch 3 --interval-ms 250 --watch-prefix anim\n"
        ),
    )
    p.add_argument("--bin", default=DEFAULT_BIN, help="dst binary (default: %(default)s)")
    p.add_argument("--cols", type=int, default=DEFAULT_COLS, help="pty width in cells (default 53)")
    p.add_argument("--rows", type=int, default=DEFAULT_ROWS, help="pty height in cells (default 12)")
    p.add_argument("--home", help="HOME for the child (point it at a dir holding .dst/config.yml)")
    p.add_argument("--cwd", help="working directory for dst (default: current directory)")
    p.add_argument("--env", action="append", default=[], metavar="K=V",
                   help="extra environment variable for the child (repeatable)")
    p.add_argument("--out", default=DEFAULT_OUT, help="snapshot output directory (default: %(default)s)")
    p.add_argument("--scenario", help="JSON scenario file ('-' for stdin)")
    p.add_argument("--watch", type=float, default=0.0, metavar="SECONDS",
                   help="watch mode: snapshot every --interval-ms for SECONDS")
    p.add_argument("--interval-ms", type=int, default=300,
                   help="watch mode snapshot interval (default 300)")
    p.add_argument("--watch-prefix", default="watch", help="watch snapshot name prefix")
    p.add_argument("--startup-ms", type=int, default=DEFAULT_STARTUP_MS,
                   help="settle time after dst's first paint (default 650)")
    p.add_argument("--after-ms", type=int, default=DEFAULT_AFTER_MS,
                   help="delay after each send/key step (default 100)")
    p.add_argument("--pid-file", default=None,
                   help="write the dst child PID here (for RSS sampling)")
    p.add_argument("--vtime", type=int, default=3,
                   help="terminal VTIME in tenths of a second for idle reads (default 3)")
    p.add_argument("--scale", type=int, default=DEFAULT_SCALE,
                   help="output PNG is the 320x170 device panel upscaled by N "
                        "with nearest-neighbour sampling -> 320*N x 170*N px "
                        "(default 3 -> 960x510)")
    p.add_argument("--native", action="store_true",
                   help="save the 1:1 320x170 panel framebuffer instead of an "
                        "upscaled PNG (implies --scale 1; for pixel inspection)")
    p.add_argument("--font-mode", choices=("auto", "bitmap", "ttf"), default="auto",
                   help="glyph source: auto/bitmap = vendored X11 6x13 bitmap "
                        "font (vector+subpixel only for glyphs it lacks), ttf = "
                        "host vector font for every glyph (comparison mode)")
    p.add_argument("--subpixel", choices=("on", "off"), default="on",
                   help="RGB-stripe subpixel antialiasing for vector glyphs "
                        "(default on; off = grayscale antialiasing)")
    p.add_argument("--mono-font", help="override monospace font path (Menlo; used by --font-mode ttf)")
    p.add_argument("--cjk-font", help="override CJK font path (PingFang SC etc.)")
    p.add_argument("--symbol-font", help="override symbol font path (Apple Symbols)")
    p.add_argument("--text-only", action="store_true", help="skip PNG rendering")
    p.add_argument("--record-dir", default=None,
                   help="record every rendered frame here (PNG + frames.json)")
    p.add_argument("--record-interval-ms", type=int, default=120,
                   help="minimum time between recorded frames (default 120)")
    p.add_argument("--raw-log", help="also write the raw pty byte stream here")
    p.add_argument("--quit-timeout-ms", type=int, default=DEFAULT_QUIT_TIMEOUT_MS,
                   help="how long to wait for a clean Ctrl-C exit (default 2000)")
    # ordered step options (handled by the scanner below, listed for --help)
    p.add_argument("--key", action="append", metavar="NAME",
                   help="send a named key: " + "|".join(sorted(KEYMAP)))
    p.add_argument("--wait-quiet", action="append", metavar="QUIET_MS[:TIMEOUT_MS]",
                   help="wait until output is quiet (default timeout 15000)")
    p.add_argument("--snap", action="append", metavar="NAME",
                   help="capture a snapshot at this point in the sequence")
    p.add_argument("--wait-stable", action="append", metavar="STABLE_MS[:TIMEOUT_MS]",
                   help="wait until the rendered screen has not changed for "
                        "STABLE_MS (turn-finished detector; default timeout 15000)")
    p.add_argument("--wait-for", action="append", metavar="TEXT[:TIMEOUT_MS]",
                   help="wait until TEXT appears on screen (default timeout 15000)")
    return p


def scan_ordered(argv):
    """Split argv into (global argv, ordered ops).

    argparse cannot preserve interleaving of repeatable options, so the ordered
    step options are extracted manually first.  Everything else is passed on to
    argparse unchanged.
    """
    ops = []
    rest = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            rest.append(a)
            i += 1
            continue
        if a in STEP_OPTS:
            if i + 1 >= len(argv):
                raise SystemExit(f"screen_sim: {a} needs a value")
            val = argv[i + 1]
            if a == "--send":
                ops.append({"op": "send", "text": val})
            elif a == "--send-line":
                ops.append({"op": "send_line", "text": val})
            elif a == "--key":
                ops.append({"op": "key", "key": val})
            elif a in ("--wait-ms", "--delay-ms"):
                ops.append({"op": "wait", "ms": int(val)})
            elif a == "--snap":
                ops.append({"op": "snap", "name": val})
            elif a == "--wait-quiet":
                if ":" in val:
                    q, t = val.split(":", 1)
                    ops.append({"op": "wait_quiet", "quiet_ms": int(q), "timeout_ms": int(t)})
                else:
                    ops.append({"op": "wait_quiet", "quiet_ms": int(val), "timeout_ms": 15000})
            elif a == "--wait-stable":
                if ":" in val:
                    q, t = val.split(":", 1)
                    ops.append({"op": "wait_stable", "stable_ms": int(q),
                                "timeout_ms": int(t)})
                else:
                    ops.append({"op": "wait_stable", "stable_ms": int(val),
                                "timeout_ms": 15000})
            elif a == "--wait-for":
                if ":" in val and not val.startswith(":"):
                    text, t = val.rsplit(":", 1)
                    if t.isdigit():
                        ops.append({"op": "wait_for", "text": text,
                                    "timeout_ms": int(t)})
                    else:
                        ops.append({"op": "wait_for", "text": val,
                                    "timeout_ms": 15000})
                else:
                    ops.append({"op": "wait_for", "text": val, "timeout_ms": 15000})
            i += 2
            continue
        # Also catch --opt=value forms of global options.
        rest.append(a)
        i += 1
    return rest, ops


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = build_parser()
    rest, ops = scan_ordered(argv)
    args = parser.parse_args(rest)

    extra_env = {}
    for pair in args.env:
        if "=" not in pair:
            raise SystemExit(f"screen_sim: --env expects K=V, got {pair!r}")
        k, v = pair.split("=", 1)
        extra_env[k] = v

    if args.scenario:
        ops = load_scenario(args.scenario) + ops

    renderer = None
    if not args.text_only:
        if Image is None:
            raise SystemExit("screen_sim: Pillow not installed; rerun with --text-only")
        renderer = ScreenRenderer(
            args.cols, args.rows, scale=args.scale,
            mono_path=args.mono_font, cjk_path=args.cjk_font,
            symbol_path=args.symbol_font, font_mode=args.font_mode,
            subpixel=args.subpixel != "off", native=args.native)
        renderer.pet_fb_out = extra_env.get("DST_PET_FB_OUT")
        out_w, out_h = renderer.output_size
        log(f"renderer: {renderer.font_mode} font mode, subpixel="
            f"{'on' if renderer.subpixel else 'off'}, "
            f"{renderer.panel_w}x{renderer.panel_h} panel -> {out_w}x{out_h} PNG")
        if renderer.faces.get("cjk") is None:
            log("warning: no CJK font found; wide glyphs will be blank")
        if renderer.faces.get("mono") is None and not renderer.bdf:
            log("warning: no monospace font found; text will be blank")

    sim = PtySim(
        binary=os.path.abspath(os.path.expanduser(args.bin)),
        cols=args.cols, rows=args.rows, pid_file=args.pid_file,
        home=args.home, cwd=args.cwd, env=extra_env,
        vtime=args.vtime, startup_ms=args.startup_ms, after_ms=args.after_ms,
        raw_log=args.raw_log, out_dir=args.out, renderer=renderer,
        text_only=args.text_only, record_dir=args.record_dir,
        record_interval_ms=args.record_interval_ms)
    sim.start()
    log(f"spawned dst pid={sim.pid} pty={args.cols}x{args.rows} "
        f"HOME={sim.home or '(inherited)'} cwd={sim.cwd}")

    final_text = ""
    try:
        if args.watch > 0:
            run_watch(sim, args.watch, args.interval_ms, args.watch_prefix)
        if ops:
            run_ops(sim, ops)
        final_text = sim.capture("final", final=True)
        sim.flush_recording()
    except SystemExit:
        raise
    except BaseException as exc:
        log(f"error during scenario: {exc!r}")
        try:
            final_text = sim.capture("final", final=True)
        except Exception:
            pass
        raise
    finally:
        sim.shutdown(args.quit_timeout_ms)
        sim.write_manifest()

    log(f"dst exit code={sim.returncode}, {sim.total_bytes} bytes, "
        f"{len(sim._snapshots)} snapshots in {args.out}")
    # The final text screen goes to stdout (pipes/CI friendly).
    sys.stdout.write(final_text + "\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        sys.stderr.write("\nscreen_sim: interrupted\n")
        raise SystemExit(130)
