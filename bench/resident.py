"""Every pool map in one session: how long a map switch takes once the maps are resident, and what they hold.

    python3 bench/resident.py [--map de_cache] [--runs 3] [--rounds 2] [--budget-mb N] [--binary target/release/nades-sandbox]

Each run is one `nades-sandbox --bench-maps all:<rounds>`: the window opens on `--map`, the other packed maps are opened in
the background, then every map is picked in turn, `--rounds` times, the first map last in
each round. For each pick the run reports the frames drawn from the pick to the frame that shows the map
(1 when it was resident), that frame's own time on the window's thread with the GPU's share (frames are
finished on the GPU, vsync off), and the process's footprint once it shows. The first round's picks are each map's
first frame in the window; later rounds' are not.

The load average (`sysctl -n vm.loadavg`) is recorded before every run. Evidence goes to .runs/resident-<time>/
(runs.jsonl, results.json, report.md); nothing else is written. Point NADES_SANDBOX_STATE at a scratch file: the run
rewrites it.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load() -> str:
    return subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.strip()


def gpu_ms(pick: dict) -> float:
    return sum(ms for part, ms in pick["parts"] if part == "gpu")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--map", default="de_cache")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--budget-mb", type=int)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    opts = parser.parse_args()
    out = ROOT / ".runs" / f"resident-{time.strftime('%Y%m%d-%H%M%S')}"
    out.mkdir(parents=True)
    log = (out / "runs.jsonl").open("w")
    args = [str(opts.binary), "--map", opts.map, "--bench-maps", f"all:{opts.rounds}"]
    if opts.budget_mb:
        args += ["--budget-mb", str(opts.budget_mb)]

    # a newly linked binary's first launch waits while macOS checks it
    subprocess.run([str(opts.binary), "--map", opts.map, "--exit-after-first-frame"], capture_output=True, timeout=120)
    runs = []
    for i in range(opts.runs):
        before = load()
        done = subprocess.run(args, capture_output=True, text=True, timeout=600)
        if done.returncode != 0:
            raise SystemExit(f"{' '.join(args)} failed ({done.returncode}): {done.stderr[-800:]}")
        result = json.loads(done.stdout.strip().splitlines()[-1])
        line = {"run": i + 1, "load": before, "load_after": load(), **result}
        log.write(json.dumps(line) + "\n")
        log.flush()
        runs.append(line)
    log.close()

    maps = len({p["map"] for p in runs[0]["maps"]})
    first = [p for r in runs for p in r["maps"][:maps]]
    later = [p for r in runs for p in r["maps"][maps:]]
    resident = [p for p in first + later if p["resident"]]
    median = lambda xs: round(statistics.median(xs), 2) if xs else None  # noqa: E731
    p95 = lambda xs: round(sorted(xs)[round((len(xs) - 1) * 0.95)], 2) if xs else None  # noqa: E731
    results = {
        "map": opts.map,
        "binary": str(opts.binary),
        "runs": len(runs),
        "picks": len(first) + len(later),
        "resident_picks": len(resident),
        "frames_to_show": sorted({p["frames"] for p in resident}),
        "first_visit": {"frame_median_ms": median([p["frame_ms"] for p in first]), "frame_max_ms": max(p["frame_ms"] for p in first), "gpu_median_ms": median([gpu_ms(p) for p in first])},
        "later_visits": {"frame_median_ms": median([p["frame_ms"] for p in later]), "frame_max_ms": max((p["frame_ms"] for p in later), default=None), "gpu_median_ms": median([gpu_ms(p) for p in later])},
        "switch_cpu_median_ms": median([p["frame_ms"] - gpu_ms(p) for p in resident]),
        "settled_ms": [round(r["settled_ms"]) for r in runs],
        "while_opening": [r["while_opening"] for r in runs],
        "opened": runs[0]["opened"],
        "budget_mb": runs[0]["budget_mb"],
        "held_mb": [r["held_mb"] for r in runs],
        "footprint_mb": [r["footprint_mb"] for r in runs],
        "peak_footprint_mb": [r["peak_footprint_mb"] for r in runs],
        "footprint_after_each_pick_mb": {"min": min(p["footprint_mb"] for p in first + later), "max": max(p["footprint_mb"] for p in first + later)},
        "loads": [r["load"] for r in runs],
        "per_map": {
            m: {
                "first_visit_frame_ms": [round(p["frame_ms"], 2) for p in first if p["map"] == m],
                "later_visit_frame_ms": [round(p["frame_ms"], 2) for p in later if p["map"] == m],
                "frames": sorted({p["frames"] for p in first + later if p["map"] == m}),
            }
            for m in sorted({p["map"] for p in first})
        },
    }
    (out / "results.json").write_text(json.dumps(results, indent=1) + "\n")
    report = [
        f"# Every map in one session, from {opts.map}: {time.strftime('%Y-%m-%d %H:%M')}",
        "",
        f"- {results['picks']} picks in {len(runs)} runs of {opts.rounds} rounds; {results['resident_picks']} of them were resident, and each of those showed in "
        + ("the next frame" if results["frames_to_show"] == [1] else f"{results['frames_to_show']} frames")
        + ".",
        f"- The switching frame, finished on the GPU (vsync off): a map's first visit median {results['first_visit']['frame_median_ms']} ms (GPU {results['first_visit']['gpu_median_ms']}), "
        f"worst {round(results['first_visit']['frame_max_ms'], 2)}; later visits median {results['later_visits']['frame_median_ms']} ms (GPU {results['later_visits']['gpu_median_ms']}), "
        f"worst {results['later_visits']['frame_max_ms'] and round(results['later_visits']['frame_max_ms'], 2)}; the window's own share (median) {results['switch_cpu_median_ms']} ms.",
        f"- Budget {results['budget_mb']} MB; held on the GPU at the end {results['held_mb']} MB; footprint at the end {results['footprint_mb']} MB, peak {results['peak_footprint_mb']} MB; "
        f"after each pick {results['footprint_after_each_pick_mb']['min']}–{results['footprint_after_each_pick_mb']['max']} MB.",
        f"- Every map open and warm {results['settled_ms']} ms after start; the shown map's frames meanwhile: "
        + "; ".join(f"median {w['median_ms'] and round(w['median_ms'], 1)}, p95 {w['p95_ms'] and round(w['p95_ms'], 1)}, worst {w['max_ms'] and round(w['max_ms'], 1)} ms" for w in results["while_opening"])
        + ".",
        "- Opened in the background: " + ", ".join(f"{o['map']} {round(o['ms'])} ms ({o['mb']} MB)" for o in results["opened"]) + ".",
        f"- Load averages: {'; '.join(results['loads'])}.",
        "",
    ]
    (out / "report.md").write_text("\n".join(report))
    print("\n".join(report))
    print(f"evidence: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
