"""Does the replay draw its players and effects where the facts put them? For each case (a demo, a round, a moment, a
camera), the sandbox renders the moment offscreen four times: everything, the players alone, the throws alone, and the
bare map (`--replay-layers`). What the players drew is where their picture differs from the bare map's, and so for the
throws. Each living player in the frame must have drawn pixels within a body's width of where the facts project it (the
replay draws them through walls, faint, so none can hide), in its side's colour (what it changes: T gold adds more red than blue,
CT blue more blue than red). Each live smoke, fire and grenade in flight is looked for at its point, unless a wall hides it.

    python3 bench/replay_check.py --data <cs2-throws files or folder> [--packs DIR] [--cases cases.json]

The default cases are eleven moments of five rounds on four maps of the ESL Pro League S24 demos (S30). Evidence goes to
.runs/replay-<time>/: each case's four PNGs, its facts (JSON) and report.md."""

from __future__ import annotations

import argparse
import json
import math
import os
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

CASES = [
    {"demo": "vitality-vs-mouz-m1-cache", "round": 16, "time": 28.5, "camera": "chase:ropz"},
    {"demo": "vitality-vs-mouz-m1-cache", "round": 16, "time": 28.5, "camera": "chase:xelex"},
    {"demo": "vitality-vs-mouz-m1-cache", "round": 16, "time": 28.5, "camera": "top"},
    {"demo": "vitality-vs-mouz-m1-cache", "round": 16, "time": 10.5, "camera": "overview"},
    {"demo": "vitality-vs-mouz-m1-cache", "round": 16, "time": 50.0, "camera": "top"},
    {"demo": "vitality-vs-mouz-m2-inferno", "round": 4, "time": 30.0, "camera": "overview"},
    {"demo": "vitality-vs-mouz-m2-inferno", "round": 4, "time": 30.0, "camera": "chase:ZywOo"},
    {"demo": "aurora-vs-vitality-m3-mirage", "round": 7, "time": 20.0, "camera": "overview"},
    {"demo": "aurora-vs-vitality-m3-mirage", "round": 7, "time": 20.0, "camera": "top"},
    {"demo": "vitality-vs-mouz-m3-dust2", "round": 10, "time": 25.0, "camera": "overview"},
    {"demo": "vitality-vs-mouz-m3-dust2", "round": 10, "time": 25.0, "camera": "chase:apEX"},
]
LAYERS = ("all", "players", "effects", "map")


def read_png(path: Path) -> tuple[int, int, int, bytearray]:
    """width, height, channels, rows of pixels: 8-bit RGB or RGBA, not interlaced."""
    data = path.read_bytes()
    at, idat = 8, b""
    while at < len(data):
        n = struct.unpack(">I", data[at : at + 4])[0]
        kind, body = data[at + 4 : at + 8], data[at + 8 : at + 8 + n]
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            assert depth == 8 and colour in (2, 6), f"{path}: {depth}-bit colour type {colour}"
            channels = 3 if colour == 2 else 4
        elif kind == b"IDAT":
            idat += body
        at += 12 + n
    raw = zlib.decompress(idat)
    stride = width * channels
    out = bytearray(height * stride)
    previous = bytearray(stride)
    for y in range(height):
        f, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1 : (y + 1) * (stride + 1)])
        for x in range(stride):
            a = line[x - channels] if x >= channels else 0
            b = previous[x]
            c = previous[x - channels] if x >= channels else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out[y * stride : (y + 1) * stride] = line
        previous = line
    return width, height, channels, out


def project(cam: dict, p: list[float]):
    pitch, yaw = math.radians(cam["pitch"]), math.radians(cam["yaw"])
    f = (math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), -math.sin(pitch))
    n = math.hypot(f[1], f[0]) or 1.0
    r = (f[1] / n, -f[0] / n, 0.0)
    u = (r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0])
    d = [p[i] - cam["eye"][i] for i in range(3)]
    z = sum(d[i] * f[i] for i in range(3))
    if z <= 1.0:
        return None
    w, h = cam["size"]
    t = math.tan(math.radians(cam["fov"]) / 2)
    x = sum(d[i] * r[i] for i in range(3)) / (z * t * w / h)
    y = sum(d[i] * u[i] for i in range(3)) / (z * t)
    return (x + 1) / 2 * w, (1 - y) / 2 * h, z, h / 2 / t / z  # pixels per unit at that depth


