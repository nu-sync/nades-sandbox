#!/usr/bin/env python3
"""cs2-throws version 1: the schema and a reference writer, with Python's standard library only.

A cs2-throws file is one SQLite file of what CS2 demos record about grenade throws, named after the game's own
properties and events (docs/standalone-api.md §3). Any parser can write one; nades-sandbox reads it with
`--data <file>` and checks it with `nades-sandbox check <file>`.

    python3 tools/cs2_throws.py schema OUT.sqlite     # an empty version 1 file
    python3 tools/cs2_throws.py fixture OUT.sqlite    # a small worked example that fills every table

Conventions are the game's: Hammer units with z up; angles in degrees as m_angEyeAngles stores them, pitch positive
looking down; server ticks, 64 a second; sides 'T' and 'CT'; grenades 'smoke', 'flash', 'he', 'molotov',
'incendiary' and 'decoy'.

Only a demo's id and map, and a throw's tick, grenade, side and start (plus a detonation or a flight to land it), are
required. Leave out what your parser does not give: each feature of the sandbox names the facts it needs, and
`nades-sandbox check` says which are on for a file.

To convert a parser's output, open a `Writer`, add each demo, its rounds, throws and samples as plain values, and close
it. `fixture()` below is the worked example: read it beside the schema.
"""

from __future__ import annotations

import math
import sqlite3
import struct
import sys
from datetime import datetime, timezone
from pathlib import Path

FORMAT = "cs2-throws"
VERSION = "1.0"

SCHEMA = """
create table meta (key text primary key, value text not null);
-- required: format = 'cs2-throws', version = '1.0'; optional: producer (free text), created (ISO 8601, UTC)

create table demos (
    demo_id   text primary key,  -- recommended: the .dem file's SHA-256, hex
    map       text not null,     -- the demo header's map name, e.g. 'de_inferno'
    played_at integer,           -- unix ms, UTC, when known
    file_name text
);

create table rounds (            -- optional
    demo_id text not null, number integer not null,
    freeze_end_tick integer,     -- round_freeze_end
    plant_tick integer,          -- bomb_planted
    end_tick integer,            -- round_end
    winner text,                 -- 'T' | 'CT'
    primary key (demo_id, number)
);

create table throws (
    throw_id text primary key,   -- unique in the file; recommended '<demo_id>:<tick>:<projectile entity index>'
    demo_id  text not null,
    round    integer,
    tick     integer not null,   -- the projectile's first tick
    grenade  text not null,      -- from the projectile's class; molotov or incendiary by m_bIsIncGrenade
    side     text not null,      -- the thrower's m_iTeamNum
    player   text, steamid text, team text,  -- m_iszPlayerName, m_steamID (SteamID64), m_szClanTeamname
    place    text,               -- the thrower's m_szLastPlaceName
    start_x real not null, start_y real not null, start_z real not null,  -- m_vInitialPosition
    vel_x real, vel_y real, vel_z real,                                    -- m_vInitialVelocity
    -- the thrower's pawn at `tick`, when known
    origin_x real, origin_y real, origin_z real,  -- the pawn's origin (its feet)
    takeoff_x real, takeoff_y real, takeoff_z real,  -- released in the air: its origin at its last tick on the ground
    pitch real, yaw real,                          -- m_angEyeAngles
    pvel_x real, pvel_y real, pvel_z real,         -- m_vecVelocity
    on_ground integer,                             -- m_hGroundEntity is set
    duck real,                                     -- m_pMovementServices.m_flDuckAmount: 0 standing, 1 crouched
    walking integer,                               -- m_bIsWalking
    -- what it did
    detonate_tick integer, det_x real, det_y real, det_z real,  -- the grenade's detonate event
    end_tick integer,                                           -- its expiry event (smokegrenade_expired, inferno_expire)
    flight blob  -- the projectile's networked positions; per point, little-endian i32 tick, f32 x, f32 y, f32 z
);
create index throws_by_demo on throws (demo_id, tick);

create table smoke_voxels (      -- optional
    throw_id   text primary key,
    first_tick integer not null, -- the tick m_nVoxelFrameDataSize first became non-zero
    x real, y real, z real,      -- m_vSmokeDetonationPos
    bytes blob not null          -- m_VoxelFrameData[0 .. m_nVoxelFrameDataSize], as last networked
);

create table flames (            -- optional; the inferno's arrays
    throw_id text not null, slot integer not null,
    x real not null, y real not null, z real not null,         -- m_firePositions[slot]
    nx real, ny real, nz real,                                 -- m_BurnNormal[slot]
    first_tick integer not null, last_tick integer not null    -- while m_bFireIsBurning[slot]
);
create index flames_by_throw on flames (throw_id);

create table blinds (            -- optional; player_blind
    throw_id text not null, player text, steamid text, side text,
    x real, y real, z real,      -- the victim's origin at the event, when known
    seconds  real not null       -- blind_duration
);
create index blinds_by_throw on blinds (throw_id);

create table player_samples (    -- optional; any rate (viewpoints want 2 a second or more)
    demo_id text not null, tick integer not null,
    player text, steamid text, side text not null,
    x real not null, y real not null, z real not null,  -- the pawn's origin
    pitch real, yaw real, duck real,
    alive integer not null,      -- m_lifeState
    place text                   -- m_szLastPlaceName
);
create index samples_by_tick on player_samples (demo_id, tick);

create table tags (              -- optional; a producer's own labels
    subject text not null,       -- a demo_id or a throw_id
    key     text not null,       -- '<namespace>.<name>', e.g. 'nades.lineup', 'hltv.event'
    value   text not null
);
create index tags_by_subject on tags (subject);
"""

