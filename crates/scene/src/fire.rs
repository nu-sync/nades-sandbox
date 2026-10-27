//! A molotov's or incendiary's fire as CS2 draws it, from the flames the demo networked: a port of show-me-the-nades'
//! `web/app/src/scene/fire.ts` (the particles), with the colours and styles of `render/fire.ts` (what the renderer
//! draws them with).
//!
//! CS2's particle systems for it (`particles/inferno_fx/molotov_groundfire` and `incendiary_groundfire`, read from the
//! game's files by show-me-the-nades, its D24) are all fed by the fire's flame positions: a fill of flames from every
//! burning point with dimmer licks among them, flames along the fire's outline whose faint edges run through blue and
//! purple, a pillar of flame where the grenade broke, a fireball when the bottle breaks, a glowing film on the ground
//! and a scorch, and when it goes out, glowing flecks and thin steam. Lifetimes, sizes and colours are the
//! definitions' own, in seconds and Hammer units.
//!
//! Every particle is a pure function of time: an emitter's slots each emit once per period at their own phase, and a
//! particle's random draws hash its emitter, slot and cycle. Scrubbing the time shows the same fire at the same moment,
//! backwards too.

use std::f64::consts::PI;

#[expect(clippy::approx_constant, reason = "Preserve the web fire.ts ember phase exactly.")]
const EMBER_PHASE: f64 = 6.28;

use crate::camera::{hypot2, hypot3};

pub type V3 = [f64; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireKind {
    Molotov,
    Incendiary,
}

impl FireKind {
    pub fn of(grenade: nades_model::Grenade) -> Self {
        if grenade == nades_model::Grenade::Incendiary { FireKind::Incendiary } else { FireKind::Molotov }
    }
}

/// A flame the fire spread to, in seconds after detonation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flame {
    pub pos: V3,
    /// The surface it burns on.
    pub normal: V3,
    pub start: f64,
    pub end: f64,
}

/// Which of CS2's systems a sprite stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Fill,
    Lick,
    Outline,
    Pillar,
    Fireball,
    Flash,
    Ember,
    Steam,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub layer: Layer,
    /// The ellipsoid's centre.
    pub pos: V3,
    /// Its half-width.
    pub radius: f64,
    /// Its half-height: a flame reaches about 0.75 of it above its centre.
    pub height: f64,
    pub rotation: f64,
    pub alpha: f64,
    /// 0 at birth, 1 at death: it plays the sprite's flipbook.
    pub age: f64,
    pub seed: f64,
    pub tint: V3,
}

/// The ground under one flame: the burning film's light and the scorch it leaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub pos: V3,
    pub normal: V3,
    pub radius: f64,
    pub glow: f64,
    pub scorch: f64,
    pub seed: f64,
}

/// A point on the fire's edge, and while it is one: until a flame that covers it catches.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Edge {
    pos: V3,
    normal: V3,
    start: f64,
    end: f64,
}

/// A heap within the fire: its middle on the ground, how wide, how tall (of PEAK), and when it breathes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Peak {
    at: V3,
    radius: f64,
    strength: f64,
    phase: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fire {
    pub kind: FireKind,
    pub flames: Vec<Flame>,
    /// Where the grenade broke: the first flame.
    origin: usize,
    /// When it went out. CS2 puts out every flame at once, extinguished or not.
    pub end: f64,
    edge: Vec<Edge>,
    pub centre: V3,
    /// How far from the centre the fire's fill reaches, along the ground.
    reach: f64,
    peaks: Vec<Peak>,
}

const TICK: f64 = 1.0 / 64.0;
/// Everything still burning fades out this fast once the fire goes out.
const END_CAP: f64 = 0.25;
/// Flecks and steam are shown for this long after the fire; the scrubber runs this far past it.
pub const AFTERMATH: f64 = 2.5;
/// When the break's fireball has gone and the fire has settled onto the ground.
const SETTLED: f64 = 0.7;
const FILL_SPREAD: f64 = 24.0;
const EDGE_RADIUS: f64 = 38.0;
const EDGE_POINTS: usize = 14;
const SLOTS_FILL: usize = 7;
const SLOTS_LICK: usize = 4;
const SLOTS_OUTLINE: usize = 2;
const SLOTS_PILLAR: usize = 10;
const FIREBALLS: usize = 7;
const FLASHES: usize = 4;
const EMBERS: usize = 4;
const STEAM: usize = 2;
const TOP: f64 = 0.75;
const FOOT: f64 = 0.35;
const CREAM: V3 = [244.0 / 255.0, 237.0 / 255.0, 208.0 / 255.0];
const WHITE: V3 = [1.0, 1.0, 1.0];
// emitter ids keep every system's random draws apart
const FILL: i64 = 1_000;
const LICK: i64 = 2_000;
const OUTLINE: i64 = 3_000;
const PILLAR: i64 = 50_000;
const BURST: i64 = 60_000;
const AFTER: i64 = 70_000;
const PEAKS: i64 = 80_000;
/// How far up a flame of the carpet reaches, in units off the ground, as it is born and as it dies.
pub const CARPET: [f64; 2] = [5.0, 12.0];
/// How much higher than CARPET the tongues reach where the fire heaps highest.
pub const PEAK: f64 = 9.0;

fn burns(kind: FireKind) -> f64 {
    match kind {
        FireKind::Molotov => 7.031_25,
        FireKind::Incendiary => 5.5,
    }
}

