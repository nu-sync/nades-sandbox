//! The shell (egui): the map and the filters along the top, the lineup list on the left, the lineup's details and the
//! four views on the right, the time scrubber along the bottom, and the 3D view in the middle (with the crosshair in
//! the thrower's views). It reads the sandbox's state and returns what the person asked for; the window applies it.

use std::ops::Range;

use nades_model::{DemoId, Grenade, Library as MapLibrary, Lineup, LineupId, LineupQuery, Side, ThrowId};
use nades_scene::camera::Source;
use nades_scene::stage::ViewMode;

use crate::library::Prepared;

/// Fixed panel sizes (points), so the 3D view's size is known before the first frame.
pub const LEFT: f32 = 300.0;
pub const RIGHT: f32 = 290.0;
pub const TOP: f32 = 34.0;
pub const BOTTOM: f32 = 46.0;

/// The round-time filter's presets: seconds since the freeze time ended.
pub const ROUND_TIMES: [(&str, Option<Range<f32>>); 4] = [("any time", None), ("0–20 s", Some(0.0..20.0)), ("20–60 s", Some(20.0..60.0)), ("60 s +", Some(60.0..f32::INFINITY))];
/// The plant filter's choices.
pub const PLANTS: [(&str, Option<bool>); 3] = [("plant: any", None), ("before plant", Some(false)), ("after plant", Some(true))];
/// The grenade buttons, with their labels.
const GRENADES: [(Grenade, &str); 6] = [(Grenade::Smoke, "smoke"), (Grenade::Flash, "flash"), (Grenade::Molotov, "molotov"), (Grenade::Incendiary, "incendiary"), (Grenade::He, "HE"), (Grenade::Decoy, "decoy")];

/// What the person asked for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Map(String),
    Side(Option<Side>),
    Grenade(Option<Grenade>),
    Team(Option<String>),
    Player(Option<String>),
    RoundTime(Option<Range<f32>>),
    Plant(Option<bool>),
    MinThrows(u32),
    Select(LineupId),
    Step(i32),
    View(ViewMode),
    Time(f64),
    Play(bool),
    Copy(String),
    Replay(ReplayAction),
}

/// What the replay's controls ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayAction {
    Open,
    Close,
    Demo(DemoId),
    Round(u32),
    Play(bool),
    /// Seconds into the round.
    Seek(f64),
    Speed(f64),
    /// The player a camera follows (by track), or the free camera.
    Follow(Option<usize>),
    /// Behind the followed player rather than through their eyes.
    Chase(bool),
    /// The lineup view of a throw in the round.
    Lineup(ThrowId),
}

/// What the shell shows of the replay.
#[derive(Debug, Clone, Default)]
pub struct ReplayStatus {
    pub demos: Vec<(DemoId, String)>,
    pub demo: Option<DemoId>,
    pub rounds: Vec<(u32, String)>,
    pub round: Option<u32>,
    /// A throw's id, its line, its seconds into the round, and whether a lineup view can open on it.
    pub throws: Vec<(ThrowId, String, f64, bool)>,
    /// A track's index, the player's name, CT, alive now.
    pub players: Vec<(usize, String, bool, bool)>,
    pub follow: Option<usize>,
    pub chase: bool,
    pub seconds: f64,
    pub length: f64,
    pub playing: bool,
    pub speed: f64,
    /// Loading, a failure, or where the players come from.
    pub note: Option<String>,
}

/// No filter: the query the shell shows before anything is read.
static ANY: LineupQuery = LineupQuery { side: None, grenade: None, team: None, player: None, round_seconds: None, after_plant: None, tag: None, min_throws: 1, limit: 300 };

