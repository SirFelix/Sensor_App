use std::cell::Cell;
use std::collections::VecDeque;

use egui::{Align, Color32, CornerRadius, DragValue, Frame, Layout, Margin, RichText, Stroke, Ui};
use egui_plot::{HLine, HoverPosition, Line, LineStyle, Plot, PlotBounds, PlotPoint, PlotPoints};

use crate::config::SensorConfig;
use crate::recording::ReviewChannel;
use crate::sources::{Sample, SourceHandle};
use crate::theme::*;

pub const TILE_MIN_WIDTH: f32 = 380.0;
pub const TILE_HEIGHT: f32 = 360.0;
pub const TILE_GAP: f32 = 16.0;
const MAX_DRAWN_POINTS: usize = 4000;

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
    /// Samples pushed since the graph was last cleared; anchors decimation
    /// buckets to the stream so they don't shift as the graph scrolls.
    pub total: u64,
    /// Displayed Y range, eased between frames so the axis doesn't jump.
    pub y_view: Cell<Option<(f64, f64)>>,
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
            total: 0,
            y_view: Cell::new(None),
        }
    }

    pub fn connected(&self) -> bool {
        self.source.is_some()
    }

    pub fn clear_graph(&mut self, origin: f64) {
        self.history.clear();
        self.origin = origin;
        self.total = 0;
        self.y_view.set(None);
    }

    pub fn push(&mut self, samples: &[Sample]) {
        self.history.extend(samples.iter().copied());
        self.total += samples.len() as u64;
        self.last = samples.last().copied().or(self.last);
        self.trim();
    }

    pub fn trim(&mut self) {
        let keep = self.cfg.visible_points.max(2);
        while self.history.len() > keep {
            self.history.pop_front();
        }
    }

    pub fn value(&self) -> Option<f64> {
        self.last.map(|s| self.cfg.value_of(s.raw))
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
        .y_axis_min_width(48.0)
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

pub fn live_tile(ui: &mut Ui, s: &Sensor, expanded: bool, recording: bool) -> Option<TileAction> {
    let cfg = &s.cfg;
    let meta = vec![format!("{} · {}", cfg.kind.short(), cfg.connection_label())];
    let mut act = header(ui, &cfg.name, &meta, expanded, |ui| {
        if recording && s.connected() {
            badge(ui, "rec", ERR);
        }
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
    });

    ui.horizontal(|ui| {
        let value = s.value();
        let color = match value {
            Some(v) if !cfg.in_range(v) => ERR,
            _ => TEXT,
        };
        let big = value.map_or("—".to_string(), |v| fmt_value(v, cfg.decimals));
        ui.label(RichText::new(big).size(if expanded { 44.0 } else { 32.0 }).strong().color(color));
        ui.label(RichText::new(&cfg.unit).size(14.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(last) = s.last {
                let unit = match cfg.kind.raw_unit() {
                    "raw" => "",
                    u => u,
                };
                ui.label(RichText::new(format!("raw {} {unit}", fmt_value(last.raw, 4))).color(MUTED));
            }
        });
    });

    let footer = 86.0;
    let plot_h = if expanded { (ui.available_height() - footer).max(160.0) } else { 150.0 };
    chart_frame().show(ui, |ui| live_plot(ui, s, plot_h));

    ui.horizontal(|ui| {
        let vals: Vec<f64> = s.history.iter().map(|p| cfg.value_of(p.raw)).collect();
        if !vals.is_empty() {
            let (lo, hi) = min_max(&vals);
            let mean = vals.iter().sum::<f64>() / vals.len() as f64;
            ui.label(
                RichText::new(format!(
                    "min {}  max {}  avg {}",
                    fmt_value(lo, cfg.decimals),
                    fmt_value(hi, cfg.decimals),
                    fmt_value(mean, cfg.decimals)
                ))
                .size(12.0)
                .color(MUTED),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut pts = cfg.visible_points;
            let resp = ui
                .add(DragValue::new(&mut pts).range(10..=1_000_000).speed(5.0).suffix(" pts"))
                .on_hover_text("Points shown before the graph starts scrolling");
            if resp.changed() {
                act = Some(TileAction::SetPoints(pts));
            }
            let rate = s.rate_hz().map_or(String::new(), |r| format!("{r:.1} Hz · "));
            ui.label(RichText::new(format!("{rate}{} of", s.history.len())).size(12.0).color(MUTED));
        });
    });

    ui.horizontal(|ui| {
        if s.connected() {
            if ui.button("Disconnect").clicked() {
                act = Some(TileAction::Disconnect);
            }
        } else if ui.add(egui::Button::new(RichText::new("Connect").color(Color32::WHITE)).fill(ACCENT)).clicked() {
            act = Some(TileAction::Connect);
        }
        if ui.add_enabled(s.last.is_some(), egui::Button::new("Tare")).on_hover_text("Zero the reading by adjusting the offset").clicked() {
            act = Some(TileAction::Tare);
        }
        if ui.button("Clear").on_hover_text("Clear the graph").clicked() {
            act = Some(TileAction::Clear);
        }
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

fn live_plot(ui: &mut Ui, s: &Sensor, height: f32) {
    let cfg = &s.cfg;
    let pts: Vec<[f64; 2]> = s.history.iter().map(|p| [p.t - s.origin, cfg.value_of(p.raw)]).collect();

    let (x0, x1) = match (pts.first(), pts.last()) {
        (Some(a), Some(b)) if pts.len() > 1 => {
            // Fill left-to-right until `visible_points` are on screen, then scroll.
            let dt = (b[0] - a[0]) / (pts.len() - 1) as f64;
            let full = a[0] + dt * (cfg.visible_points.max(2) - 1) as f64;
            (a[0], b[0].max(full).max(a[0] + 1e-3))
        }
        (Some(a), _) => (a[0], a[0] + 1.0),
        _ => (0.0, 10.0),
    };
    let range_ok = cfg.range_min < cfg.range_max;
    let (y0, y1) = if cfg.range_as_axis && range_ok {
        (cfg.range_min, cfg.range_max)
    } else if pts.is_empty() {
        if range_ok { (cfg.range_min, cfg.range_max) } else { (0.0, 1.0) }
    } else {
        let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
        let (lo, hi) = min_max(&ys);
        let pad = ((hi - lo) * 0.08).max(hi.abs().max(1.0) * 1e-3);
        (lo - pad, hi + pad)
    };

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
            pui.set_plot_bounds(PlotBounds::from_min_max([x0, y0], [x1, y1]));
            if range_ok && !cfg.range_as_axis {
                for y in [cfg.range_min, cfg.range_max] {
                    if y > y0 && y < y1 {
                        pui.hline(HLine::new("range", y).color(ERR.gamma_multiply(0.5)).style(LineStyle::dashed_loose()));
                    }
                }
            }
            pui.line(Line::new(cfg.name.clone(), PlotPoints::from(decimate(&pts, 0, pts.len()))).color(ACCENT_2).width(1.5));
        });
}

pub fn review_tile(ui: &mut Ui, ch: &ReviewChannel, idx: usize, expanded: bool) -> Option<TileAction> {
    let duration = ch.t.last().copied().unwrap_or(0.0) - ch.t.first().copied().unwrap_or(0.0);
    let rate = if duration > 0.0 { (ch.t.len().saturating_sub(1)) as f64 / duration } else { 0.0 };
    let mut meta = vec![format!("{} samples · {} · {rate:.1} Hz avg", ch.t.len(), fmt_duration(duration))];
    if let Some(cfg) = &ch.config {
        meta.push(format!("{} · {}", cfg.kind.short(), cfg.connection_label()));
    }
    let act = header(ui, &ch.name, &meta, expanded, |ui| {
        badge(ui, "recorded", ACCENT_2);
    });
    let decimals = ch.config.as_ref().map_or(3, |c| c.decimals);

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

    let plot_h = if expanded { (ui.available_height() - 28.0).max(160.0) } else { TILE_HEIGHT - 150.0 };
    chart_frame().show(ui, |ui| review_plot(ui, ch, idx, plot_h, expanded, decimals));
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
            pui.line(Line::new(ch.name.clone(), PlotPoints::from(decimate(&pts, 0, pts.len()))).color(ACCENT_2).width(1.5));
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

/// Min/max bucket decimation keeps spikes visible while capping drawn points.
fn decimate(pts: &[[f64; 2]], start: usize, end: usize) -> Vec<[f64; 2]> {
    let slice = &pts[start..end];
    if slice.len() <= MAX_DRAWN_POINTS {
        return slice.to_vec();
    }
    let buckets = MAX_DRAWN_POINTS / 2;
    let size = slice.len().div_ceil(buckets);
    let mut out = Vec::with_capacity(buckets * 2);
    for chunk in slice.chunks(size) {
        let (mut lo, mut hi) = (0, 0);
        for (i, p) in chunk.iter().enumerate() {
            if p[1] < chunk[lo][1] {
                lo = i;
            }
            if p[1] > chunk[hi][1] {
                hi = i;
            }
        }
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        out.push(chunk[a]);
        if b != a {
            out.push(chunk[b]);
        }
    }
    out
}

pub fn min_max(v: &[f64]) -> (f64, f64) {
    v.iter()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}