/// A fire's flames as the facts record them, in seconds after `detonate_tick`.
pub fn flames_of(rows: &[nades_model::Flame], detonate_tick: i32) -> Vec<Flame> {
    rows.iter()
        .map(|f| {
            let n = f.normal.map_or([0.0, 0.0, 1.0], |n| n.map(f64::from));
            Flame {
                pos: f.pos.map(f64::from),
                normal: normalize(n),
                start: f64::from(f.first.0 - detonate_tick) * TICK,
                end: f64::from(f.last.0 + 1 - detonate_tick) * TICK,
            }
        })
        .collect()
}

pub fn build_fire(flames: Vec<Flame>, kind: FireKind) -> Option<Fire> {
    let mut origin: Option<usize> = None;
    for (i, f) in flames.iter().enumerate() {
        if origin.is_none_or(|o| f.start < flames[o].start) {
            origin = Some(i);
        }
    }
    let origin = origin?;
    let mut edge = Vec::new();
    for (i, f) in flames.iter().enumerate() {
        let (u, v) = basis(f.normal);
        for k in 0..EDGE_POINTS {
            let a = (2.0 * PI * (k as f64 + hash(OUTLINE, i as i64, k as i64, 9))) / EDGE_POINTS as f64;
            let pos = add3(f.pos, scale(u, a.cos() * EDGE_RADIUS), scale(v, a.sin() * EDGE_RADIUS));
            // covered once a neighbour catches near enough that the point lies inside the fire, not on its rim
            let mut end = f.end;
            for (j, g) in flames.iter().enumerate() {
                if j != i && distance(pos, g.pos) < EDGE_RADIUS * 1.3 && g.start < end {
                    end = f.start.max(g.start);
                }
            }
            if end > f.start {
                edge.push(Edge { pos, normal: f.normal, start: f.start, end });
            }
        }
    }
    let n = flames.len() as f64;
    let centre = flames.iter().fold([0.0; 3], |c, f| [c[0] + f.pos[0] / n, c[1] + f.pos[1] / n, c[2] + f.pos[2] / n]);
    let reach = flames.iter().map(|f| hypot2(f.pos[0] - centre[0], f.pos[1] - centre[1])).fold(f64::NEG_INFINITY, f64::max) + FILL_SPREAD;
    // the fire's own draws: the same fire heaps the same way every time it is shown
    let seed = js_round(centre[0]) * 31.0 + js_round(centre[1]);
    let draw = |k: i64, c: i64| hash(PEAKS, k, seed as i64, c);
    let count = 2 + (draw(0, 0) * 3.0).floor() as usize;
    let peaks = (0..count)
        .map(|k| {
            let k = k as i64;
            let f = &flames[((draw(k + 1, 1) * n).floor() as usize) % flames.len()];
            let (u, v) = basis(f.normal);
            Peak { at: on_disc(f.pos, u, v, FILL_SPREAD * draw(k + 1, 2), draw(k + 1, 3)), radius: lerp(35.0, 70.0, draw(k + 1, 4)), strength: lerp(0.55, 0.95, draw(k + 1, 5)), phase: draw(k + 1, 6) }
        })
        .collect();
    let end = flames.iter().map(|f| f.end).fold(f64::NEG_INFINITY, f64::max);
    Some(Fire { kind, flames, origin, end, edge, centre, reach, peaks })
}

/// A carpet flame's top at normalized age n: it climbs through CARPET as it burns, a little higher or lower each.
fn climb(n: f64, r: f64) -> f64 {
    lerp(CARPET[0], CARPET[1], n) * lerp(0.94, 1.06, r)
}

impl Fire {
    fn origin(&self) -> &Flame {
        &self.flames[self.origin]
    }

    /// What a tongue standing on `ground`, born at `born`, adds to its climb: up to PEAK where the fire heaps highest.
    fn rise(&self, ground: V3, born: f64, n: f64, spike: f64) -> f64 {
        let heap = |at: V3, radius: f64| {
            let out = clamp01(hypot2(ground[0] - at[0], ground[1] - at[1]) / radius);
            (1.0 - out * out).powf(1.5)
        };
        let mut shape = 0.75 * heap(self.centre, self.reach);
        for p in &self.peaks {
            let breath = lerp(0.5, 1.0, 0.5 + 0.5 * (2.0 * PI * (born / lerp(2.0, 3.0, p.phase) + p.phase)).sin());
            shape += p.strength * breath * heap(p.at, p.radius);
        }
        PEAK * shape.min(1.0) * lerp(0.3, 1.0, spike.powf(1.8)) * lerp(0.35, 1.0, smoothstep(0.0, 0.7, n))
    }

    /// The most sprites alive at once, which the renderer sizes its buffers by.
    pub fn sprite_bound(&self) -> usize {
        2 * (self.flames.len() * (SLOTS_FILL + SLOTS_LICK) + self.edge.len() * SLOTS_OUTLINE + 2 * SLOTS_PILLAR) + FIREBALLS + FLASHES + self.flames.len() * (EMBERS + STEAM)
    }

    /// How much of the fire is up: 1 until it goes out, then 0 within END_CAP.
    fn cap_at(&self, t: f64) -> f64 {
        1.0 - clamp01((t - self.end) / END_CAP)
    }

    /// The fire's age in the definitions' seconds, where curves are written for a 7-second molotov.
    fn authored(&self, t: f64) -> f64 {
        (t * burns(FireKind::Molotov)) / burns(self.kind)
    }

    /// Every sprite alive `t` seconds after detonation.
    pub fn sprites_at(&self, t: f64) -> Vec<Sprite> {
        let mut out = Vec::new();
        if t < 0.0 {
            return out;
        }
        let cap = self.cap_at(t);
        if cap > 0.0 {
            for (i, f) in self.flames.iter().enumerate() {
                self.fill(f, i as i64, t, cap, &mut out);
                self.lick(f, i as i64, t, cap, &mut out);
            }
            for (i, e) in self.edge.iter().enumerate() {
                outline(e, i as i64, t, cap, &mut out);
            }
            self.pillar(t, cap, &mut out);
        }
        self.burst(t, &mut out);
        self.aftermath(t, &mut out);
        out
    }

