use std::cell::Cell;
use std::collections::VecDeque;

use egui::{Align, Color32, CornerRadius, DragValue, Frame, Layout, Margin, RichText, Stroke, Ui};
use egui_plot::{AxisHints, HLine, HPlacement, HoverPosition, Line, LineStyle, Plot, PlotBounds, PlotPoint, PlotPoints};

use crate::config::{ScrollMode, SensorConfig};
use crate::recording::ReviewChannel;
use crate::sources::{Sample, SourceHandle};
use crate::theme::*;

pub const TILE_MIN_WIDTH: f32 = 380.0;
pub const TILE_HEIGHT: f32 = 360.0;
pub const TILE_GAP: f32 = 16.0;

pub enum Status {
    Idle,
    Connecting,
    Live(String),
    Warning(String),
}

pub struct Sensor {
    pub cfg: SensorConfig,
    pub source: Option<SourceHandle>,
    pub status: Status,
    pub history: VecDeque<Sample>,
    /// Graph x = 0 at this app-clock time (when the sensor was connected).
    pub origin: f64,
    pub last: Option<Sample>,
    /// Description from the most recent successful connection.
    pub link_info: String,
    /// Displayed Y range, kept between frames so the axis doesn't jump.
    pub y_view: Cell<Option<(f64, f64)>>,
    /// Displayed time span in seconds, kept between frames.
    pub x_span: Cell<Option<f64>>,
    /// (estimated delivery lag, app time of last frame) for clock-driven scrolling.
    scroll: Cell<Option<(f64, f64)>>,
    /// Left edge of the view in step mode, in graph time.
    step_x0: Cell<Option<f64>>,
}

impl Sensor {
    pub fn new(cfg: SensorConfig) -> Self {
        Self {
            cfg,
            source: None,
            status: Status::Idle,
            history: VecDeque::new(),
            origin: 0.0,
            last: None,
            link_info: String::new(),
            y_view: Cell::new(None),
            x_span: Cell::new(None),
            scroll: Cell::new(None),
            step_x0: Cell::new(None),
        }
    }

    /// Forget the remembered axes so the next frame fits them afresh.
    pub fn reset_view(&self) {
        self.y_view.set(None);
        self.x_span.set(None);
        self.scroll.set(None);
        self.step_x0.set(None);
    }

    pub fn connected(&self) -> bool {
        self.source.is_some()
    }

    pub fn clear_graph(&mut self, origin: f64) {
        self.history.clear();
        self.origin = origin;
        self.reset_view();
    }

    pub fn push(&mut self, samples: &[Sample]) {
        self.history.extend(samples.iter().copied());
        self.last = samples.last().copied().or(self.last);
        self.trim();
    }

    /// Keeps the visible window plus one averaging window before it, so the
    /// average at the left edge of the graph is over a full window too.
    pub fn trim(&mut self) {
        let keep = self.cfg.visible_points.max(2) + self.cfg.avg_points;
        while self.history.len() > keep {
            self.history.pop_front();
        }
    }

    /// Index into `history` of the first sample shown on the graph.
    fn visible_start(&self) -> usize {
        self.history.len().saturating_sub(self.cfg.visible_points.max(2))
    }

    fn visible_count(&self) -> usize {
        self.history.len() - self.visible_start()
    }

    pub fn value(&self) -> Option<f64> {
        self.last.map(|s| self.cfg.value_of(s.raw))
    }

    pub fn avg_value(&self) -> Option<f64> {
        let n = self.cfg.avg_points.max(1);
        let vals: Vec<f64> = self.history.iter().rev().take(n).map(|p| self.cfg.value_of(p.raw)).filter(|v| v.is_finite()).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    }

    pub fn rate_hz(&self) -> Option<f64> {
        let (a, b) = (self.history.front()?, self.history.back()?);
        let span = b.t - a.t;
        (self.history.len() > 1 && span > 0.0).then(|| (self.history.len() - 1) as f64 / span)
    }
}

#[derive(Clone, Copy)]
pub enum TileAction {
    Connect,
    Disconnect,
    Edit,
    Delete,
    Expand,
    Collapse,
    Tare,
    Clear,
    SetPoints(usize),
    ToggleRaw,
    ToggleAvg,
    SetAvgPoints(usize),
    SetScrollMode(ScrollMode),
    SetStepPoints(usize),
}

