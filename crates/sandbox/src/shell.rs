//! The shell (egui): the map and the filters along the top, the lineup list on the left, the lineup's details and the
//! four views on the right, the time scrubber along the bottom, and the 3D view in the middle (with the crosshair in
//! the thrower's views). It reads the sandbox's state and returns what the person asked for; the window applies it.

use nades_data::{Lineup, Nade, Phase, Side};
use nades_scene::camera::Source;
use nades_scene::stage::ViewMode;

use crate::library::Prepared;

/// Fixed panel sizes (points), so the 3D view's size is known before the first frame.
pub const LEFT: f32 = 300.0;
pub const RIGHT: f32 = 290.0;
pub const TOP: f32 = 34.0;
pub const BOTTOM: f32 = 46.0;

/// What the person asked for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Map(String),
    Side(Option<Side>),
    Nade(Option<Nade>),
    Team(Option<String>),
    Player(Option<String>),
    Phase(Option<Phase>),
    MinThrows(i64),
    Select(String),
    Step(i32),
    View(ViewMode),
    Time(f64),
    Play(bool),
    Copy(String),
}

/// What the shell shows.
pub struct Status<'a> {
    pub map: &'a str,
    pub maps: &'a [String],
    pub side: Option<Side>,
    pub nade: Option<Nade>,
    pub team: Option<&'a str>,
    pub player: Option<&'a str>,
    pub phase: Option<Phase>,
    pub min_throws: i64,
    pub teams: &'a [String],
    pub players: &'a [String],
    pub lineups: &'a [Lineup],
    pub search: &'a mut String,
    pub selected: Option<&'a str>,
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
}

impl<'a> Status<'a> {
    /// The shell before anything is read: for laying it out once while the window is made.
    pub fn empty(map: &'a str, maps: &'a [String], search: &'a mut String) -> Self {
        Self {
            map,
            maps,
            side: None,
            nade: None,
            team: None,
            player: None,
            phase: None,
            min_throws: 1,
            teams: &[],
            players: &[],
            lineups: &[],
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
        }
    }
}

fn nade_color(nade: &str) -> egui::Color32 {
    match nade {
        "smoke" => egui::Color32::from_rgb(0xd9, 0xdd, 0xe3),
        "flash" => egui::Color32::from_rgb(0xff, 0xe3, 0x6b),
        "he" => egui::Color32::from_rgb(0xff, 0x5a, 0x4f),
        "molotov" => egui::Color32::from_rgb(0xff, 0x8a, 0x2a),
        _ => egui::Color32::from_rgb(0x9a, 0xa6, 0xb2),
    }
}

fn place(p: &Option<String>) -> &str {
    p.as_deref().filter(|s| !s.is_empty()).unwrap_or("?")
}

