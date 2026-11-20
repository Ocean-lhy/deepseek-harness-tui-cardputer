#!/usr/bin/env python3
"""Design mockup for the Cardputer Zero desk pet (docs/pet/DESIGN.md).

Composes real `screen_sim.py --native` captures of the current dst build with
candidate pet layouts on the 320x170 panel, so the pet form can be judged at
the device's true pixel size. Nothing here is product code: the pixel layers
are drawn in Python to show *what* the Rust implementation should produce.

Inputs (all local paths, nothing is downloaded):
  --shots DIR      screen_sim native captures: 01_idle.png and 02_working.png
  --frames FILE    dsh-TUI src/components/whaleFrames.ts (MIT; art from
                   lhh010/dsh-ui-whale, BSD-3-Clause)
  --musume DIR     dsh-whale-musume assets/generated (MIT) - optional
  --out FILE       contact sheet PNG (each panel 2x nearest-neighbour)

See docs/pet/DESIGN.md section 3 for the exact commands and commits.
"""
import argparse
import os
import re

from PIL import Image, ImageDraw, ImageFont

W, H = 320, 170
CELL_W, CELL_H, MARGIN = 6, 14, 1
TEXT_ROWS = 10                       # rows 0..9 above the live/input rows
DOCK_COLS = 10                       # right-hand dock width in cells (60 px)
DOCK_X = MARGIN + (53 - DOCK_COLS) * CELL_W
BODY_H = TEXT_ROWS * CELL_H          # 140 px
BG = (14, 16, 20)
SEA_BG = (10, 18, 40)

PALETTE = {                          # dsh-ui-whale palette
    "D": (32, 56, 100), "B": (0, 102, 255), "L": (180, 199, 231),
    "W": (242, 242, 242), "H": (204, 51, 153), "Z": (128, 128, 128),
}
ANSI8 = {                            # what the 8-colour fbterm palette can show
    "D": (85, 85, 204), "B": (85, 85, 255), "L": (85, 204, 204),
    "W": (255, 255, 255), "H": (255, 85, 255), "Z": (128, 128, 128),
}


def rgb565(c):
    return ((c[0] >> 3) << 3, (c[1] >> 2) << 2, (c[2] >> 3) << 3)


def load_frames(path):
    src = open(path, encoding="utf-8").read()
    frames = {}
    for m in re.finditer(r"name: '([^']+)',\s*rows: \[(.*?)\]", src, re.S):
        frames[m.group(1)] = re.findall(r"'([.DBLWHZ]+)'", m.group(2))
    return frames


def trim(rows):
    ys = [y for y, r in enumerate(rows) if r.strip(".")]
    xs = [x for r in rows for x, ch in enumerate(r) if ch != "."]
    return [r[min(xs):max(xs) + 1] for r in rows[min(ys):max(ys) + 1]]


