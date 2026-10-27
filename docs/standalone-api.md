# A standalone, general-purpose sandbox: the proposed APIs (for review)

Proposal, revision 2, 2026-10-26. Step 1 (the core, the file adapter and the switch away from show-me-the-nades'
database) is implemented; docs/decisions.md S24 records it and the refinements to §4 it needed. §3 is at version 2.0
(S25): integer keys inside the file, a third of version 1's size. Step 2, the demo adapter, is implemented (S28).

**The goal.** Anyone with CS2 demos sees their throws in 3D on any CS2 map. Nothing the sandbox exposes depends on
how show-me-the-nades, or any other tool, stores or labels its data.

**The test** for every name in the contract: either the game records it, or the sandbox computes it by a rule
written here. A producer's own opinions travel only as tags. The sandbox shows tags but never interprets them.

Revision 1 failed that test in twelve places (§1). It also left the maps tied to show-me-the-nades' export scripts
(§7).

## 1. What revision 1 took from show-me-the-nades

| Revision 1 | Where it came from | Revision 2 |
|---|---|---|
| format `nades-lineups`, producer `nades export-lineups` | one producer's name | format `cs2-throws`; the producer is free text in `meta` |
| `lineup_id` | its lineup registry, in its ingest order | the sandbox groups throws itself (§4) |
| `technique` | its physics classifier | the sandbox classifies from the release facts (§4) |
| `practice_command`, `practice_ready` | its SQL | the sandbox writes them (§4) |
| `phase` (early < 20 s, mid < 60 s, late, post-plant) | its SQL thresholds | a round-time range and an after-plant filter, from the round's ticks (§4) |
| `organization` | its alias table of tracked pro teams | dropped: the demo's own team names, plus tags |
| `to_place` | callouts it learned from player positions | the sandbox learns them from the game's place names on player samples (§4) |
| `event`, `match_time` | HLTV archive names and archive file times | optional `played_at`; the event as a tag |
| `match_id`, ids "stable within one dataset" | its own ids | `demo_id`, recommended to be the demo file's SHA-256, so producers agree |
| `airtime`, `effect_seconds`, `landing` | computed by it | computed by the sandbox from the detonation and expiry ticks (§4) |
| the lineup and viewer rules, labelled "show-me-the-nades' rules" | its SQL macros | the sandbox's own rules, one implementation for every source |
| `~/.cache/nades`, `NADES_*` | its cache folder and variables | the sandbox's own folders and variables (§8) |

Everything kept is already defined by the game: Hammer units, CS2's angles, 64-tick time, the smoke's voxel log as
networked, the fire's flames, and the thrower's place name.

## 2. Three layers

1. **Facts:** what a CS2 demo records, named after the game's own properties and events. Any parser can produce
   them: awpy, demoparser2 or your own.
2. **Derivations:** everything computed from facts, such as grouping, technique, practice commands, round time,
   landing callouts and viewpoints. One implementation lives in the sandbox's core and serves every source.
3. **Tags:** a producer's own labels, as `namespace.key = value` on a demo or a throw.

**The required minimum is small:**
- per demo: an id and the map name;
- per throw: its tick, grenade, thrower side and start position, plus a detonation position or a flight to land it.

Every other fact is optional. Each feature names the facts it needs:

