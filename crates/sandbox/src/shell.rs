//! The shell (egui): the map along the top, the panels either side, the timings along the bottom, and the 3D view in
//! the middle. It reads the sandbox's state and returns what the person asked for; the window applies it.

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
}

impl<'a> Status<'a> {
    /// The shell before anything is read: for laying it out once while the window is made.
    pub fn empty(map: &'a str, maps: &'a [String]) -> Self {
        Self { map, maps, notice: None, timings: String::new(), loading_map: None, copied: false }
    }
}

/// Lays the shell out; returns the actions and the 3D view's rectangle (points).
pub fn show(ui: &mut egui::Ui, status: Status<'_>) -> (Vec<Action>, egui::Rect) {
    let mut actions = Vec::new();
    top_bar(ui, &status, &mut actions);
    bottom_bar(ui, &status);
    egui::Panel::left("left").exact_size(LEFT).show(ui, |ui| {
        if let Some(notice) = status.notice {
            ui.label(egui::RichText::new(notice).color(egui::Color32::from_rgb(255, 170, 90)));
        }
    });
    egui::Panel::right("right").exact_size(RIGHT).show(ui, |_| {});
    let rect = ui.available_rect_before_wrap();
    if let Some(map) = status.loading_map {
        ui.painter().text(rect.center_top() + egui::vec2(0.0, 24.0), egui::Align2::CENTER_TOP, format!("loading {map}…"), egui::FontId::proportional(18.0), egui::Color32::WHITE);
    }
    (actions, rect)
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
        });
    });
}

fn bottom_bar(ui: &mut egui::Ui, status: &Status<'_>) {
    egui::Panel::bottom("time").exact_size(BOTTOM).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&status.timings).weak()).truncate());
            });
        });
    });
}
