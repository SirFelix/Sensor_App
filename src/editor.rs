use egui::{Color32, ComboBox, DragValue, Frame, Grid, RichText, Stroke, TextEdit, Ui};
use egui_plot::{Line, PlotPoints, Points};

use crate::config::*;
use crate::scaling::{CalPoint, ScalingMode};
use crate::theme::*;

pub struct Editor {
    pub cfg: SensorConfig,
    pub is_new: bool,
}

pub enum EditorResult {
    Open,
    Cancel,
    Save,
}

/// Lists the editor needs from the app (scanned hardware and live readings).
pub struct EditorContext<'a> {
    pub ni_channels: &'a Option<Result<Vec<String>, String>>,
    pub ports: &'a [String],
    pub live_raw: Option<f64>,
    pub rescan_ni: &'a mut bool,
    pub rescan_ports: &'a mut bool,
}

impl Editor {
    pub fn new(cfg: SensorConfig, is_new: bool) -> Self {
        Self { cfg, is_new }
    }

    pub fn show(&mut self, ui: &mut Ui, ctx: EditorContext) -> EditorResult {
        let mut result = EditorResult::Open;
        ui.horizontal(|ui| {
            ui.heading(if self.is_new { "Add sensor" } else { "Edit sensor" });
        });
        ui.separator();

        let max_h = ui.ctx().content_rect().height() * 0.78;
        egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(860.0);
            self.identity(ui);
            self.connection(ui, ctx.ni_channels, ctx.ports, ctx.rescan_ni, ctx.rescan_ports);
            self.scaling(ui, ctx.live_raw);
            self.range_display(ui);
        });

        ui.separator();
        ui.horizontal(|ui| {
            let valid = !self.cfg.name.trim().is_empty();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = egui::Button::new(RichText::new("Save sensor").color(Color32::WHITE)).fill(ACCENT);
                if ui.add_enabled(valid, save).clicked() {
                    let _ = self.cfg.scaling.refit();
                    result = EditorResult::Save;
                }
                if ui.button("Cancel").clicked() {
                    result = EditorResult::Cancel;
                }
                if !valid {
                    ui.label(RichText::new("Name is required").color(ERR));
                }
            });
        });
        result
    }

    fn identity(&mut self, ui: &mut Ui) {
        fieldset(ui, "Identity", |ui| {
            grid(ui, "identity", |ui| {
                ui.label("Name");
                ui.add(TextEdit::singleline(&mut self.cfg.name).desired_width(320.0));
                ui.end_row();

                ui.label("Sensor connection");
                let before = self.cfg.kind;
                ComboBox::from_id_salt("kind")
                    .width(320.0)
                    .selected_text(self.cfg.kind.label())
                    .show_ui(ui, |ui| {
                        for k in SourceKind::ALL {
                            ui.selectable_value(&mut self.cfg.kind, k, k.label());
                        }
                    });
                if self.cfg.kind != before {
                    self.cfg.apply_kind_defaults();
                }
                ui.end_row();
            });
        });
    }

    fn connection(
        &mut self,
        ui: &mut Ui,
        ni_channels: &Option<Result<Vec<String>, String>>,
        ports: &[String],
        rescan_ni: &mut bool,
        rescan_ports: &mut bool,
    ) {
        let kind = self.cfg.kind;
        fieldset(ui, "Connection", |ui| {
            grid(ui, "connection", |ui| match kind {
                SourceKind::NiVoltage | SourceKind::NiCurrent => {
                    let ni = &mut self.cfg.ni;
                    ui.label("Physical channel");
                    ui.horizontal(|ui| {
                        let list = match ni_channels {
                            Some(Ok(list)) => list.as_slice(),
                            _ => &[],
                        };
                        ComboBox::from_id_salt("ni_chan")
                            .width(240.0)
                            .selected_text(if ni.channel.is_empty() { "Select channel…" } else { &ni.channel })
                            .show_ui(ui, |ui| {
                                if list.is_empty() {
                                    ui.label(RichText::new("No channels found — press Scan").color(MUTED));
                                }
                                for c in list {
                                    ui.selectable_value(&mut ni.channel, c.clone(), c);
                                }
                            });
                        if ui.button("Scan").on_hover_text("Search for NI devices and modules").clicked() {
                            *rescan_ni = true;
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.vertical(|ui| {
                        ui.add(TextEdit::singleline(&mut ni.channel).hint_text("e.g. cDAQ1Mod1/ai0 or Dev1/ai0").desired_width(320.0));
                        if let Some(Err(e)) = ni_channels {
                            ui.label(RichText::new(e).size(12.0).color(WARN));
                        }
                    });
                    ui.end_row();

                    ui.label("Terminal configuration");
                    ComboBox::from_id_salt("terminal").width(320.0).selected_text(ni.terminal.label()).show_ui(ui, |ui| {
                        for t in TerminalConfig::ALL {
                            ui.selectable_value(&mut ni.terminal, t, t.label());
                        }
                    });
                    ui.end_row();

                    let unit = kind.raw_unit();
                    ui.label(format!("Input range ({unit})"));
                    ui.horizontal(|ui| {
                        num(ui, &mut ni.min);
                        ui.label("to");
                        num(ui, &mut ni.max);
                        let presets: &[(f64, f64)] = if kind == SourceKind::NiCurrent {
                            &[(4.0, 20.0), (0.0, 20.0)]
                        } else {
                            &[(0.0, 10.0), (-10.0, 10.0), (0.0, 5.0), (-5.0, 5.0), (-1.0, 1.0)]
                        };
                        for &(lo, hi) in presets {
                            if ui.small_button(format!("{lo}…{hi}")).clicked() {
                                ni.min = lo;
                                ni.max = hi;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Sample rate (Hz)");
                    ui.add(DragValue::new(&mut ni.sample_rate_hz).range(0.1..=250_000.0).speed(1.0));
                    ui.end_row();

                    if kind == SourceKind::NiCurrent {
                        ui.label("Shunt resistor");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut ni.shunt, ShuntLocation::Default, "Default");
                            ui.selectable_value(&mut ni.shunt, ShuntLocation::Internal, "Internal");
                            ui.selectable_value(&mut ni.shunt, ShuntLocation::External, "External");
                            if ni.shunt == ShuntLocation::External {
                                ui.add(DragValue::new(&mut ni.ext_shunt_ohms).range(0.1..=100_000.0).suffix(" Ω"));
                            }
                        });
                        ui.end_row();
                    }
                }
                SourceKind::SerialAscii | SourceKind::ModbusRtu => {
                    serial_fields(ui, &mut self.cfg.serial, ports, rescan_ports);
                    if kind == SourceKind::SerialAscii {
                        let a = &mut self.cfg.ascii;
                        ui.label("Poll command");
                        ui.vertical(|ui| {
                            ui.add(TextEdit::singleline(&mut a.poll_command).hint_text(r"blank = sensor streams, e.g. P?\r\n").desired_width(320.0));
                            ui.label(RichText::new(r"Escapes: \r \n \t \xNN").size(11.5).color(MUTED));
                        });
                        ui.end_row();
                        ui.label("Poll interval (ms)");
                        ui.add(DragValue::new(&mut a.poll_interval_ms).range(1..=600_000));
                        ui.end_row();
                        ui.label("Field delimiter");
                        ui.horizontal(|ui| {
                            ui.add(TextEdit::singleline(&mut a.delimiter).desired_width(60.0));
                            ui.label(RichText::new("blank = whitespace").size(12.0).color(MUTED));
                        });
                        ui.end_row();
                        ui.label("Field index (0 = first)");
                        ui.add(DragValue::new(&mut a.field_index).range(0..=64));
                        ui.end_row();
                    } else {
                        modbus_fields(ui, &mut self.cfg.modbus, false);
                        ui.label("Presets");
                        if ui.button("Keller Series 30 (P1 bar → psi)").clicked() {
                            self.cfg.keller_preset();
                        }
                        ui.end_row();
                    }
                }
                SourceKind::ModbusTcp => {
                    modbus_fields(ui, &mut self.cfg.modbus, true);
                    ui.label("Reply timeout (ms)");
                    ui.add(DragValue::new(&mut self.cfg.serial.timeout_ms).range(10..=10_000));
                    ui.end_row();
                }
                SourceKind::Simulated => {
                    let s = &mut self.cfg.sim;
                    ui.label("Waveform");
                    ui.horizontal(|ui| {
                        for w in [Waveform::Sine, Waveform::Square, Waveform::Triangle, Waveform::Ramp] {
                            ui.selectable_value(&mut s.waveform, w, format!("{w:?}"));
                        }
                    });
                    ui.end_row();
                    ui.label("Offset / amplitude (raw)");
                    ui.horizontal(|ui| {
                        num(ui, &mut s.offset);
                        ui.label("±");
                        num(ui, &mut s.amplitude);
                    });
                    ui.end_row();
                    ui.label("Frequency (Hz)");
                    ui.add(DragValue::new(&mut s.frequency_hz).range(0.0..=1000.0).speed(0.01));
                    ui.end_row();
                    ui.label("Noise σ (raw)");
                    ui.add(DragValue::new(&mut s.noise).range(0.0..=1e9).speed(0.01));
                    ui.end_row();
                    ui.label("Sample rate (Hz)");
                    ui.add(DragValue::new(&mut s.sample_rate_hz).range(0.1..=10_000.0));
                    ui.end_row();
                }
            });
        });
    }

    fn scaling(&mut self, ui: &mut Ui, live_raw: Option<f64>) {
        let raw_unit = self.cfg.kind.raw_unit();
        let cfg = &mut self.cfg;
        fieldset(ui, "Units & scaling", |ui| {
            grid(ui, "scaling", |ui| {
                ui.label("Units");
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut cfg.unit).desired_width(110.0));
                    ComboBox::from_id_salt("unit_presets").width(120.0).selected_text("Presets").show_ui(ui, |ui| {
                        for (group, units) in UNIT_GROUPS {
                            ui.label(RichText::new(*group).size(11.5).color(MUTED));
                            for u in *units {
                                ui.selectable_value(&mut cfg.unit, u.to_string(), format!("   {u}"));
                            }
                        }
                    });
                });
                ui.end_row();

                ui.label("Scaling");
                ComboBox::from_id_salt("scaling_mode").width(320.0).selected_text(cfg.scaling.mode.label()).show_ui(ui, |ui| {
                    for m in ScalingMode::ALL {
                        ui.selectable_value(&mut cfg.scaling.mode, m, m.label());
                    }
                });
                ui.end_row();

                let sc = &mut cfg.scaling;
                match sc.mode {
                    ScalingMode::Linear => {
                        ui.label("Gain");
                        num(ui, &mut sc.gain);
                        ui.end_row();
                        ui.label(format!("Intercept ({})", cfg.unit));
                        num(ui, &mut sc.intercept);
                        ui.end_row();
                    }
                    ScalingMode::Range => {
                        ui.label(format!("Raw span ({raw_unit})"));
                        ui.horizontal(|ui| {
                            num(ui, &mut sc.raw_lo);
                            ui.label("to");
                            num(ui, &mut sc.raw_hi);
                        });
                        ui.end_row();
                        ui.label(format!("Value span ({})", cfg.unit));
                        ui.horizontal(|ui| {
                            num(ui, &mut sc.eng_lo);
                            ui.label("to");
                            num(ui, &mut sc.eng_hi);
                        });
                        ui.end_row();
                    }
                    ScalingMode::Polynomial => {
                        ui.label("Polynomial order");
                        ui.horizontal(|ui| {
                            ui.add(DragValue::new(&mut sc.poly_order).range(1..=6));
                            ui.label(RichText::new("1 = straight line, 2 = quadratic…").size(12.0).color(MUTED));
                        });
                        ui.end_row();
                    }
                    ScalingMode::Table => {}
                }

                ui.label(format!("Offset ({})", cfg.unit));
                ui.horizontal(|ui| {
                    num(ui, &mut cfg.offset);
                    ui.label(RichText::new("added after scaling (Tare sets this)").size(12.0).color(MUTED));
                });
                ui.end_row();
            });

            if cfg.scaling.mode.uses_points() {
                ui.add_space(6.0);
                points_table(ui, cfg, raw_unit, live_raw);
            }
            if cfg.scaling.mode == ScalingMode::Polynomial
                && let Err(e) = cfg.scaling.refit() {
                    ui.label(RichText::new(e).color(ERR));
                }
            ui.add_space(4.0);
            ui.label(RichText::new(cfg.scaling.describe()).monospace().color(ACCENT_2));
            if let Some(raw) = live_raw {
                ui.label(
                    RichText::new(format!(
                        "Live: {raw:.5} {raw_unit}  →  {:.5} {}",
                        cfg.value_of(raw),
                        cfg.unit
                    ))
                    .color(OK),
                );
            }
            scaling_preview(ui, cfg, live_raw);
        });
    }

    fn range_display(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        fieldset(ui, "Range & display", |ui| {
            grid(ui, "range", |ui| {
                ui.label(format!("Min / max range ({})", cfg.unit));
                ui.horizontal(|ui| {
                    num(ui, &mut cfg.range_min);
                    ui.label("to");
                    num(ui, &mut cfg.range_max);
                });
                ui.end_row();
                ui.label("");
                ui.vertical(|ui| {
                    ui.checkbox(&mut cfg.range_as_axis, "Use range as the fixed graph Y axis");
                    ui.checkbox(&mut cfg.clamp_to_range, "Clamp values to the range");
                    ui.label(RichText::new("Readings outside the range turn red.").size(12.0).color(MUTED));
                });
                ui.end_row();
                ui.label("Points before scrolling");
                ui.add(DragValue::new(&mut cfg.visible_points).range(10..=1_000_000).speed(5.0));
                ui.end_row();
                ui.label("Graph motion");
                ui.horizontal(|ui| {
                    for mode in [ScrollMode::Continuous, ScrollMode::Step] {
                        ui.radio_value(&mut cfg.scroll_mode, mode, mode.label());
                    }
                    if cfg.scroll_mode == ScrollMode::Step {
                        ui.label(RichText::new("by").color(MUTED));
                        ui.add(DragValue::new(&mut cfg.step_points).range(1..=cfg.visible_points.max(1)).speed(5.0).suffix(" pts"));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.label(
                    RichText::new("Slide keeps the newest reading at the right edge. Step holds the graph still while it fills, then jumps over by the step.")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.end_row();
                ui.label("Decimals shown");
                ui.add(DragValue::new(&mut cfg.decimals).range(0..=8));
                ui.end_row();
                ui.label("Show");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut cfg.show_raw, "Raw trace & reading");
                    ui.checkbox(&mut cfg.show_avg, "Moving average");
                });
                ui.end_row();
                ui.label("Average over");
                ui.add(DragValue::new(&mut cfg.avg_points).range(2..=100_000).speed(1.0).suffix(" pts"));
                ui.end_row();
            });
        });
    }
}