pub fn tile_frame() -> Frame {
    Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(12)
        .inner_margin(16)
        .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(90) })
}

/// Shared plot look: muted grid with enough spacing for readable axis labels.
pub fn styled_plot<'a>(id: impl egui::AsIdSalt) -> Plot<'a> {
    Plot::new(id)
        .show_background(false)
        .grid_color(Color32::from_gray(150))
        .grid_spacing(egui::Rangef::new(10.0, 120.0))
        .custom_y_axes(vec![
            AxisHints::new_y().min_thickness(48.0),
            AxisHints::new_y().min_thickness(48.0).placement(HPlacement::Right),
        ])
}

fn chart_frame() -> Frame {
    Frame::new()
        .fill(BG_CHART)
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x3a)))
        .corner_radius(8)
        .inner_margin(Margin::same(4))
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    let fill = if color == MUTED { BTN } else { color.gamma_multiply(0.16) };
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text.to_uppercase()).size(11.0).color(color));
        })
        .response
}

pub fn fmt_value(v: f64, decimals: usize) -> String {
    if v.is_finite() {
        format!("{v:.decimals$}")
    } else {
        "—".into()
    }
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn header(ui: &mut Ui, title: &str, meta: &[String], expanded: bool, badges: impl FnOnce(&mut Ui)) -> Option<TileAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 190.0);
            ui.add(egui::Label::new(RichText::new(title).size(16.0).strong()).truncate());
            for line in meta {
                ui.add(egui::Label::new(RichText::new(line).size(12.0).color(MUTED)).truncate());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            let (label, tip, action) = if expanded {
                ("🗗 Restore", "Back to the tile grid (Esc)", TileAction::Collapse)
            } else {
                ("⛶ Expand", "Fill the window with this tile", TileAction::Expand)
            };
            if ui.add(egui::Button::new(RichText::new(label).size(12.0))).on_hover_text(tip).clicked() {
                act = Some(action);
            }
            badges(ui);
        });
    });
    act
}

fn status_badge(ui: &mut Ui, s: &Sensor) {
    let (text, color, tip) = match &s.status {
        Status::Idle => ("idle", MUTED, "Not connected"),
        Status::Connecting => ("connecting", WARN, "Opening the connection…"),
        Status::Live(info) => ("live", OK, info.as_str()),
        Status::Warning(msg) => ("warning", WARN, msg.as_str()),
    };
    let r = badge(ui, text, color);
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
}

/// The big reading is the raw value, or the average when raw is switched off;
/// with both on, the average sits next to it in the trace colour.
fn value_label(ui: &mut Ui, s: &Sensor, size: f32) {
    let cfg = &s.cfg;
    let avg_only = cfg.show_avg && !cfg.show_raw;
    let value = if avg_only { s.avg_value() } else { s.value() };
    let color = match value {
        Some(v) if !cfg.in_range(v) => ERR,
        _ if avg_only => AVG,
        _ => TEXT,
    };
    let big = value.map_or("—".to_string(), |v| fmt_value(v, cfg.decimals));
    ui.label(RichText::new(big).size(size).strong().color(color));
    ui.label(RichText::new(&cfg.unit).size(14.0).color(MUTED));
    if avg_only {
        ui.label(RichText::new("avg").size(12.0).color(AVG));
    } else if cfg.show_avg
        && let Some(avg) = s.avg_value()
    {
        ui.add_space(6.0);
        ui.label(RichText::new("avg").size(12.0).color(AVG));
        ui.label(RichText::new(fmt_value(avg, cfg.decimals)).size(size * 0.6).strong().color(AVG));
    }
}

/// Raw / Avg trace switches. Laid out right-to-left.
fn trace_toggles(ui: &mut Ui, s: &Sensor, act: &mut Option<TileAction>) {
    let cfg = &s.cfg;
    let chip = |ui: &mut Ui, on: bool, text: &str, color: Color32, tip: &str| {
        let text = RichText::new(text).size(12.0).color(if on { color } else { MUTED });
        ui.add(egui::Button::selectable(on, text)).on_hover_text(tip).clicked()
    };
    let tip = format!("Moving average of the last {} points", cfg.avg_points);
    if chip(ui, cfg.show_avg, "Avg", AVG, &tip) {
        *act = Some(TileAction::ToggleAvg);
    }
    if chip(ui, cfg.show_raw, "Raw", ACCENT_2, "Raw trace and reading") {
        *act = Some(TileAction::ToggleRaw);
    }
}

