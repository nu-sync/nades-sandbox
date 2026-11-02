const first: Record<string, string> = {
  leftover: "Last frame of the previous spot",
  spot: "At the spot, level, drawing the grenade",
  aim: "Looking along the setang",
  black: "One mostly black frame",
  aimpoint: "Zoomed on the aim point, no arms",
  lineup: "Lineup again, grenade in hand",
  throw: "Throw animation and jump",
};

export const title = "A jumpthrow.pro lineup video";

export const state = (v: any) => {
  if (first[v.phase]) return first[v.phase];
  const rest = v.grenade === "resting";
  if (v.phase === "chase") return rest ? "Riding the grenade, at rest, not yet gone off" : "Riding the grenade in flight";
  return rest ? "Effect going off, camera on the resting grenade" : "Effect going off, grenade still falling";
};

export const group = (v: any) =>
  v.phase === "chase" ? "Flight" : v.phase === "aftermath" ? "Effect" : "First person";

export const colors = { "First person": "blue", Flight: "teal", Effect: "orange" };

const labels: Record<string, string> = {
  Teleport: "0.1 s: cut to the lineup spot",
  CutToAim: "2.1 s: cut to the aim",
  CutToDark: "3.2 s: cut to black",
  CutToZoom: "3.3 s: cut to the zoom",
  CutBack: "6.5 s: cut back out",
  Throw: "9.7 s: the throw starts",
  Release: "10.3 s: cut to the grenade camera",
  Settle: "the grenade comes to rest",
  GoOff: "release + airtime: it goes off",
  Loop: "the clip ends and loops",
};

export const step = (action: string) => labels[action] ?? action;
