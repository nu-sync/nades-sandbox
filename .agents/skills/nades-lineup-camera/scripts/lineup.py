#!/usr/bin/env python3
"""Render a CS2 grenade lineup through the real nades-mcp server, as an agent does, and keep the evidence.

    python3 .agents/skills/nades-lineup-camera/scripts/lineup.py --map mirage --grenade smoke \
        --lineup "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0" --technique jumpthrow \
        [--draft] [--seconds 20] [--out .runs/lineup-<time>] [--server target/release/nades-mcp] [--simulate-only]

It starts its own nades-mcp over stdio (newline-delimited JSON-RPC 2.0), calls throws_simulate and then lineup_render,
and writes into --out: lineup.mp4, its stills, its JSON, calls.jsonl (every request and result), ffprobe.json and
frames.png (a contact sheet when Pillow is importable). The server and its renderer are this process's children and end
with it; nothing else is touched. Exit 0 on success, 1 on a failed render, 2 when a prerequisite is missing.
"""
import argparse, json, shutil, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]


def main() -> int:
    a = argparse.ArgumentParser()
    a.add_argument("--map", required=True)
    a.add_argument("--grenade", required=True)
    a.add_argument("--lineup", required=True)
    a.add_argument("--technique", default=None)
    a.add_argument("--seconds", type=float, default=None)
    a.add_argument("--draft", action="store_true", help="640x360 at 20 fps: a few seconds to render")
    a.add_argument("--simulate-only", action="store_true")
    a.add_argument("--out", type=Path, default=ROOT / ".runs" / f"lineup-{time.strftime('%Y%m%d-%H%M%S')}")
    a.add_argument("--server", type=Path, default=ROOT / "target/release/nades-mcp")
    o = a.parse_args()
    if not o.server.is_file():
        print(f"missing {o.server}: run `cargo build --release` (unavailable, not a pass)", file=sys.stderr)
        return 2
    if not o.simulate_only and not shutil.which("ffmpeg"):
        print("missing ffmpeg with libx264 on PATH (unavailable, not a pass)", file=sys.stderr)
        return 2
    o.out.mkdir(parents=True, exist_ok=True)
    log = open(o.out / "calls.jsonl", "w")
    server = subprocess.Popen([str(o.server)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(o.out / "server.log", "w"), text=True)
    ids = iter(range(1, 1_000_000))

    def send(message):
        server.stdin.write(json.dumps(message) + "\n")
        server.stdin.flush()

    def request(method, params):
        n = next(ids)
        send({"jsonrpc": "2.0", "id": n, "method": method, "params": params})
        while True:
            line = server.stdout.readline()
            if not line:
                raise SystemExit("nades-mcp closed its output")
            m = json.loads(line)
            if m.get("id") == n:
                return m

    def call(tool, arguments):
        r = request("tools/call", {"name": tool, "arguments": arguments})["result"]
        log.write(json.dumps({"tool": tool, "arguments": arguments, "is_error": r.get("isError", False), "result": r.get("structuredContent")}) + "\n")
        log.flush()
        return r.get("structuredContent"), r.get("isError", False)

    try:
        request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "nades-lineup-camera", "version": "1"}})
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        throw = {"grenade": o.grenade, "console": o.lineup}
        technique = {}
        if o.technique:
            # the simulate tool takes fields: read the technique the way lineup_render does, through its own render
            technique = {"technique": o.technique}
        lineup = {"map": o.map, "grenade": o.grenade, "lineup": o.lineup, **technique}
        if o.simulate_only:
            words = (o.technique or "").lower()
            movement = "run_jumpthrow" if "run" in words and "jump" in words else "jumpthrow" if "jump" in words else "run" if "run" in words else "walk" if "walk" in words else "stand"
            throw.update(movement=movement, crouch=("crouch" in words or "duck" in words))
            mapname = o.map if o.map.startswith("de_") else f"de_{o.map}"
            result, error = call("throws_simulate", {"scene": {"map": mapname, "throws": [throw]}})
            print(json.dumps(result, indent=1) if not error else result)
            return 1 if error else 0
        args = {**lineup, "output": str((o.out / "lineup.mp4").resolve())}
        if o.seconds:
            args["seconds"] = o.seconds
        if o.draft:
            args.update(width=640, height=360, fps=20)
        started = time.time()
        result, error = call("lineup_render", args)
        took = time.time() - started
        if error or result["video"]["state"] != "completed":
            print(f"render failed: {json.dumps(result)[:2000]}", file=sys.stderr)
            return 1
        print(result["summary"])
        print(f"technique understood: {result['technique']['understood']}; rendered in {took:.1f} s")
        probe = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height,nb_read_frames,r_frame_rate:format=duration", "-of", "json", str(o.out / "lineup.mp4")], capture_output=True, text=True)
        (o.out / "ffprobe.json").write_text(probe.stdout)
        sheet(o.out, float(json.loads(probe.stdout or "{}").get("format", {}).get("duration", 0)))
        print(f"evidence: {o.out}")
        return 0
    finally:
        server.stdin.close()
        try:
            server.wait(timeout=20)
        except subprocess.TimeoutExpired:
            server.kill()


def sheet(out: Path, duration: float) -> None:
    """Sixteen frames along the video as one image, each labelled with its second."""
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        print("Pillow not importable: no contact sheet (run with `uv run --with pillow`)", file=sys.stderr)
        return
    frames = out / "frames"
    frames.mkdir(exist_ok=True)
    tiles = []
    for k in range(16):
        t = duration * (k + 0.5) / 16
        f = frames / f"{t:06.2f}.png"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{t:.2f}", "-i", str(out / "lineup.mp4"), "-frames:v", "1", "-vf", "scale=480:-2", str(f)], check=False)
        if f.exists():
            im = Image.open(f).convert("RGB")
            ImageDraw.Draw(im).text((6, 6), f"{t:.1f} s", fill=(255, 255, 0))
            tiles.append(im)
    if tiles:
        w, h = tiles[0].size
        c = Image.new("RGB", (w * 4, h * ((len(tiles) + 3) // 4)))
        for i, im in enumerate(tiles):
            c.paste(im, ((i % 4) * w, (i // 4) * h))
        c.save(out / "frames.png")


if __name__ == "__main__":
    sys.exit(main())
