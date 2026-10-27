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
