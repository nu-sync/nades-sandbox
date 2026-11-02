#!/usr/bin/env python3
"""The jumpthrow.pro video template (references/template.json) as a timeline.

  python3 template.py              # every phase: start, end, length, how it is entered
  python3 template.py --at 12.4    # the phase on screen at 12.4 s
  python3 template.py --check      # template.json agrees with itself and with statechart/JumpthrowVideo.tla

Exit 0 when fine, 1 when --check finds a problem.
"""
import argparse, json, re, sys
from pathlib import Path

REFS = Path(__file__).resolve().parents[1] / "references"


def load():
    return json.loads((REFS / "template.json").read_text())


def phase_at(phases, t):
    for p in phases:
        if p["start"] <= t < p["end"]:
            return p
    return phases[-1] if t >= phases[-1]["end"] else None


def check(t):
    problems = []
    phases = t["phases"]
    if phases[0]["start"] != 0:
        problems.append("the first phase does not start at 0")
    for a, b in zip(phases, phases[1:]):
        if a["end"] != b["start"]:
            problems.append(f"{a['name']} ends at {a['end']} but {b['name']} starts at {b['start']}")
    for p in phases:
        if p["end"] <= p["start"]:
            problems.append(f"{p['name']} has no length")
    if abs(phases[-1]["end"] - t["reference"]["duration"]) > 0.05:
        problems.append("the last phase does not end at the video's duration")
    release = next(e["at"] for e in t["events"] if e["name"] == "release")
    detonation = next(e["at"] for e in t["events"] if e["name"] == "detonation")
    if abs(release + t["reference"]["airtime"] - detonation) > 0.5:
        problems.append("release + airtime is not within 0.5 s of the detonation")
    spec = (REFS / "statechart" / "JumpthrowVideo.tla").read_text()
    named = re.search(r'phase,\s*\\\*([^\n]*)', spec).group(1)
    in_spec = re.findall(r'"([a-z]+)"', named)
    if in_spec != [p["name"] for p in phases]:
        problems.append(f"phases {[p['name'] for p in phases]} differ from the spec's {in_spec}")
    for p in phases:
        if not re.search(rf'view\s*=\s*"{p["view"]}"|view\' = "{p["view"]}"', spec):
            problems.append(f"{p['name']}'s view {p['view']} is not in the spec")
    return problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--at", type=float)
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    t = load()
    if a.check:
        problems = check(t)
        for p in problems:
            print("problem:", p)
        print("ok" if not problems else f"{len(problems)} problem(s)")
        sys.exit(1 if problems else 0)
    if a.at is not None:
        p = phase_at(t["phases"], a.at)
        print(json.dumps(p, indent=2) if p else "before the video")
        return
    for p in t["phases"]:
        print(f"{p['start']:6.2f}–{p['end']:6.2f}  {p['end'] - p['start']:5.2f} s  {p['name']:<9}  {p['view']:<7}  {p['enter']}")


if __name__ == "__main__":
    main()