    /// main_center: from every burning point, flames living 1–2 s that grow from 0.6 to 1.2 of their size.
    fn fill(&self, f: &Flame, i: i64, t: f64, cap: f64, out: &mut Vec<Sprite>) {
        let near = lerp(1.1, 0.5, clamp01((distance(f.pos, self.origin().pos) - 25.0) / 225.0));
        let (u, v) = basis(f.normal);
        emitted(FILL + i, f.start, f.end, SLOTS_FILL, 2.0, t, |slot, cycle, age, _| {
            let r = |c: i64| hash(FILL + i, slot, cycle, c);
            let life = 1.0 + r(1);
            if age >= life {
                return;
            }
            let n = age / life;
            let ground = on_disc(f.pos, u, v, FILL_SPREAD * r(2).sqrt(), r(3));
            let up = self.rise(ground, t - age, n, r(10));
            let (pos, height) = standing(ground, (climb(n, r(9)) + up) * cap);
            out.push(Sprite {
                layer: Layer::Fill,
                pos,
                height,
                radius: (lerp(36.0, 55.0, r(4)) * near).clamp(10.0, 55.0) * 0.75 * lerp(0.6, 1.2, smoothstep(0.0, 1.0, n)) * lerp(0.25, 1.0, cap) * slim(up),
                rotation: lerp(-60.0, 90.0, r(5)) * (PI / 180.0),
                alpha: lerp(0.39, 0.59, r(6)) * clamp01(age / 0.12) * clamp01((life - age) / 0.5) * cap,
                age: n,
                seed: r(7),
                tint: mix(WHITE, [239.0 / 255.0, 216.0 / 255.0, 135.0 / 255.0], r(8)),
            });
        });
    }

    /// main_fancy: dimmer flames living about a second that burn out to black.
    fn lick(&self, f: &Flame, i: i64, t: f64, cap: f64, out: &mut Vec<Sprite>) {
        let near = lerp(1.2, 0.5, clamp01((distance(f.pos, self.origin().pos) - 25.0) / 175.0));
        let late = curve(&[(6.0, 1.0), (6.42, 0.92), (7.1, 0.4), (8.0, 0.2)], self.authored(t));
        let (u, v) = basis(f.normal);
        emitted(LICK + i, f.start, f.end, SLOTS_LICK, 1.1, t, |slot, cycle, age, _| {
            let r = |c: i64| hash(LICK + i, slot, cycle, c);
            let life = 1.0 + 0.1 * r(1);
            if age >= life {
                return;
            }
            let n = age / life;
            let ground = on_disc(f.pos, u, v, FILL_SPREAD * r(2).sqrt(), r(3));
            let up = self.rise(ground, t - age, n, r(10));
            let (pos, height) = standing(ground, (climb(n, r(9)) + up) * late * cap);
            out.push(Sprite {
                layer: Layer::Lick,
                pos,
                height,
                radius: (lerp(25.0, 55.0, r(4)) * near).clamp(20.0, 65.0) * lerp(0.5, 1.0, n.powf(0.75)) * late * lerp(0.25, 1.0, cap) * slim(up),
                rotation: lerp(-60.0, 90.0, r(5)) * (PI / 180.0),
                alpha: clamp01(age / 0.1) * (1.0 - smoothstep(0.85, 1.0, n)) * cap,
                age: n,
                seed: r(7),
                tint: [117.0 / 255.0, 118.0 / 255.0, 112.0 / 255.0],
            });
        });
    }

    /// The pillar where it broke: bright sprites, 10–45 units wide, gone 7–8 s in.
    fn pillar(&self, t: f64, cap: f64, out: &mut Vec<Sprite>) {
        let o = *self.origin();
        let (u, v) = basis(o.normal);
        let gone = 1.0 - clamp01(self.authored(t) - 7.0);
        for p in &PILLARS {
            emitted(p.id, o.start, self.end, SLOTS_PILLAR, 1.5, t, |slot, cycle, age, birth| {
                let r = |c: i64| hash(p.id, slot, cycle, c);
                let at = self.authored(birth - o.start);
                if r(1) > lerp(p.rate[0], p.rate[1], clamp01(at / 10.0)) / p.rate[0] {
                    return;
                }
                let life = (p.life)(at);
                if age >= life {
                    return;
                }
                let n = age / life;
                let grow = (p.grow)(n);
                let ground = on_disc(o.pos, u, v, (p.disc)(at) * r(2).sqrt(), r(3));
                let settled = climb(n, r(5)) + self.rise(ground, t - age, n, r(10));
                let (pos, height) = standing(ground, pillar_top(t - o.start, r(5), settled) * cap);
                out.push(Sprite {
                    layer: Layer::Pillar,
                    pos,
                    height,
                    radius: (p.size)(at) * lerp(0.35, 1.0, r(4)) * grow * if t - o.start < SETTLED { 1.0 } else { 0.7 } * lerp(0.25, 1.0, cap),
                    rotation: lerp(-45.0, 45.0, r(6)) * (PI / 180.0),
                    alpha: lerp(p.alpha[0], p.alpha[1], r(7)) * (p.fade)(n) * gone * cap,
                    age: n,
                    seed: r(8),
                    tint: mix(CREAM, p.colour, r(9)),
                });
            });
        }
    }