fn avg_points_control(ui: &mut Ui, s: &Sensor, act: &mut Option<TileAction>) {
    let mut n = s.cfg.avg_points;
    let secs = s.rate_hz().map_or(String::new(), |r| format!(" ({})", fmt_secs(n as f64 / r)));
    let resp = ui
        .add(DragValue::new(&mut n).range(2..=100_000).speed(1.0).prefix("avg ").suffix(" pts"))
        .on_hover_text(format!("Points in the moving average{secs}"));
    if resp.changed() {
        *act = Some(TileAction::SetAvgPoints(n));
    }
}

/// Slide / Step switch and step size. Laid out right-to-left.
fn motion_controls(ui: &mut Ui, s: &Sensor, act: &mut Option<TileAction>) {
    let cfg = &s.cfg;
    if cfg.scroll_mode == ScrollMode::Step {
        let mut n = cfg.step_points;
        let resp = ui
            .add(DragValue::new(&mut n).range(1..=cfg.visible_points.max(1)).speed(5.0).prefix("by ").suffix(" pts"))
            .on_hover_text("How far the graph jumps each time it fills");
        if resp.changed() {
            *act = Some(TileAction::SetStepPoints(n));
        }
    }
    for (mode, tip) in [
        (ScrollMode::Step, "Hold still while new readings fill in, then jump over"),
        (ScrollMode::Continuous, "Slide continuously with the newest reading at the right edge"),
    ] {
        let on = cfg.scroll_mode == mode;
        let text = RichText::new(mode.label()).size(12.0).color(if on { TEXT } else { MUTED });
        if ui.add(egui::Button::selectable(on, text)).on_hover_text(tip).clicked() && !on {
            *act = Some(TileAction::SetScrollMode(mode));
        }
    }
}

fn fmt_secs(s: f64) -> String {
    if s < 1.0 { format!("{:.0} ms", s * 1000.0) } else { format!("{s:.2} s") }
}

fn raw_label(ui: &mut Ui, s: &Sensor) {
    if let Some(last) = s.last {
        let unit = match s.cfg.kind.raw_unit() {
            "raw" => "",
            u => u,
        };
        ui.label(RichText::new(format!("raw {} {unit}", fmt_value(last.raw, 4))).color(MUTED));
    }
}

fn stats_text(s: &Sensor) -> Option<String> {
    let cfg = &s.cfg;
    let vals: Vec<f64> = s.history.iter().skip(s.visible_start()).map(|p| cfg.value_of(p.raw)).collect();
    if vals.is_empty() {
        return None;
    }
    let (lo, hi) = min_max(&vals);
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    Some(format!(
        "min {}  max {}  avg {}",
        fmt_value(lo, cfg.decimals),
        fmt_value(hi, cfg.decimals),
        fmt_value(mean, cfg.decimals)
    ))
}

/// Laid out right-to-left: the first widget added ends up rightmost.
fn points_control(ui: &mut Ui, s: &Sensor, act: &mut Option<TileAction>) {
    let mut pts = s.cfg.visible_points;
    let resp = ui
        .add(DragValue::new(&mut pts).range(10..=1_000_000).speed(5.0).suffix(" pts"))
        .on_hover_text("Points shown before the graph starts scrolling");
    if resp.changed() {
        *act = Some(TileAction::SetPoints(pts));
    }
    let rate = s.rate_hz().map_or(String::new(), |r| format!("{r:.1} Hz · "));
    ui.label(RichText::new(format!("{rate}{} of", s.visible_count())).size(12.0).color(MUTED));
}