/// What the shell shows.
pub struct Status<'a> {
    pub map: &'a str,
    pub maps: &'a [String],
    pub query: &'a LineupQuery,
    pub teams: &'a [String],
    pub players: &'a [String],
    pub lineups: &'a [Lineup],
    /// The map's facts and rules the lineups come from: their throws' details.
    pub facts: Option<&'a MapLibrary>,
    pub search: &'a mut String,
    pub selected: Option<LineupId>,
    pub prepared: Option<&'a Prepared>,
    pub view: ViewMode,
    pub time: f64,
    pub timeline: (f64, f64),
    pub playing: bool,
    pub notice: Option<&'a str>,
    /// One line of timings: the first frame, the list, the stage.
    pub timings: String,
    pub loading_map: Option<&'a str>,
    pub copied: bool,
    /// The camera was taken by hand or by app control.
    pub free: bool,
    /// Demos dropped onto the window: what is being read, or what was.
    pub import: Option<String>,
    /// The replay, when it is open: then the panels are its.
    pub replay: Option<ReplayStatus>,
}

impl<'a> Status<'a> {
    /// The shell before anything is read: for laying it out once while the window is made.
    pub fn empty(map: &'a str, maps: &'a [String], search: &'a mut String) -> Self {
        Self {
            map,
            maps,
            query: &ANY,
            teams: &[],
            players: &[],
            lineups: &[],
            facts: None,
            search,
            selected: None,
            prepared: None,
            view: ViewMode::Lineup,
            time: 0.0,
            timeline: (-1.0, 1.0),
            playing: false,
            notice: None,
            timings: String::new(),
            loading_map: None,
            copied: false,
            free: false,
            import: None,
            replay: None,
        }
    }
}

fn nade_color(grenade: Grenade) -> egui::Color32 {
    match grenade {
        Grenade::Smoke => egui::Color32::from_rgb(0xd9, 0xdd, 0xe3),
        Grenade::Flash => egui::Color32::from_rgb(0xff, 0xe3, 0x6b),
        Grenade::He => egui::Color32::from_rgb(0xff, 0x5a, 0x4f),
        Grenade::Molotov | Grenade::Incendiary => egui::Color32::from_rgb(0xff, 0x8a, 0x2a),
        Grenade::Decoy => egui::Color32::from_rgb(0x9a, 0xa6, 0xb2),
    }
}

fn place(p: &Option<String>) -> &str {
    p.as_deref().filter(|s| !s.is_empty()).unwrap_or("?")
}

/// Lays the shell out; returns the actions and the 3D view's rectangle (points).
pub fn show(ui: &mut egui::Ui, mut status: Status<'_>) -> (Vec<Action>, egui::Rect) {
    let mut actions = Vec::new();
    top_bar(ui, &status, &mut actions);
    if let Some(replay) = &status.replay {
        replay_bar(ui, replay, &mut actions);
        replay_list(ui, replay, &mut actions);
        replay_details(ui, replay, status.import.as_deref(), &mut actions);
        let rect = ui.available_rect_before_wrap();
        replay_keys(ui, replay, &mut actions);
        return (actions, rect);
    }
    bottom_bar(ui, &status, &mut actions);
    lineup_list(ui, &mut status, &mut actions);
    details(ui, &status, &mut actions);
    let rect = ui.available_rect_before_wrap();
    if matches!(status.view, ViewMode::Lineup | ViewMode::Aim) && status.prepared.is_some() && !status.free {
        crosshair(ui.painter(), rect.center());
    }
    if let Some(map) = status.loading_map {
        ui.painter().text(rect.center_top() + egui::vec2(0.0, 24.0), egui::Align2::CENTER_TOP, format!("loading {map}…"), egui::FontId::proportional(18.0), egui::Color32::WHITE);
    }
    keys(ui, &status, &mut actions);
    (actions, rect)
}

fn replay_keys(ui: &egui::Ui, replay: &ReplayStatus, actions: &mut Vec<Action>) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    ui.input(|i| {
        if i.key_pressed(egui::Key::Space) {
            actions.push(Action::Replay(ReplayAction::Play(!replay.playing)));
        }
        for (key, by) in [(egui::Key::ArrowLeft, -5.0), (egui::Key::ArrowRight, 5.0)] {
            if i.key_pressed(key) {
                actions.push(Action::Replay(ReplayAction::Seek((replay.seconds + by).clamp(0.0, replay.length))));
            }
        }
        if i.key_pressed(egui::Key::Escape) {
            actions.push(Action::Replay(ReplayAction::Close));
        }
    });
}

