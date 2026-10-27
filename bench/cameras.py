"""Do the sandbox's views open from the web view's cameras? For a sample of throws on every pool map, each view's camera
(Lineup, Aim, Result, Orbit's first pose) as `nades-sandbox --cameras` chooses it, beside the web view's own choice
(bench/cameras.ts: show-me-the-nades' stage.ts fed by its API and awpy's mesh).

    python3 bench/cameras.py --nades <show-me-the-nades checkout> [--per 3] [--maps de_cache,de_dust2] [--db PATH]
                             [--data FILE] [--port 8899]

- The database is cloned (APFS copy-on-write) into the run's folder and the web server reads the clone; the sandbox
  reads the clone's facts, exported as a cs2-throws file by the checkout's `nades export --cs2-throws` (bench/
  web_compare.py), so both read the same throws: nothing touches nades.duckdb itself. --db names another file to
  clone, --data a cs2-throws file already exported from it.
- The throws are the web's own Lineups list, most thrown first: --per of each grenade (smoke, flash, HE, molotov) a map.
- A camera matches when its eye is within 1 unit and its angles within 0.1° (and the result shot is the same player's
  view at the same tick, or both fall back the same way).

Evidence goes to .runs/cameras-<time>/ (cases.json, web.jsonl, sandbox.jsonl, results.json, report.md); the clone is
deleted at the end.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import web_compare

ROOT = Path(__file__).resolve().parent.parent
EYE_UNITS = 1.0
ANGLE_DEGREES = 0.1
VIEWS = ("lineup", "aim", "result", "orbit")


def get(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as response:
        return json.loads(response.read())


def wait_for(url: str, seconds: float = 60) -> None:
    started = time.time()
    while time.time() - started < seconds:
        try:
            get(url)
            return
        except Exception:
            time.sleep(0.25)
    raise SystemExit(f"the web server never answered {url}")


def angle(a: float, b: float) -> float:
    d = (a - b + 180.0) % 360.0 - 180.0
    return abs(d)


def compare(web: dict, ours: dict) -> dict:
    out = {}
    for view in VIEWS:
        w, o = web[view], ours[view]
        eye = math.dist(w["eye"], o["eye"])
        pitch, yaw = angle(w["pitch"], o["pitch"]), angle(w["yaw"], o["yaw"])
        fov = abs((w.get("fov") or 0) - (o.get("fov") or 0))
        same = eye <= EYE_UNITS and pitch <= ANGLE_DEGREES and yaw <= ANGLE_DEGREES and fov <= 1e-3
        if view == "result":
            same = same and w["kind"] == o["kind"] and w.get("player") == o.get("player") and w.get("tick") == o.get("tick")
        out[view] = {"eye": round(eye, 4), "pitch": round(pitch, 5), "yaw": round(yaw, 5), "fov": round(fov, 6), "match": same}
    out["result_kind"] = [web["result"]["kind"], ours["result"]["kind"]]
    out["focus"] = round(math.dist(web["focus"], ours["focus"]), 4)
    out["result_time"] = [web["result_time"], ours["result_time"]]
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--nades", type=Path, help="the show-me-the-nades checkout to compare with (else SHOW_ME_THE_NADES)")
    parser.add_argument("--per", type=int, default=3)
    parser.add_argument("--maps", default="")
    parser.add_argument("--db", type=Path, help="the database to clone (default: the checkout's nades.duckdb)")
    parser.add_argument("--data", type=Path, help="a cs2-throws file exported from that database (default: export the clone)")
    parser.add_argument("--port", type=int, default=8899)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nades-sandbox")
    opts = parser.parse_args()
    nades = web_compare.checkout(opts.nades)
    out = ROOT / ".runs" / f"cameras-{time.strftime('%Y%m%d-%H%M%S')}"
    out.mkdir(parents=True)
    clone = web_compare.clone(opts.db or nades / "nades.duckdb", out)
    data = web_compare.export(nades, clone, out, opts.data)
    ids = web_compare.file_ids(data)
    env = {**os.environ, "NADES_DB": str(clone), "NADES_WEB_PORT": str(opts.port)}
    server = subprocess.Popen(["bun", str(nades / "web/server/src/main.ts")], cwd=nades / "web/server", env=env, stdout=subprocess.DEVNULL, stderr=open(out / "server.log", "w"))
    try:
        api = f"http://localhost:{opts.port}"
        wait_for(f"{api}/api/maps")
        maps = [m for m in opts.maps.split(",") if m] or sorted(p.stem for p in (Path.home() / ".cache/nades/packs").glob("de_*.pack"))
        cases = []
        for m in maps:
            for nade in ("smoke", "flash", "he", "molotov"):
                rows = get(f"{api}/api/lineups?map={m}&nade={nade}&rank=frequent&limit={opts.per}")["rows"]
                cases += [{"throw": r["example_throw_id"], "map": m, "nade": nade, "lineup": r["lineup_id"]} for r in rows if r.get("example_throw_id")]
        (out / "cases.json").write_text(json.dumps(cases, indent=1))
        web = subprocess.run(["bun", str(ROOT / "bench/cameras.ts"), "--web", str(nades / "web/app"), "--api", api, "--cases", str(out / "cases.json")], cwd=nades / "web/app", capture_output=True, text=True, timeout=1800)
        (out / "web.jsonl").write_text(web.stdout)
        if web.returncode != 0:
            raise SystemExit(f"cameras.ts failed: {web.stderr[-2000:]}")
    finally:
        server.terminate()
        server.wait(timeout=30)
    # the same throws in the file's ids
    (out / "cases-file.json").write_text(json.dumps([{**c, "throw": ids.get(c["throw"], c["throw"])} for c in cases], indent=1))
    ours = subprocess.run([str(opts.binary), "--cameras", str(out / "cases-file.json"), "--data", str(data)], capture_output=True, text=True, timeout=1800)
    (out / "sandbox.jsonl").write_text(ours.stdout)
    if ours.returncode != 0:
        raise SystemExit(f"nades-sandbox --cameras failed: {ours.stderr[-2000:]}")
    web_compare.remove_clone(out)

    web_rows = {r["throw"]: r for r in map(json.loads, filter(None, web.stdout.splitlines()))}
    web_id = {file_id: web for web, file_id in ids.items()}
    our_rows = {web_id.get(r["throw"], r["throw"]): r for r in map(json.loads, filter(None, ours.stdout.splitlines()))}
    results = []
    for c in cases:
        w, o = web_rows.get(c["throw"]), our_rows.get(c["throw"])
        if not w or not o:
            results.append({**c, "missing": "web" if not w else "sandbox"})
            continue
        results.append({**c, **compare(w, o), "collider": [w["collider"], o["collider"]], "ms": [round(w["ms"], 2), round(o["ms"], 2)]})
    compared = [r for r in results if "missing" not in r]
    by_view = {v: sum(r[v]["match"] for r in compared) for v in VIEWS}
    worst = {v: max((r[v]["eye"] for r in compared), default=0) for v in VIEWS}
    summary = {"cases": len(cases), "compared": len(compared), "maps": maps, "matches": by_view, "worst_eye_units": worst, "eye_units": EYE_UNITS, "angle_degrees": ANGLE_DEGREES}
    (out / "results.json").write_text(json.dumps({"summary": summary, "results": results}, indent=1))
    lines = [
        f"# Same cameras: {out.name}",
        "",
        f"{len(compared)} throws on {len(maps)} maps ({opts.per} of each grenade a map, the web's Lineups order), each view's camera as the",
        f"sandbox opens it against the web view's own choice. A match is an eye within {EYE_UNITS} unit and angles within {ANGLE_DEGREES}°",
        "(the result: the same player's view at the same tick, or the same fallback).",
        "",
        "| View | Match | Worst eye (units) |",
        "|---|---:|---:|",
    ]
    lines += [f"| {v} | {by_view[v]} / {len(compared)} | {worst[v]:.4f} |" for v in VIEWS]
    misses = [r for r in compared if not all(r[v]["match"] for v in VIEWS)]
    if misses:
        lines += ["", "## Differences", "", "| Map | Throw | Grenade | View | Eye (units) | Pitch (°) | Yaw (°) | Result (web, sandbox) |", "|---|---|---|---|---:|---:|---:|---|"]
        for r in misses:
            for v in VIEWS:
                if not r[v]["match"]:
                    lines.append(f"| {r['map']} | {r['throw']} | {r['nade']} | {v} | {r[v]['eye']} | {r[v]['pitch']} | {r[v]['yaw']} | {' / '.join(r['result_kind'])} |")
    lost = [r for r in results if "missing" in r]
    if lost:
        lines += ["", f"Not compared: {', '.join(r['throw'] + ' (no ' + r['missing'] + ' row)' for r in lost)}"]
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nevidence: {out}")
    return 0 if all(by_view[v] == len(compared) for v in VIEWS) else 1


if __name__ == "__main__":
    sys.exit(main())
