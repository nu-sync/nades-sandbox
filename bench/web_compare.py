"""What the benches that compare the sandbox with show-me-the-nades' web view share (bench/cameras.py, bench/effects.py).

- The checkout is named, never assumed: `--nades <dir>`, else SHOW_ME_THE_NADES.
- Its database is cloned (APFS copy-on-write) into the run's folder; the web view's server reads the clone.
- The sandbox reads facts only: the clone exported as a cs2-throws file by the checkout's own `nades export --cs2-throws`
  (or a file given with `--data`, exported from the same database).
- The web view names throws `<sha256[:12]>-<tick>-<entity>`; the file names them `<sha256>:<tick>:<entity>`.
"""

from __future__ import annotations

import os
import sqlite3
import subprocess
from pathlib import Path


def checkout(given: Path | None) -> Path:
    path = given or (Path(os.environ["SHOW_ME_THE_NADES"]) if os.environ.get("SHOW_ME_THE_NADES") else None)
    if path is None:
        raise SystemExit("name the show-me-the-nades checkout to compare with: --nades <dir> or SHOW_ME_THE_NADES=<dir>")
    if not (path / "web/app").is_dir():
        raise SystemExit(f"{path} is not a show-me-the-nades checkout (it has no web/app)")
    return path.resolve()


def clone(db: Path, out: Path) -> Path:
    target = out / "nades.duckdb"
    subprocess.run(["cp", "-c", str(db), str(target)], check=True)
    return target


def export(nades: Path, database: Path, out: Path, data: Path | None) -> Path:
    """The cs2-throws file the sandbox reads: `data` when given, else `database` exported by the checkout's nades."""
    if data is not None:
        return data
    binary = nades / "target/release/nades"
    target = out / "throws.sqlite"
    ran = subprocess.run([str(binary), "--db", str(database), "export", "--cs2-throws", str(target)], capture_output=True, text=True)
    if ran.returncode != 0:
        raise SystemExit(f"{binary} could not export the clone (`nades export --cs2-throws`; build it, or pass --data): {(ran.stderr or ran.stdout)[-1500:]}")
    return target


def file_ids(data: Path) -> dict[str, str]:
    """The web view's throw ids, each with the file's id of the same throw."""
    with sqlite3.connect(f"file:{data}?mode=ro", uri=True) as db:
        ids = [row[0] for row in db.execute("select throw_id from throws")]
    out = {}
    for tid in ids:
        sha, tick, entity = tid.split(":")
        out[f"{sha[:12]}-{tick}-{entity}"] = tid
    return out


def remove_clone(out: Path) -> None:
    for f in out.glob("nades.duckdb*"):
        f.unlink()
