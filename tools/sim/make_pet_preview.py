#!/usr/bin/env python3
"""Offscreen previews of the whale girl through dst's own renderer.

Pixel frames come from `dst --pet-preview` (pet_art::render). The TUI shots
come from screen_sim driving the debug binary, with DST_PET_FB_OUT composited
by screen_sim (magenta is transparent). Release binaries are not used: a
release dst checks for updates unless DST_NO_UPDATE_CHECK=1, and this script
sets that variable anyway.

This is not an acceptance pass. It only records what this build draws.
"""

import os
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SIM = os.path.join(ROOT, "tools", "sim", "screen_sim.py")
BIN = os.path.join(ROOT, "target", "debug", "dst")
OUT = os.environ.get("PET_PREVIEW_OUT", os.path.join(ROOT, "docs", "pet", "preview"))


def run(cmd, env=None, check=True):
    merged = os.environ.copy()
    merged["DST_NO_UPDATE_CHECK"] = "1"
    if env:
        merged.update(env)
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=ROOT, env=merged, check=check)


def ppm_to_png(src, dest):
    from PIL import Image
    im = Image.open(src).convert("RGB")
    im.save(dest)


def sheet(directory, dest):
    from PIL import Image, ImageDraw
    names = ["idle", "blink", "think", "talk", "happy", "sad", "eat", "blush", "sleep", "tool"]
    tiles = []
    for name in names:
        im = Image.open(os.path.join(directory, f"dock-{name}.ppm")).convert("RGB")
        # Crop the dock (x=259, 60x140) so the contact sheet is the pet, not
        # a page of magenta.
        tiles.append(im.crop((259, 0, 319, 140)))
    w, h = tiles[0].size
    canvas = Image.new("RGB", (w * len(tiles), h + 14), (12, 16, 28))
    draw = ImageDraw.Draw(canvas)
    for i, tile in enumerate(tiles):
        canvas.paste(tile, (i * w, 14))
        draw.text((i * w + 2, 1), names[i], fill=(220, 220, 220))
    canvas.save(dest)


def write_home(path, text):
    os.makedirs(os.path.join(path, ".dst"), exist_ok=True)
    cfg = os.path.join(path, ".dst", "config.yml")
    with open(cfg, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.chmod(cfg, 0o600)


def snap(home, out_dir, ppm, steps):
    os.makedirs(out_dir, exist_ok=True)
    if os.path.exists(ppm):
        os.remove(ppm)
    cmd = [
        sys.executable, SIM,
        "--bin", BIN,
        "--home", home,
        "--cwd", ROOT,
        "--out", out_dir,
        "--native",
        "--startup-ms", "1200",
        "--env", "DST_NO_UPDATE_CHECK=1",
        "--env", f"DST_PET_FB_OUT={ppm}",
    ]
    cmd.extend(steps)
    cmd.extend(["--wait-ms", "200", "--key", "CtrlC"])
    run(cmd)


def main():
    os.makedirs(OUT, exist_ok=True)
    if not os.path.exists(BIN):
        run(["cargo", "build", "--bin", "dst"])
    frames = os.path.join(OUT, "frames")
    t0 = time.perf_counter()
    run([BIN, "--pet-preview", "sheet", frames])
    dt = time.perf_counter() - t0
    print(f"sheet render {dt:.3f}s (10 dock frames + 3 room frames)")
    for name in ("dock-idle", "dock-tool", "dock-happy", "dock-sleep", "room-idle", "room-sleep", "room-happy"):
        ppm_to_png(os.path.join(frames, name + ".ppm"), os.path.join(OUT, name + ".png"))
    sheet(frames, os.path.join(OUT, "contact.png"))

    # Idle pixel room, then a scripted turn so the work pose is a real Ev.
    home = os.path.join(OUT, "home-pixel")
    write_home(home, "model: deepseek-flash\napi:\n  key: test\ntui:\n  tips_shown: true\n  hw_survey_done: true\nsandbox:\n  mode: full\npet:\n  enabled: true\n  render: pixel\n")
    script = os.path.join(OUT, "script.json")
    with open(script, "w", encoding="utf-8") as fh:
        fh.write(
            '{"responses":[{"content":"","tool_calls":[{"id":"t1","name":"bash",'
            '"arguments":{"command":"sleep 3","timeout_s":8}}],"finish_reason":"tool_calls"},'
            '{"content":"done","finish_reason":"stop"}]}'
        )
    mock_log = os.path.join(OUT, "mock.jsonl")
    mock = subprocess.Popen(
        [sys.executable, "-u", os.path.join(ROOT, "tools", "sim", "mock_api.py"),
         "--port", "0", "--script", script, "--log", mock_log],
        cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    url = None
    deadline = time.time() + 15
    assert mock.stdout is not None
    while time.time() < deadline:
        line = mock.stdout.readline()
        if not line:
            break
        if line.startswith("MOCK_API_URL="):
            url = line.split("=", 1)[1].strip()
            break
    if not url:
        mock.kill()
        raise SystemExit("mock api did not start")
    # Point the home config at the mock. The key stays a test key, not a secret.
    write_home(
        home,
        "model: deepseek-flash\n"
        f"api:\n  base: {url}\n  key: test\n"
        "tui:\n  tips_shown: true\n  hw_survey_done: true\n"
        "sandbox:\n  mode: full\n"
        "pet:\n  enabled: true\n  dock: true\n  room: true\n  render: pixel\n",
    )
    try:
        snap(home, os.path.join(OUT, "sim"), os.path.join(OUT, "live.ppm"), [
            "--wait-ms", "900", "--snap", "idle-room",
            "--send", "look around", "--key", "Enter",
            "--wait-for", "bash:20000", "--snap", "working",
            "--wait-for", "done:20000", "--snap", "replied",
            "--send", "/pet pat", "--key", "Enter", "--wait-ms", "600", "--snap", "pat",
            "--send", "/pet dock off", "--key", "Enter", "--wait-ms", "500", "--snap", "dock-off",
        ])
    finally:
        mock.terminate()
        try:
            mock.wait(timeout=3)
        except subprocess.TimeoutExpired:
            mock.kill()

    text_home = os.path.join(OUT, "home-text")
    write_home(text_home, "model: deepseek-flash\napi:\n  key: test\ntui:\n  tips_shown: true\n  hw_survey_done: true\npet:\n  enabled: true\n  render: text\n  room: true\n")
    # No DST_PET_FB_OUT: this shot must be the character fallback alone.
    run([
        sys.executable, SIM, "--bin", BIN, "--home", text_home, "--cwd", ROOT,
        "--out", os.path.join(OUT, "sim-text"), "--native", "--startup-ms", "800",
        "--env", "DST_NO_UPDATE_CHECK=1",
        "--wait-ms", "600", "--snap", "text-room",
        "--wait-ms", "200", "--key", "CtrlC",
    ])
    print("previews in", OUT)


if __name__ == "__main__":
    main()
