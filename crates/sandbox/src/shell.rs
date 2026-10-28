//! The shell (egui): the map and the views along the top, the throw tool and the scene's throws on the left, the
//! selected throw on the right, the scene's clock along the bottom, and the 3D view in the middle, with the crosshair
//! while aiming. It reads the simulator's state and returns what the person asked for; the window applies it.

use nades_control::scene::{Click, Grenade, Movement, SceneThrow, ViewName};

use crate::scene::{SceneState, Tool};

/// Whether the shell shows: off by default, so the window is the 3D view alone and the simulator is driven through
/// MCP (S34); `--ui` or F1 shows it, for development.
static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn shown() -> bool {
    SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_shown(on: bool) {
    SHOWN.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Fixed panel sizes (points), so the 3D view's size is known before the first frame.
pub const LEFT: f32 = 300.0;
pub const RIGHT: f32 = 290.0;
pub const TOP: f32 = 34.0;
pub const BOTTOM: f32 = 46.0;

/// What the person asked for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Map(String),
    Copy(String),
    /// The throw tool's settings.
    Tool(Tool),
    /// A throw from the camera, with the tool's settings.
    Throw,
    Select(usize),
    Edit(usize, SceneThrow),
    /// Throw `k` from where the camera stands and looks.
    Reaim(usize),
    Duplicate(usize),
    Delete(usize),
    View(ViewName),
    Time(f64),
    Play(bool),
    Arcs(bool),
    Save,
    Open,
    /// An empty scene on the map.
    Clear,
}

/// What the shell shows.
pub struct Status<'a> {
    pub map: &'a str,
    pub maps: &'a [String],
    pub notice: Option<&'a str>,
    /// One line of timings: the first frame, the maps on the GPU.
    pub timings: String,
    pub loading_map: Option<&'a str>,
    pub copied: bool,
    /// The scene, once the window has one (none while the shell is laid out before the window exists).
    pub scene: Option<&'a mut SceneState>,
    /// The practice command of the selected throw.
    pub practice: Option<String>,
    /// The camera was taken by hand or by app control.
    pub free: bool,
}

impl<'a> Status<'a> {
    /// The shell before anything is read: for laying it out once while the window is made.
    pub fn empty(map: &'a str, maps: &'a [String]) -> Self {
        Self { map, maps, notice: None, timings: String::new(), loading_map: None, copied: false, scene: None, practice: None, free: true }
    }
}

pub fn nade_color(grenade: Grenade) -> egui::Color32 {
    match grenade {
        Grenade::Smoke => egui::Color32::from_rgb(0xd9, 0xdd, 0xe3),
        Grenade::Flash => egui::Color32::from_rgb(0xff, 0xe3, 0x6b),
        Grenade::He => egui::Color32::from_rgb(0xff, 0x5a, 0x4f),
        Grenade::Molotov | Grenade::Incendiary => egui::Color32::from_rgb(0xff, 0x8a, 0x2a),
        Grenade::Decoy => egui::Color32::from_rgb(0x9a, 0xa6, 0xb2),
    }
}

/// A throw's line in the list.
fn line(k: usize, t: &SceneThrow) -> String {
    let crouch = if t.crouch { " crouched" } else { "" };
    let label = t.label.as_deref().map(|l| format!(" — {l}")).unwrap_or_default();
    format!("{}. {} · {}{crouch} · {} · {:.1} s{label}", k + 1, t.grenade.name(), t.movement.name(), t.click.name(), t.start)
}

/// Lays the shell out; returns the actions and the 3D view's rectangle (points).
pub fn show(ui: &mut egui::Ui, mut status: Status<'_>) -> (Vec<Action>, egui::Rect) {
    let mut actions = Vec::new();
    top_bar(ui, &status, &mut actions);
    bottom_bar(ui, &status, &mut actions);
    egui::Panel::left("tool").exact_size(LEFT).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| left(ui, &mut status, &mut actions));
    });
    egui::Panel::right("selected").exact_size(RIGHT).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| right(ui, &status, &mut actions));
    });
    let rect = ui.available_rect_before_wrap();
    // the crosshair is the aim's: with the throw tool, or in the thrower's eye until the grenade leaves the hand
    let aiming = status.scene.as_ref().is_some_and(|s| match s.view {
        ViewName::Free => true,
        ViewName::Thrower => s.viewed().is_some_and(|(t, b)| s.time < t.start + b.path.release.t),
        _ => false,
    });
    if aiming {
        crosshair(ui.painter(), rect.center());
    }
    if status.scene.as_ref().is_some_and(|s| s.tool.picking) {
        ui.painter().text(rect.center_top() + egui::vec2(0.0, 12.0), egui::Align2::CENTER_TOP, "click where to stand", egui::FontId::proportional(16.0), egui::Color32::WHITE);
    }
    if let Some(map) = status.loading_map {
        ui.painter().text(rect.center_top() + egui::vec2(0.0, 36.0), egui::Align2::CENTER_TOP, format!("loading {map}…"), egui::FontId::proportional(18.0), egui::Color32::WHITE);
    }
    keys(ui, &status, &mut actions);
    (actions, rect)
}