    /// molotov_explosion: fireballs shot up from where it broke, and flashes that a lift carries up.
    fn burst(&self, t: f64, out: &mut Vec<Sprite>) {
        let o = *self.origin();
        let (u, v) = basis(o.normal);
        for k in 0..FIREBALLS {
            let r = |c: i64| hash(BURST, k as i64, 0, c);
            let age = t - o.start - if k < 5 { 0.0 } else { (k as f64 - 4.0) * 0.05 };
            let life = lerp(0.4, 0.55, r(1).powf(1.5));
            if age < 0.0 || age >= life {
                continue;
            }
            let n = age / life;
            let lean = 0.5 * r(2);
            let dir = normalize(add3(o.normal, scale(u, (2.0 * PI * r(3)).cos() * lean), scale(v, (2.0 * PI * r(3)).sin() * lean)));
            let radius = lerp(3.0, 5.0, r(5)) * (1.0 + 8.0 * (1.0 - (1.0 - n).powi(3)));
            out.push(Sprite {
                layer: Layer::Fireball,
                pos: add(o.pos, scale(dir, 6.0 + lerp(45.0, 65.0, r(4)) * age)),
                radius,
                height: radius,
                rotation: 2.0 * PI * r(6),
                alpha: 1.0 - smoothstep(0.7, 1.0, n),
                age: n,
                seed: r(7),
                tint: CREAM,
            });
        }
        for k in 0..FLASHES {
            let r = |c: i64| hash(BURST + 1, k as i64, 0, c);
            let age = t - o.start;
            let life = lerp(0.2, 0.35, r(1));
            if age < 0.0 || age >= life {
                continue;
            }
            let n = age / life;
            let radius = lerp(20.0, 40.0, r(4)) * (1.0 + 1.5 * (1.0 - (1.0 - n).powi(2)));
            out.push(Sprite {
                layer: Layer::Flash,
                pos: [o.pos[0] + lerp(-8.0, 8.0, r(2)), o.pos[1] + lerp(-8.0, 8.0, r(3)), o.pos[2] + 6.0 + 40.0 * age],
                radius,
                height: radius,
                rotation: lerp(-45.0, 45.0, r(5)) * (PI / 180.0),
                alpha: 1.0 - smoothstep(0.5, 1.0, n),
                age: n,
                seed: r(6),
                tint: mix([243.0 / 255.0, 229.0 / 255.0, 195.0 / 255.0], [218.0 / 255.0, 146.0 / 255.0, 69.0 / 255.0], r(7)),
            });
        }
    }

    /// molotov_groundfire_remnant: once it is out, glowing flecks on the ground and steam rising 5 units a second.
    fn aftermath(&self, t: f64, out: &mut Vec<Sprite>) {
        let since = t - self.end;
        if since < 0.0 {
            return;
        }
        for (i, f) in self.flames.iter().enumerate() {
            let i = i as i64;
            let (u, v) = basis(f.normal);
            for k in 0..EMBERS {
                let r = |c: i64| hash(AFTER + i, k as i64, 0, c);
                let life = 3.0 + 5.0 * r(1);
                if since >= life {
                    continue;
                }
                let radius = lerp(1.4, 3.0, r(4));
                out.push(Sprite {
                    layer: Layer::Ember,
                    pos: add(on_disc(f.pos, u, v, 34.0 * r(2).sqrt(), r(3)), scale(f.normal, 1.5)),
                    radius,
                    height: radius,
                    rotation: 0.0,
                    alpha: (1.0 - since / life).powf(1.5) * (0.6 + 0.4 * (t * lerp(6.0, 13.0, r(5)) + EMBER_PHASE * r(6)).sin()),
                    age: since / life,
                    seed: r(7),
                    tint: mix([244.0 / 255.0, 155.0 / 255.0, 0.0], [157.0 / 255.0, 58.0 / 255.0, 4.0 / 255.0], r(8)),
                });
            }
            for k in 0..STEAM {
                let r = |c: i64| hash(AFTER + 500 + i, k as i64, 0, c);
                let life = 3.0 + 3.0 * r(1);
                if since >= life {
                    continue;
                }
                let n = since / life;
                let at = on_disc(f.pos, u, v, 20.0 * r(2).sqrt(), r(3));
                let radius = lerp(14.0, 22.0, r(6)) * (1.0 + 1.2 * n);
                out.push(Sprite {
                    layer: Layer::Steam,
                    pos: [at[0] + 6.0 * since * lerp(-1.0, 1.0, r(4)), at[1] + 6.0 * since * lerp(-1.0, 1.0, r(5)), at[2] + 10.0 + 5.0 * since],
                    radius,
                    height: radius * 1.2,
                    rotation: 2.0 * PI * r(7),
                    alpha: 0.32 * smoothstep(0.0, 0.12, n) * (1.0 - n).powf(1.3),
                    age: n,
                    seed: r(8),
                    tint: [117.0 / 255.0, 118.0 / 255.0, 112.0 / 255.0],
                });
            }
        }
    }

