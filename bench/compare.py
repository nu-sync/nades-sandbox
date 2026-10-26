# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2", "pillow>=11"]
# ///
"""Compare the two M1 builds by the rule in docs/m1-bakeoff.md, and write the evidence.

    uv run bench/compare.py --a target/release/nades-sandbox --b prototypes/m1-bevy/target/release/nades-sandbox-bevy \
        [--runs 7] [--frames 600] [--frame-runs 3] [--skip screenshots,frames,loc,build]

Every run is a fresh process. Each binary gets one throwaway launch first (macOS checks a new executable). Then the
first-frame runs alternate A, B, B, A, ..., wrapped in `/usr/bin/time -l` for peak memory. Results, screenshots and a
report go to .runs/m1-compare-<time>/.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent


def load_average() -> str:
    return subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.strip()


def json_line(stdout: str) -> dict:
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            return json.loads(line)
    raise ValueError(f"no JSON line in output: {stdout[-400:]!r}")


def first_frame(binary: str) -> dict:
    """One launch with --exit-after-first-frame, under /usr/bin/time -l."""
    run = subprocess.run(["/usr/bin/time", "-l", binary, "--exit-after-first-frame"], capture_output=True, text=True, timeout=120)
    if run.returncode != 0:
        raise RuntimeError(f"{binary} exited {run.returncode}: {run.stderr[-600:]}")
    data = json_line(run.stdout)
    marks = dict(data["marks"])
    missing = [m for m in ("frame:presented", "window:visible") if m not in marks]
    if missing:
        raise ValueError(f"{binary}: no {', '.join(missing)} mark")
    rss = re.search(r"(\d+)\s+maximum resident set size", run.stderr)
    # the map is on screen once it is presented and the window is visible, whichever comes last
    on_screen = max(marks["frame:presented"], marks["window:visible"])
    return {
        "total_ms": (data.get("pre_main_ms") or 0.0) + on_screen,
        "presented_ms": (data.get("pre_main_ms") or 0.0) + marks["frame:presented"],
        "visible_ms": (data.get("pre_main_ms") or 0.0) + marks["window:visible"],
        "pre_main_ms": data.get("pre_main_ms"),
        "marks": data["marks"],
        "max_rss_mb": int(rss.group(1)) / 1e6 if rss else None,
        "load": load_average(),
    }


def bench_frames(binary: str, frames: int) -> dict:
    run = subprocess.run([binary, "--bench-frames", str(frames)], capture_output=True, text=True, timeout=600)
    if run.returncode != 0:
        raise RuntimeError(f"{binary} --bench-frames exited {run.returncode}: {run.stderr[-600:]}")
    return json_line(run.stdout) | {"load": load_average()}


def screenshot(binary: str, path: Path) -> None:
    run = subprocess.run([binary, "--screenshot", str(path)], capture_output=True, text=True, timeout=120)
    if run.returncode != 0 or not path.exists():
        raise RuntimeError(f"{binary} --screenshot failed ({run.returncode}): {run.stderr[-600:]}")


def compare_images(a: Path, b: Path, out: Path) -> dict:
    ia = np.asarray(Image.open(a).convert("RGB"), dtype=np.int16)
    ib = np.asarray(Image.open(b).convert("RGB"), dtype=np.int16)
    if ia.shape != ib.shape:
        return {"same_size": False, "a": list(ia.shape), "b": list(ib.shape)}
    diff = np.abs(ia - ib)
    Image.fromarray(np.clip(diff * 8, 0, 255).astype(np.uint8)).save(out / "diff-x8.png")
    side = np.concatenate([ia, ib], axis=1).astype(np.uint8)
    Image.fromarray(side).save(out / "side-by-side.png")
    return {
        "same_size": True,
        "size": [int(ia.shape[1]), int(ia.shape[0])],
        "mean_abs_diff": float(diff.mean()),
        "share_over_16": float((diff.max(axis=2) > 16).mean()),
        "max": int(diff.max()),
    }


def lines_of_code(paths: list[str]) -> int:
    total = 0
    for p in paths:
        for f in (ROOT / p).rglob("*"):
            if f.suffix in (".rs", ".wgsl") and "target" not in f.parts and f.is_file():
                total += sum(1 for line in f.read_text(errors="replace").splitlines() if line.strip())
    return total


def clean_build(command: str, cwd: str) -> float:
    with tempfile.TemporaryDirectory(prefix="m1-build-") as target:
        started = time.monotonic()
        subprocess.run(command, shell=True, cwd=ROOT / cwd, check=True, env={**__import__("os").environ, "CARGO_TARGET_DIR": target}, capture_output=True)
        return time.monotonic() - started


def median(xs: list[float]) -> float:
    return statistics.median(xs)


def decide(r: dict) -> list[str]:
    """The rule in docs/m1-bakeoff.md, step by step, as text."""
    out = []
    gate = r.get("gate", {})
    if gate and not all(gate.values()):
        out.append(f"Gate not passed: {gate}. The rule's first step decides (a failing build loses).")
    fa, fb = r["first_frame"]["A"]["median_ms"], r["first_frame"]["B"]["median_ms"]
    pairs = r["first_frame"]["pairs"]
    a_wins = sum(1 for x, y in pairs if x < y)
    b_wins = len(pairs) - a_wins
    out.append(f"First frame: A median {fa:.1f} ms, B median {fb:.1f} ms; A lower in {a_wins} of {len(pairs)} pairs.")
    lower, other, wins = ("A", "B", a_wins) if fa < fb else ("B", "A", b_wins)
    gain = 1 - min(fa, fb) / max(fa, fb)
    frames = r.get("frames")
    memory = {k: r["first_frame"][k]["max_rss_mb_median"] for k in ("A", "B")}
    worse = []
    if frames:
        if frames[lower]["median_ms"] > 1.25 * frames[other]["median_ms"]:
            worse.append("steady frame time")
    if memory[lower] and memory[other] and memory[lower] > 1.25 * memory[other]:
        worse.append("peak memory")
    # the rule's bar is 6 of 7 pairs: the same share at any run count
    if gain >= 0.10 and wins / len(pairs) >= 6 / 7 and not worse:
        out.append(f"{lower} wins on the first frame: {gain:.0%} lower, lower in {wins} of {len(pairs)} pairs.")
        return out
    out.append(f"No first-frame win ({gain:.0%} apart, {wins} of {len(pairs)} pairs{'; ' + lower + ' worse on ' + ', '.join(worse) if worse else ''}): tie-breaks.")
    if frames:
        sa, sb = frames["A"]["median_ms"], frames["B"]["median_ms"]
        if 1 - min(sa, sb) / max(sa, sb) >= 0.10:
            out.append(f"{'A' if sa < sb else 'B'} wins on steady frame time: {sa:.2f} against {sb:.2f} ms.")
            return out
        out.append(f"Steady frame time within 10%: {sa:.2f} against {sb:.2f} ms.")
    loc = r.get("loc")
    if loc and loc["A"] != loc["B"]:
        out.append(f"{'A' if loc['A'] < loc['B'] else 'B'} wins on design-specific code: {loc['A']} against {loc['B']} lines.")
        return out
    build = r.get("build")
    if build:
        out.append(f"{'A' if build['A'] < build['B'] else 'B'} wins on the clean build: {build['A']:.0f} against {build['B']:.0f} s.")
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True, help="design A's binary")
    ap.add_argument("--b", required=True, help="design B's binary")
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--frames", type=int, default=600)
    ap.add_argument("--frame-runs", type=int, default=3)
    ap.add_argument("--loc-a", default="crates/render,crates/sandbox")
    ap.add_argument("--loc-b", default="prototypes/m1-bevy/src")
    ap.add_argument("--build-a", default="cargo build --release -p nades-sandbox")
    ap.add_argument("--build-b", default="cargo build --release")
    ap.add_argument("--build-b-cwd", default="prototypes/m1-bevy")
    ap.add_argument("--skip", default="", help="comma-separated: screenshots,frames,loc,build")
    args = ap.parse_args()
    skip = set(filter(None, args.skip.split(",")))
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / ".runs" / f"m1-compare-{stamp}"
    out.mkdir(parents=True)
    binaries = {"A": args.a, "B": args.b}
    results: dict = {"binaries": binaries, "load_start": load_average()}

    for name, binary in binaries.items():
        subprocess.run([binary, "--exit-after-first-frame"], capture_output=True, timeout=120)  # the throwaway launch
    runs = {"A": [], "B": []}
    for i in range(args.runs):
        order = ("A", "B") if i % 2 == 0 else ("B", "A")
        for name in order:
            r = first_frame(binaries[name])
            runs[name].append(r)
            print(f"run {i + 1} {name}: {r['total_ms']:.1f} ms, peak {r['max_rss_mb']:.0f} MB, load {r['load']}", flush=True)
    results["first_frame"] = {
        name: {
            "runs": runs[name],
            "median_ms": median([r["total_ms"] for r in runs[name]]),
            "max_rss_mb_median": median([r["max_rss_mb"] for r in runs[name] if r["max_rss_mb"]]),
        }
        for name in ("A", "B")
    }
    results["first_frame"]["pairs"] = [(runs["A"][i]["total_ms"], runs["B"][i]["total_ms"]) for i in range(args.runs)]

    if "frames" not in skip:
        frames = {"A": [], "B": []}
        for i in range(args.frame_runs):
            for name in (("A", "B") if i % 2 == 0 else ("B", "A")):
                frames[name].append(bench_frames(binaries[name], args.frames))
                print(f"frames {i + 1} {name}: {frames[name][-1]}", flush=True)
        results["frames"] = {name: {"runs": frames[name], "median_ms": median([f["median_ms"] for f in frames[name]])} for name in ("A", "B")}

    if "screenshots" not in skip:
        for name, binary in binaries.items():
            screenshot(binary, out / f"{name}.png")
        results["screenshots"] = compare_images(out / "A.png", out / "B.png", out)
        print(f"screenshots: {results['screenshots']}", flush=True)
        s = results["screenshots"]
        results["gate"] = {"screenshots_match": bool(s.get("same_size") and s["mean_abs_diff"] <= 4.0)}

    if "loc" not in skip:
        results["loc"] = {"A": lines_of_code(args.loc_a.split(",")), "B": lines_of_code(args.loc_b.split(","))}

    if "build" not in skip:
        results["build"] = {"A": clean_build(args.build_a, "."), "B": clean_build(args.build_b, args.build_b_cwd)}

    results["load_end"] = load_average()
    results["decision"] = decide(results)
    (out / "results.json").write_text(json.dumps(results, indent=1))
    report = ["# M1 comparison", "", f"Load at start {results['load_start']}, at end {results['load_end']}.", ""]
    report += [f"- {line}" for line in results["decision"]]
    (out / "report.md").write_text("\n".join(report) + "\n")
    print("\n".join(report))
    print(f"\nevidence: {out}")


if __name__ == "__main__":
    main()
