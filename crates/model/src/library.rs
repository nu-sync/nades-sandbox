//! One map's facts with the rules applied: every throw's derivations and group, and the questions the app asks of
//! them (a filtered slice's lineups, the filters' choices, a throw's derivations and viewpoints). The same for every
//! source; filters run in memory.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::{Range, RangeInclusive};

use crate::derive;
pub use crate::derive::Practice;
use crate::facts::{Demo, DemoId, Grenade, LineupId, MapFacts, Round, Sample, Side, Throw, ThrowId, Tick, Vec3};
use crate::lineups::Grouping;
use crate::places::Callouts;
use crate::technique::{self, Release, Technique};
use crate::viewpoints;
pub use crate::viewpoints::Viewpoint;

/// Everything the sandbox derives of one throw.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    /// The detonation, or else the flight's last point.
    pub landing: Option<Vec3>,
    pub airtime: Option<f32>,
    /// From the detonation to the expiry, or the grenade's nominal length when they are not both recorded.
    pub effect_seconds: f32,
    /// Whether `effect_seconds` was recorded rather than nominal.
    pub effect_recorded: bool,
    /// The thrower's eye at release.
    pub eye: Option<Vec3>,
    pub technique: Option<Technique>,
    /// The release model solved for the throw (what `technique` is read from).
    pub release: Option<Release>,
    pub practice: Option<Practice>,
    /// The landing's callout.
    pub to: Option<String>,
    pub round_seconds: Option<f32>,
    pub after_plant: Option<bool>,
    /// Its group; None for a throw outside play or one that cannot be landed.
    pub lineup: Option<LineupId>,
}

impl Derived {
    /// What a throw's own facts and its round give: everything but the landing's callout and the group, which need
    /// the rest of the map ([`Library`]).
    pub fn of(t: &Throw, round: Option<&Round>) -> Derived {
        let solved = technique::of(t);
        let technique = solved.map(|(t, _)| t);
        let landing = derive::landing(t);
        let (effect_seconds, effect_recorded) = derive::effect_seconds(t);
        Derived {
            landing,
            airtime: derive::airtime(t),
            effect_seconds,
            effect_recorded,
            eye: derive::eye(t, technique.as_ref()).map(|e| e.map(|v| v as f32)),
            technique,
            release: solved.map(|(_, r)| r),
            practice: derive::practice(t, technique.as_ref()),
            to: None,
            round_seconds: derive::round_seconds(t, round),
            after_plant: derive::after_plant(t, round),
            lineup: None,
        }
    }
}

/// A cut of one map's throws.
#[derive(Debug, Clone, PartialEq)]
pub struct LineupQuery {
    pub side: Option<Side>,
    pub grenade: Option<Grenade>,
    /// The team's name as the demo has it, any case.
    pub team: Option<String>,
    /// The player's name, any case.
    pub player: Option<String>,
    /// Seconds since the round's freeze time ended, from the start up to (not including) the end.
    pub round_seconds: Option<Range<f32>>,
    pub after_plant: Option<bool>,
    /// A producer's tag on the throw or its demo, `(key, value)`.
    pub tag: Option<(String, String)>,
    /// Lineups thrown fewer times in the slice are left out.
    pub min_throws: u32,
    pub limit: u32,
}

impl Default for LineupQuery {
    fn default() -> Self {
        Self { side: None, grenade: None, team: None, player: None, round_seconds: None, after_plant: None, tag: None, min_throws: 1, limit: 300 }
    }
}

/// One lineup in a slice.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineup {
    pub id: LineupId,
    /// The commonest grenade of its throws (a fire's lineup can hold molotovs and incendiaries).
    pub grenade: Grenade,
    pub side: Option<Side>,
    pub throws: u32,
    pub demos: u32,
    /// The commonest start place and landing callout.
    pub from: Option<String>,
    pub to: Option<String>,
    pub technique: Option<Technique>,
    pub teams: Vec<String>,
    pub players: Vec<String>,
    /// The throw it is shown by: the earliest with its commonest technique.
    pub example: ThrowId,
}

/// What the filters offer: the teams on the map, most demos first, and the players who threw (for one team, or all),
/// busiest first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facets {
    pub teams: Vec<String>,
    pub players: Vec<String>,
}