GRENADES = ("smoke", "flash", "he", "molotov", "incendiary", "decoy")
SIDES = ("T", "CT")
THROW_COLUMNS = (
    "throw_id", "demo_id", "round", "tick", "grenade", "side", "player", "steamid", "team", "place",
    "start_x", "start_y", "start_z", "vel_x", "vel_y", "vel_z", "origin_x", "origin_y", "origin_z",
    "takeoff_x", "takeoff_y", "takeoff_z", "pitch", "yaw", "pvel_x", "pvel_y", "pvel_z", "on_ground", "duck",
    "walking", "detonate_tick", "det_x", "det_y", "det_z", "end_tick", "flight",
)


def flight_blob(points: list[tuple[int, float, float, float]]) -> bytes:
    """A flight as the format stores it: per point, little-endian i32 tick, f32 x, y, z."""
    return b"".join(struct.pack("<ifff", tick, x, y, z) for tick, x, y, z in points)


def xyz(prefix: str, point) -> dict:
    return {} if point is None else {f"{prefix}x": point[0], f"{prefix}y": point[1], f"{prefix}z": point[2]}


class Writer:
    """Writes one cs2-throws file. Values are plain Python: None for what is not known."""

    def __init__(self, path: Path | str, producer: str = "tools/cs2_throws.py") -> None:
        self.path = Path(path)
        if self.path.exists():
            self.path.unlink()
        self.db = sqlite3.connect(self.path)
        self.db.executescript(SCHEMA)
        created = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        self.db.executemany("insert into meta values (?, ?)", [("format", FORMAT), ("version", VERSION), ("producer", producer), ("created", created)])

    def demo(self, demo_id: str, map: str, played_at: int | None = None, file_name: str | None = None) -> None:
        self.db.execute("insert into demos values (?, ?, ?, ?)", (demo_id, map, played_at, file_name))

    def round(self, demo_id: str, number: int, freeze_end_tick=None, plant_tick=None, end_tick=None, winner=None) -> None:
        self.db.execute("insert into rounds values (?, ?, ?, ?, ?, ?)", (demo_id, number, freeze_end_tick, plant_tick, end_tick, winner))

    def throw(self, throw_id: str, demo_id: str, tick: int, grenade: str, side: str, start, *, round=None, player=None,
              steamid=None, team=None, place=None, velocity=None, origin=None, takeoff=None, pitch=None, yaw=None,
              pawn_velocity=None, on_ground=None, duck=None, walking=None, detonation=None, end_tick=None, flight=None) -> None:
        """`detonation` is (tick, (x, y, z)); `flight` is [(tick, x, y, z), ...] from the release on."""
        if grenade not in GRENADES or side not in SIDES:
            raise ValueError(f"{throw_id}: grenade {grenade!r} or side {side!r} is not one of {GRENADES} / {SIDES}")
        row = {
            "throw_id": throw_id, "demo_id": demo_id, "round": round, "tick": tick, "grenade": grenade, "side": side,
            "player": player, "steamid": None if steamid is None else str(steamid), "team": team, "place": place,
            **xyz("start_", start), **xyz("vel_", velocity), **xyz("origin_", origin), **xyz("takeoff_", takeoff),
            "pitch": pitch, "yaw": yaw, **xyz("pvel_", pawn_velocity),
            "on_ground": None if on_ground is None else int(on_ground), "duck": duck,
            "walking": None if walking is None else int(walking),
            "end_tick": end_tick, "flight": None if flight is None else flight_blob(flight),
        }
        if detonation is not None:
            row.update({"detonate_tick": detonation[0], **xyz("det_", detonation[1])})
        values = [row.get(c) for c in THROW_COLUMNS]
        self.db.execute(f"insert into throws ({', '.join(THROW_COLUMNS)}) values ({', '.join('?' * len(THROW_COLUMNS))})", values)

    def smoke_voxels(self, throw_id: str, first_tick: int, origin, data: bytes) -> None:
        x, y, z = origin if origin is not None else (None, None, None)
        self.db.execute("insert into smoke_voxels values (?, ?, ?, ?, ?, ?)", (throw_id, first_tick, x, y, z, data))

    def flame(self, throw_id: str, slot: int, pos, first_tick: int, last_tick: int, normal=None) -> None:
        nx, ny, nz = normal if normal is not None else (None, None, None)
        self.db.execute("insert into flames values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", (throw_id, slot, *pos, nx, ny, nz, first_tick, last_tick))

    def blind(self, throw_id: str, seconds: float, player=None, steamid=None, side=None, pos=None) -> None:
        x, y, z = pos if pos is not None else (None, None, None)
        self.db.execute("insert into blinds values (?, ?, ?, ?, ?, ?, ?, ?)", (throw_id, player, None if steamid is None else str(steamid), side, x, y, z, seconds))

    def sample(self, demo_id: str, tick: int, side: str, origin, alive: bool, player=None, steamid=None, pitch=None, yaw=None, duck=None, place=None) -> None:
        self.db.execute("insert into player_samples values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        (demo_id, tick, player, None if steamid is None else str(steamid), side, *origin, pitch, yaw, duck, int(alive), place))

    def tag(self, subject: str, key: str, value: str) -> None:
        self.db.execute("insert into tags values (?, ?, ?)", (subject, key, value))

    def close(self) -> None:
        self.db.commit()
        self.db.close()

    def __enter__(self) -> "Writer":
        return self

    def __exit__(self, *exc) -> None:
        self.close()


# The worked example. Its numbers are made up, but each throw is shaped the way CS2 records one.

def release_velocity(pitch: float, yaw: float, strength: float, player_velocity) -> tuple[float, float, float]:
    """CS2's release model, for making up a throw's m_vInitialVelocity (a parser reads it from the projectile)."""
    throw_pitch = -10.0 + pitch * 80.0 / 90.0 if pitch < 90.0 else -10.0 + (360.0 - pitch) * -80.0 / 90.0
    elevation, heading = math.radians(-throw_pitch), math.radians(yaw)
    speed = 675.0 * (0.3 + 0.7 * strength)
    f = (math.cos(elevation) * math.cos(heading), math.cos(elevation) * math.sin(heading), math.sin(elevation))
    return tuple(f[k] * speed + 1.25 * player_velocity[k] for k in range(3))


def voxel_log() -> bytes:
    """A smoke's voxel log of three frames: the first sends two emitters and one brick, the last says it is fading."""
    def frame(number: int, payload: bytes) -> bytes:
        return struct.pack("<HH", number, len(payload)) + payload
    first = bytes([0, 3, 2]) + bytes([16, 16, 13, 5]) + struct.pack("<f", 0.0) + bytes([15, 16, 13, 5]) + struct.pack("<f", 1.0)
    first += struct.pack("<HHQ", 1, 286, 0x0F0F0F0F00000000) + bytes([0])
    return frame(0, first) + frame(1, bytes([0, 0, 0])) + frame(2, bytes([1, 0, 0]))


def arc(tick: int, start, land, ticks: int, peak: float = 120.0) -> list[tuple[int, float, float, float]]:
    """A flight from `start` to `land` over `ticks`, a point every 2 ticks."""
    points = []
    for k in range(0, ticks + 1, 2):
        f = k / ticks
        points.append((tick + k, start[0] + (land[0] - start[0]) * f, start[1] + (land[1] - start[1]) * f, start[2] + (land[2] - start[2]) * f + peak * 4 * f * (1 - f)))
    return points


def fixture(path: Path | str) -> None:
    """Three demos (two on de_mirage, one on de_inferno) whose throws touch every table and every rule."""
    with Writer(path, producer="tools/cs2_throws.py fixture") as w:
        d1, d2, d3 = "fixture-demo-1", "fixture-demo-2", "fixture-demo-3"
        w.demo(d1, "de_mirage", played_at=1767225600000, file_name="fixture-1.dem")
        w.demo(d2, "de_mirage", played_at=1767312000000, file_name="fixture-2.dem")
        w.demo(d3, "de_inferno", file_name="fixture-3.dem")
        w.tag(d1, "fixture.event", "Fixture Cup")
        w.round(d1, 1, freeze_end_tick=1000, plant_tick=3500, end_tick=6000, winner="T")
        w.round(d1, 2, freeze_end_tick=7000, end_tick=12000, winner="CT")
        w.round(d2, 1, freeze_end_tick=500, end_tick=5000, winner="CT")

        # Two window smokes from T spawn, standing, left click: one lineup across both demos.
        stand = dict(pitch=-30.0, yaw=135.0, pawn_velocity=(0.0, 0.0, 0.0), on_ground=True, duck=0.0, walking=False)
        for throw_id, demo, rnd, tick, origin, land in (
            ("m1-window-a", d1, 1, 1500, (1200.0, -200.0, -160.0), (-400.0, -650.0, -200.0)),
            ("m2-window-b", d2, 1, 900, (1203.0, -201.0, -160.0), (-404.0, -647.0, -200.0)),
        ):
            start = (origin[0] + 5.0, origin[1] - 3.0, origin[2] + 64.0)
            w.throw(throw_id, demo, tick, "smoke", "T", start, round=rnd, player="ropz", steamid=76561198000000001, team="MOUZ",
                    place="TSpawn", velocity=release_velocity(-30.0, 135.0, 1.0, (0, 0, 0)), origin=origin, **stand,
                    detonation=(tick + 120, land), end_tick=tick + 120 + 18 * 64, flight=arc(tick, start, land, 120))
            w.smoke_voxels(throw_id, tick + 120, land, voxel_log())
            w.tag(throw_id, "fixture.lineup", "window")

        # A jump-throw smoke: released in the air at the jump-throw's vertical speed, from its take-off point.
        takeoff = (1100.0, 100.0, -160.0)
        w.throw("m1-jungle-jump", d1, 1600, "smoke", "T", (1101.0, 101.0, -80.0), round=1, player="torzsi", team="MOUZ", place="TSpawn",
                velocity=release_velocity(-45.0, 150.0, 1.0, (0.0, 0.0, 218.8)), origin=(1100.0, 100.0, -150.0), takeoff=takeoff,
                pitch=-45.0, yaw=150.0, pawn_velocity=(0.0, 0.0, 218.8), on_ground=False, duck=0.0, walking=False,
                detonation=(1700, (-900.0, 300.0, -100.0)), end_tick=1700 + 18 * 64)

        # A T molotov and a CT incendiary from the same spot: one fire lineup.
        for throw_id, grenade, side, rnd, tick, team in (("m1-molly", "molotov", "T", 1, 2000, "MOUZ"), ("m1-incendiary", "incendiary", "CT", 2, 8000, "Vitality")):
            w.throw(throw_id, d1, tick, grenade, side, (300.0, -1500.0, -100.0), round=rnd, player="jimpphat" if side == "T" else "ZywOo",
                    team=team, place="Palace", detonation=(tick + 90, (-200.0, -2000.0, -180.0)), end_tick=tick + 90 + 7 * 64)
            for slot, (dx, first) in enumerate(((0.0, 0), (40.0, 6), (80.0, 12))):
                w.flame(throw_id, slot, (-200.0 + dx, -2000.0, -180.0), tick + 90 + first, tick + 90 + 7 * 64, normal=(0.0, 0.0, 1.0))

        # A flash that blinded a CT and a teammate.
        w.throw("m1-flash", d1, 2500, "flash", "T", (-100.0, -1000.0, -100.0), round=1, player="ropz", team="MOUZ", place="Palace",
                detonation=(2560, (-500.0, -1200.0, 0.0)))
        w.blind("m1-flash", 2.4, player="ZywOo", side="CT", pos=(-900.0, -1300.0, -170.0))
        w.blind("m1-flash", 0.7, player="torzsi", side="T", pos=(-150.0, -1100.0, -160.0))

        # An HE after the plant, a decoy, a warm-up smoke, and a throw known only by its flight.
        w.throw("m1-he", d1, 4000, "he", "T", (-300.0, -1800.0, -100.0), round=1, player="ropz", team="MOUZ", detonation=(4100, (-600.0, -2100.0, -170.0)))
        w.throw("m1-decoy", d1, 4200, "decoy", "T", (0.0, 0.0, -100.0), round=1, detonation=(4300, (100.0, 100.0, -160.0)))
        w.throw("m1-warmup", d1, 200, "smoke", "T", (1205.0, -203.0, -96.0), detonation=(320, (-400.0, -650.0, -200.0)))
        w.throw("m2-flight-only", d2, 1200, "flash", "CT", (-1000.0, -500.0, -100.0), round=1, flight=arc(1200, (-1000.0, -500.0, -100.0), (-700.0, -300.0, -50.0), 40))

        # The bare minimum on another map: a tick, a grenade, a side, a start and where it went off.
        w.throw("i3-minimum", d3, 640, "smoke", "CT", (500.0, 2000.0, 128.0), detonation=(700, (900.0, 2500.0, 128.0)))

        # Samples: where players stood (with their places) and looked, every 32 ticks.
        for tick in range(1000, 4600, 32):
            w.sample(d1, tick, "CT", (-900.0, -1300.0, -170.0), alive=True, player="ZywOo", pitch=2.0, yaw=60.0, place="BombsiteA")
            w.sample(d1, tick, "CT", (-450.0, -700.0, -200.0), alive=tick < 3000, player="apEX", pitch=0.0, yaw=-30.0, place="Window")
            w.sample(d1, tick, "T", (1200.0, -200.0, -160.0), alive=True, player="ropz", pitch=-30.0, yaw=135.0, place="TSpawn")
        for tick in range(500, 2500, 32):
            w.sample(d2, tick, "CT", (-700.0, -900.0, -200.0), alive=True, player="flameZ", pitch=5.0, yaw=10.0, place="Window")
        w.sample(d3, 700, "T", (900.0, 2500.0, 128.0), alive=True, player="someone", place="Banana")


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[1] not in ("schema", "fixture"):
        print(__doc__.strip().splitlines()[0])
        print("usage: cs2_throws.py schema|fixture OUT.sqlite", file=sys.stderr)
        return 2
    out = Path(argv[2])
    if argv[1] == "schema":
        Writer(out).close()
    else:
        fixture(out)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