    /// The ground under each flame: a dim burning film fading out over the fire's last two seconds, and a scorch over
    /// 10 s that is still there once the fire is out.
    pub fn patches_at(&self, t: f64) -> Vec<Patch> {
        let late = 1.0 - clamp01((self.authored(t) - 5.0) / 2.0);
        let cap = self.cap_at(t);
        self.flames
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let since = t - f.start;
                (since >= 0.0).then(|| Patch {
                    pos: f.pos,
                    normal: f.normal,
                    radius: lerp(56.0, 72.0, hash(AFTER + 900, i as i64, 0, 0)),
                    glow: smoothstep(0.0, 0.15, since) * late * cap,
                    scorch: curve(&[(0.0, 0.11), (0.07, 0.87), (0.51, 1.0), (1.0, 0.0)], since / 10.0),
                    seed: hash(AFTER + 900, i as i64, 0, 1),
                })
            })
            .collect()
    }

    /// The fire's light, relative to one burning flame: the square root of how many burn, flickering, plus the
    /// break's flash.
    pub fn light_at(&self, t: f64) -> f64 {
        if t < 0.0 {
            return 0.0;
        }
        let burning = self.flames.iter().filter(|f| f.start <= t && t < f.end).count() as f64;
        let flicker = 0.86 + 0.09 * (t * 13.1).sin() + 0.05 * (t * 31.7 + 1.3).sin();
        let flash = if t < 0.4 { 2.5 * (1.0 - t / 0.4).powi(2) } else { 0.0 };
        burning.sqrt() * flicker * self.cap_at(t) + flash
    }

    /// When a result shot shows it: once nearly every flame has caught and grown in, no later than 2.5 s, and before
    /// it goes out when a smoke puts it out early.
    pub fn fullest_at(&self) -> f64 {
        let grown = |t: f64| self.flames.iter().fold(0.0, |s, f| s + if t < f.end { clamp01((t - f.start) / 0.6) } else { 0.0 });
        let mut best = 0.0f64;
        let mut t = 0.0;
        while t <= self.end {
            best = best.max(grown(t));
            t += 0.05;
        }
        let mut t = 0.0;
        while t < self.end && grown(t) < 0.95 * best {
            t += 0.05;
        }
        t.min(2.5).min(self.end - 0.05).max(0.0)
    }

    /// Seconds the view shows: until the flecks and steam have gone.
    pub fn lasts(&self) -> f64 {
        self.end + AFTERMATH
    }
}

/// outline: narrow flames along the edge, black as they appear and orange as they die, blended over the scene.
fn outline(e: &Edge, i: i64, t: f64, cap: f64, out: &mut Vec<Sprite>) {
    emitted(OUTLINE + i, e.start, e.end, SLOTS_OUTLINE, 1.2, t, |slot, cycle, age, _| {
        let r = |c: i64| hash(OUTLINE + i, slot, cycle, c);
        let life = 0.5 + 0.7 * r(1).powf(1.4);
        if age >= life {
            return;
        }
        let n = age / life;
        let ground = [e.pos[0] + lerp(-6.0, 6.0, r(2)), e.pos[1] + lerp(-6.0, 6.0, r(3)), e.pos[2]];
        let (pos, height) = standing(ground, climb(n, r(8)) * cap);
        out.push(Sprite {
            layer: Layer::Outline,
            pos,
            height,
            radius: lerp(20.0, 30.0, r(4)) * lerp(0.5, 1.0, n.powf(0.85)) * 0.3 * lerp(0.25, 1.0, cap),
            rotation: lerp(-25.0, 25.0, r(5)) * (PI / 180.0),
            alpha: (2.0 * clamp01(age / 0.08) * (1.0 - n).powf(0.6)).min(1.0) * clamp01(n / 0.1) * cap,
            age: n,
            seed: r(7),
            tint: if n < 0.6 { WHITE } else { mix(WHITE, [1.0, 140.0 / 255.0, 0.0], (n - 0.6) / 0.4) },
        });
    });
}

struct PillarSystem {
    id: i64,
    rate: [f64; 2],
    life: fn(f64) -> f64,
    disc: fn(f64) -> f64,
    size: fn(f64) -> f64,
    grow: fn(f64) -> f64,
    alpha: [f64; 2],
    fade: fn(f64) -> f64,
    colour: V3,
}

/// child_flame01a and child_flame03a: emitting 10 a second at first and 5 or 2 by 10 s, born in a widening disc.
const PILLARS: [PillarSystem; 2] = [
    PillarSystem {
        id: PILLAR,
        rate: [10.0, 5.0],
        life: |at| lerp(0.65, 1.5, clamp01(at / 0.5)),
        disc: |at| lerp(8.0, 50.0, clamp01((at - 3.0) / 4.0)),
        size: |at| lerp(28.0, 45.0, clamp01(at / 0.65)),
        grow: |n| lerp(1.5, 0.75, n.powf(0.8)),
        alpha: [0.47, 0.59],
        fade: |n| 1.0 - smoothstep(0.75, 1.0, n),
        colour: [235.0 / 255.0, 172.0 / 255.0, 95.0 / 255.0],
    },
    PillarSystem {
        id: PILLAR + 100,
        rate: [10.0, 2.0],
        life: |at| lerp(0.75, 1.5, clamp01(at)),
        disc: |at| lerp(5.0, 50.0, clamp01((at - 2.0) / 6.0)),
        size: |at| lerp(24.0, 55.0, clamp01(at / 0.65)),
        grow: |n| lerp(0.8, 1.25, n.powf(0.75)),
        alpha: [0.78, 0.98],
        fade: |n| smoothstep(0.0, 0.2, n) * (1.0 - smoothstep(0.5, 1.0, n)),
        colour: [231.0 / 255.0, 215.0 / 255.0, 202.0 / 255.0],
    },
];

/// How far up a pillar flame reaches, t after the break: 36–50 units as the bottle breaks, down into the carpet by the
/// time the fire settles.
fn pillar_top(t: f64, r: f64, settled: f64) -> f64 {
    lerp(lerp(36.0, 50.0, r), settled, smoothstep(0.0, SETTLED, t))
}

/// How much narrower a tongue is for what `rise` added to it: up to a third at the full PEAK.
fn slim(up: f64) -> f64 {
    1.0 - 0.35 * (up / PEAK)
}

/// A flame standing on `ground`, reaching `top` above it: its centre and half-height.
fn standing(ground: V3, top: f64) -> (V3, f64) {
    let height = top / (FOOT + TOP);
    ([ground[0], ground[1], ground[2] + FOOT * height], height)
}