fn points_table(ui: &mut Ui, cfg: &mut SensorConfig, raw_unit: &str, live_raw: Option<f64>) {
    ui.label(RichText::new("Calibration points (correlate raw readings with known values)").color(MUTED));
    let mut remove = None;
    Grid::new("cal_points").num_columns(3).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
        ui.label(RichText::new(format!("Raw ({raw_unit})")).strong());
        ui.label(RichText::new(format!("Value ({})", cfg.unit)).strong());
        ui.label("");
        ui.end_row();
        for (i, p) in cfg.scaling.points.iter_mut().enumerate() {
            num(ui, &mut p.raw);
            num(ui, &mut p.value);
            if ui.small_button("✕").on_hover_text("Remove point").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        cfg.scaling.points.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.button("+ Add point").clicked() {
            let next = cfg.scaling.points.last().map_or(CalPoint { raw: 0.0, value: 0.0 }, |p| CalPoint {
                raw: p.raw + 1.0,
                value: p.value,
            });
            cfg.scaling.points.push(next);
        }
        let capture = ui
            .add_enabled(live_raw.is_some(), egui::Button::new("Capture live raw"))
            .on_hover_text("Adds a point at the current raw reading; then type the known value")
            .on_disabled_hover_text("Connect the sensor to capture its current reading");
        if capture.clicked()
            && let Some(raw) = live_raw {
                let value = cfg.scaling.apply(raw);
                cfg.scaling.points.push(CalPoint { raw, value });
            }
        if ui.button("Sort").clicked() {
            cfg.scaling.points = cfg.scaling.sorted_points();
        }
    });
}

