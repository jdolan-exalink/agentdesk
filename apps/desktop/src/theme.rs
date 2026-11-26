use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    OpenVmsLight,
    DraculaDark,
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::OpenVmsLight
    }
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenVmsLight => "OpenVMS Claro",
            Self::DraculaDark => "Dracula Oscuro",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::OpenVmsLight => Self::DraculaDark,
            Self::DraculaDark => Self::OpenVmsLight,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    #[allow(dead_code)]
    pub mode: ThemeMode,
    pub canvas_bg: Color32,
    pub sidebar_bg: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub input_bg: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_text: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub mono_highlight: Color32,
    pub badge_bg: Color32,
}

pub fn palette_for(mode: ThemeMode) -> Palette {
    match mode {
        ThemeMode::OpenVmsLight => Palette {
            mode,
            canvas_bg: Color32::from_rgb(0xEE, 0xF2, 0xF7),
            sidebar_bg: Color32::from_rgb(0xE2, 0xE8, 0xF0),
            card_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            card_border: Color32::from_rgb(0xC4, 0xD0, 0xDF),
            input_bg: Color32::from_rgb(0xF8, 0xFA, 0xFC),
            text_primary: Color32::from_rgb(0x0F, 0x17, 0x2A),
            text_muted: Color32::from_rgb(0x47, 0x55, 0x69),
            accent: Color32::from_rgb(0x1D, 0x4E, 0xD8),
            accent_hover: Color32::from_rgb(0x1E, 0x40, 0xAF),
            accent_text: Color32::WHITE,
            ok: Color32::from_rgb(0x15, 0x80, 0x3D),
            warn: Color32::from_rgb(0xB4, 0x53, 0x09),
            danger: Color32::from_rgb(0xDC, 0x26, 0x26),
            mono_highlight: Color32::from_rgb(0x0F, 0x4C, 0x81),
            badge_bg: Color32::from_rgb(0xDB, 0xEA, 0xFE),
        },
        ThemeMode::DraculaDark => Palette {
            mode,
            canvas_bg: Color32::from_rgb(0x28, 0x2A, 0x36),
            sidebar_bg: Color32::from_rgb(0x21, 0x22, 0x2C),
            card_bg: Color32::from_rgb(0x31, 0x34, 0x44),
            card_border: Color32::from_rgb(0x44, 0x47, 0x5A),
            input_bg: Color32::from_rgb(0x21, 0x22, 0x2C),
            text_primary: Color32::from_rgb(0xF8, 0xF8, 0xF2),
            text_muted: Color32::from_rgb(0x94, 0xA3, 0xD8),
            accent: Color32::from_rgb(0xBD, 0x93, 0xF9),
            accent_hover: Color32::from_rgb(0xCA, 0xA9, 0xFA),
            accent_text: Color32::from_rgb(0x1E, 0x1F, 0x29),
            ok: Color32::from_rgb(0x50, 0xFA, 0x7B),
            warn: Color32::from_rgb(0xFF, 0xB8, 0x6C),
            danger: Color32::from_rgb(0xFF, 0x55, 0x55),
            mono_highlight: Color32::from_rgb(0x8B, 0xE9, 0xFD),
            badge_bg: Color32::from_rgb(0x44, 0x47, 0x5A),
        },
    }
}

pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) -> Palette {
    let p = palette_for(mode);
    let mut visuals = match mode {
        ThemeMode::OpenVmsLight => egui::Visuals::light(),
        ThemeMode::DraculaDark => egui::Visuals::dark(),
    };

    visuals.window_fill = p.canvas_bg;
    visuals.panel_fill = p.canvas_bg;
    visuals.extreme_bg_color = p.input_bg;
    visuals.faint_bg_color = p.sidebar_bg;
    visuals.code_bg_color = p.badge_bg;
    visuals.override_text_color = Some(p.text_primary);
    visuals.selection.bg_fill = p.accent;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent_text);
    visuals.hyperlink_color = p.accent;

    let radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = p.card_bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.card_border);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text_primary);

    visuals.widgets.inactive.bg_fill = p.sidebar_bg;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.card_border);
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text_primary);

    visuals.widgets.hovered.bg_fill = p.badge_bg;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.2_f32, p.accent);
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, p.text_primary);

    visuals.widgets.active.bg_fill = p.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.accent_hover);
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.accent_text);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.indent = 16.0;
    ctx.set_style(style);

    p
}

pub fn styled_card(p: &Palette) -> egui::Frame {
    egui::Frame::NONE
        .fill(p.card_bg)
        .stroke(Stroke::new(1.0_f32, p.card_border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
}

pub fn badge_frame(p: &Palette) -> egui::Frame {
    egui::Frame::NONE
        .fill(p.badge_bg)
        .stroke(Stroke::new(1.0_f32, p.card_border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
}

pub fn primary_button<'a>(p: &Palette, text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(
        RichText::new(text.into())
            .color(p.accent_text)
            .strong()
            .font(FontId::proportional(14.0)),
    )
    .fill(p.accent)
    .stroke(Stroke::new(1.0_f32, p.accent_hover))
    .corner_radius(CornerRadius::same(8))
}

pub fn danger_button<'a>(p: &Palette, text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(
        RichText::new(text.into())
            .color(Color32::WHITE)
            .strong(),
    )
    .fill(p.danger)
    .stroke(Stroke::new(1.0_f32, p.danger))
    .corner_radius(CornerRadius::same(8))
}
