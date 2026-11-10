use std::path::PathBuf;
use std::time::Duration;

use egui::{Align, Color32, Context, Frame, Layout, RichText, Stroke, Ui};
use serde::{Deserialize, Serialize};

use crate::config::{SensorConfig, SourceKind};
use crate::editor::{Editor, EditorContext, EditorResult};
use crate::recording::{self, RecState, Recorder, RecordingEntry, ReviewSession};
use crate::scaling::ScalingMode;
use crate::sources::{self, nidaq, serial, Clock, SourceEvent};
use crate::theme::{self, *};
use crate::tiles::{self, Sensor, Status, TileAction, TILE_GAP, TILE_HEIGHT, TILE_MIN_WIDTH};

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    sensors: Vec<SensorConfig>,
    next_id: u64,
    rec_root: PathBuf,
    rec_name: String,
}

impl Default for Saved {
    fn default() -> Self {
        Self { sensors: Vec::new(), next_id: 1, rec_root: default_rec_root(), rec_name: String::new() }
    }
}

fn default_rec_root() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Sensor Plotter Recordings")
}

#[derive(Serialize, Deserialize)]
struct Profile {
    sensors: Vec<SensorConfig>,
}

pub struct SensorPlotterApp {
    clock: Clock,
    sensors: Vec<Sensor>,
    next_id: u64,
    expanded: Option<u64>,
    editor: Option<Editor>,
    confirm_delete: Option<u64>,

    recorder: Option<Recorder>,
    rec_root: PathBuf,
    rec_name: String,
    browser: Option<Vec<RecordingEntry>>,
    review: Option<ReviewSession>,
    review_expanded: Option<usize>,

    ni_channels: Option<Result<Vec<String>, String>>,
    ports: Vec<String>,
    toast: Option<(String, f64, bool)>,
}