pub struct Library {
    demos: Vec<Demo>,
    demo_at: HashMap<DemoId, usize>,
    rounds: HashMap<(DemoId, u32), Round>,
    /// In grouping order.
    throws: Vec<Throw>,
    at: HashMap<ThrowId, usize>,
    derived: Vec<Derived>,
    callouts: Callouts,
    groups: u32,
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The most frequent of `counts`, ties to the first in order.
fn commonest<K: Ord + Clone>(counts: &BTreeMap<K, u32>) -> Option<K> {
    let mut best: Option<(&K, u32)> = None;
    for (k, &n) in counts {
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((k, n));
        }
    }
    best.map(|(k, _)| k.clone())
}

impl Library {
    /// One map's facts and its place-labelled sample points (crate::ThrowSource::places), with every rule applied.
    pub fn new(facts: MapFacts, places: Vec<(Vec3, String)>) -> Library {
        let MapFacts { demos, rounds, mut throws } = facts;
        let callouts = Callouts::learn(places.iter().map(|(p, name)| (*p, name.as_str())));
        let demo_at: HashMap<DemoId, usize> = demos.iter().enumerate().map(|(i, d)| (d.id.clone(), i)).collect();
        let with_rounds: HashSet<DemoId> = rounds.iter().map(|r| r.demo.clone()).collect();
        let rounds: HashMap<(DemoId, u32), Round> = rounds.into_iter().map(|r| ((r.demo.clone(), r.number), r)).collect();
        let played = |t: &Throw| demo_at.get(&t.demo).and_then(|&i| demos[i].played_at);
        throws.sort_by(|a, b| {
            let (pa, pb) = (played(a), played(b));
            (pa.is_none(), pa, &a.demo, a.tick, &a.id).cmp(&(pb.is_none(), pb, &b.demo, b.tick, &b.id))
        });
        let mut grouping = Grouping::default();
        let derived = throws
            .iter()
            .map(|t| {
                let round = t.round.and_then(|n| rounds.get(&(t.demo.clone(), n)));
                let mut d = Derived::of(t, round);
                d.to = d.landing.and_then(|l| callouts.lookup(l)).map(str::to_string);
                d.lineup = d.landing.filter(|_| derive::in_play(t, round, with_rounds.contains(&t.demo))).map(|l| grouping.assign(t.grenade, t.start, l));
                d
            })
            .collect();
        let at = throws.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
        let groups = grouping.started();
        Library { demos, demo_at, rounds, throws, at, derived, callouts, groups }
    }

    /// The throws, in grouping order (`played_at`, `demo_id`, `tick`, `throw_id`), each with its derivations.
    pub fn throws(&self) -> impl Iterator<Item = (&Throw, &Derived)> {
        self.throws.iter().zip(&self.derived)
    }

    pub fn throw(&self, id: &ThrowId) -> Option<&Throw> {
        self.at.get(id).map(|&i| &self.throws[i])
    }

    pub fn derived(&self, id: &ThrowId) -> Option<&Derived> {
        self.at.get(id).map(|&i| &self.derived[i])
    }

    pub fn demo(&self, id: &DemoId) -> Option<&Demo> {
        self.demo_at.get(id).map(|&i| &self.demos[i])
    }

    pub fn demos(&self) -> &[Demo] {
        &self.demos
    }

    /// A demo's rounds, in order.
    pub fn rounds_of(&self, demo: &DemoId) -> Vec<&Round> {
        let mut out: Vec<&Round> = self.rounds.values().filter(|r| &r.demo == demo).collect();
        out.sort_by_key(|r| r.number);
        out
    }

    pub fn round_of(&self, throw: &Throw) -> Option<&Round> {
        self.rounds.get(&(throw.demo.clone(), throw.round?))
    }

    /// How many groups the map's throws form.
    pub fn groups(&self) -> u32 {
        self.groups
    }

    /// How many cells the callouts were learned into.
    pub fn callout_cells(&self) -> usize {
        self.callouts.cells()
    }

    fn matches(&self, q: &LineupQuery, t: &Throw, d: &Derived) -> bool {
        let tagged = |(key, value): &(String, String)| {
            let has = |tags: &crate::Tags| tags.iter().any(|(k, v)| k == key && v == value);
            has(&t.tags) || self.demo(&t.demo).is_some_and(|demo| has(&demo.tags))
        };
        q.side.is_none_or(|s| t.side == s)
            && q.grenade.is_none_or(|g| t.grenade == g)
            && q.team.as_deref().is_none_or(|team| t.team.as_deref().is_some_and(|x| same(x, team)))
            && q.player.as_deref().is_none_or(|player| t.player.name.as_deref().is_some_and(|x| same(x, player)))
            && q.round_seconds.as_ref().is_none_or(|r| d.round_seconds.is_some_and(|s| r.contains(&s)))
            && q.after_plant.is_none_or(|a| d.after_plant == Some(a))
            && q.tag.as_ref().is_none_or(tagged)
    }

