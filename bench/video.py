"""How long a lineup video takes, and where its time goes: `lineup_render` through nades-mcp, as an agent calls it, with
the timings each job reports (S38).

    python3 bench/video.py [--runs 5] [--baseline DIR] [--candidate DIR] [--size 1920x1080] [--fps 60]
                           [--map de_mirage --grenade smoke --technique jumpthrow --lineup "setpos …; setang …"]

- DIR holds a build's `nades-mcp` and `nades-sandbox` (default: target/release). With `--baseline`, the two builds'
  runs alternate, so the machine's load falls on both alike.
- A throwaway launch of each binary first: a newly linked binary's first launch waits while macOS checks it.
- Each run is a fresh MCP session rendering the lineup once; the load average (`sysctl -n vm.loadavg`) is recorded
  before it. The MP4 and stills are deleted after each run; the JSON beside them is kept.
- Reported per build, as medians over the runs: the job's wall clock (startup, frames, encoder's tail, total), each
  frame stage's total, the renderer's and the encoder's CPU seconds, the cores each kept busy over the frames, and
  the renderer's peak footprint.

Evidence goes to .runs/video-<time>/ (runs.jsonl, results.json, report.md); nothing else is written.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# the MCP specification's example: a jump-throw smoke on Mirage
LINEUP = "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0"
STAGES = ["camera", "record", "wait", "convert", "stills", "write"]


def load() -> str:
    return subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.strip()


class Session:
    """One nades-mcp over stdio, JSON-RPC a line at a time."""

    def __init__(self, mcp: Path):
        self.proc = subprocess.Popen([str(mcp)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.id = 0
        self.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bench-video", "version": "0"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message: dict) -> None:
        assert self.proc.stdin
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def call(self, method: str, params: dict) -> dict:
        self.id += 1
        self.send({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})
        assert self.proc.stdout
        while True:
            reply = json.loads(self.proc.stdout.readline())
            if reply.get("id") == self.id:
                return reply

    def tool(self, name: str, arguments: dict) -> dict:
        result = self.call("tools/call", {"name": name, "arguments": arguments})["result"]
        if result.get("isError"):
            raise RuntimeError(json.dumps(result.get("structuredContent", result))[:2000])
        return result["structuredContent"]

    def close(self) -> None:
        assert self.proc.stdin
        self.proc.stdin.close()
        self.proc.wait(timeout=30)


def render(build: Path, args: argparse.Namespace, out: Path, tag: str) -> dict:
    width, height = (int(v) for v in args.size.split("x"))
    session = Session(build / "nades-mcp")
    before = load()
    started = time.monotonic()
    try:
        result = session.tool("lineup_render", {"map": args.map, "grenade": args.grenade, "lineup": args.lineup,
                                                "technique": args.technique, "width": width, "height": height,
                                                "fps": args.fps, "output": str(out / f"{tag}.mp4")})
    finally:
        session.close()
    wall = time.monotonic() - started
    video = result["video"]
    for still in result["stills"]:
        Path(still[1]).unlink(missing_ok=True)
    Path(video["path"]).unlink(missing_ok=True)
    return {"tag": tag, "build": str(build), "loadavg": before, "call_seconds": wall, "frames": video["frames_total"],
            "megabytes": video["megabytes"], "timings": video["timings"]}


def summarize(runs: list[dict]) -> dict:
    med = lambda values: statistics.median(values) if values else None  # noqa: E731
    t = [r["timings"] for r in runs]
    # builds before S38's process use report none
    use = lambda x, who, key: x.get(who, {}).get(key)  # noqa: E731
    cores = lambda who: med([use(x, who, "cpu_seconds") / x["frames_seconds"] for x in t if use(x, who, "cpu_seconds") is not None and x["frames_seconds"] > 0])  # noqa: E731
    return {
        "runs": len(runs),
        "frames": runs[0]["frames"],
        "total_s": med([x["total_seconds"] for x in t]),
        "startup_s": med([x["startup_seconds"] for x in t]),
        "frames_s": med([x["frames_seconds"] for x in t]),
        "encode_tail_s": med([x["encode_tail_seconds"] for x in t]),
        "stage_total_s": {s: med([x["frame"][s]["total_ms"] / 1e3 for x in t]) for s in STAGES},
        "stage_median_ms": {s: med([x["frame"][s]["median_ms"] for x in t]) for s in STAGES},
        "renderer_cpu_s": med([use(x, "renderer", "cpu_seconds") for x in t if use(x, "renderer", "cpu_seconds") is not None]),
        "encoder_cpu_s": med([use(x, "encoder", "cpu_seconds") for x in t if use(x, "encoder", "cpu_seconds") is not None]),
        "renderer_cores": cores("renderer"),
        "encoder_cores": cores("encoder"),
        "renderer_peak_mb": med([use(x, "renderer", "peak_footprint_bytes") / 1e6 for x in t if use(x, "renderer", "peak_footprint_bytes")]),
        "megabytes": med([r["megabytes"] for r in runs]),
        "loadavg": [r["loadavg"] for r in runs],
    }


def report(results: dict, args: argparse.Namespace) -> str:
    names = list(results)
    fmt = lambda v, d=2: "—" if v is None else f"{v:.{d}f}"  # noqa: E731
    rows = [("total, s", "total_s"), ("startup, s", "startup_s"), ("frames, s", "frames_s"), ("encoder's tail, s", "encode_tail_s")]
    lines = [f"# Lineup video: {args.map} {args.grenade} ({args.technique}), {args.size} at {args.fps} fps",
             "", f"`{args.lineup}`, {results[names[0]]['frames']} frames, medians of {args.runs} runs each.", "",
             "| | " + " | ".join(names) + " |", "|---|" + "---|" * len(names)]
    lines += [f"| {label} | " + " | ".join(fmt(results[n][key]) for n in names) + " |" for label, key in rows]
    lines += [f"| {s}, s (median ms/frame) | " + " | ".join(f"{fmt(results[n]['stage_total_s'][s])} ({fmt(results[n]['stage_median_ms'][s])})" for n in names) + " |" for s in STAGES]
    lines += [f"| renderer CPU, s (cores) | " + " | ".join(f"{fmt(results[n]['renderer_cpu_s'], 1)} ({fmt(results[n]['renderer_cores'], 1)})" for n in names) + " |",
              f"| encoder CPU, s (cores) | " + " | ".join(f"{fmt(results[n]['encoder_cpu_s'], 1)} ({fmt(results[n]['encoder_cores'], 1)})" for n in names) + " |",
              f"| renderer peak footprint, MB | " + " | ".join(fmt(results[n]['renderer_peak_mb'], 0) for n in names) + " |",
              f"| MP4, MB | " + " | ".join(fmt(results[n]['megabytes'], 1) for n in names) + " |", ""]
    lines += [f"Load averages ({n}): " + "; ".join(results[n]["loadavg"]) for n in names]
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--candidate", type=Path, default=ROOT / "target/release")
    parser.add_argument("--map", default="de_mirage")
    parser.add_argument("--grenade", default="smoke")
    parser.add_argument("--technique", default="jumpthrow")
    parser.add_argument("--lineup", default=LINEUP)
    parser.add_argument("--size", default="1920x1080")
    parser.add_argument("--fps", type=int, default=60)
    args = parser.parse_args()
    builds = {"candidate": args.candidate.resolve()}
    if args.baseline:
        builds = {"baseline": args.baseline.resolve(), **builds}
    out = ROOT / ".runs" / time.strftime("video-%Y%m%d-%H%M%S")
    out.mkdir(parents=True)
    for build in builds.values():
        subprocess.run([str(build / "nades-sandbox"), "--version"], capture_output=True)
        Session(build / "nades-mcp").close()
    runs: dict[str, list[dict]] = {name: [] for name in builds}
    with open(out / "runs.jsonl", "w") as log:
        for k in range(args.runs):
            for name, build in builds.items():
                run = render(build, args, out, f"{name}-{k}")
                runs[name].append(run)
                log.write(json.dumps(run) + "\n")
                log.flush()
                t = run["timings"]
                print(f"{name} {k + 1}/{args.runs}: {t['total_seconds']:.2f} s (load {run['loadavg']})", flush=True)
    results = {name: summarize(r) for name, r in runs.items()}
    (out / "results.json").write_text(json.dumps(results, indent=1))
    text = report(results, args)
    (out / "report.md").write_text(text)
    print(text)
    print(f"Evidence: {out}")


if __name__ == "__main__":
    main()