impl SensorPlotterApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let saved: Saved = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        let next_id = saved.sensors.iter().map(|c| c.id + 1).max().unwrap_or(1).max(saved.next_id);
        Self {
            clock: Clock::new(),
            sensors: saved.sensors.into_iter().map(Sensor::new).collect(),
            next_id,
            expanded: None,
            editor: None,
            confirm_delete: None,
            recorder: None,
            rec_root: saved.rec_root,
            rec_name: saved.rec_name,
            browser: None,
            review: None,
            review_expanded: None,
            ni_channels: None,
            ports: serial::list_ports(),
            toast: None,
        }
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), self.clock.now() + if error { 8.0 } else { 4.0 }, error));
    }

    /// A live tile is expanded: the top bar is hidden so the graph fills the window.
    fn live_fullscreen(&self) -> bool {
        self.review.is_none() && self.expanded.is_some_and(|id| self.sensors.iter().any(|s| s.cfg.id == id))
    }

    fn review_fullscreen(&self) -> bool {
        self.review.as_ref().is_some_and(|r| self.review_expanded.is_some_and(|i| i < r.channels.len()))
    }

    fn sensor_mut(&mut self, id: u64) -> Option<&mut Sensor> {
        self.sensors.iter_mut().find(|s| s.cfg.id == id)
    }

    fn connect(&mut self, id: u64) {
        let clock = self.clock.clone();
        if let Some(s) = self.sensor_mut(id) {
            s.source = Some(sources::spawn(&s.cfg, &clock));
            s.status = Status::Connecting;
            s.clear_graph(clock.now());
            s.last = None;
        }
    }

    fn disconnect(&mut self, id: u64) {
        if let Some(s) = self.sensor_mut(id) {
            s.source = None;
            s.status = Status::Idle;
        }
    }

    /// Moves samples from the source threads into tiles and the recorder.
    fn drain_sources(&mut self) {
        let mut rec_error = None;
        for s in &mut self.sensors {
            let Some(src) = &s.source else { continue };
            let mut batch = Vec::new();
            while let Ok(ev) = src.rx.try_recv() {
                match ev {
                    SourceEvent::Connected(info) => {
                        s.link_info = info.clone();
                        s.status = Status::Live(info);
                    }
                    SourceEvent::Warning(msg) => s.status = Status::Warning(msg),
                    SourceEvent::Samples(v) => batch.extend(v),
                }
            }
            if batch.is_empty() {
                continue;
            }
            if matches!(s.status, Status::Warning(_) | Status::Connecting) {
                s.status = Status::Live(s.link_info.clone());
            }
            s.push(&batch);
            if let Some(rec) = &mut self.recorder
                && let Err(e) = rec.write(&s.cfg, &batch) {
                    rec_error = Some(e);
                }
        }
        if let Some(e) = rec_error {
            self.stop_recording();
            self.notify(format!("Recording stopped: {e}"), true);
        }
    }

    fn start_recording(&mut self) {
        match Recorder::start(&self.rec_root, &self.rec_name, self.clock.now()) {
            Ok(r) => {
                let dir = r.dir.display().to_string();
                self.recorder = Some(r);
                let none_live = !self.sensors.iter().any(Sensor::connected);
                self.notify(
                    if none_live {
                        format!("Recording to {dir} — connect sensors to capture data")
                    } else {
                        format!("Recording to {dir}")
                    },
                    false,
                );
            }
            Err(e) => self.notify(e, true),
        }
    }

    fn stop_recording(&mut self) {
        if let Some(rec) = self.recorder.take() {
            match rec.stop(self.clock.now()) {
                Ok(dir) => self.notify(format!("Saved recording to {}", dir.display()), false),
                Err(e) => self.notify(e, true),
            }
        }
    }

    fn open_review(&mut self, result: Result<ReviewSession, String>) {
        match result {
            Ok(r) if r.channels.is_empty() => self.notify("That recording has no sensor data", true),
            Ok(r) => {
                self.review = Some(r);
                self.review_expanded = None;
                self.browser = None;
            }
            Err(e) => self.notify(e, true),
        }
    }

    fn apply_tile_action(&mut self, id: u64, act: TileAction) {
        match act {
            TileAction::Connect => self.connect(id),
            TileAction::Disconnect => self.disconnect(id),
            TileAction::Edit => {
                if let Some(s) = self.sensor_mut(id) {
                    self.editor = Some(Editor::new(s.cfg.clone(), false));
                }
            }
            TileAction::Delete => self.confirm_delete = Some(id),
            TileAction::Expand => self.expanded = Some(id),
            TileAction::Collapse => self.expanded = None,
            TileAction::Tare => {
                if let Some(s) = self.sensor_mut(id)
                    && let Some(v) = s.value() {
                        s.cfg.offset -= v;
                    }
            }
            TileAction::Clear => {
                let now = self.clock.now();
                if let Some(s) = self.sensor_mut(id) {
                    s.clear_graph(now);
                }
            }
            TileAction::SetPoints(n) => {
                if let Some(s) = self.sensor_mut(id) {
                    s.cfg.visible_points = n;
                    s.trim();
                    s.reset_view();
                }
            }
            TileAction::ToggleRaw | TileAction::ToggleAvg => {
                if let Some(s) = self.sensor_mut(id) {
                    let cfg = &mut s.cfg;
                    if matches!(act, TileAction::ToggleRaw) {
                        cfg.show_raw = !cfg.show_raw;
                        cfg.show_avg |= !cfg.show_raw;
                    } else {
                        cfg.show_avg = !cfg.show_avg;
                        cfg.show_raw |= !cfg.show_avg;
                    }
                    s.y_view.set(None);
                }
            }
            TileAction::SetAvgPoints(n) => {
                if let Some(s) = self.sensor_mut(id) {
                    s.cfg.avg_points = n;
                }
            }
            TileAction::SetScrollMode(mode) => {
                if let Some(s) = self.sensor_mut(id) {
                    s.cfg.scroll_mode = mode;
                    s.reset_view();
                }
            }
            TileAction::SetStepPoints(n) => {
                if let Some(s) = self.sensor_mut(id) {
                    s.cfg.step_points = n;
                }
            }
        }
    }

    fn save_editor(&mut self, mut cfg: SensorConfig, is_new: bool) {
        if is_new {
            cfg.id = self.next_id;
            self.next_id += 1;
            self.sensors.push(Sensor::new(cfg));
            return;
        }
        let id = cfg.id;
        let Some(s) = self.sensor_mut(id) else { return };
        let reconnect = s.connected() && s.cfg.connection_key() != cfg.connection_key();
        if !cfg.show_raw && !cfg.show_avg {
            cfg.show_raw = true;
        }
        s.cfg = cfg;
        s.trim();
        s.reset_view();
        if reconnect {
            self.connect(id);
        }
    }

    fn add_demo(&mut self) {
        let mut cfg = SensorConfig {
            id: self.next_id,
            name: format!("Demo pressure {}", self.next_id),
            kind: SourceKind::Simulated,
            unit: "psi".into(),
            range_min: 0.0,
            range_max: 5000.0,
            ..Default::default()
        };
        cfg.scaling.mode = ScalingMode::Range;
        cfg.scaling.raw_lo = 0.0;
        cfg.scaling.raw_hi = 10.0;
        cfg.scaling.eng_lo = 0.0;
        cfg.scaling.eng_hi = 5000.0;
        self.next_id += 1;
        let id = cfg.id;
        self.sensors.push(Sensor::new(cfg));
        self.connect(id);
    }

    fn save_profile(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Sensor profile", &["json"]).set_file_name("sensors.json").save_file() else {
            return;
        };
        let profile = Profile { sensors: self.sensors.iter().map(|s| s.cfg.clone()).collect() };
        let result = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string()).and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.notify(format!("Saved profile {}", path.display()), false),
            Err(e) => self.notify(format!("Could not save profile: {e}"), true),
        }
    }

    fn load_profile(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Sensor profile", &["json"]).pick_file() else { return };
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Profile>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(p) => {
                self.sensors.clear();
                self.expanded = None;
                for mut cfg in p.sensors {
                    cfg.id = self.next_id;
                    self.next_id += 1;
                    self.sensors.push(Sensor::new(cfg));
                }
                self.notify(format!("Loaded {}", path.display()), false);
            }
            Err(e) => self.notify(format!("Could not load profile: {e}"), true),
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        egui::Panel::top("topbar")
            .frame(Frame::new().fill(BG_TOP).inner_margin(egui::Margin::symmetric(24, 14)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    brand(ui);
                    ui.add_space(24.0);
                    if primary(ui, "+ Add sensor").clicked() {
                        self.editor = Some(Editor::new(SensorConfig::default(), true));
                    }
                    let any = !self.sensors.is_empty();
                    if ui.add_enabled(any, egui::Button::new("Connect all")).clicked() {
                        let ids: Vec<u64> = self.sensors.iter().filter(|s| !s.connected()).map(|s| s.cfg.id).collect();
                        ids.into_iter().for_each(|id| self.connect(id));
                    }
                    if ui.add_enabled(any, egui::Button::new("Disconnect all")).clicked() {
                        let ids: Vec<u64> = self.sensors.iter().map(|s| s.cfg.id).collect();
                        ids.into_iter().for_each(|id| self.disconnect(id));
                    }
                    ui.separator();
                    self.record_controls(ui);
                    ui.separator();
                    if ui.button("Recordings…").on_hover_text("Open previously recorded files").clicked() {
                        self.browser = Some(recording::list(&self.rec_root));
                    }
                    if ui.button("Save profile").clicked() {
                        self.save_profile();
                    }
                    if ui.button("Load profile").clicked() {
                        self.load_profile();
                    }
                });
            });
    }

    fn record_controls(&mut self, ui: &mut Ui) {
        let now = self.clock.now();
        match self.recorder.as_ref().map(|r| (r.state, r.recorded_secs(now))) {
            None => {
                ui.add(egui::TextEdit::singleline(&mut self.rec_name).hint_text("Recording name").desired_width(150.0));
                let btn = egui::Button::new(RichText::new("● Record").color(Color32::WHITE)).fill(Color32::from_rgb(0xb3, 0x26, 0x26));
                if ui.add(btn).on_hover_text("Start writing connected sensors to disk").clicked() {
                    self.start_recording();
                }
            }
            Some((state, secs)) => {
                let (label, color) = match state {
                    RecState::Recording => ("● REC", ERR),
                    RecState::Paused => ("❚❚ PAUSED", WARN),
                };
                ui.label(RichText::new(format!("{label} {}", tiles::fmt_duration(secs))).strong().color(color));
                match state {
                    RecState::Recording => {
                        if ui.button("❚❚ Pause").clicked()
                            && let Some(r) = &mut self.recorder {
                                r.pause(now);
                            }
                    }
                    RecState::Paused => {
                        if primary(ui, "▶ Resume").clicked()
                            && let Some(r) = &mut self.recorder {
                                r.resume(now);
                            }
                    }
                }
                if ui.button("■ Stop").clicked() {
                    self.stop_recording();
                }
            }
        }
    }

    fn board(&mut self, ui: &mut Ui) {
        let mut actions: Vec<(u64, TileAction)> = Vec::new();
        let recording = self.recorder.as_ref().is_some_and(|r| r.state == RecState::Recording);
        let now = self.clock.now();
        let rec_label = self.recorder.as_ref().map(|r| match r.state {
            RecState::Recording => (format!("● REC {}", tiles::fmt_duration(r.recorded_secs(now))), ERR),
            RecState::Paused => (format!("❚❚ PAUSED {}", tiles::fmt_duration(r.recorded_secs(now))), WARN),
        });
        let mut add_clicked = false;
        let mut demo_clicked = false;
        let mut open_clicked = false;
        let full = self.live_fullscreen();

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(if full { 6 } else { 24 }))
            .show(ui, |ui| {
                if self.sensors.is_empty() {
                    (add_clicked, demo_clicked, open_clicked) = empty_state(ui);
                    return;
                }
                if let Some(id) = self.expanded
                    && let Some(s) = self.sensors.iter().find(|s| s.cfg.id == id) {
                        tiles::tile_frame().inner_margin(10).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_height(ui.available_height());
                            if let Some(a) = tiles::live_tile_full(ui, s, now, rec_label) {
                                actions.push((id, a));
                            }
                        });
                        return;
                    }
                tile_grid(ui, self.sensors.len(), |ui, i| {
                    let s = &self.sensors[i];
                    if let Some(a) = tiles::live_tile(ui, s, now, recording) {
                        actions.push((s.cfg.id, a));
                    }
                });
            });

        for (id, a) in actions {
            self.apply_tile_action(id, a);
        }
        if add_clicked {
            self.editor = Some(Editor::new(SensorConfig::default(), true));
        }
        if demo_clicked {
            self.add_demo();
        }
        if open_clicked {
            self.browser = Some(recording::list(&self.rec_root));
        }
    }

    fn review_board(&mut self, ui: &mut Ui) {
        let mut close = false;
        let mut expand: Option<Option<usize>> = None;
        let full = self.review_fullscreen();
        let Some(review) = &self.review else { return };

        if !full {
            egui::Panel::top("review_bar")
                .frame(Frame::new().fill(Color32::from_rgb(0x1f, 0x2a, 0x33)).inner_margin(egui::Margin::symmetric(24, 10)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Reviewing").color(ACCENT_2).strong());
                        ui.label(RichText::new(&review.title).strong());
                        ui.label(RichText::new(review.path.display().to_string()).size(12.0).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if primary(ui, "Back to live").clicked() {
                                close = true;
                            }
                            if ui.button("Show in Explorer").clicked() {
                                let target = if review.path.is_dir() { review.path.clone() } else { review.path.parent().map(|p| p.to_path_buf()).unwrap_or_default() };
                                let _ = std::process::Command::new("explorer").arg(target).spawn();
                            }
                        });
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(if full { 6 } else { 24 }))
            .show(ui, |ui| {
                if let Some(i) = self.review_expanded.filter(|&i| i < review.channels.len()) {
                    tiles::tile_frame().inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_height(ui.available_height());
                        if let Some(TileAction::Collapse) = tiles::review_tile(ui, &review.channels[i], i, true) {
                            expand = Some(None);
                        }
                    });
                    return;
                }
                tile_grid(ui, review.channels.len(), |ui, i| {
                    if let Some(TileAction::Expand) = tiles::review_tile(ui, &review.channels[i], i, false) {
                        expand = Some(Some(i));
                    }
                });
            });

        if let Some(e) = expand {
            self.review_expanded = e;
        }
        if close {
            self.review = None;
        }
    }

    fn editor_modal(&mut self, ctx: &Context) {
        let Some(editor) = &mut self.editor else { return };
        let live_raw = self
            .sensors
            .iter()
            .find(|s| !editor.is_new && s.cfg.id == editor.cfg.id && s.connected())
            .and_then(|s| s.last.map(|l| l.raw));
        let mut rescan_ni = false;
        let mut rescan_ports = false;
        if editor.cfg.kind.is_ni() && self.ni_channels.is_none() {
            rescan_ni = true;
        }
        let modal = egui::Modal::new(egui::Id::unique("editor")).frame(modal_frame()).show(ctx, |ui| {
            editor.show(
                ui,
                EditorContext {
                    ni_channels: &self.ni_channels,
                    ports: &self.ports,
                    live_raw,
                    rescan_ni: &mut rescan_ni,
                    rescan_ports: &mut rescan_ports,
                },
            )
        });
        if rescan_ni {
            self.ni_channels = Some(nidaq::list_ai_channels());
        }
        if rescan_ports {
            self.ports = serial::list_ports();
        }
        match modal.inner {
            EditorResult::Open => {}
            EditorResult::Cancel => self.editor = None,
            EditorResult::Save => {
                if let Some(ed) = self.editor.take() {
                    self.save_editor(ed.cfg, ed.is_new);
                }
            }
        }
    }

    fn delete_modal(&mut self, ctx: &Context) {
        let Some(id) = self.confirm_delete else { return };
        let name = self.sensors.iter().find(|s| s.cfg.id == id).map(|s| s.cfg.name.clone()).unwrap_or_default();
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("confirm_delete")).frame(modal_frame()).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Delete sensor?");
            ui.label(format!("“{name}” and its settings will be removed. Recorded files are not affected."));
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let del = egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(Color32::from_rgb(0xb3, 0x26, 0x26));
                if ui.add(del).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.sensors.retain(|s| s.cfg.id != id);
                if self.expanded == Some(id) {
                    self.expanded = None;
                }
                self.confirm_delete = None;
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    fn browser_modal(&mut self, ctx: &Context) {
        let Some(entries) = &self.browser else { return };
        let mut open: Option<PathBuf> = None;
        let mut open_csv = false;
        let mut open_folder = false;
        let mut change_root = false;
        let mut close = false;

        let resp = egui::Modal::new(egui::Id::unique("recordings")).frame(modal_frame()).show(ctx, |ui| {
            ui.set_width(820.0);
            ui.horizontal(|ui| {
                ui.heading("Recordings");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("✕").clicked() {
                        close = true;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.rec_root.display().to_string()).color(MUTED));
                if ui.button("Change folder…").clicked() {
                    change_root = true;
                }
                if ui.button("Open other folder…").clicked() {
                    open_folder = true;
                }
                if ui.button("Open CSV…").on_hover_text("Any CSV with time in the first column").clicked() {
                    open_csv = true;
                }
            });
            ui.separator();
            if entries.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("No recordings in this folder yet").color(MUTED)));
                ui.add_space(20.0);
                return;
            }
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                egui::Grid::new("rec_list").num_columns(5).striped(true).spacing([18.0, 8.0]).show(ui, |ui| {
                    for h in ["Name", "Started", "Length", "Sensors", ""] {
                        ui.label(RichText::new(h).color(MUTED));
                    }
                    ui.end_row();
                    for e in entries {
                        ui.label(RichText::new(&e.meta.name).strong());
                        ui.label(e.meta.started.format("%Y-%m-%d %H:%M:%S").to_string());
                        let len = if e.meta.ended.is_some() { tiles::fmt_duration(e.meta.duration_s) } else { "unfinished".into() };
                        ui.label(len);
                        let names: Vec<&str> = e.meta.sensors.iter().map(|s| s.config.name.as_str()).collect();
                        ui.allocate_ui(egui::vec2(260.0, 20.0), |ui| {
                            ui.add(egui::Label::new(names.join(", ")).truncate()).on_hover_text(names.join("\n"));
                        });
                        ui.horizontal(|ui| {
                            if primary(ui, "Open").clicked() {
                                open = Some(e.dir.clone());
                            }
                            if ui.button("Folder").clicked() {
                                let _ = std::process::Command::new("explorer").arg(&e.dir).spawn();
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        });
        if resp.should_close() {
            close = true;
        }

        if change_root
            && let Some(dir) = rfd::FileDialog::new().set_directory(&self.rec_root).pick_folder() {
                self.rec_root = dir;
                self.browser = Some(recording::list(&self.rec_root));
            }
        if open_folder {
            open = rfd::FileDialog::new().set_directory(&self.rec_root).pick_folder();
        }
        if let Some(dir) = open {
            self.open_review(recording::load_session(&dir));
        }
        if open_csv
            && let Some(file) = rfd::FileDialog::new().add_filter("CSV", &["csv", "txt"]).set_directory(&self.rec_root).pick_file() {
                self.open_review(recording::load_generic_csv(&file));
            }
        if close {
            self.browser = None;
        }
    }

    fn toast(&mut self, ctx: &Context) {
        let Some((msg, until, error)) = &self.toast else { return };
        if self.clock.now() > *until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -20.0])
            .show(ctx, |ui| {
                Frame::new()
                    .fill(BG_ELEV)
                    .stroke(Stroke::new(1.0, if *error { ERR } else { ACCENT }))
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.label(RichText::new(msg).color(if *error { ERR } else { TEXT }));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

impl eframe::App for SensorPlotterApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_sources();

        // Live graphs redraw every display refresh (vsync-paced) so scrolling
        // advances at an even cadence instead of on a drifting timer.
        if self.sensors.iter().any(Sensor::connected) {
            ctx.request_repaint();
        } else if self.recorder.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.editor.is_none() && self.browser.is_none() {
            self.expanded = None;
            self.review_expanded = None;
        }

        if !self.live_fullscreen() && !self.review_fullscreen() {
            self.top_bar(ui);
        }
        if self.review.is_some() {
            self.review_board(ui);
        } else {
            self.board(ui);
        }
        self.editor_modal(&ctx);
        self.delete_modal(&ctx);
        self.browser_modal(&ctx);
        self.toast(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let saved = Saved {
            sensors: self.sensors.iter().map(|s| s.cfg.clone()).collect(),
            next_id: self.next_id,
            rec_root: self.rec_root.clone(),
            rec_name: self.rec_name.clone(),
        };
        eframe::set_value(storage, eframe::APP_KEY, &saved);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_recording();
    }
}

fn brand(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
    let mesh = gradient_square(rect, ACCENT, ACCENT_2);
    ui.painter().add(mesh);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sensor Plotter").size(20.0).strong());
            ui.label(RichText::new(env!("CARGO_PKG_VERSION")).size(12.0).color(MUTED));
        });
        ui.label(RichText::new("Live sensor readings, scaling, and recording").size(12.0).color(MUTED));
    });
}