| Feature | Needs, beyond the minimum |
|---|---|
| the throw list, grouping, filters by side, grenade, player and team, the orbit view | nothing (player and team names when known) |
| the drawn trajectory | the flight |
| the thrower's and aim views, the practice command | the thrower's origin and eye angles (the duck amount for crouched throws, the take-off point for jump-throws) |
| technique (movement, click) | the initial velocity (or the flight's first points), the thrower's velocity, eye angles and ground state |
| the smoke as simulated | smoke voxels; otherwise a stand-in cloud of nominal size, labelled as such |
| the fire's spread | flames; otherwise a stand-in fire at the detonation, labelled |
| who was flashed | blinds |
| the opponent's view | player samples |
| landing callouts | player samples with place names |
| round-time and after-plant filters | rounds |
| one player across name changes | Steam ids |

`nades-sandbox check <file>` prints this table for a given file: what works, what is off and which fact is missing.

## 3. The facts: `cs2-throws`, version 2

The container is one SQLite file:
- any language writes it, Python's standard library included;
- reads by key are indexed;
- its file format has been stable for decades;
- the reader, `rusqlite` bundled, is light.

**Conventions are the game's own.** Positions are in Hammer units with z up. Angles are in degrees as
`m_angEyeAngles` stores them; pitch is positive looking down. Ticks are server ticks, 64 a second. Sides are `T` and
`CT`; grenades are `smoke`, `flash`, `he`, `molotov`, `incendiary` and `decoy`.

**Keys.** `demo_id` and `throw_id` name a demo and a throw across files, and tags and the sandbox's API use them. Inside
one file, tables refer to a demo, a player, a place and a throw by small integer keys the writer picks: `demos.demo`,
`players.player` (per demo), `places.place` and `throws.throw`. A writer that leaves the key out of an insert gets
SQLite's next row id. A player is one name and Steam id pair, so a player renamed mid-demo has two keys.

**Writing it well.** Set `pragma page_size = 16384` before the first `create table`: the file is ≈10% smaller than
with SQLite's 4 KB default, mostly in voxel logs. Write each table in its key order; `player_samples` and `flames` are
stored in key order, so their keys are their only index.

```sql
create table meta (key text primary key, value text not null);
-- required: format = 'cs2-throws', version = '2.1' (or '2.0', without player_tracks); optional: producer, created

create table demos (
    demo      integer primary key,  -- the file's own key for the demo; other tables refer to it by this
    demo_id   text not null unique, -- the demo's id across files; recommended: the .dem file's SHA-256, hex
    map       text not null,        -- the demo header's map name, e.g. 'de_inferno'
    played_at integer,              -- unix ms, UTC, when known
    file_name text
);

create table players (           -- optional; each name and Steam id pair a demo shows
    demo    integer not null,    -- demos.demo
    player  integer not null,    -- the file's own key for the player within the demo
    name    text,                -- m_iszPlayerName
    steamid text,                -- m_steamID (SteamID64), decimal
    primary key (demo, player)
) without rowid;

create table places (            -- optional; the place names the file uses
    place integer primary key,
    name  text not null unique   -- m_szLastPlaceName
);

create table rounds (            -- optional
    demo integer not null, number integer not null,
    freeze_end_tick integer,     -- round_freeze_end
    plant_tick integer,          -- bomb_planted
    end_tick integer,            -- round_end
    winner text,                 -- 'T' | 'CT'
    primary key (demo, number)
) without rowid;

create table throws (
    throw    integer primary key,  -- the file's own key for the throw; other tables refer to it by this
    throw_id text not null unique, -- the throw's id across files; recommended '<demo_id>:<tick>:<projectile entity index>'
    demo     integer not null,     -- demos.demo
    round    integer,
    tick     integer not null,     -- the projectile's first tick
    grenade  text not null,        -- from the projectile's class; molotov or incendiary by m_bIsIncGrenade
    side     text not null,        -- the thrower's m_iTeamNum
    player   integer,              -- players.player in this demo: the thrower
    team     text,                 -- the thrower's m_szClanTeamname
    place    integer,              -- places.place: the thrower's m_szLastPlaceName
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
create index throws_by_demo on throws (demo, tick);

create table smoke_voxels (      -- optional
    throw      integer primary key, -- throws.throw
    first_tick integer not null, -- the tick m_nVoxelFrameDataSize first became non-zero
    x real, y real, z real,      -- m_vSmokeDetonationPos
    bytes blob not null          -- m_VoxelFrameData[0 .. m_nVoxelFrameDataSize], as last networked
);

create table flames (            -- optional; the inferno's arrays
    throw integer not null, slot integer not null,             -- throws.throw
    x real not null, y real not null, z real not null,         -- m_firePositions[slot]
    nx real, ny real, nz real,                                 -- m_BurnNormal[slot]
    first_tick integer not null, last_tick integer not null,   -- while m_bFireIsBurning[slot]
    primary key (throw, slot)
) without rowid;

create table blinds (            -- optional; player_blind
    throw  integer not null,     -- throws.throw
    player integer,              -- players.player in the throw's demo: who was blinded
    side   text,
    x real, y real, z real,      -- the victim's origin at the event, when known
    seconds real not null        -- blind_duration
);
create index blinds_by_throw on blinds (throw);

create table player_samples (    -- optional; any rate (viewpoints want 2 a second or more)
    demo   integer not null,     -- demos.demo
    tick   integer not null,
    player integer not null,     -- players.player in this demo
    side   text not null,
    x real not null, y real not null, z real not null,  -- the pawn's origin
    pitch real, yaw real, duck real,
    alive  integer not null,     -- m_lifeState
    place  integer,              -- places.place: m_szLastPlaceName
    primary key (demo, tick, player)
) without rowid;

create table player_tracks (     -- optional (2.1): each player through each round, finer than samples, for replays
    demo      integer not null,  -- demos.demo
    round     integer not null,  -- rounds.number
    player    integer not null,  -- players.player in this demo
    side      text not null,     -- the player's m_iTeamNum in the round
    died_tick integer,           -- the first tick the player was dead (m_lifeState), when they died
    points    blob not null,     -- alive, from the round's freeze end: per point, little-endian i32 tick, f32 x, y, z
                                 -- (the pawn's origin), f32 pitch, yaw (m_angEyeAngles); 16 a second recommended
    primary key (demo, round, player)
) without rowid;

create table demo_tags (         -- optional; a producer's own labels of a demo
    demo  integer not null,      -- demos.demo
    key   text not null,         -- '<namespace>.<name>', e.g. 'hltv.event'
    value text not null
);
create index demo_tags_by_demo on demo_tags (demo);

create table throw_tags (        -- optional; a producer's own labels of a throw
    throw integer not null,      -- throws.throw
    key   text not null,         -- '<namespace>.<name>', e.g. 'nades.lineup'
    value text not null
);
create index throw_tags_by_throw on throw_tags (throw);
```

**Compatibility:**
- A reader accepts any 2.x and ignores the tables and columns it does not know.
- 2.1 (S30) adds the optional `player_tracks` for replays; a 2.0 file is a 2.1 file without it.
- A producer bumps the major version for any change of meaning.
- A reader refuses another major version and names both versions.

**Size:** ≈340 MB per 100 demos, as measured for version 2.0: show-me-the-nades' export of 247 demos (116,480 throws,
7.78 M player samples, twice a second in live rounds) is 830 MB. Player samples are 497 MB of it (64 bytes a row,
most of it their six reals), throws with their flights 177 MB, smoke voxel logs 97 MB, flames 24 MB, tags 22 MB. One
map's facts read in 0.14–0.21 s, 17% faster than from version 1. Version 2.1's `player_tracks` add 7.6 MB a demo at 16 points a second
(45.6 MB for six ESL Pro League demos; their files go from 3.5 to 10–15 MB), and show-me-the-nades' export leaves them out.