/// The replay's timeline: play, the scrubber, the time, the speed.
fn replay_bar(ui: &mut egui::Ui, replay: &ReplayStatus, actions: &mut Vec<Action>) {
    egui::Panel::bottom("replay time").exact_size(BOTTOM).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            if ui.button(if replay.playing { "Pause" } else { "Play" }).clicked() {
                actions.push(Action::Replay(ReplayAction::Play(!replay.playing)));
            }
            let mut seconds = replay.seconds;
            ui.spacing_mut().slider_width = (ui.available_width() - 330.0).max(120.0);
            if ui.add(egui::Slider::new(&mut seconds, 0.0..=replay.length.max(0.1)).show_value(false)).changed() {
                actions.push(Action::Replay(ReplayAction::Seek(seconds)));
            }
            let clock = |s: f64| format!("{}:{:04.1}", (s / 60.0).floor() as u32, s % 60.0);
            ui.monospace(format!("{} / {}", clock(replay.seconds), clock(replay.length)));
            for speed in [0.25, 0.5, 1.0, 2.0, 4.0] {
                let label = if speed < 1.0 { format!("{speed}×") } else { format!("{speed:.0}×") };
                if ui.selectable_label(replay.speed == speed, label).clicked() {
                    actions.push(Action::Replay(ReplayAction::Speed(speed)));
                }
            }
        });
    });
}

/// The demo, its rounds, and the round's throws.
fn replay_list(ui: &mut egui::Ui, replay: &ReplayStatus, actions: &mut Vec<Action>) {
    egui::Panel::left("replay rounds").exact_size(LEFT).show(ui, |ui| {
        let current = replay.demo.as_ref().and_then(|d| replay.demos.iter().find(|(id, _)| id == d)).map_or("Pick a demo", |(_, name)| name.as_str());
        egui::ComboBox::from_id_salt("replay demo").selected_text(current).width(LEFT - 16.0).show_ui(ui, |ui| {
            for (id, name) in &replay.demos {
                if ui.selectable_label(replay.demo.as_ref() == Some(id), name).clicked() {
                    actions.push(Action::Replay(ReplayAction::Demo(id.clone())));
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("replay rounds").max_height(ui.available_height() * 0.45).show(ui, |ui| {
            for (number, line) in &replay.rounds {
                if ui.selectable_label(replay.round == Some(*number), line).clicked() {
                    actions.push(Action::Replay(ReplayAction::Round(*number)));
                }
            }
        });
        ui.separator();
        ui.weak("Throws: click to jump there");
        egui::ScrollArea::vertical().id_salt("replay throws").show(ui, |ui| {
            for (id, line, at, lineup) in &replay.throws {
                ui.horizontal(|ui| {
                    if ui.selectable_label(replay.seconds >= *at - 0.05 && replay.seconds < at + 2.0, line).clicked() {
                        actions.push(Action::Replay(ReplayAction::Seek((at - 0.5).max(0.0))));
                    }
                    if *lineup && ui.small_button("view").on_hover_text("Its lineup view").clicked() {
                        actions.push(Action::Replay(ReplayAction::Lineup(id.clone())));
                    }
                });
            }
        });
    });
}

/// The camera and the players.
fn replay_details(ui: &mut egui::Ui, replay: &ReplayStatus, import: Option<&str>, actions: &mut Vec<Action>) {
    egui::Panel::right("replay players").exact_size(RIGHT).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        if let Some(line) = import {
            ui.label(egui::RichText::new(line).strong());
            ui.separator();
        }
        if let Some(note) = &replay.note {
            ui.colored_label(egui::Color32::from_rgb(255, 170, 90), note);
            ui.separator();
        }
        ui.horizontal(|ui| {
            if ui.selectable_label(replay.follow.is_none(), "Free camera").clicked() {
                actions.push(Action::Replay(ReplayAction::Follow(None)));
            }
            if ui.selectable_label(replay.follow.is_some() && !replay.chase, "Eyes").on_hover_text("Through the followed player's eyes").clicked() {
                actions.push(Action::Replay(ReplayAction::Chase(false)));
            }
            if ui.selectable_label(replay.follow.is_some() && replay.chase, "Behind").on_hover_text("Behind the followed player").clicked() {
                actions.push(Action::Replay(ReplayAction::Chase(true)));
            }
        });
        ui.separator();
        for ct in [false, true] {
            ui.strong(if ct { "CT" } else { "T" });
            for (i, name, side_ct, alive) in replay.players.iter().filter(|p| p.2 == ct) {
                let colour = if *side_ct { egui::Color32::from_rgb(90, 150, 255) } else { egui::Color32::from_rgb(240, 170, 60) };
                let text = egui::RichText::new(if *alive { name.clone() } else { format!("{name} (dead)") }).color(if *alive { colour } else { egui::Color32::GRAY });
                if ui.selectable_label(replay.follow == Some(*i), text).clicked() {
                    actions.push(Action::Replay(ReplayAction::Follow(Some(*i))));
                }
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add(egui::Label::new(egui::RichText::new("space: play · ←/→: 5 s · esc: back to the lineups · fly: WASD + right drag").weak().small()).wrap());
        });
    });
}

fn keys(ui: &egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowRight) {
            actions.push(Action::Step(1));
        }
        if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowLeft) {
            actions.push(Action::Step(-1));
        }
        if i.key_pressed(egui::Key::Space) {
            actions.push(Action::Play(!status.playing));
        }
        for (key, view) in [(egui::Key::Num1, ViewMode::Lineup), (egui::Key::Num2, ViewMode::Aim), (egui::Key::Num3, ViewMode::Result), (egui::Key::Num4, ViewMode::Orbit)] {
            if i.key_pressed(key) {
                actions.push(Action::View(view));
            }
        }
    });
}