fn gradient_square(rect: egui::Rect, a: Color32, b: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), a);
    mesh.colored_vertex(rect.right_top(), a.lerp_to_gamma(b, 0.5));
    mesh.colored_vertex(rect.right_bottom(), b);
    mesh.colored_vertex(rect.left_bottom(), a.lerp_to_gamma(b, 0.5));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    egui::Shape::mesh(mesh)
}

fn primary(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(ACCENT).stroke(Stroke::new(1.0, ACCENT)))
}

fn modal_frame() -> Frame {
    Frame::new().fill(BG_ELEV).stroke(Stroke::new(1.0, LINE)).corner_radius(12).inner_margin(18)
}

/// Responsive grid of fixed-height tiles, at least TILE_MIN_WIDTH wide.
fn tile_grid(ui: &mut Ui, count: usize, mut tile: impl FnMut(&mut Ui, usize)) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let width = ui.available_width();
        let cols = (((width + TILE_GAP) / (TILE_MIN_WIDTH + TILE_GAP)).floor() as usize).max(1);
        let tile_w = (width - TILE_GAP * (cols - 1) as f32) / cols as f32;
        for row in (0..count).collect::<Vec<_>>().chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = TILE_GAP;
                for &i in row {
                    ui.allocate_ui_with_layout(egui::vec2(tile_w, TILE_HEIGHT), Layout::top_down(Align::Min), |ui| {
                        tiles::tile_frame().show(ui, |ui| {
                            ui.set_width(tile_w - 34.0);
                            ui.set_height(TILE_HEIGHT - 34.0);
                            ui.spacing_mut().item_spacing.y = 6.0;
                            tile(ui, i);
                        });
                    });
                }
            });
            ui.add_space(TILE_GAP);
        }
    });
}

fn empty_state(ui: &mut Ui) -> (bool, bool, bool) {
    let mut out = (false, false, false);
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.12);
        Frame::new()
            .stroke(Stroke::new(1.0, Color32::from_rgb(0x4a, 0x4a, 0x4a)))
            .fill(Color32::from_white_alpha(5))
            .corner_radius(16)
            .inner_margin(egui::Margin::symmetric(32, 40))
            .show(ui, |ui| {
                ui.set_max_width(600.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No sensors yet").size(28.0).strong());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Add an NI-DAQmx module channel (0-10 V, 4-20 mA), an RS485 / serial instrument, \
                             or a Modbus device. Set its units and scaling, then connect to watch it live and record.",
                        )
                        .color(MUTED),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        let w = 3.0 * 150.0;
                        ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                        out.0 = primary(ui, "Add sensor").clicked();
                        out.1 = ui.button("Add simulated demo").clicked();
                        out.2 = ui.button("Open recording").clicked();
                    });
                });
            });
    });
    out
}