**Version 1** (2026-10-26, no outside users) repeated `demo_id`, the player's name and Steam id, and the place as
text in every sample row, with a separate index over them: 2.47 GB for the same 247 demos, 77% of it samples. Version 2
changed only how rows refer to each other; every fact and every column's meaning is as it was. The sandbox refuses a 1.x
file and says to write it again.

## 4. What the sandbox derives

These are pure functions in `crates/model`, with tests. Adapters never compute them.

- **Times.** Seconds are ticks / 64. The airtime runs from the throw's tick to its detonation. The effect lasts from
  the detonation to the expiry; without an expiry tick, the grenade's nominal duration applies.
- **Landing.** The detonation position, or else the flight's last point.
- **Eye.** The origin plus CS2's view height, from 64 standing to 46 crouched by the duck amount
  (`camera::eye_at` today).
- **Initial velocity**, when only the flight is known: from its first two points, corrected for the grenade's
  gravity. This is a proposal. It is checked against `m_vInitialVelocity` on the throws that record both before
  any technique relies on it.
- **Technique.** CS2's release model,
  `v0 = forward(throw_pitch(pitch), yaw) · 675 · lerp(0.3, 1, strength) + 1.25 · v_player`, is solved for two
  values:
  - the throw strength, which gives the click: left 1, middle 0.5, right 0;
  - the player's vertical speed at release, which marks a jump-throw: ≈218.8 u/s, ≈221.9 crouched.

  Movement is stand, walk, run, jump-throw, run-jump-throw, jump or run-jump, from the horizontal speed, the
  walking flag and the ground state. This is the game's model, verified on real demos (median residual 0.004 u/s).
  Its port keeps those tests.
