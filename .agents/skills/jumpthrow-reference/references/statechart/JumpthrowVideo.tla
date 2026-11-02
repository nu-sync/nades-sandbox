---- MODULE JumpthrowVideo ----
\* One jumpthrow.pro lineup video, as observed on 2026-11-02 (references/findings.md).
\* Every change of shot before the release is a hard cut; times are in template.json.

VARIABLES
    phase,    \* "leftover" "spot" "aim" "black" "aimpoint" "lineup" "throw" "chase" "aftermath"
    view,     \* "eye" (first person, normal FOV, arms), "zoom" (narrow FOV, no arms), "dark", "grenade" (locked to it)
    grenade,  \* "holstered" "held" "flying" "resting"
    effect    \* "none" "going off"

vars == <<phase, view, grenade, effect>>

Init == phase = "leftover" /\ view = "eye" /\ grenade = "holstered" /\ effect = "none"

Teleport  == phase = "leftover" /\ phase' = "spot" /\ grenade' = "held" /\ UNCHANGED <<view, effect>>
CutToAim  == phase = "spot" /\ phase' = "aim" /\ UNCHANGED <<view, grenade, effect>>
CutToDark == phase = "aim" /\ phase' = "black" /\ view' = "dark" /\ UNCHANGED <<grenade, effect>>
CutToZoom == phase = "black" /\ phase' = "aimpoint" /\ view' = "zoom" /\ UNCHANGED <<grenade, effect>>
CutBack   == phase = "aimpoint" /\ phase' = "lineup" /\ view' = "eye" /\ UNCHANGED <<grenade, effect>>
Throw     == phase = "lineup" /\ phase' = "throw" /\ UNCHANGED <<view, grenade, effect>>
Release   == phase = "throw" /\ phase' = "chase" /\ view' = "grenade" /\ grenade' = "flying"
             /\ UNCHANGED effect

\* A smoke lands and rests before it goes off; a flash goes off in the air and falls after.
Settle    == phase \in {"chase", "aftermath"} /\ grenade = "flying" /\ grenade' = "resting"
             /\ UNCHANGED <<phase, view, effect>>
GoOff     == phase = "chase" /\ phase' = "aftermath" /\ effect' = "going off" /\ UNCHANGED <<view, grenade>>

\* The MP4 loops: the last frame is followed by the first.
Loop      == phase = "aftermath" /\ phase' = "leftover" /\ view' = "eye" /\ grenade' = "holstered"
             /\ effect' = "none"

Next == Teleport \/ CutToAim \/ CutToDark \/ CutToZoom \/ CutBack \/ Throw \/ Release
        \/ Settle \/ GoOff \/ Loop

\* Rules a viewer expects.
ZoomOnlyWithGrenadeInHand == view = "zoom" => grenade = "held"
GrenadeCameraOnlyAfterRelease == view = "grenade" => grenade \in {"flying", "resting"}
EffectSeenFromGrenadeCamera == effect = "going off" => view = "grenade"
\* The camera never sits inside the map. A camera locked to a grenade at rest sits in the floor or a wall
\* (the clipping seen after every landing).
CameraOutsideMap == ~(view = "grenade" /\ grenade = "resting")
====
