//! Palette and widget styling shared with the Sensor Emulator web UI.

use egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
pub const BG_TOP: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub const BG_ELEV: Color32 = Color32::from_rgb(0x25, 0x25, 0x26);
pub const BG_CARD: Color32 = Color32::from_rgb(0x2d, 0x2d, 0x30);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1b);
pub const BG_CHART: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const LINE: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
pub const BTN: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
pub const BTN_STROKE: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0x7a, 0xcc);
pub const ACCENT_2: Color32 = Color32::from_rgb(0x4f, 0xc3, 0xf7);
/// Moving-average trace and readout.
pub const AVG: Color32 = Color32::from_rgb(0xff, 0xa7, 0x26);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xe8);
pub const MUTED: Color32 = Color32::from_rgb(0x9b, 0x9b, 0x9b);
pub const OK: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
pub const WARN: Color32 = Color32::from_rgb(0xcc, 0xa7, 0x00);
pub const ERR: Color32 = Color32::from_rgb(0xf1, 0x4c, 0x4c);

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = BG_ELEV;
    visuals.window_stroke = Stroke::new(1.0, LINE);
    visuals.extreme_bg_color = BG_INPUT;
    visuals.faint_bg_color = Color32::from_rgb(0x26, 0x26, 0x28);
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = ACCENT_2;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.window_corner_radius = 12.into();
    visuals.menu_corner_radius = 8.into();

    let w = &mut visuals.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    for (state, fill, stroke) in [
        (&mut w.inactive, BTN, BTN_STROKE),
        (&mut w.hovered, Color32::from_rgb(0x45, 0x45, 0x48), ACCENT),
        (&mut w.active, Color32::from_rgb(0x50, 0x50, 0x54), ACCENT),
        (&mut w.open, BTN, ACCENT),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = 6.into();
        state.expansion = 0.0;
    }
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.combo_width = 200.0;
        use egui::{FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

/// Segoe UI to match the emulator, with Segoe UI Symbol for ● ■ ▶ glyphs.
/// Falls back to egui's bundled fonts if the Windows fonts are missing.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut added = Vec::new();
    for (name, file) in [("segoe", "segoeui.ttf"), ("segoe_sym", "seguisym.ttf")] {
        if let Ok(bytes) = std::fs::read(format!(r"{windir}\Fonts\{file}")) {
            fonts.font_data.insert(name.into(), FontData::from_owned(bytes).into());
            added.push(name);
        }
    }
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    for (i, name) in added.iter().enumerate() {
        prop.insert(i, name.to_string());
    }
    if added.contains(&"segoe_sym") {
        fonts.families.entry(FontFamily::Monospace).or_default().push("segoe_sym".into());
    }
    ctx.set_fonts(fonts);
}