def sprite(rows, scale, palette=PALETTE):
    rows = trim(rows)
    img = Image.new("RGBA", (len(rows[0]) * scale, len(rows) * scale), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for y, r in enumerate(rows):
        for x, ch in enumerate(r):
            if ch in palette:
                d.rectangle([x * scale, y * scale, x * scale + scale - 1, y * scale + scale - 1],
                            fill=rgb565(palette[ch]))
    return img


def label_font(size):
    for p in ["/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
              "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf"]:
        if os.path.exists(p):
            return ImageFont.truetype(p, size)
    return ImageFont.load_default()


def clear_dock(img, rows=TEXT_ROWS):
    ImageDraw.Draw(img).rectangle([DOCK_X, MARGIN, W - 1, MARGIN + rows * CELL_H - 1], fill=BG)


def bars(d, x, y, items, font):
    for i, (name, value, color) in enumerate(items):
        yy = y + i * 14
        d.text((x, yy), name, font=font, fill=(200, 200, 200))
        d.rectangle([x + 14, yy + 4, x + 56, yy + 9], outline=(90, 90, 90))
        d.rectangle([x + 15, yy + 5, x + 15 + int(40 * value), yy + 8], fill=rgb565(color))


def tool_icon(d, x, y):
    # 14x9 laptop: the "terminal/bash" work prop drawn next to the whale.
    d.rectangle([x + 1, y, x + 12, y + 6], fill=rgb565((60, 60, 70)), outline=rgb565((200, 200, 210)))
    d.line([x + 3, y + 2, x + 6, y + 2], fill=rgb565((85, 255, 85)))
    d.line([x + 3, y + 4, x + 8, y + 4], fill=rgb565((85, 255, 85)))
    d.rectangle([x, y + 7, x + 13, y + 8], fill=rgb565((200, 200, 210)))


def dock_whale(base, frames, font):
    img = base.copy()
    clear_dock(img)
    d = ImageDraw.Draw(img)
    whale = sprite(frames["tail2"], 2)
    img.paste(whale, (DOCK_X + (60 - whale.width) // 2, MARGIN + 4), whale)
    tool_icon(d, DOCK_X + 4, MARGIN + 52)
    d.text((DOCK_X + 21, MARGIN + 50), "bash", font=font, fill=(212, 160, 23))
    bars(d, DOCK_X + 2, MARGIN + 70, [("心", 0.72, (255, 85, 170)), ("饱", 0.55, (255, 170, 60)),
                                      ("感", 0.30, (85, 204, 255))], font)
    d.text((DOCK_X + 2, MARGIN + 114), "Lv3 第12天", font=font, fill=(160, 160, 160))
    return img


def dock_musume(base, musume_dir, font):
    img = base.copy()
    clear_dock(img)
    d = ImageDraw.Draw(img)
    pose = Image.open(os.path.join(musume_dir, "dsh-whale-state-tool.webp")).convert("RGBA")
    pose = pose.resize((60, 60), Image.LANCZOS)
    flat = Image.new("RGBA", pose.size, BG + (255,))
    flat.alpha_composite(pose)
    flat = Image.eval(flat.convert("RGB"), lambda v: v)
    px = flat.load()
    for y in range(flat.height):
        for x in range(flat.width):
            px[x, y] = rgb565(px[x, y])
    img.paste(flat, (DOCK_X, MARGIN + 2))
    bars(d, DOCK_X + 2, MARGIN + 70, [("心", 0.72, (255, 85, 170)), ("饱", 0.55, (255, 170, 60)),
                                      ("感", 0.30, (85, 204, 255))], font)
    d.text((DOCK_X + 2, MARGIN + 114), "Lv3 第12天", font=font, fill=(160, 160, 160))
    return img


def room(base, layer, font, caption):
    img = base.copy()
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, W - 1, MARGIN + BODY_H - 1], fill=rgb565(SEA_BG))
    for x in range(0, W, 2):          # sea line
        y = 118 + (2 if (x // 8) % 3 == 0 else 0)
        d.point((x, y), fill=rgb565((60, 110, 200)))
    for sx, sy in [(20, 10), (75, 26), (150, 8), (230, 20), (290, 12)]:
        d.point((sx, sy), fill=(200, 200, 200))
    img.paste(layer, ((W - layer.width) // 2, 118 - layer.height + 6), layer)
    d.text((4, MARGIN + BODY_H - 16), caption, font=font, fill=(150, 150, 160))
    return img


def halfblock(rows):
    """The whale as fbterm would show it with `▀` cells: 1x2 sprite px per cell."""
    rows = trim(rows)
    if len(rows) % 2:
        rows.append("." * len(rows[0]))
    rows = rows[:2 * TEXT_ROWS]
    img = Image.new("RGBA", (len(rows[0]) * CELL_W, len(rows) // 2 * CELL_H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for cy in range(len(rows) // 2):
        for x in range(len(rows[0])):
            for half, ch in enumerate((rows[2 * cy][x], rows[2 * cy + 1][x])):
                if ch in ANSI8:
                    y0 = cy * CELL_H + half * 7
                    d.rectangle([x * CELL_W, y0, x * CELL_W + CELL_W - 1, y0 + 6], fill=ANSI8[ch])
    return img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--shots", required=True)
    ap.add_argument("--frames", required=True)
    ap.add_argument("--musume")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    idle = Image.open(os.path.join(a.shots, "01_idle.png")).convert("RGB")
    work = Image.open(os.path.join(a.shots, "02_working.png")).convert("RGB")
    frames = load_frames(a.frames)
    small = label_font(10)

    panels = [
        ("现状 · 空闲：字符鲸 splash（art.rs，8 色）", idle),
        ("现状 · 工作中：53 列 transcript", work),
        ("A 推荐 · 工作中：像素侧栏 60px，大肥鱼 ×2", dock_whale(work, frames, small)),
        ("B 推荐 · 空闲：像素宠物房，大肥鱼 ×3 睡觉",
         room(idle, sprite(frames["sleep3"], 3), small, "大肥鱼睡着了 · /pet 打开")),
        ("D 备选 · 纯字符：上半块字符 + 8 色", room(idle, halfblock(frames["standard"]), small, "")),
    ]
    if a.musume:
        pose = Image.open(os.path.join(a.musume, "dsh-whale-state-sleep.webp")).convert("RGBA")
        pose = pose.resize((128, 128), Image.LANCZOS)
        panels.insert(4, ("C 备选 · 工作中：鲸鱼娘立绘缩到 60px", dock_musume(work, a.musume, small)))
        panels.insert(5, ("C 备选 · 空闲：鲸鱼娘 128px", room(idle, pose, small, "")))

    scale, pad, head = 2, 12, 22
    cols = 2
    rows = (len(panels) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (W * scale + pad) + pad, rows * (H * scale + head + pad) + pad), (32, 34, 40))
    d = ImageDraw.Draw(sheet)
    font = label_font(15)
    for i, (title, img) in enumerate(panels):
        x = pad + (i % cols) * (W * scale + pad)
        y = pad + (i // cols) * (H * scale + head + pad)
        d.text((x, y), title, font=font, fill=(230, 230, 230))
        sheet.paste(img.resize((W * scale, H * scale), Image.NEAREST), (x, y + head))
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    sheet.save(a.out, optimize=True)
    print(a.out, sheet.size)


if __name__ == "__main__":
    main()