- **Practice command.** `setpos <spot>; setang <pitch> <yaw> 0`, where the spot is the take-off point for a throw
  released in the air and the origin otherwise. It is ready for stationary throws and jump-throws whose click is
  known. Moving throws also need their run start, so they are not ready.
- **Lineups.** Throws are grouped per map and grenade (a molotov and an incendiary are one kind: a fire), taken in
  order (`played_at`, `demo_id`, `tick`, `throw_id`). A throw joins the earliest group whose first throw started within
  48 units of its start and landed within 150 units of its landing; otherwise it starts a group. So the same file
  always gives the same groups. A group's id is its place in that order, so it holds for one file only. Throws outside
  play are not grouped: in a demo with rounds, those of the warm-up and those after their round ended.
- **A lineup in a filtered slice** shows:
  - its throws and distinct demos;
  - its commonest side, start place, landing callout and technique;
  - its teams and players;
  - an example: the earliest throw with the commonest technique.
- **Round time.** Seconds since the freeze ended; a throw is after the plant when `plant_tick ≤ tick`. The filter
  takes a range in seconds, with presets.
- **Landing callout.** Place-labelled samples fill 32-unit cells. A landing takes the place of the nearest cell, with
  vertical distance counted half and nothing beyond 320 units. Coverage grows with every demo.
- **Viewpoints.** The other side's living players in the throw's demo and round, from 1.5 s after the detonation (or
  the release) until the effect's recorded end, or for 5 s without one, nearest the landing first. The result view then
  picks one with a line of sight through the map's collider, as today.
- **Blinded players.** A blind without a recorded position stands at its player's sample nearest the flash's
  detonation, within 0.5 s (by Steam id, else by name).

## 5. The API