/// Calls `visit` for each particle a continuous emitter has alive at t, by slot and cycle. Every slot also fires once
/// in the first 0.3 s (cycle −1), so a flame flares up as it catches rather than filling in over a whole period.
fn emitted(id: i64, start: f64, end: f64, slots: usize, period: f64, t: f64, mut visit: impl FnMut(i64, i64, f64, f64)) {
    for slot in 0..slots as i64 {
        let phase = hash(id, slot, 0, 0) * period;
        let flare = start + (0.3 * phase) / period;
        if t >= flare && flare <= end {
            visit(slot, -1, t - flare, flare);
        }
        let cycle = ((t - start - phase) / period).floor();
        if cycle < 0.0 {
            continue;
        }
        let birth = start + phase + cycle * period;
        if birth <= end {
            visit(slot, cycle as i64, t - birth, birth);
        }
    }
}

/// A hash of four integers to [0, 1), as fire.ts's (32-bit, wrapping like Math.imul).
pub fn hash(a: i64, b: i64, c: i64, d: i64) -> f64 {
    let mut h: u32 = 0x811c_9dc5;
    for x in [a, b, c, d] {
        h = (h ^ (x as i32 as u32)).wrapping_mul(0x0100_0193);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
    }
    f64::from(h) / 4_294_967_296.0
}

/// Piecewise linear through (x, y) points, held flat beyond the ends.
fn curve(points: &[(f64, f64)], x: f64) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return lerp(y0, y1, (x - x0) / (x1 - x0));
        }
    }
    points[points.len() - 1].1
}

/// JavaScript's Math.round: halves go up.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn on_disc(centre: V3, u: V3, v: V3, radius: f64, turn: f64) -> V3 {
    add3(centre, scale(u, (2.0 * PI * turn).cos() * radius), scale(v, (2.0 * PI * turn).sin() * radius))
}