fn scaling_preview(ui: &mut Ui, cfg: &SensorConfig, live_raw: Option<f64>) {
    let sc = &cfg.scaling;
    let (mut lo, mut hi) = match sc.mode {
        ScalingMode::Range => (sc.raw_lo.min(sc.raw_hi), sc.raw_lo.max(sc.raw_hi)),
        ScalingMode::Polynomial | ScalingMode::Table if sc.points.len() >= 2 => {
            let pts = sc.sorted_points();
            (pts[0].raw, pts[pts.len() - 1].raw)
        }
        _ if cfg.kind.is_ni() => (cfg.ni.min, cfg.ni.max),
        _ => (0.0, 10.0),
    };
    if let Some(r) = live_raw {
        lo = lo.min(r);
        hi = hi.max(r);
    }
    if hi <= lo {
        hi = lo + 1.0;
    }
    let pad = (hi - lo) * 0.1;
    let (lo, hi) = (lo - pad, hi + pad);
    let curve: PlotPoints = (0..=200).map(|i| {
        let x = lo + (hi - lo) * i as f64 / 200.0;
        [x, cfg.scaling.apply(x) + cfg.offset]
    }).collect();

    ui.add_space(4.0);
    Frame::new().fill(BG_CHART).corner_radius(8).inner_margin(4).show(ui, |ui| {
        crate::tiles::styled_plot("scaling_preview")
            .height(150.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .x_axis_label(format!("raw ({})", cfg.kind.raw_unit()))
            .y_axis_label(cfg.unit.clone())
            .show(ui, |pui| {
                pui.line(Line::new("scaling", curve).color(ACCENT_2).width(1.5));
                if sc.mode.uses_points() {
                    let pts: PlotPoints = sc.points.iter().map(|p| [p.raw, p.value + cfg.offset]).collect();
                    pui.points(Points::new("points", pts).radius(4.0).color(WARN));
                }
                if let Some(r) = live_raw {
                    pui.points(Points::new("live", vec![[r, cfg.value_of(r)]]).radius(5.0).color(OK));
                }
            });
    });
}

fn serial_fields(ui: &mut Ui, s: &mut SerialSettings, ports: &[String], rescan: &mut bool) {
    ui.label("COM port");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("port")
            .width(140.0)
            .selected_text(if s.port.is_empty() { "Select port…" } else { &s.port })
            .show_ui(ui, |ui| {
                if ports.is_empty() {
                    ui.label(RichText::new("No ports found").color(MUTED));
                }
                for p in ports {
                    ui.selectable_value(&mut s.port, p.clone(), p);
                }
            });
        ui.add(TextEdit::singleline(&mut s.port).hint_text("COM3").desired_width(80.0));
        if ui.button("Refresh").clicked() {
            *rescan = true;
        }
    });
    ui.end_row();

    ui.label("Baud rate");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("baud").width(100.0).selected_text(s.baud.to_string()).show_ui(ui, |ui| {
            for b in [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                ui.selectable_value(&mut s.baud, b, b.to_string());
            }
        });
        ui.add(DragValue::new(&mut s.baud).range(50..=4_000_000));
    });
    ui.end_row();

    ui.label("Framing");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("bits").width(60.0).selected_text(format!("{} data", s.data_bits)).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.data_bits, 8, "8");
            ui.selectable_value(&mut s.data_bits, 7, "7");
        });
        ComboBox::from_id_salt("parity").width(80.0).selected_text(format!("{:?}", s.parity)).show_ui(ui, |ui| {
            for p in [Parity::None, Parity::Even, Parity::Odd] {
                ui.selectable_value(&mut s.parity, p, format!("{p:?}"));
            }
        });
        ComboBox::from_id_salt("stop").width(60.0).selected_text(format!("{} stop", s.stop_bits)).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.stop_bits, 1, "1");
            ui.selectable_value(&mut s.stop_bits, 2, "2");
        });
    });
    ui.end_row();

    ui.label("Reply timeout (ms)");
    ui.add(DragValue::new(&mut s.timeout_ms).range(10..=10_000));
    ui.end_row();
}