```rust
// crates/model: the sandbox's vocabulary and rules, no I/O
pub type Vec3 = [f32; 3];
pub struct Tick(pub i32); // 64 a second
pub struct MapName(pub String); // "de_inferno"
pub struct DemoId(pub String);
pub struct ThrowId(pub String);
pub struct LineupId(pub u32); // the sandbox's own; see §4
pub enum Grenade { Smoke, Flash, He, Molotov, Incendiary, Decoy }
pub enum Side { T, Ct }
pub type Tags = Vec<(String, String)>;

// Facts, as §3 records them
pub struct Demo { pub id: DemoId, pub map: MapName, pub played_at: Option<i64>, pub tags: Tags }
pub struct Round {
    pub demo: DemoId, pub number: u32,
    pub freeze_end: Option<Tick>, pub plant: Option<Tick>, pub end: Option<Tick>, pub winner: Option<Side>,
}
pub struct Throw {
    pub id: ThrowId, pub demo: DemoId, pub round: Option<u32>, pub tick: Tick,
    pub grenade: Grenade, pub side: Side, pub player: Player, pub team: Option<String>, pub place: Option<String>,
    pub start: Vec3, pub velocity: Option<Vec3>, pub thrower: Option<Pawn>,
    pub detonation: Option<(Tick, Vec3)>, pub end: Option<Tick>, pub tags: Tags,
}
pub struct Player { pub name: Option<String>, pub steamid: Option<u64> }
pub struct Pawn {
    pub origin: Vec3, pub takeoff: Option<Vec3>, pub pitch: Option<f32>, pub yaw: Option<f32>,
    pub velocity: Option<Vec3>, pub on_ground: Option<bool>, pub duck: Option<f32>, pub walking: Option<bool>,
}
/// A throw's large parts, loaded on demand.
pub struct Tracks { pub flight: Vec<(Tick, Vec3)>, pub effect: Effect }
/// One variant per kind of grenade, so a smoke cannot carry flames.
pub enum Effect { Smoke(Option<SmokeVoxels>), Fire(Vec<Flame>), Flash(Vec<Blind>), Explosion, Nothing }
pub struct SmokeVoxels { pub first: Tick, pub origin: Vec3, pub bytes: Vec<u8> }
pub struct Flame { pub slot: u32, pub pos: Vec3, pub normal: Option<Vec3>, pub first: Tick, pub last: Tick }
pub struct Blind { pub player: Player, pub side: Option<Side>, pub pos: Option<Vec3>, pub seconds: f32 }
pub struct Sample {
    pub tick: Tick, pub player: Player, pub side: Side, pub origin: Vec3,
    pub pitch: Option<f32>, pub yaw: Option<f32>, pub duck: Option<f32>, pub alive: bool, pub place: Option<String>,
}
/// The game's voxel-log encoding, owned here like the VTEX reader (ported with its tests).
pub mod smoke { pub fn decode(bytes: &[u8]) -> Result<Vec<Frame>, DecodeError>; }

// The port: facts only. An adapter implements this and nothing else.
pub trait ThrowSource: Send + Sync {
    /// Changes whenever the data does, so the app reloads.
    fn revision(&self) -> Result<Revision, SourceError>;
    fn maps(&self) -> Result<Vec<MapName>, SourceError>;
    /// A map's demos, rounds and throws, without flights or effects: what grouping and the filters read.
    fn map(&self, map: &MapName) -> Result<MapFacts, SourceError>;
    fn tracks(&self, throw: &ThrowId) -> Result<Option<Tracks>, SourceError>;
    /// One demo's samples between two ticks: for viewpoints, and later for the replay.
    fn samples(&self, demo: &DemoId, ticks: RangeInclusive<Tick>) -> Result<Vec<Sample>, SourceError>;
    /// A map's place-labelled sample points: for landing callouts.
    fn places(&self, map: &MapName) -> Result<Vec<(Vec3, String)>, SourceError>;
}
pub struct MapFacts { pub demos: Vec<Demo>, pub rounds: Vec<Round>, pub throws: Vec<Throw> }
#[non_exhaustive]
pub enum SourceError {
    Busy,                                                   // a producer is writing; try again
    Missing(PathBuf),
    Incompatible { found: String, supported: &'static str }, // another major version
    Corrupt(String),
    Io(std::io::Error),
}

// The rules (§4): one map's facts, grouped and indexed, the same for every source
pub struct Library { .. }
impl Library {
    pub fn new(facts: MapFacts, places: Vec<(Vec3, String)>) -> Library;
    pub fn lineups(&self, query: &LineupQuery) -> Vec<Lineup>; // most thrown first
    pub fn facets(&self, team: Option<&str>) -> Facets;       // teams, and one team's players (or all)
    pub fn derived(&self, throw: &ThrowId) -> Option<Derived>;
    pub fn viewpoints(&self, throw: &ThrowId, samples: &[Sample]) -> Vec<Viewpoint>;
}
pub struct LineupQuery {
    pub side: Option<Side>, pub grenade: Option<Grenade>, pub team: Option<String>, pub player: Option<String>,
    pub round_seconds: Option<RangeInclusive<f32>>, pub after_plant: Option<bool>, pub tag: Option<(String, String)>,
    pub min_throws: u32, // default 1
    pub limit: u32,      // default 300
}
pub struct Lineup {
    pub id: LineupId, pub grenade: Grenade, pub side: Option<Side>, pub throws: u32, pub demos: u32,
    pub from: Option<String>, pub to: Option<String>, pub technique: Option<Technique>,
    pub teams: Vec<String>, pub players: Vec<String>, pub example: ThrowId,
}
pub struct Technique { pub movement: Movement, pub click: Option<Click>, pub crouched: bool }
pub enum Movement { Stand, Walk, Run, Jumpthrow, RunJumpthrow, Jump, RunJump }
pub enum Click { Left, Middle, Right }
pub struct Derived {
    pub landing: Option<Vec3>, pub airtime: Option<f32>, pub effect_seconds: f32, pub eye: Option<Vec3>,
    pub technique: Option<Technique>, pub practice: Option<Practice>, pub to: Option<String>,
    pub round_seconds: Option<f32>, pub after_plant: Option<bool>,
}
pub struct Practice { pub command: String, pub ready: bool }
pub struct Viewpoint {
    pub tick: Tick, pub player: Player, pub side: Side, pub eye: Vec3,
    pub pitch: Option<f32>, pub yaw: Option<f32>, pub distance: f32,
}
```