/// Two unit vectors spanning the surface with this normal.
fn basis(n: V3) -> (V3, V3) {
    let u = normalize(if n[2].abs() < 0.9 { cross(n, [0.0, 0.0, 1.0]) } else { cross(n, [1.0, 0.0, 0.0]) });
    (u, cross(n, u))
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = clamp01((x - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: V3, b: V3, t: f64) -> V3 {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn add3(a: V3, b: V3, c: V3) -> V3 {
    add(add(a, b), c)
}

fn scale(v: V3, k: f64) -> V3 {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn distance(a: V3, b: V3) -> f64 {
    hypot3(a[0] - b[0], a[1] - b[1], a[2] - b[2])
}

fn normalize(v: V3) -> V3 {
    let l = hypot3(v[0], v[1], v[2]);
    let l = if l == 0.0 { 1.0 } else { l };
    [v[0] / l, v[1] / l, v[2] / l]
}

/// The gradients CS2's fire systems colour a flame by (sRGB), one row each of the palette texture, by texel brightness.
const GRADIENTS: [&[(f64, [u8; 3])]; 6] = [
    // molotov_groundfire_main_center
    &[(0.0, [0, 0, 0]), (0.06, [128, 10, 10]), (0.17, [167, 55, 46]), (0.26, [210, 92, 29]), (0.84, [210, 146, 97]), (0.95, [255, 235, 201]), (1.0, [255, 255, 255])],
    // incendiary_groundfire_main_center
    &[(0.0, [0, 0, 0]), (0.4, [32, 203, 183]), (0.75, [222, 199, 109]), (1.0, [255, 255, 189])],
    // molotov_groundfire_outline: faint edges run through blue and purple
    &[(0.04, [0, 0, 0]), (0.4, [21, 21, 199]), (0.5, [108, 10, 128]), (0.64, [167, 55, 46]), (0.7, [210, 92, 29]), (0.8, [210, 146, 97]), (0.93, [255, 235, 201]), (1.0, [255, 255, 255])],
    // incendiary_groundfire_outline
    &[(0.0, [0, 0, 0]), (0.4, [0, 255, 255]), (0.75, [222, 199, 109]), (1.0, [255, 255, 189])],
    // molotov_groundfire_snapshot_child_base, both kinds
    &[(0.0, [0, 0, 0]), (0.06, [128, 10, 10]), (0.12, [167, 55, 46]), (0.2, [210, 127, 86]), (0.45, [210, 146, 97]), (0.95, [255, 235, 201]), (1.0, [255, 255, 255])],
    // the pillar and the break's fireballs colour a plain fire texture by the particle's cream: a warm ramp
    &[(0.0, [0, 0, 0]), (0.25, [110, 34, 10]), (0.55, [226, 140, 64]), (0.85, [255, 226, 180]), (1.0, [255, 255, 255])],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gradient {
    Fill,
    FillIncendiary,
    Outline,
    OutlineIncendiary,
    Film,
    Warm,
}

impl Gradient {
    /// The texture coordinate of its row in [`palette`].
    pub fn row(self) -> f32 {
        (self as usize as f32 + 0.5) / GRADIENTS.len() as f32
    }
}

pub const PALETTE_WIDTH: usize = 256;

/// The gradients, one row each, 256 texels of sRGB colour by brightness (RGBA8).
pub fn palette() -> Vec<u8> {
    let mut data = vec![0u8; PALETTE_WIDTH * GRADIENTS.len() * 4];
    for (r, stops) in GRADIENTS.iter().enumerate() {
        for x in 0..PALETTE_WIDTH {
            let t = x as f64 / (PALETTE_WIDTH - 1) as f64;
            let i = stops.iter().position(|s| s.0 >= t);
            let hi = stops[i.unwrap_or(stops.len() - 1)];
            let lo = stops[i.unwrap_or(stops.len()).saturating_sub(1)];
            let k = if hi.0 == lo.0 { 1.0 } else { (t - lo.0) / (hi.0 - lo.0) };
            let o = (r * PALETTE_WIDTH + x) * 4;
            for c in 0..3 {
                data[o + c] = js_round(f64::from(lo.1[c]) + (f64::from(hi.1[c]) - f64::from(lo.1[c])) * k.clamp(0.0, 1.0)) as u8;
            }
            data[o + 3] = 255;
        }
    }
    data
}

/// The fire's stand-in texture: four channels of tiling value-noise fbm at 4, 4, 8 and 16 cells, 128² (RGBA8).
pub fn fire_noise() -> Vec<u8> {
    const SIZE: usize = 128;
    let mut seed = 11.0f64;
    let mut random = move || {
        seed = (seed * 16807.0) % 2_147_483_647.0;
        (seed - 1.0) / 2_147_483_646.0
    };
    // the lattices in the order fire.ts first asks for them
    let mut lattices: Vec<((usize, usize), Vec<f32>)> = Vec::new();
    for (cells, salt) in [(4, 10), (4, 20), (8, 30), (16, 40)] {
        for octave in 0..4 {
            let key = (cells << octave, salt + octave);
            lattices.push((key, (0..key.0 * key.0).map(|_| random() as f32).collect()));
        }
    }
    let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
    let value = |cells: usize, salt: usize, x: f64, y: f64| {
        let g = &lattices.iter().find(|(k, _)| *k == (cells, salt)).expect("made above").1;
        let (fx, fy) = (x * cells as f64, y * cells as f64);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (smooth(fx - ix), smooth(fy - iy));
        let at = |i: f64, j: f64| f64::from(g[(j as usize % cells) * cells + (i as usize % cells)]);
        let top = at(ix, iy) + (at(ix + 1.0, iy) - at(ix, iy)) * tx;
        let bottom = at(ix, iy + 1.0) + (at(ix + 1.0, iy + 1.0) - at(ix, iy + 1.0)) * tx;
        top + (bottom - top) * ty
    };
    let fbm = |cells: usize, salt: usize, x: f64, y: f64| {
        let (mut sum, mut weight, mut total) = (0.0, 0.5, 0.0);
        for octave in 0..4 {
            sum += weight * value(cells << octave, salt + octave, x, y);
            total += weight;
            weight *= 0.5;
        }
        ((sum / total - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0)
    };
    let mut data = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (u, v) = (x as f64 / SIZE as f64, y as f64 / SIZE as f64);
            let o = (y * SIZE + x) * 4;
            for (c, (cells, salt)) in [(4, 10), (4, 20), (8, 30), (16, 40)].into_iter().enumerate() {
                data[o + c] = js_round(fbm(cells, salt, u, v) * 255.0) as u8;
            }
        }
    }
    data
}

/// How a layer draws: its gradient, overbright, whether it covers what is behind (1) or only adds light (0), what it
/// is (0 a flame, 1 steam, 2 a fleck), how far up its gradient its texels reach, its desaturation, and the flipbook it
/// plays (by slot in the fire's three: fill, lick, outline; None for the stand-in alone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub gradient: Gradient,
    pub overbright: f64,
    pub cover: f64,
    pub kind: f64,
    pub level: f64,
    pub desaturate: f64,
    pub book: Option<usize>,
}

pub fn style(kind: FireKind, layer: Layer) -> Style {
    let incendiary = kind == FireKind::Incendiary;
    let fill = if incendiary {
        Style { gradient: Gradient::FillIncendiary, overbright: 3.0, cover: 0.0, kind: 0.0, level: 1.2, desaturate: 0.25, book: Some(0) }
    } else {
        Style { gradient: Gradient::Fill, overbright: 4.0, cover: 0.0, kind: 0.0, level: 1.0, desaturate: 0.1, book: Some(0) }
    };
    let plain = |overbright: f64| Style { gradient: Gradient::Warm, overbright, cover: 0.0, kind: 0.0, level: 1.0, desaturate: 0.0, book: None };
    match layer {
        Layer::Fill => fill,
        Layer::Lick => Style { overbright: 2.0, book: Some(1), ..fill },
        Layer::Outline if incendiary => Style { gradient: Gradient::OutlineIncendiary, overbright: 1.5, cover: 1.0, kind: 0.0, level: 0.95, desaturate: 0.15, book: Some(2) },
        Layer::Outline => Style { gradient: Gradient::Outline, overbright: 3.0, cover: 1.0, kind: 0.0, level: 0.92, desaturate: 0.0, book: Some(2) },
        Layer::Pillar => Style { desaturate: 0.15, ..plain(3.5) },
        Layer::Fireball => plain(2.5),
        Layer::Flash => plain(2.0),
        Layer::Ember => Style { kind: 2.0, ..plain(4.0) },
        Layer::Steam => Style { cover: 1.0, kind: 1.0, ..plain(1.1) },
    }
}

/// The flipbooks each kind's fill, lick and outline play (molotov_groundfire_main_center, _main_fancy and _outline;
/// the incendiary's main_center and outline both play a_v02).
pub fn books(kind: FireKind) -> [&'static str; 3] {
    match kind {
        FireKind::Molotov => ["fire_small_sim_a", "fire_small_sim_b", "fire_small_sim_b_desat"],
        FireKind::Incendiary => ["fire_small_sim_a_v02", "fire_small_sim_a_v02", "fire_small_sim_a_v02"],
    }
}

/// One sprite as the renderer's instance buffer holds it (render/fire.ts's attributes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Instance {
    /// Centre (Hammer units).
    pub pos: [f32; 3],
    pub pad: f32,
    /// Half-width, half-height, roll, seed.
    pub shape: [f32; 4],
    /// Alpha, age, the gradient's row, what it is.
    pub look: [f32; 4],
    /// Linear colour times overbright, and whether it covers.
    pub colour: [f32; 4],
    /// Level, desaturation, and unused.
    pub tone: [f32; 4],
    /// The flipbook's slot (−1 for none), its sequence's first frame and count, and the frame-rate jitter.
    pub book: [f32; 4],
}

/// The sprites as instances; `sequences` holds each of the fire's three flipbooks' (first frame, count) when this
/// machine has them.
pub fn instances(kind: FireKind, sprites: &[Sprite], sequences: Option<&[Vec<(u32, u32)>; 3]>) -> Vec<Instance> {
    sprites
        .iter()
        .map(|s| {
            let st = style(kind, s.layer);
            // particle colours are sRGB, the scene is linear
            let colour = s.tint.map(|c| (c.powf(2.2) * st.overbright) as f32);
            let book = match (st.book, sequences) {
                (Some(slot), Some(all)) if !all[slot].is_empty() => {
                    let list = &all[slot];
                    let pick = ((((s.seed * 9.73) % 1.0) * list.len() as f64).floor() as usize).min(list.len() - 1);
                    let (start, count) = list[pick];
                    [slot as f32, start as f32, count as f32, (0.9 + 0.2 * ((s.seed * 4.21) % 1.0)) as f32]
                }
                _ => [-1.0, 0.0, 1.0, 1.0],
            };
            Instance {
                pos: s.pos.map(|v| v as f32),
                pad: 0.0,
                shape: [s.radius as f32, s.height as f32, s.rotation as f32, s.seed as f32],
                look: [s.alpha as f32, s.age as f32, st.gradient.row(), st.kind as f32],
                colour: [colour[0], colour[1], colour[2], st.cover as f32],
                tone: [st.level as f32, st.desaturate as f32, 0.0, 0.0],
                book,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flame(x: f64, start: f64, end: f64) -> Flame {
        Flame { pos: [x, 0.0, 0.0], normal: [0.0, 0.0, 1.0], start, end }
    }

    /// The hash's values as fire.ts computes them in JavaScript.
    #[test]
    fn the_hash_is_fire_ts_s() {
        assert_eq!(hash(0, 0, 0, 0), 0.8140980841126293);
        assert_eq!(hash(1000, 3, -1, 7), 0.17138250404968858);
        assert_eq!(hash(80_000, 2, 31 * 300 + 600, 5), 0.15319942473433912);
        // ToInt32 wraps what does not fit
        assert_eq!(hash(-5, 10_000_000_000, 3, 0), 0.8140305860433728);
    }

    #[test]
    fn a_fire_burns_from_its_first_flame_and_goes_out_with_its_last() {
        let fire = build_fire(vec![flame(0.0, 0.0, 5.0), flame(50.0, 0.5, 6.0), flame(100.0, 1.0, 7.0)], FireKind::Molotov).unwrap();
        assert_eq!((fire.origin, fire.end), (0, 7.0));
        assert_eq!(fire.centre, [50.0, 0.0, 0.0]);
        assert!(fire.sprites_at(-0.1).is_empty());
        // the bottle breaks: fireballs and flashes, then the carpet
        let early = fire.sprites_at(0.05);
        assert!(early.iter().any(|s| s.layer == Layer::Fireball) && early.iter().any(|s| s.layer == Layer::Flash));
        let burning = fire.sprites_at(3.0);
        assert!(burning.iter().all(|s| !matches!(s.layer, Layer::Fireball | Layer::Flash | Layer::Ember | Layer::Steam)));
        assert!(burning.iter().filter(|s| s.layer == Layer::Fill).count() > 10);
        assert!(burning.len() <= fire.sprite_bound());
        // the carpet hugs the ground: no flame of it reaches 30 units up
        assert!(burning.iter().filter(|s| s.layer != Layer::Pillar).all(|s| s.pos[2] + s.height * TOP < 30.0));
        // out, then flecks and steam
        let after = fire.sprites_at(8.0);
        assert!(after.iter().all(|s| matches!(s.layer, Layer::Ember | Layer::Steam)) && !after.is_empty());
        assert_eq!(fire.sprites_at(3.0), burning, "a pure function of time");
        assert!(fire.light_at(3.0) > 1.0 && fire.light_at(7.3) == 0.0);
        assert_eq!(fire.patches_at(0.7).len(), 2);
        let full = fire.fullest_at();
        assert!(full > 1.0 && full <= 2.5, "{full}");
        assert_eq!(fire.lasts(), 9.5);
    }

    #[test]
    fn flames_take_their_ticks_from_the_detonation() {
        let rows = [nades_model::Flame { slot: 0, pos: [1.0, 2.0, 3.0], normal: None, first: nades_model::Tick(3100), last: nades_model::Tick(3163) }];
        let flames = flames_of(&rows, 3100);
        assert_eq!(flames[0], Flame { pos: [1.0, 2.0, 3.0], normal: [0.0, 0.0, 1.0], start: 0.0, end: 1.0 });
    }

    #[test]
    fn the_palette_runs_through_each_gradient() {
        let p = palette();
        let at = |row: usize, x: usize| &p[(row * PALETTE_WIDTH + x) * 4..(row * PALETTE_WIDTH + x) * 4 + 4];
        assert_eq!(at(0, 0), [0, 0, 0, 255]);
        assert_eq!(at(0, 255), [255, 255, 255, 255]);
        assert_eq!(at(1, 102), [32, 203, 183, 255]);
        let noise = fire_noise();
        assert_eq!(noise.len(), 128 * 128 * 4);
        let mean = noise.iter().map(|&v| f64::from(v)).sum::<f64>() / noise.len() as f64;
        assert!((mean - 127.0).abs() < 30.0, "{mean}");
    }
}