fn connection_buttons(ui: &mut Ui, s: &Sensor, act: &mut Option<TileAction>) {
    if s.connected() {
        if ui.button("Disconnect").clicked() {
            *act = Some(TileAction::Disconnect);
        }
    } else if ui.add(egui::Button::new(RichText::new("Connect").color(Color32::WHITE)).fill(ACCENT)).clicked() {
        *act = Some(TileAction::Connect);
    }
    if ui.add_enabled(s.last.is_some(), egui::Button::new("Tare")).on_hover_text("Zero the reading by adjusting the offset").clicked() {
        *act = Some(TileAction::Tare);
    }
    if ui.button("Clear").on_hover_text("Clear the graph").clicked() {
        *act = Some(TileAction::Clear);
    }
}

pub fn live_tile(ui: &mut Ui, s: &Sensor, now: f64, recording: bool) -> Option<TileAction> {
    let cfg = &s.cfg;
    let meta = vec![format!("{} · {}", cfg.kind.short(), cfg.connection_label())];
    let mut act = header(ui, &cfg.name, &meta, false, |ui| {
        if recording && s.connected() {
            badge(ui, "rec", ERR);
        }
        status_badge(ui, s);
    });

    ui.horizontal(|ui| {
        value_label(ui, s, 32.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            trace_toggles(ui, s, &mut act);
            if !s.cfg.show_avg {
                raw_label(ui, s);
            }
        });
    });

    chart_frame().show(ui, |ui| live_plot(ui, s, now, 150.0));

    ui.horizontal(|ui| {
        if let Some(text) = stats_text(s) {
            ui.label(RichText::new(text).size(12.0).color(MUTED));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| points_control(ui, s, &mut act));
    });

    ui.horizontal(|ui| {
        connection_buttons(ui, s, &mut act);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::from_rgb(0xff, 0xb4, 0xb4))).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, ERR))).clicked() {
                act = Some(TileAction::Delete);
            }
            if ui.button("Edit").clicked() {
                act = Some(TileAction::Edit);
            }
        });
    });

    if let Status::Warning(msg) = &s.status {
        ui.add(egui::Label::new(RichText::new(msg).size(12.0).color(ERR)).truncate());
    }
    act
}

/// Expanded view: two slim rows of readout and controls so the graph gets the window.
pub fn live_tile_full(ui: &mut Ui, s: &Sensor, now: f64, rec: Option<(String, Color32)>) -> Option<TileAction> {
    let cfg = &s.cfg;
    let mut act = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&cfg.name).size(16.0).strong());
        ui.label(RichText::new(format!("{} · {}", cfg.kind.short(), cfg.connection_label())).size(12.0).color(MUTED));
        status_badge(ui, s);
        if let Some((text, color)) = &rec {
            ui.label(RichText::new(text).strong().color(*color));
        }
        ui.add_space(12.0);
        value_label(ui, s, 28.0);
        raw_label(ui, s);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("🗗 Restore").on_hover_text("Back to the tile grid (Esc)").clicked() {
                act = Some(TileAction::Collapse);
            }
            if ui.button("Edit").clicked() {
                act = Some(TileAction::Edit);
            }
            ui.separator();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| connection_buttons(ui, s, &mut act));
        });
    });
    ui.horizontal(|ui| {
        if let Some(text) = stats_text(s) {
            ui.label(RichText::new(text).size(12.0).color(MUTED));
        }
        if let Status::Warning(msg) = &s.status {
            ui.add(egui::Label::new(RichText::new(msg).size(12.0).color(ERR)).truncate());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            points_control(ui, s, &mut act);
            ui.separator();
            motion_controls(ui, s, &mut act);
            ui.separator();
            if s.cfg.show_avg {
                avg_points_control(ui, s, &mut act);
            }
            trace_toggles(ui, s, &mut act);
        });
    });
    let h = (ui.available_height() - 10.0).max(160.0);
    chart_frame().show(ui, |ui| live_plot(ui, s, now, h));
    act
}