fn keys(ui: &egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    let Some(s) = status.scene.as_ref() else { return };
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    ui.input(|i| {
        if i.key_pressed(egui::Key::Space) {
            actions.push(Action::Play(!s.playing));
        }
        if i.key_pressed(egui::Key::T) {
            actions.push(Action::Throw);
        }
        if i.key_pressed(egui::Key::Escape) {
            actions.push(Action::View(ViewName::Free));
        }
        for (key, view) in [(egui::Key::Num1, ViewName::Free), (egui::Key::Num2, ViewName::Thrower), (egui::Key::Num3, ViewName::Follow), (egui::Key::Num4, ViewName::Orbit)] {
            if i.key_pressed(key) {
                actions.push(Action::View(view));
            }
        }
        if let Some(k) = s.selected
            && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            actions.push(Action::Delete(k));
        }
    });
}

fn top_bar(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::top("maps").exact_size(TOP).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            egui::ComboBox::from_id_salt("map").selected_text(status.map).width(110.0).show_ui(ui, |ui| {
                for map in status.maps {
                    if ui.selectable_label(map == status.map, map).clicked() && map != status.map {
                        actions.push(Action::Map(map.clone()));
                    }
                }
            });
            let Some(s) = status.scene.as_ref() else { return };
            ui.separator();
            let shown = if status.free { ViewName::Free } else { s.view };
            for (view, label, hint) in [
                (ViewName::Free, "Free", "Fly anywhere: WASD, E/Q, Shift; hold the right button to look (1)"),
                (ViewName::Thrower, "Thrower", "The selected throw's eye as it lets go (2)"),
                (ViewName::Follow, "Follow", "Behind the grenade in flight, then over what it left (3)"),
                (ViewName::Orbit, "Orbit", "Around what the throw left: drag to turn, scroll to zoom (4)"),
            ] {
                let enabled = view == ViewName::Free || s.viewed().is_some();
                if ui.add_enabled(enabled, egui::Button::selectable(shown == view, label)).on_hover_text(hint).clicked() {
                    actions.push(Action::View(view));
                }
            }
            ui.separator();
            let mut arcs = s.arcs;
            if ui.checkbox(&mut arcs, "Arcs").on_hover_text("Draw every throw's flight").changed() {
                actions.push(Action::Arcs(arcs));
            }
        });
    });
}

