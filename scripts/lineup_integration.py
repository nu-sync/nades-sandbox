#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow==12.1.1"]
# ///
"""Render four fixed lineup cases through the real MCP server for visual review.

    uv run scripts/lineup_integration.py --open

Requires a release build, local map/effects packs, a GPU, FFmpeg/libx264 and ffprobe.
The numerical bounds detect large regressions; they do not prove in-game accuracy.
"""

import argparse
import json
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / ".agents/skills/nades-lineup-camera/scripts/lineup.py"
CASES = [
    ("dust2-mid-doors", "dust2", "smoke", "setpos -1141 -808 180.154739; setang -21.317932 73.115051", "jumpthrow", (-412, 1625, -125)),
    ("inferno-banana-car", "inferno", "molotov", "setpos 126.85 940.57 82.03; setang -14.82 85.84 0", "run", (469, 1990, 141)),
    ("cache-z-connector", "cache", "smoke", "setpos 1793.97 833.92 1612.03; setang -8.99 -159.73 0", "jumpthrow", (-487, -9, 1666)),
    ("ancient-elbow", "ancient", "smoke", "setpos -1171.938843 1159.968750 129.118561; setang -26.455465 -83.088531 0", "jumpthrow", (-881, -755, 37)),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--open", action="store_true", help="open the output directory in Finder (macOS)")
    parser.add_argument("--draft", action="store_true", help="render at 640x360 and 20 fps")
    args = parser.parse_args()
    output = Path(tempfile.mkdtemp(prefix="nades-integration-"))
    print(f"Review directory: {output}", flush=True)
    failures = []
    for name, map_name, grenade, lineup, technique, expected in CASES:
        case_dir = output / "evidence" / name
        command = [sys.executable, str(HELPER), "--map", map_name, "--grenade", grenade,
                   "--lineup", lineup, "--technique", technique, "--out", str(case_dir)]
        if args.draft:
            command.append("--draft")
        print(f"\n{name}", flush=True)
        try:
            subprocess.run(command, cwd=ROOT, check=True)
            calls = [json.loads(line) for line in (case_dir / "calls.jsonl").read_text().splitlines()]
            result = next(call["result"] for call in calls if call["tool"] == "lineup_render" and not call["is_error"])
            video = result["video"]
            report = video["reports"][0]
            actual = report["detonation"]["at"]
            if math.dist(actual, expected) > 250:
                raise ValueError(f"detonation {actual} is over 250 units from baseline {expected}")
            if report["grenade"] != grenade or video["state"] != "completed":
                raise ValueError("incorrect grenade or incomplete video")
            probe = json.loads((case_dir / "ffprobe.json").read_text())
            stream = probe["streams"][0]
            dimensions = (640, 360) if args.draft else (1920, 1080)
            if (stream["codec_name"], stream["width"], stream["height"]) != ("h264", *dimensions) or int(stream["nb_read_frames"]) < 1:
                raise ValueError(f"unexpected video stream: {stream}")
            for path in [case_dir / "frames.png", case_dir / "lineup.json", *(case_dir / f"lineup-{still}.png" for still in ("aim", "aim_point", "flight", "bloom", "result"))]:
                if not path.is_file() or path.stat().st_size == 0:
                    raise ValueError(f"missing evidence: {path}")
            shutil.copyfile(case_dir / "lineup.mp4", output / f"{name}.mp4")
            print(f"PASS {name}: detonation {actual}", flush=True)
        except (subprocess.CalledProcessError, OSError, ValueError, KeyError, IndexError, StopIteration) as error:
            failures.append(f"{name}: {error}")
            print(f"FAIL {failures[-1]}", file=sys.stderr, flush=True)
    (output / "results.json").write_text(json.dumps({"failures": failures, "cases": [case[0] for case in CASES], "draft": args.draft}, indent=2) + "\n")
    if args.open:
        if sys.platform == "darwin":
            subprocess.run(["open", str(output)], check=True)
        else:
            print("Finder is only available on macOS", file=sys.stderr)
    print(f"\n{'FAILED' if failures else 'PASS'}: {output}", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