fn live_plot(ui: &mut Ui, s: &Sensor, now: f64, height: f32) {
    let cfg = &s.cfg;
    let all: Vec<[f64; 2]> = s.history.iter().map(|p| [p.t - s.origin, cfg.value_of(p.raw)]).collect();
    let start = s.visible_start();
    let avg: Vec<[f64; 2]> = if cfg.show_avg { moving_average(&all, cfg.avg_points)[start..].to_vec() } else { Vec::new() };
    let pts = all[start..].to_vec();
    let edge = scroll_edge(s, now - s.origin, pts.last().map(|p| p[0]));

    // Time shown across the plot. Held steady against small sample-rate
    // jitter, because any change rescales every point on screen.
    let span = match (pts.first(), pts.last()) {
        (Some(a), Some(b)) if pts.len() > 1 && b[0] > a[0] => {
            let dt = (b[0] - a[0]) / (pts.len() - 1) as f64;
            let target = dt * (cfg.visible_points.max(2) - 1) as f64;
            let span = match s.x_span.get() {
                Some(prev) if ((target - prev) / prev).abs() < 0.02 => prev,
                _ => target,
            };
            s.x_span.set(Some(span));
            span
        }
        _ => 10.0,
    };
    let step_x0 = if cfg.scroll_mode == ScrollMode::Step { step_origin(s, &pts, span) } else { None };

    let range_ok = cfg.range_min < cfg.range_max;
    let (y0, y1) = if cfg.range_as_axis && range_ok {
        (cfg.range_min, cfg.range_max)
    } else if pts.is_empty() {
        if range_ok { (cfg.range_min, cfg.range_max) } else { (0.0, 1.0) }
    } else {
        let shown = if cfg.show_raw || avg.is_empty() { &pts } else { &avg };
        let left = step_x0.unwrap_or(f64::NEG_INFINITY);
        let ys: Vec<f64> = shown.iter().filter(|p| p[0] >= left).map(|p| p[1]).collect();
        let (lo, hi) = min_max(&ys);
        stable_range(s.y_view.get(), lo, hi)
    };
    s.y_view.set(Some((y0, y1)));

    let unit = cfg.unit.clone();
    let decimals = cfg.decimals;
    styled_plot(("live", cfg.id))
        .height(height)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .label_formatter(move |h| {
            let p = hover_point(h);
            Some(format!("{:.2} s\n{} {unit}", p.x, fmt_value(p.y, decimals)))
        })
        .show(ui, |pui| {
            // One column per physical pixel, on a grid fixed in time. The view
            // only ever moves by whole columns, so each frame is the previous
            // image shifted by whole pixels and nothing re-rasterizes.
            let cols = (pui.response().rect.width() * pui.ctx().pixels_per_point()).round().max(1.0) as f64;
            let col_dt = span / cols;
            let k0 = match (step_x0, pts.first(), edge) {
                (Some(x0), _, _) => (x0 / col_dt).floor(),
                (None, Some(a), Some(e)) => (a[0] / col_dt).floor().max((e / col_dt).ceil() - cols),
                _ => 0.0,
            };
            let (x0, x1) = (k0 * col_dt, (k0 + cols) * col_dt);
            pui.set_plot_bounds(PlotBounds::from_min_max([x0, y0], [x1, y1]));
            if range_ok && !cfg.range_as_axis {
                for y in [cfg.range_min, cfg.range_max] {
                    if y > y0 && y < y1 {
                        pui.hline(HLine::new("range", y).color(ERR.gamma_multiply(0.5)).style(LineStyle::dashed_loose()));
                    }
                }
            }
            let dense = pts.len() as f64 > cols;
            let thin = |line: Vec<[f64; 2]>| if dense { column_envelope(&line, col_dt) } else { line };
            if cfg.show_raw || avg.is_empty() {
                let color = if avg.is_empty() { ACCENT_2 } else { ACCENT_2.gamma_multiply(0.55) };
                pui.line(Line::new(cfg.name.clone(), PlotPoints::from(thin(pts))).color(color).width(if dense { 1.0 } else { 1.5 }));
            }
            if !avg.is_empty() {
                pui.line(Line::new(format!("{} avg", cfg.name), PlotPoints::from(thin(avg))).color(AVG).width(2.0));
            }
        });
}

/// Left edge of the view in step mode: fixed until the newest reading reaches
/// the right edge, then advanced by whole steps so a fresh gap opens on the right.
fn step_origin(s: &Sensor, pts: &[[f64; 2]], span: f64) -> Option<f64> {
    let (a, b) = (pts.first()?[0], pts.last()?[0]);
    let n = s.cfg.visible_points.max(2);
    let step = span * s.cfg.step_points.clamp(1, n) as f64 / (n - 1) as f64;
    let x0 = next_step_origin(s.step_x0.get().unwrap_or(a), b, span, step);
    s.step_x0.set(Some(x0));
    Some(x0)
}