The app holds an `Arc<dyn ThrowSource>` and a `Library` per open map. It never sees an adapter's types. Filters run
in memory: about 6,500 throws a map for 100 pro demos, where today every filter change is a DuckDB query.

## 6. Adapters and producers

| Adapter | Crate | Reads | Notes |
|---|---|---|---|
| **The file** (default) | `crates/source-file` | `cs2-throws` files | several files merge into one source; a demo in two files (the same `demo_id`) is taken once |
| **Demos** | `crates/source-demo` | `.dem` files and folders, parsed with awpy 3.1 (MIT, crates.io) | writes one `cs2-throws` file per demo into the sandbox's cache (`<sha256>.sqlite`), then reads it with the file adapter; a demo already there is not parsed again; drag a demo onto the window |
| Fixtures | `nades_model::fixture` | memory | for tests |

The demo adapter makes the sandbox the format's first producer. Its own reader is then never a second-class path.

**Producers outside the sandbox write the same file:**
- **show-me-the-nades:** `nades export --cs2-throws <file>`. It writes facts from its database, with its own labels
  as `nades.*` tags: `nades.lineup`, `nades.organization` and `nades.event`, plus `nades.technique` and
  `nades.land_place` for the parity test (§10). `nades ingest` can run it after each ingest. The sandbox never
  opens that database.
- **Anyone's pipeline:** a reference writer in the repository, `tools/cs2_throws.py`, uses only the standard
  library. It carries the schema and a worked example that converts a parser's output. A parser that reads entity
  properties can fill every table. One that only gives positions and events still enables most features (§2).

## 7. Maps: any CS2 map, from the game's own files

The map list already comes from the packs present, not from a pool. Building a pack, though, reads files that
show-me-the-nades' scripts make:

| The packer reads today | Made by | Replacement |
|---|---|---|
| `~/.cache/nades/maps/<map>.glb`: geometry and materials | `export_map.py`, running Source 2 Viewer's CLI | `nades-pack` runs Source 2 Viewer's CLI (a public tool) itself; later it reads the map's VPK directly |
| `<map>.post.json`, `.lut.bin`, `.sky.json`, `.skybox.json`, `.skybox.glb` | `export_map.py` | read by the packer from the map's own entity lump and materials |
| `~/.cache/nades/cs2/export/<map>/…/default_ents.vents` | `export_map.py`'s decompile | the packer decompiles the entity lump itself |
| `~/.cache/nades/maps/<map>.mesh`: the collider | awpy's published meshes, via `just map-meshes` (only the maps awpy publishes) | the map's own physics in its VPK, built into the pack's Collider section, for any map |
| `~/.cache/nades/cs2/effects/web`: smoke noise, fire textures | `cs2_effects.py` | read by the packer from the game's `pak01` into one shared effects pack |
| `~/.cache/nades/cs2/{game,depot}` | `export_map.py`'s depot download | a configured CS2 folder: an install on Windows or Linux, or a depot download by the sandbox's own setup command (macOS has no CS2 client) |

