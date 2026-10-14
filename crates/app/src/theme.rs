//! Catppuccin Mocha look.
use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

pub const BASE: Color32 = Color32::from_rgb(30, 30, 46);
pub const MANTLE: Color32 = Color32::from_rgb(24, 24, 37);
pub const SURFACE0: Color32 = Color32::from_rgb(49, 50, 68);
pub const SURFACE1: Color32 = Color32::from_rgb(69, 71, 90);
pub const TEXT: Color32 = Color32::from_rgb(205, 214, 244);
pub const SUBTEXT: Color32 = Color32::from_rgb(166, 173, 200);
pub const MAUVE: Color32 = Color32::from_rgb(203, 166, 247);
pub const GREEN: Color32 = Color32::from_rgb(166, 227, 161);
pub const YELLOW: Color32 = Color32::from_rgb(249, 226, 175);
pub const RED: Color32 = Color32::from_rgb(243, 139, 168);
pub const BLUE: Color32 = Color32::from_rgb(137, 180, 250);

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = BASE;
    v.window_fill = BASE;
    v.extreme_bg_color = MANTLE;
    v.faint_bg_color = Color32::from_rgb(36, 36, 54);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = BLUE;
    v.selection.bg_fill = MAUVE.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, MAUVE);
    let r = CornerRadius::same(6);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_fill = SURFACE0;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SURFACE0);
    v.widgets.inactive.bg_fill = SURFACE0;
    v.widgets.inactive.weak_bg_fill = SURFACE0;
    v.widgets.hovered.bg_fill = SURFACE1;
    v.widgets.hovered.weak_bg_fill = SURFACE1;
    v.widgets.active.bg_fill = MAUVE.gamma_multiply(0.6);
    v.widgets.active.weak_bg_fill = MAUVE.gamma_multiply(0.6);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}