fn next_step_origin(x0: f64, newest: f64, span: f64, step: f64) -> f64 {
    if newest > x0 + span { x0 + step * ((newest - x0 - span) / step).ceil() } else { x0 }
}

/// Right edge of the view in graph time. Sources deliver samples in bursts, so
/// following the newest sample would make the graph lurch; instead it advances
/// with the clock, trailing the newest data by the largest recent delivery lag.
fn scroll_edge(s: &Sensor, now: f64, newest: Option<f64>) -> Option<f64> {
    let newest = newest?;
    if !s.connected() {
        s.scroll.set(None);
        return Some(newest);
    }
    let lag = (now - newest).max(0.0);
    let est = match s.scroll.get() {
        Some((prev, prev_now)) => lag.max(prev - 0.05 * (now - prev_now).max(0.0)),
        None => lag,
    };
    s.scroll.set(Some((est, now)));
    Some(now - est)
}

/// Trailing moving average over `n` samples (fewer at the very start).
fn moving_average(pts: &[[f64; 2]], n: usize) -> Vec<[f64; 2]> {
    let n = n.max(1);
    let mut out = Vec::with_capacity(pts.len());
    let (mut sum, mut count) = (0.0, 0usize);
    for (i, p) in pts.iter().enumerate() {
        if p[1].is_finite() {
            sum += p[1];
            count += 1;
        }
        if i >= n {
            let old = pts[i - n][1];
            if old.is_finite() {
                sum -= old;
                count -= 1;
            }
        }
        out.push([p[0], if count > 0 { sum / count as f64 } else { f64::NAN }]);
    }
    out
}

/// Holds the axis on rounded values and only re-fits when the data leaves the
/// range or shrinks to a small part of it, so the scale isn't redrawn every frame.
fn stable_range(prev: Option<(f64, f64)>, lo: f64, hi: f64) -> (f64, f64) {
    if !(lo.is_finite() && hi.is_finite()) {
        return prev.unwrap_or((0.0, 1.0));
    }
    if let Some((p0, p1)) = prev
        && lo >= p0
        && hi <= p1
        && hi - lo >= 0.35 * (p1 - p0)
    {
        return (p0, p1);
    }
    let span = (hi - lo).max(hi.abs().max(1.0) * 1e-3);
    let step = nice_step(span / 4.0);
    (((lo - span * 0.1) / step).floor() * step, ((hi + span * 0.1) / step).ceil() * step)
}

fn nice_step(x: f64) -> f64 {
    let mag = 10f64.powf(x.log10().floor());
    let f = x / mag - 1e-9;
    mag * if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    }
}

/// Min/max of the samples in each `col_dt`-wide column of an absolute time
/// grid, drawn at the column centre in the order they occurred.
fn column_envelope(pts: &[[f64; 2]], col_dt: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pts.len() {
        let k = (pts[i][0] / col_dt).floor();
        let x = (k + 0.5) * col_dt;
        let (mut lo, mut hi) = (i, i);
        let mut j = i;
        while j < pts.len() && (pts[j][0] / col_dt).floor() == k {
            if pts[j][1] < pts[lo][1] {
                lo = j;
            }
            if pts[j][1] > pts[hi][1] {
                hi = j;
            }
            j += 1;
        }
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        out.push([x, pts[a][1]]);
        if b != a {
            out.push([x, pts[b][1]]);
        }
        i = j;
    }
    out
}

/// Min/max decimation with bucket edges fixed to absolute sample indices
/// (`first_index` is the stream index of `pts[0]`), so a given stretch of data
/// is always reduced the same way no matter how far it has scrolled.
fn decimate_anchored(pts: &[[f64; 2]], first_index: u64, buckets: usize) -> Vec<[f64; 2]> {
    if pts.len() <= buckets * 2 {
        return pts.to_vec();
    }
    let size = pts.len().div_ceil(buckets) as u64;
    let mut out = Vec::with_capacity(buckets * 2 + 4);
    let mut i = 0;
    while i < pts.len() {
        let idx = first_index + i as u64;
        let end = (i + (size - idx % size) as usize).min(pts.len());
        let chunk = &pts[i..end];
        let (mut lo, mut hi) = (chunk[0], chunk[0]);
        for p in chunk {
            if p[1] < lo[1] {
                lo = *p;
            }
            if p[1] > hi[1] {
                hi = *p;
            }
        }
        if lo[0] <= hi[0] {
            out.push(lo);
            out.push(hi);
        } else {
            out.push(hi);
            out.push(lo);
        }
        i = end;
    }
    out
}