/// Lays the shell out; returns the actions and the 3D view's rectangle (points).
pub fn show(ui: &mut egui::Ui, mut status: Status<'_>) -> (Vec<Action>, egui::Rect) {
    let mut actions = Vec::new();
    top_bar(ui, &status, &mut actions);
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
            for (label, side) in [("Both", None), ("T", Some(Side::T)), ("CT", Some(Side::Ct))] {
                if ui.selectable_label(status.side == side, label).clicked() {
                    actions.push(Action::Side(side));
                }
            }
            ui.separator();
            if ui.selectable_label(status.nade.is_none(), "All").clicked() {
                actions.push(Action::Nade(None));
            }
            for nade in [Nade::Smoke, Nade::Flash, Nade::Molotov, Nade::He] {
                let text = egui::RichText::new(match nade {
                    Nade::He => "HE",
                    other => other.name(),
                })
                .color(nade_color(nade.name()));
                if ui.selectable_label(status.nade == Some(nade), text).clicked() {
                    actions.push(Action::Nade(Some(nade)));
                }
            }
            ui.separator();
            combo(ui, "team", "team", status.team, status.teams, |t| actions.push(Action::Team(t)));
            combo(ui, "player", "player", status.player, status.players, |p| actions.push(Action::Player(p)));
            egui::ComboBox::from_id_salt("phase").selected_text(status.phase.map_or("any phase", Phase::name)).width(90.0).show_ui(ui, |ui| {
                if ui.selectable_label(status.phase.is_none(), "any phase").clicked() {
                    actions.push(Action::Phase(None));
                }
                for phase in Phase::ALL {
                    if ui.selectable_label(status.phase == Some(phase), phase.name()).clicked() {
                        actions.push(Action::Phase(Some(phase)));
                    }
                }
            });
            ui.separator();
            let mut min = status.min_throws;
            ui.label("thrown ≥");
            if ui.add(egui::DragValue::new(&mut min).range(1..=50)).changed() {
                actions.push(Action::MinThrows(min));
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
                search.is_empty()
                    || [&l.from_place, &l.to_place, &l.technique, &l.player_list, &l.team_list].iter().any(|f| f.as_deref().is_some_and(|f| f.to_lowercase().contains(&search)))
            })
            .collect();
        let row = 20.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, shown.len(), |ui, range| {
            for l in &shown[range] {
                let selected = status.selected == Some(l.lineup_id.as_str());
                let text = egui::RichText::new(format!("{} > {}", place(&l.from_place), place(&l.to_place)));
                let response = ui
                    .horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, row), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, nade_color(&l.nade));
                        let label = ui.selectable_label(selected, text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(format!("{} {}", l.side.as_deref().unwrap_or(""), l.throws));
                        });
                        label
                    })
                    .inner;
                if response.clicked() {
                    actions.push(Action::Select(l.lineup_id.clone()));
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
        let Some(lineup) = status.selected.and_then(|id| status.lineups.iter().find(|l| l.lineup_id == id)) else {
            ui.weak("Pick a lineup.");
            if let Some(notice) = status.notice {
                ui.colored_label(egui::Color32::from_rgb(255, 170, 90), notice);
            }
            return;
        };
        ui.heading(egui::RichText::new(format!("{} · {}", lineup.nade, lineup.side.as_deref().unwrap_or("?"))).color(nade_color(&lineup.nade)));
        ui.label(format!("{} > {}", place(&lineup.from_place), place(&lineup.to_place)));
        ui.separator();
        egui::Grid::new("details").num_columns(2).spacing([10.0, 4.0]).max_col_width(RIGHT - 90.0).show(ui, |ui| {
            ui.weak("thrower");
            ui.label(format!("{} · {}", lineup.example_player.as_deref().unwrap_or("?"), lineup.example_team.as_deref().unwrap_or("?")));
            ui.end_row();
            ui.weak("technique");
            ui.label(lineup.example_technique.as_deref().or(lineup.technique.as_deref()).unwrap_or("?"));
            ui.end_row();
            ui.weak("thrown");
            ui.label(format!("{} times in {} matches", lineup.throws, lineup.matches));
            ui.end_row();
            if let Some(p) = status.prepared {
                ui.weak("throw");
                ui.label(format!("round {}, {}", p.throw.round.map_or("?".into(), |r| r.to_string()), p.throw.event.as_deref().unwrap_or("")));
                ui.end_row();
                if let Some(airtime) = p.throw.airtime {
                    ui.weak("airtime");
                    ui.label(format!("{airtime:.2} s"));
                    ui.end_row();
                }
            }
        });
        if let Some(teams) = &lineup.team_list {
            ui.add(egui::Label::new(egui::RichText::new(teams).weak().small()).wrap());
        }
        ui.separator();
        match &lineup.practice_cmd {
            Some(cmd) => {
                ui.add(egui::Label::new(egui::RichText::new(cmd).monospace().small()).wrap());
                ui.horizontal(|ui| {
                    if ui.button(if status.copied { "Copied" } else { "Copy" }).clicked() {
                        actions.push(Action::Copy(cmd.clone()));
                    }
                });
                if !lineup.practice_ready {
                    ui.weak("Thrown on the move: this is where the run started.");
                }
            }
            None => {
                ui.weak("no stance recorded for a practice command");
            }
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
