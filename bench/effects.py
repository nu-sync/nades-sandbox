# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2", "pillow>=11"]
# ///
"""Do the sandbox's effects look like the web view's? Chosen throws' result shots from both, at the moment each view
opens on (a smoke settled, a fire at its fullest, a flash or HE as it goes off) and a second before the grenade went
off, side by side.

    uv run bench/effects.py [--maps de_cache,de_mirage,de_inferno] [--db PATH] [--size 1280x720]

- The web view is shot by show-me-the-nades' own tool (its verify-nades-scene skill's effect_shots.ts, read in place),
  through its development server and API on a clone of the database; the sandbox renders the same throws, views and
  moments offscreen (`nades-sandbox --render --throw`). Both open from the same camera (bench/cameras.py).
- The maps themselves differ (the sandbox draws CS2's look, S12), so the effect is compared as what it changes: each
  renderer's shot with the effect less its shot before it. Per throw: the share of the frame the effect changes, the
  overlap of the two changed regions (intersection over union), and the effect's mean colour in each.

Evidence goes to .runs/effects-<time>/ (web/, ours/, sheet.png, results.json, report.md); the clone is deleted.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
NADES = ROOT.parent / "show-me-the-nades"
SHOTS = NADES / ".agents/skills/verify-nades-scene/scripts/effect_shots.ts"
BEFORE = -1.0
# a pixel counts as changed by the effect once its blurred luma moves this much (of 255)
CHANGED = 6.0


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def get(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as response:
        return json.loads(response.read())


def wait_for(url: str, seconds: float = 90) -> None:
    started = time.time()
    while time.time() - started < seconds:
        try:
            urllib.request.urlopen(url, timeout=5).read()
            return
        except Exception:
            time.sleep(0.5)
    raise SystemExit(f"never answered: {url}")


def luma(path: Path) -> np.ndarray:
    image = Image.open(path).convert("RGB").filter(ImageFilter.GaussianBlur(2))
    a = np.asarray(image, dtype=np.float32)
    return a @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)


def effect(after: Path, before: Path) -> tuple[np.ndarray, np.ndarray]:
    """The pixels the effect changes, and the after-shot's colours."""
    change = np.abs(luma(after) - luma(before)) > CHANGED
    return change, np.asarray(Image.open(after).convert("RGB"), dtype=np.float32)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--maps", default="de_cache,de_mirage,de_inferno")
    parser.add_argument("--db", type=Path, default=NADES / "nades.duckdb")
    parser.add_argument("--size", default="1280x720")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    opts = parser.parse_args()
    width, height = map(int, opts.size.split("x"))
    out = ROOT / ".runs" / f"effects-{time.strftime('%Y%m%d-%H%M%S')}"
    (out / "web").mkdir(parents=True)
    (out / "ours").mkdir()
    clone = out / "nades.duckdb"
    subprocess.run(["cp", "-c", str(opts.db), str(clone)], check=True)
    api_port, app_port = free_port(), free_port()
    api, app = f"http://127.0.0.1:{api_port}", f"http://localhost:{app_port}"
    env = {**os.environ, "NADES_DB": str(clone), "NADES_WEB_PORT": str(api_port), "NADES_MAPS_DIR": str(Path.home() / ".cache/nades/maps"), "NADES_API_URL": api}
    procs = [
        subprocess.Popen(["bun", "src/main.ts"], cwd=NADES / "web/server", env=env, stdout=subprocess.DEVNULL, stderr=open(out / "api.log", "w")),
        subprocess.Popen(["bun", "--bun", "vite", "dev", "--port", str(app_port), "--strictPort"], cwd=NADES / "web/app", env=env, stdout=subprocess.DEVNULL, stderr=open(out / "app.log", "w")),
    ]
    try:
        wait_for(f"{api}/api/maps")
        wait_for(app)
        cases = []
        for m in [x for x in opts.maps.split(",") if x]:
            for nade in ("smoke", "molotov", "flash", "he"):
                rows = get(f"{api}/api/lineups?map={m}&nade={nade}&rank=frequent&limit=1")["rows"]
                if rows and rows[0].get("example_throw_id"):
                    cases.append({"id": f"{m.removeprefix('de_')}-{nade}", "map": m, "throw": rows[0]["example_throw_id"], "nade": nade})
        # the moment each result view opens on, as both compute it
        (out / "throws.json").write_text(json.dumps([{"throw": c["throw"]} for c in cases]))
        cameras = subprocess.run([str(opts.binary), "--cameras", str(out / "throws.json")], env=env, capture_output=True, text=True, check=True).stdout
        opens = {r["throw"]: r["result_time"] for r in map(json.loads, filter(None, cameras.splitlines()))}
        for c in cases:
            c["times"] = [BEFORE, round(opens[c["throw"]], 2)]
            c["shots"] = [{"name": "result", "kind": "result"}]
        (out / "cases.json").write_text(json.dumps(cases, indent=1))
        shot = subprocess.run(["bun", str(SHOTS), "--app", app, "--cases", str(out / "cases.json"), "--out", str(out / "web"), "--width", str(width), "--height", str(height)], cwd=NADES / "web/app", capture_output=True, text=True, timeout=3600)
        (out / "effect_shots.log").write_text(shot.stdout + shot.stderr)
        if shot.returncode == 2:
            raise SystemExit(f"effect_shots.ts could not run: {shot.stderr[-1500:]}")
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            p.wait(timeout=30)
    for c in cases:
        for t in c["times"]:
            subprocess.run([str(opts.binary), "--render", str(out / "ours" / f"{c['id']}-result-{t:.2f}.png"), "--throw", c["throw"], "--view", "result", "--time", str(t), "--size", f"{width}x{height}"], env=env, capture_output=True, text=True, check=True)
    for f in out.glob("nades.duckdb*"):
        f.unlink()

    results, rows = [], []
    for c in cases:
        t = c["times"][1]
        name = lambda t: f"{c['id']}-result-{t:.2f}.png"  # noqa: E731
        web_after, web_before = out / "web" / name(t), out / "web" / name(BEFORE)
        our_after, our_before = out / "ours" / name(t), out / "ours" / name(BEFORE)
        if not all(p.exists() for p in (web_after, web_before, our_after, our_before)):
            results.append({**c, "missing": True})
            continue
        (wm, wc), (om, oc) = effect(web_after, web_before), effect(our_after, our_before)
        union = np.logical_or(wm, om).sum()
        colour = lambda m, c: [round(float(v), 1) for v in c[m].mean(axis=0)] if m.any() else None  # noqa: E731
        results.append({
            **c,
            "web_share": round(float(wm.mean()), 4),
            "our_share": round(float(om.mean()), 4),
            "iou": round(float(np.logical_and(wm, om).sum() / union), 3) if union else None,
            "web_colour": colour(wm, wc),
            "our_colour": colour(om, oc),
        })
        rows.append((c, web_after, our_after, wm, om))

    # the sheet: per throw, the web view's shot, the sandbox's, and the two effects' regions over each other
    thumb = (480, int(480 * height / width))
    sheet = Image.new("RGB", (thumb[0] * 3 + 40, (thumb[1] + 26) * len(rows) + 10), (24, 24, 28))
    draw = ImageDraw.Draw(sheet)
    for k, (c, wa, oa, wm, om) in enumerate(rows):
        y = 10 + k * (thumb[1] + 26)
        draw.text((10, y), f"{c['map']} {c['nade']} {c['throw']} at {c['times'][1]:.2f} s: web | sandbox | effect (web red, sandbox green)", fill=(220, 220, 220))
        sheet.paste(Image.open(wa).convert("RGB").resize(thumb), (10, y + 14))
        sheet.paste(Image.open(oa).convert("RGB").resize(thumb), (20 + thumb[0], y + 14))
        overlap = np.zeros((*wm.shape, 3), dtype=np.uint8)
        overlap[..., 0] = wm * 255
        overlap[..., 1] = om * 255
        sheet.paste(Image.fromarray(overlap).resize(thumb), (30 + 2 * thumb[0], y + 14))
    sheet.save(out / "sheet.png")
    (out / "results.json").write_text(json.dumps(results, indent=1))
    lines = [
        f"# Effects beside the web view: {out.name}",
        "",
        "Each throw's result shot at the moment the view opens on, from the same camera in both. The effect is what it",
        f"changes: the shot less the same shot {-BEFORE:.0f} s before the grenade went off (blurred luma moved by more than {CHANGED:.0f}/255).",
        "",
        "| Map | Grenade | Throw | At (s) | Web: share changed | Sandbox | Overlap (IoU) | Web colour | Sandbox colour |",
        "|---|---|---|---:|---:|---:|---:|---|---|",
    ]
    for r in results:
        if r.get("missing"):
            lines.append(f"| {r['map']} | {r['nade']} | {r['throw']} | | missing shots | | | | |")
            continue
        lines.append(f"| {r['map']} | {r['nade']} | {r['throw']} | {r['times'][1]:.2f} | {r['web_share']:.3f} | {r['our_share']:.3f} | {r['iou']} | {r['web_colour']} | {r['our_colour']} |")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nevidence: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