pub fn review_tile(ui: &mut Ui, ch: &ReviewChannel, idx: usize, expanded: bool) -> Option<TileAction> {
    let duration = ch.t.last().copied().unwrap_or(0.0) - ch.t.first().copied().unwrap_or(0.0);
    let rate = if duration > 0.0 { (ch.t.len().saturating_sub(1)) as f64 / duration } else { 0.0 };
    let mut meta = vec![format!("{} samples · {} · {rate:.1} Hz avg", ch.t.len(), fmt_duration(duration))];
    if let Some(cfg) = &ch.config {
        meta.push(format!("{} · {}", cfg.kind.short(), cfg.connection_label()));
    }
    let decimals = ch.config.as_ref().map_or(3, |c| c.decimals);

    if expanded {
        let mut act = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&ch.name).size(16.0).strong());
            ui.label(RichText::new(meta.join(" · ")).size(12.0).color(MUTED));
            badge(ui, "recorded", ACCENT_2);
            if !ch.v.is_empty() {
                let (lo, hi) = min_max(&ch.v);
                let mean = ch.v.iter().sum::<f64>() / ch.v.len() as f64;
                ui.add_space(12.0);
                for (label, v) in [("min", lo), ("max", hi), ("avg", mean)] {
                    ui.label(RichText::new(label).color(MUTED));
                    ui.label(RichText::new(fmt_value(v, decimals)).size(18.0).strong());
                }
                ui.label(RichText::new(&ch.unit).color(MUTED));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("🗗 Restore").on_hover_text("Back to the tile grid (Esc)").clicked() {
                    act = Some(TileAction::Collapse);
                }
                ui.label(RichText::new("Drag to pan · scroll / ctrl+scroll to zoom · double-click to reset").size(11.5).color(MUTED));
            });
        });
        let h = (ui.available_height() - 10.0).max(160.0);
        chart_frame().show(ui, |ui| review_plot(ui, ch, idx, h, true, decimals));
        return act;
    }

    let act = header(ui, &ch.name, &meta, false, |ui| {
        badge(ui, "recorded", ACCENT_2);
    });

    if !ch.v.is_empty() {
        let (lo, hi) = min_max(&ch.v);
        let mean = ch.v.iter().sum::<f64>() / ch.v.len() as f64;
        ui.horizontal(|ui| {
            for (label, v) in [("min", lo), ("max", hi), ("avg", mean)] {
                ui.label(RichText::new(label).color(MUTED));
                ui.label(RichText::new(fmt_value(v, decimals)).size(20.0).strong());
            }
            ui.label(RichText::new(&ch.unit).color(MUTED));
        });
    }

    chart_frame().show(ui, |ui| review_plot(ui, ch, idx, TILE_HEIGHT - 150.0, false, decimals));
    ui.label(RichText::new("Drag to pan · scroll / ctrl+scroll to zoom · double-click to reset").size(11.5).color(MUTED));
    act
}