fn left(ui: &mut egui::Ui, status: &mut Status<'_>, actions: &mut Vec<Action>) {
    if let Some(notice) = status.notice {
        ui.label(egui::RichText::new(notice).color(egui::Color32::from_rgb(255, 170, 90)));
    }
    let Some(s) = status.scene.as_mut() else { return };
    ui.heading("Throw");
    let mut tool = s.tool.clone();
    ui.horizontal_wrapped(|ui| {
        for g in Grenade::ALL {
            if ui.selectable_label(tool.grenade == g, egui::RichText::new(g.name()).color(nade_color(g))).clicked() {
                tool.grenade = g;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        for m in Movement::ALL {
            if ui.selectable_label(tool.movement == m, m.name()).clicked() {
                tool.movement = m;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut tool.crouch, "crouch");
        ui.separator();
        for c in Click::ALL {
            if ui.selectable_label(tool.click == c, c.name()).on_hover_text("left: a full throw; middle (both buttons): medium; right: an underhand lob").clicked() {
                tool.click = c;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("starts at");
        ui.add(egui::DragValue::new(&mut tool.start).range(0.0..=nades_control::scene::MAX_START).speed(0.05).suffix(" s"));
        ui.checkbox(&mut tool.preview, "preview arc");
    });
    ui.horizontal(|ui| {
        if ui.selectable_label(tool.picking, "Stand on a spot").on_hover_text("Click a point on the map to stand there; aim with the camera").clicked() {
            tool.picking = !tool.picking;
        }
        if ui.add(egui::Button::new(egui::RichText::new("Throw (T)").strong())).on_hover_text("Add the aimed throw to the scene").clicked() {
            actions.push(Action::Throw);
        }
    });
    if tool != s.tool {
        actions.push(Action::Tool(tool));
    }
    ui.label(egui::RichText::new("Stand where the camera stands (its eye 64 units over your feet, 46 crouched) and aim with it.").weak().small());
    ui.separator();
    ui.horizontal(|ui| {
        ui.heading("Scene");
        ui.label(egui::RichText::new(format!("{} throws", s.scene.throws.len())).weak());
    });
    for (k, t) in s.scene.throws.iter().enumerate() {
        let ready = s.built(t).is_some();
        let text = egui::RichText::new(line(k, t)).color(if ready { nade_color(t.grenade) } else { egui::Color32::GRAY });
        if ui.selectable_label(s.selected == Some(k), text).clicked() {
            actions.push(Action::Select(k));
        }
    }
    if s.scene.throws.is_empty() {
        ui.label(egui::RichText::new("No throws yet: aim and press Throw.").weak());
    }
    ui.separator();
    ui.label("Scene file");
    ui.add(egui::TextEdit::singleline(&mut s.file).desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            actions.push(Action::Save);
        }
        if ui.button("Open").on_hover_text("Or drop a scene file on the window").clicked() {
            actions.push(Action::Open);
        }
        if ui.button("New").on_hover_text("An empty scene on this map").clicked() {
            actions.push(Action::Clear);
        }
    });
    if let Some(notice) = &s.notice {
        ui.label(egui::RichText::new(notice).weak().small());
    }
}

fn right(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    let Some(s) = status.scene.as_ref() else { return };
    let Some(k) = s.selected.filter(|k| *k < s.scene.throws.len()) else {
        ui.label(egui::RichText::new("Select a throw in the scene to change it.").weak());
        return;
    };
    let t = &s.scene.throws[k];
    ui.heading(format!("Throw {}", k + 1));
    let mut edited = t.clone();
    ui.horizontal_wrapped(|ui| {
        for g in Grenade::ALL {
            if ui.selectable_label(edited.grenade == g, egui::RichText::new(g.name()).color(nade_color(g))).clicked() {
                edited.grenade = g;
            }
        }
    });
    egui::ComboBox::from_id_salt("movement").selected_text(edited.movement.name()).show_ui(ui, |ui| {
        for m in Movement::ALL {
            ui.selectable_value(&mut edited.movement, m, m.name());
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut edited.crouch, "crouch");
        for c in Click::ALL {
            ui.selectable_value(&mut edited.click, c, c.name());
        }
    });
    egui::Grid::new("where").num_columns(2).show(ui, |ui| {
        ui.label("starts at");
        ui.add(egui::DragValue::new(&mut edited.start).range(0.0..=nades_control::scene::MAX_START).speed(0.05).suffix(" s"));
        ui.end_row();
        ui.label("feet");
        ui.horizontal(|ui| {
            for v in &mut edited.feet {
                ui.add(egui::DragValue::new(v).speed(1.0).max_decimals(2));
            }
        });
        ui.end_row();
        ui.label("pitch, yaw");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut edited.pitch).range(-89.0..=89.0).speed(0.05).max_decimals(2));
            ui.add(egui::DragValue::new(&mut edited.yaw).speed(0.1).max_decimals(2));
        });
        ui.end_row();
    });
    if edited != *t {
        actions.push(Action::Edit(k, edited));
    }
    ui.horizontal(|ui| {
        if ui.button("Re-aim").on_hover_text("Throw it from where the camera stands and looks").clicked() {
            actions.push(Action::Reaim(k));
        }
        if ui.button("Duplicate").clicked() {
            actions.push(Action::Duplicate(k));
        }
        if ui.button("Delete").clicked() {
            actions.push(Action::Delete(k));
        }
    });
    ui.separator();
    match s.built(t) {
        Some(b) => {
            let d = b.path.detonation;
            ui.label(format!("goes off {:.2} s after it starts, at", d.t));
            ui.monospace(format!("{:.0} {:.0} {:.0}", d.pos.x, d.pos.y, d.pos.z));
            ui.label(format!("{} bounces · simulated in {:.0} ms", b.path.bounces.len(), b.ms));
        }
        None => {
            ui.label(egui::RichText::new("simulating…").weak());
        }
    }
    if let Some(command) = &status.practice {
        ui.separator();
        ui.label("Practice in CS2");
        ui.add(egui::Label::new(egui::RichText::new(command).monospace()).wrap());
        let label = if status.copied { "Copied" } else { "Copy" };
        if ui.button(label).clicked() {
            actions.push(Action::Copy(command.clone()));
        }
    }
}

fn bottom_bar(ui: &mut egui::Ui, status: &Status<'_>, actions: &mut Vec<Action>) {
    egui::Panel::bottom("time").exact_size(BOTTOM).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            if let Some(s) = status.scene.as_ref() {
                if ui.add_sized([56.0, 22.0], egui::Button::new(if s.playing { "Pause" } else { "Play" })).clicked() {
                    actions.push(Action::Play(!s.playing));
                }
                let end = s.end();
                let mut time = s.time;
                ui.spacing_mut().slider_width = (ui.available_width() - 520.0).max(120.0);
                if ui.add(egui::Slider::new(&mut time, 0.0..=end).show_value(false)).changed() {
                    actions.push(Action::Time(time));
                }
                ui.add_sized([110.0, 20.0], egui::Label::new(format!("{time:.2} / {end:.1} s")));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&status.timings).weak()).truncate());
            });
        });
    });
}

fn crosshair(painter: &egui::Painter, at: egui::Pos2) {
    let (gap, length) = (4.0, 7.0);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let from = at + egui::vec2(dx, dy) * gap;
        let to = at + egui::vec2(dx, dy) * (gap + length);
        painter.line_segment([from, to], egui::Stroke::new(3.0, egui::Color32::from_black_alpha(160)));
        painter.line_segment([from, to], egui::Stroke::new(1.5, egui::Color32::from_rgb(50, 250, 50)));
    }
}