fn modbus_fields(ui: &mut Ui, m: &mut ModbusSettings, tcp: bool) {
    if tcp {
        ui.label("Host / port");
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut m.host).desired_width(200.0));
            ui.add(DragValue::new(&mut m.tcp_port).range(1..=65535));
        });
        ui.end_row();
    }
    ui.label(if tcp { "Unit ID" } else { "Slave ID" });
    ui.add(DragValue::new(&mut m.slave_id).range(0..=247));
    ui.end_row();
    ui.label("Register");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut m.function, ModbusFunction::Holding, "Holding (FC3)");
        ui.selectable_value(&mut m.function, ModbusFunction::Input, "Input (FC4)");
        ui.add(DragValue::new(&mut m.register).range(0..=65535).prefix("addr "));
    });
    ui.end_row();
    ui.label("Data type");
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("regtype").width(150.0).selected_text(m.data_type.label()).show_ui(ui, |ui| {
            for t in RegisterType::ALL {
                ui.selectable_value(&mut m.data_type, t, t.label());
            }
        });
        if m.data_type.register_count() == 2 {
            ui.checkbox(&mut m.word_swap, "Low word first");
        }
    });
    ui.end_row();
    ui.label("Poll interval (ms)");
    ui.add(DragValue::new(&mut m.poll_interval_ms).range(1..=600_000));
    ui.end_row();
}

fn num(ui: &mut Ui, v: &mut f64) -> egui::Response {
    let speed = (v.abs() * 0.001).max(0.001);
    ui.add(DragValue::new(v).speed(speed).max_decimals(6).min_decimals(0))
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    Grid::new(id).num_columns(2).spacing([16.0, 8.0]).min_col_width(170.0).show(ui, add);
}

pub fn fieldset(ui: &mut Ui, legend: &str, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(legend).color(MUTED));
            add(ui);
        });
    ui.add_space(10.0);
}