Workshop maps use the same VPK layout, so they work once the packer reads VPKs directly.

**Status (step 3, decisions S26):** the collider (byte for byte awpy's mesh, from the map's physics), the shared
effects pack, the entity lumps, the sky and post-processing records and the textures now come from the game's files,
with Source 2 Viewer's CLI run by the packer; the §8 settings for packs, CS2's files and the CLI are in. The geometry
export (`<map>.glb`, `<map>.skybox.glb`) is still `export_map.py`'s.

## 8. Configuration

| Setting | Flag | Environment | Config file (`config.toml` in the platform's config folder) | Default |
|---|---|---|---|---|
| data | `--data <file, .dem or folder>`, repeatable | `NADES_SANDBOX_DATA` | `[data] paths` | the demo adapter's cache only |
| packs | `--packs <dir>` | `NADES_SANDBOX_PACKS` | `[assets] packs` | `packs/` in the platform's cache folder |
| CS2's files | `nades-pack --game <dir>` | `NADES_SANDBOX_GAME` | `[assets] game` | none; the setup command asks |
| Source 2 Viewer's CLI | `nades-pack --s2v <path>` | `NADES_SANDBOX_S2V` | `[tools] s2v` | none until the packer reads VPKs directly |

A flag beats the environment, which beats the file, which beats the default. The platform folders come from the
`directories` crate (under `~/Library` on macOS). No default names another project's folder; the existing
`~/.cache/nades/packs` can be named in the config file.

## 9. Agent control (MCP)

`lineups_list` and the scene summary keep their shapes. Their meaning changes:
- `lineup_id` becomes the sandbox's own id (§4), and `throw_id` the producer's;
- `destination` matches the sandbox's landing callout;
- the grenade filter gains incendiary and decoy.

The MCP server reads through the same `Library`, never a database.

## 10. How we prove it is generic

- **Two independent producers write the format,** the sandbox's demo adapter and show-me-the-nades' export, and
  both pass `nades-sandbox check`.
- **Both files agree on the same demos:** the same groups, techniques, callouts and viewpoints. This local test
  checks the ported rules against show-me-the-nades' verified results without depending on it.
- **A third producer runs in CI:** a fixture file written by `tools/cs2_throws.py`, which is neither of ours.
- **A build test** checks that the workspace has no path dependency and nothing names `../show-me-the-nades`.
  Defaults under `~/.cache/nades` (packs, the collider, effects) go with step 3 (§7).

## 11. Decisions for the review

1. **The contract.** Recommended: facts, with the sandbox's own derivations and optional tags. Revision 1's "facts
   with the producer's labels" let one producer define what every filter means.
2. **Grouping.** Recommended: earliest fit within 48 and 150 units. It gives show-me-the-nades' groups on the same
   throws in the same order. The alternative, order-independent clustering, chains neighbours into long groups.
3. **The demo parser.** Recommended: awpy (Rust, MIT), the parser whose output show-me-the-nades verified. Its
   alternative, demoparser2's Rust core, failed to build here: its build script clones GameTracking-CS2 at HEAD.
4. **Tags.** Recommended: shown and filterable now. Grouping by a tag (a producer's named lineups) can come later.
5. **The container.** Recommended: SQLite. JSON is bigger and slower to filter; Parquet's reader is heavy.
6. **Map preparation.** Recommended: Source 2 Viewer's CLI as a configured tool first, then reading the VPK
   directly. That order makes every official map work soon and workshop maps later.
7. **Order of work.** Recommended:
   1. the core, the file adapter and show-me-the-nades' export, which together remove the path dependency;
   2. the demo adapter;
   3. maps from the game's files.
