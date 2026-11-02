# What jumpthrow.pro does (observed 2026-11-02)

Sampled frame by frame in the built-in browser: two Ancient smokes from Nov 2024 (Cloud9 interz
`0197a968-ba67-7410-8f7b-4e5423a35df3`, MIBR saffee `0197a96c-357f-779c-a147-b02fd57e4fd9`) and a Mirage flash from
May 2026 (The MongolZ cobrazera `019e5533-8df6-7449-b2d0-3af57e4bb18c`). All three share one template, so it has not
changed in 18 months.

## The page

- `<video autoplay loop muted>` with an MP4 at `media.jumpthrow.pro/nades/<uuid>/video.mp4?token=…&expires=…`,
  1920×1080, 21–27 s, a `jumpthrow.pro` watermark low in the centre.
- Stills `lineup.webp` and `lineup_zoomed.webp` (1138×640): the lineup at normal FOV and zoomed on the aim point.
- Fields: Map, Type (with side, `Smoke T`), Buttons (`W`, `JUMP`), Airtime (`5.9s`, the release to the detonation),
  Console (`setpos x y z;` `setang pitch yaw;`), first seen date, team and player, a radar
  (`assets/radars/<map>.svg`) with FROM/TO, three similar nades.

## The video template

Ancient smoke (26.3 s, airtime 5.9 s); the flash matches within a few tenths.

| Time | Phase | Camera | Transition |
|---|---|---|---|
| 0.0–0.1 s | leftover | the previous position | cut (the `setpos` teleport) |
| 0.1–2.1 s | spot | first person, level, grenade being drawn | static |
| 2.1 s | aim | snaps to the `setang` (pitch −34°) | **hard cut**, no turn |
| 3.2 s | — | one mostly black frame as the zoom loads | cut |
| 3.3–6.4 s | aim point | narrow FOV on the aim point, no arms, crosshair on it | **hard cut** in and out |
| 6.5–9.6 s | lineup again | normal FOV, arms holding the grenade | static, about 3 s |
| 9.7–10.2 s | throw | the game's throw animation; the jump lifts the view | real time |
| 10.3 s | chase | **cut** to a camera just behind the grenade, looking along its flight | — |
| 10.3–16.5 s | chase | rides the arc: apex about 11.2–12 s looking down over the map, down the stairs | real time, smooth, mostly level |
| ~16.5 s | detonation | release + airtime | — |
| 16.5–26 s | aftermath | still attached to the grenade: tumbles, clips into walls and under the floor (blue void, inside-out rooms), the smoke spreading behind; ends lying on its side, rolled 90° | an artifact, not a shot |

The flash's chase snaps heading at each bounce; its grenade is visible just ahead of the camera, and a faint
trajectory line shows in some frames.

## How they make it (inferred)

A scripted CS2 session: teleport with the console line, show the view, change FOV for the zoom, throw for real, then
a spectator camera locked to the grenade entity until the clip ends. Every change is a cut; none is an eased move. The
weak part is the end, where the grenade camera never lets go.

## Against ours (`crates/sandbox/src/lineup.rs` `Sequence`)

| | jumpthrow | nades-sandbox |
|---|---|---|
| Player | first-person arms, no body | no arms, no body |
| Spot → aim | hard cut, holds about 2 s each | 2.0 s level, then turns to the setang in 1.0 s and holds 1.0 s |
| Aim-point zoom | hard cut to a narrow FOV, held about 3 s | zooms to 28° in 1.0 s at an even rate, holds 2.0 s |
| Back out | hard cut, holds about 3 s with the grenade in hand | zooms out in 0.8 s, holds 2.0 s |
| Throw | real animation, jump visible | a jump-throw's eye rises to the release point; release at 9.8 s |
| Chase | grenade-locked, real time | leaves the eye over 0.8 s keeping the jump's speed; smoothed chase 70→150 units behind, real time |
| After the burst | camera stuck to the grenade, clips | pull-out, then one turn round the effect |
| Length | 21–27 s | about 26 s for a smoke |

Since 2026-11-02 ours keeps their phases and holds but moves where they cut, with no jump in place, speed or zoom
from one frame to the next. Left: arms. Keep our pull-out and turn; they are better than their ending.