fn top_bar(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::top("filters").exact_size(TOP).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            egui::ComboBox::from_id_salt("map").selected_text(status.map).width(110.0).show_ui(ui, |ui| {
                for map in status.maps {
                    if ui.selectable_label(map == status.map, map).clicked() && map != status.map {
                        actions.push(Action::Map(map.clone()));
                    }
                }
            });
            ui.separator();
            let q = status.query;
            for (label, side) in [("Both", None), ("T", Some(Side::T)), ("CT", Some(Side::Ct))] {
                if ui.selectable_label(q.side == side, label).clicked() {
                    actions.push(Action::Side(side));
                }
            }
            ui.separator();
            if ui.selectable_label(q.grenade.is_none(), "All").clicked() {
                actions.push(Action::Grenade(None));
            }
            for (grenade, label) in GRENADES {
                if ui.selectable_label(q.grenade == Some(grenade), egui::RichText::new(label).color(nade_color(grenade))).clicked() {
                    actions.push(Action::Grenade(Some(grenade)));
                }
            }
            ui.separator();
            combo(ui, "team", "team", q.team.as_deref(), status.teams, |t| actions.push(Action::Team(t)));
            combo(ui, "player", "player", q.player.as_deref(), status.players, |p| actions.push(Action::Player(p)));
            let shown = ROUND_TIMES.iter().find(|(_, r)| *r == q.round_seconds).map_or("round time", |(label, _)| label);
            egui::ComboBox::from_id_salt("round time").selected_text(shown).width(80.0).show_ui(ui, |ui| {
                for (label, range) in ROUND_TIMES {
                    if ui.selectable_label(q.round_seconds == range, label).clicked() {
                        actions.push(Action::RoundTime(range));
                    }
                }
            });
            let shown = PLANTS.iter().find(|(_, p)| *p == q.after_plant).map_or("plant", |(label, _)| label);
            egui::ComboBox::from_id_salt("plant").selected_text(shown).width(90.0).show_ui(ui, |ui| {
                for (label, after) in PLANTS {
                    if ui.selectable_label(q.after_plant == after, label).clicked() {
                        actions.push(Action::Plant(after));
                    }
                }
            });
            ui.separator();
            let mut min = q.min_throws;
            ui.label("thrown ≥");
            if ui.add(egui::DragValue::new(&mut min).range(1..=50)).changed() {
                actions.push(Action::MinThrows(min));
            }
            ui.separator();
            let replaying = status.replay.is_some();
            if ui.selectable_label(replaying, "Replay").on_hover_text("Play a round of a demo with every player and grenade").clicked() {
                actions.push(Action::Replay(if replaying { ReplayAction::Close } else { ReplayAction::Open }));
            }
        });
    });
}