fn review_plot(ui: &mut Ui, ch: &ReviewChannel, idx: usize, height: f32, expanded: bool, decimals: usize) {
    let id = ui.make_persistent_id(("review", idx, expanded));
    let (Some(&t0), Some(&t1)) = (ch.t.first(), ch.t.last()) else {
        ui.label("No samples");
        return;
    };
    let (ylo, yhi) = min_max(&ch.v);
    let pad = ((yhi - ylo) * 0.05).max(1e-9);

    // Draw only what is near the current view, decimated, so large logs stay smooth.
    let view: Option<(f64, f64)> = ui.ctx().data(|d| d.get_temp(id));
    let (v0, v1) = view.unwrap_or((t0, t1));
    let margin = (v1 - v0) * 0.5;
    let i0 = ch.t.partition_point(|&t| t < v0 - margin);
    let i1 = ch.t.partition_point(|&t| t <= v1 + margin);
    let pts: Vec<[f64; 2]> = ch.t[i0..i1].iter().zip(&ch.v[i0..i1]).map(|(&t, &v)| [t, v]).collect();
    let buckets = (ui.available_width() as usize).max(200) * 2;

    let unit = ch.unit.clone();
    let resp = styled_plot(id)
        .height(height)
        .include_x(t0)
        .include_x(t1)
        .include_y(ylo - pad)
        .include_y(yhi + pad)
        .label_formatter(move |h| {
            let p = hover_point(h);
            Some(format!("{:.3} s\n{} {unit}", p.x, fmt_value(p.y, decimals)))
        })
        .show(ui, |pui| {
            let line = PlotPoints::from(decimate_anchored(&pts, i0 as u64, buckets));
            pui.line(Line::new(ch.name.clone(), line).color(ACCENT_2).width(1.5));
        });
    let b = resp.transform.bounds();
    let now = (b.min()[0], b.max()[0]);
    if view != Some(now) {
        ui.ctx().data_mut(|d| d.insert_temp(id, now));
        ui.ctx().request_repaint();
    }
}

fn hover_point(h: &HoverPosition<'_>) -> PlotPoint {
    match h {
        HoverPosition::NearDataPoint { position, .. } | HoverPosition::Elsewhere { position } => *position,
    }
}

pub fn min_max(v: &[f64]) -> (f64, f64) {
    v.iter()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_is_stable_while_scrolling() {
        let stream: Vec<[f64; 2]> = (0..20_000).map(|i| [i as f64, ((i * 7919) % 101) as f64]).collect();
        let window = 5000;
        let a = decimate_anchored(&stream[1000..1000 + window], 1000, 600);
        let b = decimate_anchored(&stream[1007..1007 + window], 1007, 600);
        // Away from the edges, both windows must draw identical points.
        let inner = |v: &[[f64; 2]]| v.iter().filter(|p| p[0] >= 1100.0 && p[0] < 5900.0).copied().collect::<Vec<_>>();
        assert_eq!(inner(&a), inner(&b));
        assert!(a.len() <= 600 * 2 + 4);
    }

    #[test]
    fn columns_are_fixed_in_time() {
        let stream: Vec<[f64; 2]> = (0..20_000).map(|i| [i as f64 * 0.01, ((i * 7919) % 101) as f64]).collect();
        let col_dt = 0.035;
        let a = column_envelope(&stream[1000..6000], col_dt);
        let b = column_envelope(&stream[1013..6013], col_dt);
        let inner = |v: &[[f64; 2]]| v.iter().filter(|p| p[0] > 11.0 && p[0] < 59.0).copied().collect::<Vec<_>>();
        assert_eq!(inner(&a), inner(&b));
    }

    #[test]
    fn step_mode_holds_then_jumps_by_whole_steps() {
        // 50 s view, 12.5 s steps.
        assert_eq!(next_step_origin(0.0, 30.0, 50.0, 12.5), 0.0);
        assert_eq!(next_step_origin(0.0, 50.0, 50.0, 12.5), 0.0);
        assert_eq!(next_step_origin(0.0, 50.1, 50.0, 12.5), 12.5);
        // A long stall catches up in one go, still on the step grid.
        assert_eq!(next_step_origin(0.0, 90.0, 50.0, 12.5), 50.0);
    }

    #[test]
    fn moving_average_trails_over_window() {
        let pts: Vec<[f64; 2]> = (0..6).map(|i| [i as f64, i as f64]).collect();
        let avg: Vec<f64> = moving_average(&pts, 3).iter().map(|p| p[1]).collect();
        assert_eq!(avg, vec![0.0, 0.5, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn axis_holds_until_data_needs_more_room() {
        let r = stable_range(None, 6.8, 8.8);
        assert!(r.0 <= 6.6 && r.1 >= 9.0 && r.1 - r.0 <= 3.5, "{r:?}");
        assert_eq!(stable_range(Some(r), 6.9, 8.7), r);
        assert!(stable_range(Some(r), 6.9, 9.2).1 >= 9.2);
        assert_ne!(stable_range(Some(r), 7.5, 7.6), r);
    }
}
