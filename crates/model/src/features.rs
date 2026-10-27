//! What works for a given set of facts: the feature table of docs/standalone-api.md §2, judged from a [`Census`] of the
//! facts present. `nades-sandbox check` prints it for a file.

/// How many of each fact a set of facts holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Census {
    pub demos: u64,
    pub demos_with_rounds: u64,
    pub demos_with_samples: u64,
    pub throws: u64,
    /// A detonation or a flight to land it.
    pub landed: u64,
    pub with_player: u64,
    pub with_team: u64,
    pub with_steamid: u64,
    pub with_flight: u64,
    /// The thrower's origin and eye angles.
    pub with_view: u64,
    /// Released in the air (`on_ground` false), and of those with their take-off point.
    pub airborne: u64,
    pub airborne_with_takeoff: u64,
    pub with_duck: u64,
    /// The initial velocity and the thrower's eye angles, velocity and ground state.
    pub with_release: u64,
    pub smokes: u64,
    pub smokes_with_voxels: u64,
    pub fires: u64,
    pub fires_with_flames: u64,
    pub flashes: u64,
    pub blinds: u64,
    pub samples: u64,
    pub samples_with_place: u64,
    /// Demos with replay tracks (`player_tracks`).
    pub demos_with_tracks: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Every throw (or demo) it applies to has what it needs.
    On,
    /// `with` of `of` have it.
    Partly { with: u64, of: u64 },
    Off,
    /// Nothing it applies to (no smokes, say).
    NotApplicable,
}

impl Status {
    fn of(with: u64, of: u64) -> Status {
        match (with, of) {
            (_, 0) => Status::NotApplicable,
            (0, _) => Status::Off,
            (w, o) if w >= o => Status::On,
            (with, of) => Status::Partly { with, of },
        }
    }

    pub fn label(&self) -> String {
        match self {
            Status::On => "on".into(),
            Status::Partly { with, of } => format!("partly ({with} of {of})"),
            Status::Off => "off".into(),
            Status::NotApplicable => "n/a".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub name: &'static str,
    pub status: Status,
    /// The facts it needs beyond the minimum.
    pub needs: &'static str,
    /// What is missing, or what else is worth knowing.
    pub note: Option<String>,
}

/// `n` and its noun: `unit` names the many, and loses its first word's final s for one.
pub fn count(n: u64, unit: &str) -> String {
    if n == 1 {
        let (first, rest) = unit.split_once(' ').map_or((unit, ""), |(f, r)| (f, r));
        let one = first.strip_suffix('s').unwrap_or(first);
        if rest.is_empty() { format!("1 {one}") } else { format!("1 {one} {rest}") }
    } else {
        format!("{n} {unit}")
    }
}

/// Each feature of §2 with its status for `c`.
pub fn table(c: &Census) -> Vec<Feature> {
    let missing = |what: &str, with: u64, of: u64, unit: &str| (with < of).then(|| format!("{} without {what}", count(of - with, unit)));
    let names = format!("player names on {} of {} throws, team names on {}", c.with_player, c.throws, c.with_team);
    vec![
        Feature { name: "the throw list, grouping, filters by side, grenade, player and team, the orbit view", status: Status::of(c.landed, c.throws), needs: "nothing (player and team names when known)", note: missing("a detonation or a flight to land it", c.landed, c.throws, "throws").or(Some(names)) },
        Feature { name: "the drawn trajectory", status: Status::of(c.with_flight, c.throws), needs: "the flight", note: missing("a flight", c.with_flight, c.throws, "throws") },
        Feature {
            name: "the thrower's and aim views, the practice command",
            status: Status::of(c.with_view, c.throws),
            needs: "the thrower's origin and eye angles (the duck amount for crouched throws, the take-off point for jump-throws)",
            note: missing("the thrower's origin and angles", c.with_view, c.throws, "throws")
                .or_else(|| missing("a take-off point", c.airborne_with_takeoff, c.airborne, "throws released in the air"))
                .or_else(|| missing("a duck amount", c.with_duck, c.throws, "throws")),
        },
        Feature { name: "technique (movement, click)", status: Status::of(c.with_release, c.throws), needs: "the initial velocity, the thrower's velocity, eye angles and ground state", note: missing("all of them", c.with_release, c.throws, "throws") },
        Feature { name: "the smoke as simulated", status: Status::of(c.smokes_with_voxels, c.smokes), needs: "smoke voxels", note: missing("voxels (not drawn)", c.smokes_with_voxels, c.smokes, "smokes") },
        Feature { name: "the fire's spread", status: Status::of(c.fires_with_flames, c.fires), needs: "flames", note: missing("flames (not drawn)", c.fires_with_flames, c.fires, "fires") },
        Feature {
            name: "who was flashed",
            status: if c.flashes == 0 { Status::NotApplicable } else if c.blinds == 0 { Status::Off } else { Status::On },
            needs: "blinds",
            note: (c.flashes > 0).then(|| format!("{} over {}", count(c.blinds, "blinds"), count(c.flashes, "flashes").replace("1 flashe", "1 flash"))),
        },
        Feature { name: "the opponent's view", status: Status::of(c.demos_with_samples, c.demos), needs: "player samples", note: missing("player samples", c.demos_with_samples, c.demos, "demos") },
        Feature {
            name: "landing callouts",
            status: if c.samples_with_place > 0 { Status::On } else if c.throws == 0 { Status::NotApplicable } else { Status::Off },
            needs: "player samples with place names",
            note: (c.samples > 0).then(|| format!("{} of {} samples name their place", c.samples_with_place, c.samples)),
        },
        Feature { name: "round-time and after-plant filters", status: Status::of(c.demos_with_rounds, c.demos), needs: "rounds", note: missing("rounds", c.demos_with_rounds, c.demos, "demos") },
        Feature {
            name: "the replay's players, smooth",
            status: Status::of(c.demos_with_tracks, c.demos_with_rounds),
            needs: "player tracks (2.1)",
            note: (c.demos_with_tracks < c.demos_with_rounds).then(|| format!("{} without them replay from the samples, two a second", count(c.demos_with_rounds - c.demos_with_tracks, "demos"))),
        },
        Feature { name: "one player across name changes", status: Status::of(c.with_steamid, c.throws), needs: "Steam ids", note: missing("a Steam id", c.with_steamid, c.throws, "throws") },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_feature_says_what_it_lacks() {
        let c = Census { demos: 2, demos_with_rounds: 2, throws: 10, landed: 10, with_flight: 7, with_view: 10, with_release: 0, smokes: 0, fires: 3, fires_with_flames: 3, flashes: 2, blinds: 0, ..Default::default() };
        let t = table(&c);
        let status = |name: &str| t.iter().find(|f| f.name.starts_with(name)).unwrap().status;
        assert_eq!(status("the throw list"), Status::On);
        assert_eq!(status("the drawn trajectory"), Status::Partly { with: 7, of: 10 });
        assert_eq!(t[1].note.as_deref(), Some("3 throws without a flight"));
        assert_eq!(status("technique"), Status::Off);
        assert_eq!(status("the smoke"), Status::NotApplicable);
        assert_eq!(status("the fire's spread"), Status::On);
        assert_eq!(status("who was flashed"), Status::Off);
        assert_eq!(status("the opponent's view"), Status::Off);
        assert_eq!(status("landing callouts"), Status::Off);
        assert_eq!(status("round-time"), Status::On);
        assert_eq!(t.len(), 12);
        assert_eq!((count(1, "demos"), count(2, "demos"), count(1, "throws released in the air")), ("1 demo".into(), "2 demos".into(), "1 throw released in the air".to_string()));
    }
}