fn combo(ui: &mut egui::Ui, id: &str, any: &str, current: Option<&str>, options: &[String], mut pick: impl FnMut(Option<String>)) {
    egui::ComboBox::from_id_salt(id).selected_text(current.unwrap_or(&format!("any {any}"))).width(120.0).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_none(), format!("any {any}")).clicked() {
            pick(None);
        }
        for option in options.iter().take(200) {
            if ui.selectable_label(current == Some(option.as_str()), option).clicked() {
                pick(Some(option.clone()));
            }
        }
    });
}

fn lineup_list(ui: &mut egui::Ui, status: &mut Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::left("lineups").exact_size(LEFT).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong(format!("LINEUPS  {}", status.lineups.len()));
            ui.add(egui::TextEdit::singleline(status.search).hint_text("search places, players").desired_width(f32::INFINITY));
        });
        let search = status.search.to_lowercase();
        let shown: Vec<&Lineup> = status
            .lineups
            .iter()
            .filter(|l| {
                let has = |f: &str| f.to_lowercase().contains(&search);
                search.is_empty()
                    || [&l.from, &l.to].iter().any(|f| f.as_deref().is_some_and(has))
                    || l.technique.is_some_and(|t| has(&t.name()))
                    || l.players.iter().chain(&l.teams).any(|f| has(f))
            })
            .collect();
        let row = 20.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, shown.len(), |ui, range| {
            for l in &shown[range] {
                let selected = status.selected == Some(l.id);
                let text = egui::RichText::new(format!("{} > {}", place(&l.from), place(&l.to)));
                let response = ui
                    .horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, row), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, nade_color(l.grenade));
                        let label = ui.selectable_label(selected, text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(format!("{} {}", l.side.map_or("", Side::name), l.throws));
                        });
                        label
                    })
                    .inner;
                if response.clicked() {
                    actions.push(Action::Select(l.id));
                }
                if selected && status.prepared.is_none() {
                    response.scroll_to_me(None);
                }
            }
        });
    });
}

