"""How fast a map gets on screen, and how long a frame of it takes: the start-up and steady-frame evidence for one
binary and one map, as SPEC.md's targets ask for it.

    python3 bench/startup.py [--map de_cache] [--runs 9] [--frames 600] [--binary target/release/nades-sandbox]

- A throwaway launch first: a newly linked binary's first launch waits while macOS checks it.
- First frame: `--runs` launches with `--exit-after-first-frame`; each run's time on screen is the process's time
  before `main` plus the later of `frame:presented` and `window:visible`. The median is the number SPEC.md reports.
- Steady frame, windowed: three `--bench-frames` runs (vsync off, the window's own size).
- Steady frame, offscreen: three `--bench-offscreen` runs at the window's physical size, each frame finished on the
  GPU before the next.
- Peak memory: three runs under `/usr/bin/time -l`.

The load average (`sysctl -n vm.loadavg`) is recorded before every run. Evidence goes to .runs/startup-<time>/
(runs.jsonl, results.json, report.md); nothing else is written.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load() -> str:
    return subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.strip()


# what the timed benches say when macOS keeps the window hidden or covered (main.rs, BENCH_PATIENCE)
HIDDEN = "needs the window on screen"


def run(args: list[str], timeout: float = 120) -> str | None:
    """The last line the run printed; None when the window was never on screen: the run timed out (a window macOS never
    shows draws no frames), or a timed bench stopped because macOS kept it hidden or covered."""
    try:
        done = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None
    if done.returncode != 0 and HIDDEN in done.stderr:
        return None
    if done.returncode != 0:
        raise SystemExit(f"{' '.join(args)} failed ({done.returncode}): {done.stderr[-800:]}")
    return done.stdout.strip().splitlines()[-1]


def on_screen(result: dict) -> dict:
    marks = dict(result["marks"])
    before = result.get("pre_main_ms") or 0.0
    memory = {stage: (footprint, graphics) for stage, footprint, graphics in result.get("memory", [])}
    return {
        "total_ms": round(before + max(marks["frame:presented"], marks["window:visible"]), 2),
        "presented_ms": round(before + marks["frame:presented"], 2),
        "visible_ms": round(before + marks["window:visible"], 2),
        "window_ms": round(before + marks["window"], 2),
        "loaded_ms": round(before + marks["loaded"], 2),
        # the first frame's wait for the GPU, after the loader's warm-up frames
        "first_gpu_ms": round(marks["frame:gpu"] - marks["frame:submitted"], 2),
        "pre_main_ms": before,
        # the process's own readings (macOS): footprint and its graphics share when the window shows, and the peak
        "visible_footprint_mb": memory.get("window:visible", (None, None))[0],
        "visible_graphics_mb": memory.get("window:visible", (None, None))[1],
        "peak_footprint_mb": result.get("peak_footprint_mb"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--map", default="de_cache")
    parser.add_argument("--runs", type=int, default=9)
    parser.add_argument("--frames", type=int, default=600)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    opts = parser.parse_args()
    binary = str(opts.binary)
    out = ROOT / ".runs" / f"startup-{time.strftime('%Y%m%d-%H%M%S')}"
    out.mkdir(parents=True)
    log = (out / "runs.jsonl").open("w")

    def record(kind: str, **fields) -> dict:
        line = {"kind": kind, **fields}
        log.write(json.dumps(line) + "\n")
        log.flush()
        return line

    run([binary, "--map", opts.map, "--exit-after-first-frame"])
    first = []
    for i in range(opts.runs):
        before = load()
        line = run([binary, "--map", opts.map, "--exit-after-first-frame"], 30)
        if line is None:
            record("first-frame", run=i + 1, load=before, timed_out=True)
            continue
        result = json.loads(line)
        first.append(record("first-frame", run=i + 1, load=before, **on_screen(result), result=result))
    if not first:
        raise SystemExit("no first frame: macOS never showed the window (another app full screen on its display?)")
    windowed, offscreen, memory = [], [], []
    for i in range(3):
        before = load()
        line = run([binary, "--map", opts.map, "--bench-frames", str(opts.frames)], 120)
        if line is None:
            record("bench-frames", run=i + 1, load=before, timed_out=True)
            continue
        windowed.append(record("bench-frames", run=i + 1, load=before, **json.loads(line)))
    for i in range(3):
        before = load()
        offscreen.append(record("bench-offscreen", run=i + 1, load=before, **json.loads(run([binary, "--map", opts.map, "--bench-offscreen", str(opts.frames)], 300))))
    for i in range(3):
        before = load()
        done = subprocess.run(["/usr/bin/time", "-l", binary, "--map", opts.map, "--exit-after-first-frame"], capture_output=True, text=True, timeout=120)
        peak = re.search(r"(\d+)\s+peak memory footprint", done.stderr)
        rss = re.search(r"(\d+)\s+maximum resident set size", done.stderr)
        memory.append(record("memory", run=i + 1, load=before, peak_footprint_mb=round(int(peak.group(1)) / 1e6, 1) if peak else None, max_rss_mb=round(int(rss.group(1)) / 1e6, 1) if rss else None))
    log.close()

    median = lambda xs: round(statistics.median(xs), 2)  # noqa: E731
    results = {
        "map": opts.map,
        "binary": binary,
        "first_frame": {
            "runs": len(first),
            "median_ms": median([r["total_ms"] for r in first]),
            "best_ms": min(r["total_ms"] for r in first),
            "worst_ms": max(r["total_ms"] for r in first),
            "presented_median_ms": median([r["presented_ms"] for r in first]),
            "visible_median_ms": median([r["visible_ms"] for r in first]),
            "window_median_ms": median([r["window_ms"] for r in first]),
            "loaded_median_ms": median([r["loaded_ms"] for r in first]),
            "first_gpu_median_ms": median([r["first_gpu_ms"] for r in first]),
            "loads": [r["load"] for r in first],
        },
        "steady_windowed": {"runs": [{k: r.get(k) for k in ("median_ms", "p95_ms", "display", "refresh_hz", "load")} for r in windowed]},
        "steady_offscreen": {
            "size": f"{offscreen[0]['width']}x{offscreen[0]['height']}",
            "median_ms": median([r["median_ms"] for r in offscreen]),
            "p95_ms": max(r["p95_ms"] for r in offscreen),
            "loads": [r["load"] for r in offscreen],
        },
        "memory": {
            "peak_footprint_mb": median([m["peak_footprint_mb"] for m in memory if m["peak_footprint_mb"]]),
            "max_rss_mb": median([m["max_rss_mb"] for m in memory if m["max_rss_mb"]]),
            "visible_footprint_mb": median([r["visible_footprint_mb"] for r in first if r["visible_footprint_mb"]] or [0]),
            "visible_graphics_mb": median([r["visible_graphics_mb"] for r in first if r["visible_graphics_mb"]] or [0]),
        },
    }
    (out / "results.json").write_text(json.dumps(results, indent=1) + "\n")
    f, w, o = results["first_frame"], results["steady_windowed"], results["steady_offscreen"]
    report = [
        f"# {opts.map}: start-up and steady frame, {time.strftime('%Y-%m-%d %H:%M')}",
        "",
        f"- On screen from process start: median {f['median_ms']} ms of {f['runs']} runs ({f['best_ms']}–{f['worst_ms']}); "
        f"presented {f['presented_median_ms']}, window visible {f['visible_median_ms']}, window made {f['window_median_ms']}, map loaded {f['loaded_median_ms']}, "
        f"the first frame's GPU wait {f['first_gpu_median_ms']} (medians).",
        f"- Load averages: {'; '.join(f['loads'])}.",
        "- Steady frame, windowed (vsync off), per run: "
        + ("; ".join(f"median {r['median_ms']} ms, 95th percentile {r['p95_ms']} ms on {r['display']} at {r['refresh_hz']} Hz" for r in w["runs"]) or "no run drew (the window was hidden)")
        + ".",
        f"- Steady frame, offscreen at {o['size']} (each frame finished on the GPU): median {o['median_ms']} ms, worst 95th percentile {o['p95_ms']} ms.",
        f"- Peak memory footprint {results['memory']['peak_footprint_mb']} MB, maximum resident {results['memory']['max_rss_mb']} MB (medians of 3); "
        f"when the window shows, footprint {results['memory']['visible_footprint_mb']} MB of which {results['memory']['visible_graphics_mb']} MB the GPU's (median).",
        "",
    ]
    (out / "report.md").write_text("\n".join(report))
    print("\n".join(report))
    print(f"evidence: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
