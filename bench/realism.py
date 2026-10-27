# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2", "pillow>=11"]
# ///
"""How close does the sandbox look to CS2? The same comparison show-me-the-nades' verify-scene-realism skill makes for
its web view, with the sandbox's offscreen renders.

    uv run bench/realism.py --nades <show-me-the-nades checkout> --note "what changed" [--maps cache,mirage]
                            [--binary target/release/nades-sandbox] [--baseline .runs/realism-<earlier>] [--renders DIR]

The checkout holds the skill, its reference set and the web view's history (`--nades`, else SHOW_ME_THE_NADES).

For every map that has a pack (~/.cache/nades/packs/de_<map>.pack) and a reading in the skill's reference set, the
sandbox renders the in-game lineup shot's camera (`nades-sandbox --render`, 1138×640 like the web view's renders), and
the render is measured with the skill's own functions (its scripts/compare.py, loaded where it is) beside the game's
shot and beside the web view's render from the latest run in the skill's history:

- Δ luma, Δ top half and Δ saturation, as the skill reports them (means of the frame, so a too-bright patch and a
  too-dark one can cancel);
- blurred |Δ|: the mean absolute luma difference after both are blurred to 1/16 resolution, the skill's difference map
  as a number. It does not cancel, and small camera offsets do not count. Over the whole frame, and over its top half,
  which leaves out the in-game hands and watermark (the skill reads the top half for that reason).

The verdict per map: ours differs from the game no more than the web view does, by blurred |Δ| (whole frame and top
half) and by |Δ luma|.

Nothing in show-me-the-nades is written: the reference set, the history and the web view's renders are read in place.
The game's shots come from jumpthrow.pro, whose pages and what is read from them stay in show-me-the-nades'
.verify-runs/, so nothing of them is copied here: the sheet holds the sandbox's and the web view's renders, and the
report names each shot's file to open where it is. Evidence goes to .runs/realism-<time>/ (sheet.png, report.md,
results.json, ours/), and every run appends one line to .runs/realism-history.jsonl.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

import web_compare

ROOT = Path(__file__).resolve().parent.parent
# the show-me-the-nades checkout's skill, reference set and history, once main() is told where it is
NADES = SKILL = REFS = WEB_HISTORY = Path()
PACKS = Path.home() / ".cache/nades/packs"
RUNS = ROOT / ".runs"
HISTORY = RUNS / "realism-history.jsonl"


def locate(nades: Path) -> None:
    global NADES, SKILL, REFS, WEB_HISTORY
    NADES = nades
    SKILL = NADES / ".agents/skills/verify-scene-realism"
    REFS = NADES / ".verify-runs/jumpthrow-shots-20261025"
    WEB_HISTORY = NADES / ".verify-runs/game-compare-history.jsonl"


def skill_module():
    """The skill's compare.py, loaded from where it is (its main() only runs as a script)."""
    spec = importlib.util.spec_from_file_location("nades_realism_compare", SKILL / "scripts/compare.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def web_run() -> dict:
    """The web view's latest run in the skill's history that is not discarded."""
    runs = [json.loads(line) for line in WEB_HISTORY.read_text().splitlines() if line.strip()]
    kept = [r for r in runs if not r.get("discarded")]
    return kept[-1]


def blurred_difference(c, game: Image.Image, ours: Image.Image, top: bool = False) -> float:
    """The mean absolute luma difference, display levels, of both images at the skill's size blurred to 1/16: over
    the whole frame, or its top half."""
    weights = np.array([0.2126, 0.7152, 0.0722])
    size = c.SIZE

    def small(image: Image.Image) -> np.ndarray:
        luma = np.asarray(image.convert("RGB").resize(size, Image.Resampling.BOX), dtype=np.float64) @ weights
        return np.asarray(Image.fromarray(luma.astype(np.float32), mode="F").resize((size[0] // 16, size[1] // 16), Image.Resampling.BOX), dtype=np.float64)

    difference = np.abs(small(ours) - small(game))
    return round(float((difference[: difference.shape[0] // 2] if top else difference).mean()), 1)


def render(binary: Path, reading: dict, out: Path) -> tuple[bool, str]:
    name = reading["map"]
    crouched = "ctrl" in (reading.get("keys") or [])
    args = [
        str(binary),
        "--render",
        str(out),
        "--map",
        f"de_{name}",
        "--setpos",
        ",".join(str(v) for v in reading["setpos"]),
        "--setang",
        ",".join(str(v) for v in reading["setang"]),
        "--eye",
        "46" if crouched else "64",
        "--size",
        "1138x640",
    ]
    run = subprocess.run(args, capture_output=True, text=True, timeout=300)
    return run.returncode == 0 and out.exists(), (run.stdout + run.stderr).strip()[-2000:]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--nades", type=Path, help="the show-me-the-nades checkout to compare with (else SHOW_ME_THE_NADES)")
    parser.add_argument("--note", default="")
    parser.add_argument("--maps", default="")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--renders", type=Path, help="compare renders made earlier (<map>.png) instead of rendering")
    opts = parser.parse_args()
    locate(web_compare.checkout(opts.nades))
    if not opts.note:
        print("give --note: the history is only useful if each run says what changed", file=sys.stderr)
        return 2
    c = skill_module()
    readings, problems = c.read_refs(REFS)
    if problems:
        print(f"the reference set {REFS} has problems: {problems}", file=sys.stderr)
        return 2
    wanted = [m for m in opts.maps.split(",") if m]
    readings = [r for r in readings if (not wanted or r["map"] in wanted) and (opts.renders or (PACKS / f"de_{r['map']}.pack").exists())]
    if not readings:
        print("no map has both a reading and a pack", file=sys.stderr)
        return 2
    web = web_run()
    web_dir = NADES / web["run"] / "ours"
    stamp = time.strftime("%Y%m%d-%H%M%S")
    run_dir = RUNS / f"realism-{stamp}"
    ours_dir = run_dir / "ours"
    ours_dir.mkdir(parents=True)
    load = subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.strip()

    results, rows = {}, []
    for r in readings:
        name = r["map"]
        out = ours_dir / f"{name}.png"
        if opts.renders:
            shutil.copyfile(opts.renders / f"{name}.png", out)
            ok, log = True, ""
        else:
            ok, log = render(opts.binary, r, out)
        game = Image.open(c.find_reference(REFS, r))
        ours = Image.open(out) if ok else None
        web_file = web_dir / f"{name}.png"
        web_image = Image.open(web_file) if web_file.exists() else None
        before_file = opts.baseline / "ours" / f"{name}.png" if opts.baseline else None
        before = Image.open(before_file) if before_file and before_file.exists() else None
        entry: dict = {"shot": str(c.find_reference(REFS, r)), "rendered": ok}
        if not ok:
            entry["log"] = log
        g = c.stats(game)
        entry["game"] = g
        if ours:
            o = c.stats(ours)
            entry |= {
                "ours": o,
                "luma_delta": round(o["luma"] - g["luma"], 1),
                "luma_top_delta": round(o["luma_top"] - g["luma_top"], 1),
                "saturation_delta": round(o["saturation"] - g["saturation"], 3),
                "blurred": blurred_difference(c, game, ours),
                "blurred_top": blurred_difference(c, game, ours, top=True),
            }
        web_map = web["maps"].get(name, {})
        entry["web"] = {
            "luma_delta": web_map.get("luma_delta"),
            "luma_top_delta": web_map.get("luma_top_delta"),
            "saturation_delta": web_map.get("saturation_delta"),
            "blurred": blurred_difference(c, game, web_image) if web_image else None,
            "blurred_top": blurred_difference(c, game, web_image, top=True) if web_image else None,
        }
        if before:
            entry["before"] = {"luma_delta": round(c.stats(before)["luma"] - g["luma"], 1), "blurred": blurred_difference(c, game, before)}
        if "blurred" in entry and entry["web"]["blurred"] is not None:
            entry["no_worse_than_web"] = {
                "blurred": entry["blurred"] <= entry["web"]["blurred"],
                "blurred_top": entry["blurred_top"] <= entry["web"]["blurred_top"],
                "luma": abs(entry["luma_delta"]) <= abs(entry["web"]["luma_delta"]),
            }
        results[name] = entry
        lines = lambda d: [f"luma Δ {d['luma_delta']:+.0f}", f"blurred |Δ| {d['blurred']:.0f}"] if d.get("blurred") is not None else []  # noqa: E731
        row = [
            c.cell(ours, f"{name}: sandbox" + ("" if ok else " - not rendered"), lines(entry) + [f"game luma {g['luma']:.0f}"]),
            c.cell(web_image, f"{name}: web view ({web['time']})", lines(entry["web"])),
        ]
        if opts.baseline:
            row.append(c.cell(before, f"{name}: before", lines(entry["before"]) if before else []))
        rows.append(row)

    columns = ["sandbox", "web view"] + (["before"] if opts.baseline else [])
    header = 26
    sheet = Image.new("RGB", (c.CELL[0] * len(columns), header + (c.CELL[1] + c.LABEL) * len(rows)), (16, 16, 16))
    draw = ImageDraw.Draw(sheet)
    for i, title in enumerate(columns):
        draw.text((i * c.CELL[0] + 8, 7), title, fill=(255, 255, 255))
    for y, row in enumerate(rows):
        for x, image in enumerate(row):
            sheet.paste(image, (x * c.CELL[0], header + y * (c.CELL[1] + c.LABEL)))
    sheet.save(run_dir / "sheet.png")

    compared = [e for e in results.values() if "blurred" in e]
    mean = lambda xs: round(float(np.mean(xs)), 1) if xs else None  # noqa: E731
    headline = {
        "maps": len(compared),
        "mean_abs_luma_delta": mean([abs(e["luma_delta"]) for e in compared]),
        "mean_blurred": mean([e["blurred"] for e in compared]),
        "web_mean_abs_luma_delta": mean([abs(e["web"]["luma_delta"]) for e in compared if e["web"]["luma_delta"] is not None]),
        "web_mean_blurred": mean([e["web"]["blurred"] for e in compared if e["web"]["blurred"] is not None]),
        "mean_blurred_top": mean([e["blurred_top"] for e in compared]),
        "web_mean_blurred_top": mean([e["web"]["blurred_top"] for e in compared if e["web"]["blurred_top"] is not None]),
        "no_worse_blurred": sum(1 for e in compared if e.get("no_worse_than_web", {}).get("blurred")),
        "no_worse_blurred_top": sum(1 for e in compared if e.get("no_worse_than_web", {}).get("blurred_top")),
        "no_worse_luma": sum(1 for e in compared if e.get("no_worse_than_web", {}).get("luma")),
    }
    git = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    record = {
        "time": time.strftime("%Y-%m-%d %H:%M"),
        "run": str(run_dir.relative_to(ROOT)),
        "note": opts.note,
        "load": load,
        "web_run": web["run"],
        "headline": headline,
        "maps": {n: {k: e.get(k) for k in ("luma_delta", "luma_top_delta", "saturation_delta", "blurred", "blurred_top")} | {"web": e["web"]} for n, e in results.items()},
        "dirty": bool(git),
    }
    (run_dir / "results.json").write_text(json.dumps({**record, "details": results}, indent=1) + "\n")
    with HISTORY.open("a") as f:
        f.write(json.dumps(record) + "\n")

    table = [
        f"# The sandbox beside the game, {record['time']}",
        "",
        f"- Note: {opts.note}",
        f"- References: `{REFS}`; the web view's run: `{web['run']}` ({web['time']}: {web.get('note', '')[:120]}).",
        f"- Load average: {load}.",
        f"- Headline: mean |Δ luma| {headline['mean_abs_luma_delta']} (web {headline['web_mean_abs_luma_delta']}), "
        f"mean blurred |Δ| {headline['mean_blurred']} (web {headline['web_mean_blurred']}), top half {headline['mean_blurred_top']} "
        f"(web {headline['web_mean_blurred_top']}) over {headline['maps']} maps; no worse than the web view on "
        f"{headline['no_worse_blurred']} by blurred |Δ|, {headline['no_worse_blurred_top']} by its top half, {headline['no_worse_luma']} by |Δ luma|.",
        "",
        "Δ is ours minus the game's, in display levels (0–255); saturation as (max − min) / max. Blurred |Δ| is the mean "
        "absolute luma difference at 1/16 resolution. The top half leaves out the in-game hands and watermark.",
        "",
        "| Map | Δ luma | Δ top | Δ sat | blurred \\|Δ\\| (top) | web Δ luma | web Δ top | web Δ sat | web blurred \\|Δ\\| (top) | no worse: blurred / top / luma |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for name, e in results.items():
        w = e["web"]
        if "blurred" not in e:
            table.append(f"| {name} | not rendered | | | | {w['luma_delta']} | {w['luma_top_delta']} | {w['saturation_delta']} | {w['blurred']} ({w['blurred_top']}) | |")
            continue
        verdict = e.get("no_worse_than_web", {})
        table.append(
            f"| {name} | {e['luma_delta']:+} | {e['luma_top_delta']:+} | {e['saturation_delta']:+} | {e['blurred']} ({e['blurred_top']}) "
            f"| {w['luma_delta']:+} | {w['luma_top_delta']:+} | {w['saturation_delta']:+} | {w['blurred']} ({w['blurred_top']}) "
            f"| {' / '.join('yes' if verdict.get(k) else 'NO' for k in ('blurred', 'blurred_top', 'luma'))} |"
        )
    table += ["", "The game's shots, to open where they are:", ""]
    table += [f"- {name}: `{e['shot']}`" for name, e in results.items()]
    table += ["", "![contact sheet](sheet.png)", ""]
    (run_dir / "report.md").write_text("\n".join(table))
    print("\n".join(table[:-2]))
    for name, e in results.items():
        if not e["rendered"]:
            print(f"FAIL  {name}: {e.get('log', '')[-400:]}")
    print(f"evidence: {run_dir}")
    return 0 if all(e["rendered"] for e in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