fn details(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::right("lineup").exact_size(RIGHT).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        if let Some(line) = &status.import {
            ui.label(egui::RichText::new(line).strong());
            ui.separator();
        }
        let Some(lineup) = status.selected.and_then(|id| status.lineups.iter().find(|l| l.id == id)) else {
            ui.weak("Pick a lineup.");
            if let Some(notice) = status.notice {
                ui.colored_label(egui::Color32::from_rgb(255, 170, 90), notice);
            }
            return;
        };
        let example = status.facts.and_then(|f| f.throw(&lineup.example));
        let derived = status.facts.and_then(|f| f.derived(&lineup.example));
        ui.heading(egui::RichText::new(format!("{} · {}", lineup.grenade.name(), lineup.side.map_or("?", Side::name))).color(nade_color(lineup.grenade)));
        ui.label(format!("{} > {}", place(&lineup.from), place(&lineup.to)));
        ui.separator();
        egui::Grid::new("details").num_columns(2).spacing([10.0, 4.0]).max_col_width(RIGHT - 90.0).show(ui, |ui| {
            ui.weak("thrower");
            ui.label(format!("{} · {}", example.and_then(|t| t.player.name.as_deref()).unwrap_or("?"), example.and_then(|t| t.team.as_deref()).unwrap_or("?")));
            ui.end_row();
            ui.weak("technique");
            ui.label(derived.and_then(|d| d.technique).or(lineup.technique).map_or_else(|| "?".to_string(), |t| t.name()));
            ui.end_row();
            ui.weak("thrown");
            ui.label(format!("{} times in {} demo{}", lineup.throws, lineup.demos, if lineup.demos == 1 { "" } else { "s" }));
            ui.end_row();
            if let Some(p) = status.prepared {
                ui.weak("throw");
                let demo = p.demo.as_ref().map_or_else(|| p.throw.demo.0.clone(), |d| d.file_name.clone().unwrap_or_else(|| d.id.0.clone()));
                ui.label(format!("round {}, {demo}", p.throw.round.map_or("?".into(), |r| r.to_string())));
                ui.end_row();
                if let Some(airtime) = p.derived.airtime {
                    ui.weak("airtime");
                    ui.label(format!("{airtime:.2} s"));
                    ui.end_row();
                }
            }
        });
        if !lineup.teams.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(lineup.teams.join(", ")).weak().small()).wrap());
        }
        ui.separator();
        match derived.and_then(|d| d.practice.as_ref()) {
            Some(practice) => {
                ui.add(egui::Label::new(egui::RichText::new(&practice.command).monospace().small()).wrap());
                ui.horizontal(|ui| {
                    if ui.button(if status.copied { "Copied" } else { "Copy" }).clicked() {
                        actions.push(Action::Copy(practice.command.clone()));
                    }
                });
                if !practice.ready {
                    ui.weak("Thrown on the move: the run started before this spot.");
                }
            }
            None => {
                ui.weak("no stance recorded for a practice command");
            }
        }
        // the producer's own labels, shown as they are
        if let Some(t) = example.filter(|t| !t.tags.is_empty()) {
            let tags: Vec<String> = t.tags.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            ui.add(egui::Label::new(egui::RichText::new(tags.join(" · ")).weak().small()).wrap());
        }
        ui.separator();
        ui.horizontal(|ui| {
            for view in ViewMode::ALL {
                if ui.selectable_label(status.view == view, view.name()).clicked() {
                    actions.push(Action::View(view));
                }
            }
        });
        if let Some(p) = status.prepared {
            let source = match &p.result.source {
                Source::Opponent { player, .. } => format!("Result: {}'s view", player.as_deref().unwrap_or("an opponent")),
                Source::Victim { seconds } => format!("Result: the player it blinded longest ({seconds:.1} s)"),
                Source::Overview => "Result: a spot with a clear view".into(),
            };
            ui.weak(source);
            ui.weak(format!("from {} samples of the other side", p.viewers));
            if let Some(seen) = p.result.seen {
                ui.weak(format!("sees {:.0}% of the effect", seen * 100.0));
            }
            if !p.stage.missing.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(255, 170, 90), format!("not recorded: {}", p.stage.missing.join(", ")));
            }
        } else {
            ui.weak("loading the throw…");
        }
        if let Some(notice) = status.notice {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(255, 170, 90), notice);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add(egui::Label::new(egui::RichText::new("arrow keys: lineups · 1-4: views · space: play · drag to orbit").weak().small()).wrap());
        });
    });
}

fn bottom_bar(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::bottom("time").exact_size(BOTTOM).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            if ui.add_sized([56.0, 22.0], egui::Button::new(if status.playing { "Pause" } else { "Play" })).clicked() {
                actions.push(Action::Play(!status.playing));
            }
            let (from, to) = status.timeline;
            let mut time = status.time;
            // the timings take the right, the moment's words a fixed width, the slider the rest
            ui.spacing_mut().slider_width = (ui.available_width() - 640.0).max(120.0);
            if ui.add(egui::Slider::new(&mut time, from..=to).show_value(false)).changed() {
                actions.push(Action::Time(time));
            }
            let moment = if time < 0.0 { format!("{:.1} s before it goes off", -time) } else { format!("{time:.1} s after it went off") };
            ui.add_sized([170.0, 20.0], egui::Label::new(moment));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&status.timings).weak()).truncate());
            });
        });
    });
}

/// CS2's default crosshair: four short green bars around a gap, with a dark outline.
fn crosshair(painter: &egui::Painter, at: egui::Pos2) {
    let (gap, length) = (4.0, 7.0);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let from = at + egui::vec2(dx, dy) * gap;
        let to = at + egui::vec2(dx, dy) * (gap + length);
        painter.line_segment([from, to], egui::Stroke::new(3.0, egui::Color32::from_black_alpha(160)));
        painter.line_segment([from, to], egui::Stroke::new(1.5, egui::Color32::from_rgb(50, 250, 50)));
    }
}