def drawn(img, base, x0, y0, radius):
    """Pixels within `radius` of (x0, y0) where `img` differs from `base`, and the mean change there (`img − base`): a
    player behind a wall shows only faintly over it, so its change, not its colour, tells its side."""
    w, h, ch, a = img
    _, _, _, b = base
    hits, total = 0, [0, 0, 0]
    r = max(2, int(radius))
    for y in range(max(0, int(y0) - r), min(h, int(y0) + r + 1)):
        for x in range(max(0, int(x0) - r), min(w, int(x0) + r + 1)):
            if (x - x0) ** 2 + (y - y0) ** 2 > r * r:
                continue
            k = (y * w + x) * ch
            if abs(a[k] - b[k]) + abs(a[k + 1] - b[k + 1]) + abs(a[k + 2] - b[k + 2]) > 24:
                hits += 1
                for i in range(3):
                    total[i] += a[k + i] - b[k + i]
    return hits, [v / hits for v in total] if hits else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--data", action="append", required=True)
    parser.add_argument("--packs", default=str(Path.home() / ".cache/nades/packs"))
    parser.add_argument("--cases", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    opts = parser.parse_args()
    cases = json.loads(opts.cases.read_text()) if opts.cases else CASES
    out = ROOT / ".runs" / f"replay-{time.strftime('%Y%m%d-%H%M%S')}"
    out.mkdir(parents=True)
    lines = ["# Replay check: " + out.name, "", "| case | players drawn where the facts put them | in their side's colour | dead shown | effects seen (of live) | hidden by walls |", "|---|---:|---:|---:|---:|---:|"]
    failed = 0
    for n, case in enumerate(cases):
        name = f"{n:02d}-{case['demo']}-r{case['round']}-{case['time']}-{case['camera'].replace(':', '-')}"
        images = {}
        for layer in LAYERS:
            png = out / f"{name}-{layer}.png"
            args = [str(opts.binary), "--packs", opts.packs, "--replay", case["demo"], "--round", str(case["round"]), "--time", str(case["time"]), "--replay-camera", case["camera"], "--replay-layers", layer, "--render", str(png)]
            for d in opts.data:
                args += ["--data", d]
            ran = subprocess.run(args, capture_output=True, text=True)
            if ran.returncode != 0:
                print(ran.stderr[-2000:], file=sys.stderr)
                return 2
            images[layer] = read_png(png)
        facts = json.loads((out / f"{name}-all.json").read_text())
        cam = facts["camera"]
        w, h = cam["size"]
        placed = coloured = alive_in = dead_shown = dead_in = 0
        problems = []
        for p in facts["players"]:
            if not p["alive"]:
                at = project(cam, [p["origin"][0], p["origin"][1], p["origin"][2] + 2])
                if at and 0 <= at[0] < w and 0 <= at[1] < h:
                    dead_in += 1
                    hits, _ = drawn(images["players"], images["map"], at[0], at[1], max(4, 18 * at[3]))
                    dead_shown += hits >= 2
                continue
            at = project(cam, [p["origin"][0], p["origin"][1], p["origin"][2] + 36])
            if not at or not (8 <= at[0] < w - 8 and 8 <= at[1] < h - 8):
                continue
            alive_in += 1
            hits, colour = drawn(images["players"], images["map"], at[0], at[1], max(4, 20 * at[3]))
            if hits >= 3:
                placed += 1
                gold = colour[0] > colour[2]
                if gold == (p["side"] == "T"):
                    coloured += 1
                else:
                    problems.append(f"{p['name']} ({p['side']}) changes the picture by {[round(c) for c in colour]}")
            else:
                problems.append(f"{p['name']} not drawn near ({at[0]:.0f}, {at[1]:.0f})")
        seen = live = hidden = 0
        for t in facts["throws"]:
            point = t["in_flight"] if t["in_flight"] and t["seconds"] < 0 else (t["focus"] if t["smoke"] or t["fire"] else None)
            if point is None:
                continue
            at = project(cam, point)
            if not at or not (0 <= at[0] < w and 0 <= at[1] < h):
                continue
            live += 1
            hits, _ = drawn(images["effects"], images["map"], at[0], at[1], max(4, (60 if t["smoke"] else 30 if t["fire"] else 4) * at[3]))
            if hits >= 3:
                seen += 1
            else:
                hidden += 1
        ok = placed == alive_in and coloured == placed
        failed += not ok
        lines.append(f"| {name} | {placed} of {alive_in} | {coloured} of {placed} | {dead_shown} of {dead_in} | {seen} of {live} | {hidden} |")
        lines += [f"|  ↳ {p} | | | | | |" for p in problems]
        print(lines[-1 - len(problems)], *problems, sep="\n  ")
    lines += ["", f"{len(cases) - failed} of {len(cases)} cases with every player in place and in colour."]
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print(f"evidence: {out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