    /// The slice's lineups, most thrown first (ties by id).
    pub fn lineups(&self, query: &LineupQuery) -> Vec<Lineup> {
        #[derive(Default)]
        struct Acc<'a> {
            throws: u32,
            demos: HashSet<&'a DemoId>,
            grenades: BTreeMap<Grenade, u32>,
            sides: BTreeMap<&'static str, u32>,
            from: BTreeMap<&'a str, u32>,
            to: BTreeMap<&'a str, u32>,
            techniques: BTreeMap<String, u32>,
            teams: BTreeSet<&'a str>,
            players: BTreeSet<&'a str>,
            members: Vec<usize>,
        }
        let mut groups: BTreeMap<LineupId, Acc> = BTreeMap::new();
        for (i, (t, d)) in self.throws.iter().zip(&self.derived).enumerate() {
            let Some(id) = d.lineup else { continue };
            if !self.matches(query, t, d) {
                continue;
            }
            let acc = groups.entry(id).or_default();
            acc.throws += 1;
            acc.demos.insert(&t.demo);
            *acc.grenades.entry(t.grenade).or_default() += 1;
            *acc.sides.entry(t.side.name()).or_default() += 1;
            if let Some(place) = t.place.as_deref().filter(|p| !p.is_empty()) {
                *acc.from.entry(place).or_default() += 1;
            }
            if let Some(to) = d.to.as_deref() {
                *acc.to.entry(to).or_default() += 1;
            }
            if let Some(technique) = &d.technique {
                *acc.techniques.entry(technique.name()).or_default() += 1;
            }
            acc.teams.extend(t.team.as_deref().filter(|s| !s.is_empty()));
            acc.players.extend(t.player.name.as_deref().filter(|s| !s.is_empty()));
            acc.members.push(i);
        }
        let mut out: Vec<Lineup> = groups
            .into_iter()
            .filter(|(_, acc)| acc.throws >= query.min_throws)
            .map(|(id, acc)| {
                let technique_name = commonest(&acc.techniques);
                let example = acc.members.iter().copied().find(|&i| self.derived[i].technique.map(|t| t.name()) == technique_name).unwrap_or(acc.members[0]);
                Lineup {
                    id,
                    grenade: commonest(&acc.grenades).expect("a lineup has throws"),
                    side: commonest(&acc.sides).and_then(Side::parse),
                    throws: acc.throws,
                    demos: acc.demos.len() as u32,
                    from: commonest(&acc.from).map(str::to_string),
                    to: commonest(&acc.to).map(str::to_string),
                    technique: self.derived[example].technique,
                    teams: acc.teams.into_iter().map(str::to_string).collect(),
                    players: acc.players.into_iter().map(str::to_string).collect(),
                    example: self.throws[example].id.clone(),
                }
            })
            .collect();
        out.sort_by(|a, b| b.throws.cmp(&a.throws).then(a.id.cmp(&b.id)));
        out.truncate(query.limit as usize);
        out
    }

    pub fn facets(&self, team: Option<&str>) -> Facets {
        let mut demos_of: BTreeMap<&str, HashSet<&DemoId>> = BTreeMap::new();
        let mut throws_of: BTreeMap<&str, u32> = BTreeMap::new();
        for t in &self.throws {
            if let Some(name) = t.team.as_deref().filter(|s| !s.is_empty()) {
                demos_of.entry(name).or_default().insert(&t.demo);
            }
            let on_team = team.is_none_or(|team| t.team.as_deref().is_some_and(|x| same(x, team)));
            if on_team && let Some(player) = t.player.name.as_deref().filter(|s| !s.is_empty()) {
                *throws_of.entry(player).or_default() += 1;
            }
        }
        let mut teams: Vec<(&str, usize)> = demos_of.into_iter().map(|(name, demos)| (name, demos.len())).collect();
        teams.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let mut players: Vec<(&str, u32)> = throws_of.into_iter().collect();
        players.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        Facets { teams: teams.into_iter().map(|(n, _)| n.to_string()).collect(), players: players.into_iter().map(|(n, _)| n.to_string()).collect() }
    }

    /// The demo and the ticks a throw's viewpoints are taken from (for crate::ThrowSource::samples).
    pub fn viewpoint_window(&self, id: &ThrowId) -> Option<(DemoId, RangeInclusive<Tick>)> {
        let t = self.throw(id)?;
        Some((t.demo.clone(), viewpoints::window(t, self.round_of(t))?))
    }

    /// The demo and the ticks whose samples place the players a flash blinded (crate::derive::place_blinds).
    pub fn blind_window(&self, id: &ThrowId) -> Option<(DemoId, RangeInclusive<Tick>)> {
        let t = self.throw(id)?;
        Some((t.demo.clone(), derive::blind_window(t)?))
    }

    /// The views of a throw that `samples` (its demo's, at least over its [`Library::viewpoint_window`]) offer.
    pub fn viewpoints(&self, id: &ThrowId, samples: &[Sample]) -> Vec<Viewpoint> {
        let Some(t) = self.throw(id) else { return Vec::new() };
        let Some(landing) = self.derived(id).and_then(|d| d.landing) else { return Vec::new() };
        viewpoints::viewpoints(t, landing, self.round_of(t), samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, throw};
    use crate::technique::{Click, Movement};
    use crate::{Pawn, Player};

    /// One small match on de_cache: two smokes of one lineup and a third from the same spot to elsewhere by MOUZ, an
    /// incendiary by Vitality, and a warm-up smoke; places learned around the landings.
    fn facts() -> (MapFacts, Vec<(Vec3, String)>) {
        let pawn = |x: f32, y: f32| Pawn { origin: [x, y, 30.0], pitch: Some(-12.5), yaw: Some(45.0), velocity: Some([0.0; 3]), on_ground: Some(true), duck: Some(0.0), ..Default::default() };
        let smoke = |id: &str, tick: i32, start: Vec3, land: Vec3| {
            let mut t = throw(id, "d1", Grenade::Smoke, Side::T, start);
            t.round = Some(1);
            t.tick = Tick(tick);
            t.player = Player { name: Some("ropz".into()), steamid: Some(7) };
            t.team = Some("MOUZ".into());
            t.place = Some("Garage".into());
            t.thrower = Some(pawn(start[0] - 2.0, start[1] - 2.0));
            t.velocity = Some(technique::release_velocity(-12.5, 45.0, 1.0, [0.0; 3]).map(|v| v as f32));
            t.detonation = Some((Tick(tick + 100), land));
            t.end = Some(Tick(tick + 100 + 18 * 64));
            t
        };
        let mut fire = throw("t4", "d1", Grenade::Incendiary, Side::Ct, [500.0, 600.0, 60.0]);
        (fire.round, fire.tick, fire.team, fire.player.name) = (Some(1), Tick(3000), Some("Vitality".into()), Some("ZywOo".into()));
        fire.detonation = Some((Tick(3100), [300.0, 600.0, 0.0]));
        let mut warmup = smoke("t0", 10, [12.0, 22.0, 94.0], [800.0, 900.0, 40.0]);
        warmup.round = None;
        let throws = vec![
            smoke("t2", 2500, [13.0, 23.0, 94.0], [802.0, 901.0, 40.0]),
            smoke("t1", 1500, [12.0, 22.0, 94.0], [800.0, 900.0, 40.0]),
            smoke("t3", 4000, [12.0, 22.0, 94.0], [-500.0, 900.0, 40.0]),
            fire,
            warmup,
        ];
        let rounds = vec![Round { demo: DemoId("d1".into()), number: 1, freeze_end: Some(Tick(1000)), plant: Some(Tick(3500)), end: Some(Tick(9000)), winner: Some(Side::T) }];
        let places = vec![([800.0, 900.0, 0.0], "Highway".to_string()), ([300.0, 600.0, 0.0], "Checkers".to_string())];
        (MapFacts { demos: vec![fixture::demo("d1", "de_cache")], rounds, throws }, places)
    }

    #[test]
    fn throws_are_grouped_in_order_and_lineups_come_most_thrown_first() {
        let (facts, places) = facts();
        let library = Library::new(facts, places);
        let ids: Vec<&str> = library.throws().map(|(t, _)| t.id.as_str()).collect();
        assert_eq!(ids, ["t0", "t1", "t2", "t4", "t3"], "by tick within the demo");
        let lineups = library.lineups(&LineupQuery::default());
        assert_eq!(lineups.iter().map(|l| (l.id, l.throws)).collect::<Vec<_>>(), [(LineupId(1), 2), (LineupId(2), 1), (LineupId(3), 1)]);
        let smoke = &lineups[0];
        assert_eq!((smoke.grenade, smoke.side, smoke.demos, smoke.example.as_str()), (Grenade::Smoke, Some(Side::T), 1, "t1"));
        assert_eq!((smoke.from.as_deref(), smoke.to.as_deref()), (Some("Garage"), Some("Highway")));
        assert_eq!(smoke.technique, Some(Technique { movement: Movement::Stand, click: Some(Click::Left), crouched: false }));
        assert_eq!((smoke.teams.as_slice(), smoke.players.as_slice()), (&["MOUZ".to_string()][..], &["ropz".to_string()][..]));
        // the warm-up smoke is no lineup's; the incendiary landed by Checkers
        assert_eq!(library.derived(&ThrowId("t0".into())).unwrap().lineup, None);
        assert_eq!(lineups[1].to.as_deref(), Some("Checkers"));
        let practice = library.derived(&ThrowId("t1".into())).unwrap().practice.clone().unwrap();
        assert_eq!(practice, Practice { command: "setpos 10.00 20.00 30.00; setang -12.50 45.00 0".into(), ready: true });
    }

    #[test]
    fn the_slice_cuts_by_side_grenade_team_player_round_time_plant_and_tag() {
        let (mut facts, places) = facts();
        facts.throws[0].tags.push(("nades.lineup".into(), "cache-smoke-0001".into()));
        let library = Library::new(facts, places);
        let cut = |q: LineupQuery| library.lineups(&q).into_iter().map(|l| (l.id.0, l.throws)).collect::<Vec<_>>();
        assert_eq!(cut(LineupQuery { side: Some(Side::Ct), ..Default::default() }), [(2, 1)]);
        assert_eq!(cut(LineupQuery { grenade: Some(Grenade::Incendiary), ..Default::default() }), [(2, 1)]);
        assert_eq!(cut(LineupQuery { grenade: Some(Grenade::Molotov), ..Default::default() }), []);
        assert_eq!(cut(LineupQuery { team: Some("vitality".into()), ..Default::default() }), [(2, 1)]);
        assert_eq!(cut(LineupQuery { player: Some("ROPZ".into()), ..Default::default() }), [(1, 2), (3, 1)]);
        // 7.8 s and 23.4 s into the round; t3 at 46.9 s is after the plant at 3500
        assert_eq!(cut(LineupQuery { round_seconds: Some(0.0..20.0), ..Default::default() }), [(1, 1)]);
        assert_eq!(cut(LineupQuery { after_plant: Some(true), ..Default::default() }), [(3, 1)]);
        assert_eq!(cut(LineupQuery { tag: Some(("nades.lineup".into(), "cache-smoke-0001".into())), ..Default::default() }), [(1, 1)]);
        assert_eq!(cut(LineupQuery { min_throws: 2, ..Default::default() }), [(1, 2)]);
        assert_eq!(cut(LineupQuery { limit: 1, ..Default::default() }), [(1, 2)]);
        let facets = library.facets(None);
        assert_eq!(facets.teams, ["MOUZ", "Vitality"]);
        assert_eq!(facets.players, ["ropz", "ZywOo"]);
        assert_eq!(library.facets(Some("vitality")).players, ["ZywOo"]);
    }

    #[test]
    fn a_throws_viewpoints_come_from_its_window() {
        let (facts, places) = facts();
        let library = Library::new(facts, places);
        let id = ThrowId("t1".into());
        let (demo, window) = library.viewpoint_window(&id).unwrap();
        assert_eq!((demo.as_str(), window), ("d1", Tick(1696)..=Tick(2752)));
        let sample = |tick: i32, x: f32| Sample { tick: Tick(tick), player: Player::default(), side: Side::Ct, origin: [x, 900.0, 0.0], pitch: None, yaw: None, duck: None, alive: true, place: None };
        let views = library.viewpoints(&id, &[sample(1760, 1300.0), sample(1792, 1200.0), sample(1600, 800.0)]);
        assert_eq!(views.iter().map(|v| v.tick.0).collect::<Vec<_>>(), [1792, 1760]);
    }
}
