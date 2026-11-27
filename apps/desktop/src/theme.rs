use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    System,
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
            Self::System => "Sistema",
            Self::OpenVmsLight => "Claro",
            Self::DraculaDark => "Oscuro",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::System | Self::OpenVmsLight => Self::DraculaDark,
            Self::DraculaDark => Self::OpenVmsLight,
        }
    }

    pub fn resolved(self, ctx: &egui::Context) -> Self {
        match self {
            Self::System => match ctx.system_theme() {
                Some(egui::Theme::Dark) => Self::DraculaDark,
                Some(egui::Theme::Light) | None => Self::OpenVmsLight,
            },
            mode => mode,
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
        ThemeMode::System => palette_for(ThemeMode::OpenVmsLight),
        ThemeMode::OpenVmsLight => Palette {
            mode,
            canvas_bg: Color32::from_rgb(0xF5, 0xF7, 0xFA),
            sidebar_bg: Color32::from_rgb(0xEE, 0xF2, 0xF6),
            card_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            card_border: Color32::from_rgb(0xDF, 0xE4, 0xEB),
            input_bg: Color32::from_rgb(0xF7, 0xF9, 0xFC),
            text_primary: Color32::from_rgb(0x18, 0x22, 0x30),
            text_muted: Color32::from_rgb(0x68, 0x75, 0x87),
            accent: Color32::from_rgb(0x25, 0x63, 0xEB),
            accent_hover: Color32::from_rgb(0x1D, 0x4E, 0xD8),
            accent_text: Color32::WHITE,
            ok: Color32::from_rgb(0x15, 0x96, 0x6B),
            warn: Color32::from_rgb(0xB4, 0x53, 0x09),
            danger: Color32::from_rgb(0xB9, 0x1C, 0x1C),
            mono_highlight: Color32::from_rgb(0x1D, 0x4E, 0xD8),
            badge_bg: Color32::from_rgb(0xE7, 0xEE, 0xFC),
        },
        ThemeMode::DraculaDark => Palette {
            mode,
            canvas_bg: Color32::from_rgb(0x10, 0x12, 0x16),
            sidebar_bg: Color32::from_rgb(0x17, 0x1A, 0x20),
            card_bg: Color32::from_rgb(0x17, 0x1A, 0x20),
            card_border: Color32::from_rgb(0x29, 0x2E, 0x37),
            input_bg: Color32::from_rgb(0x10, 0x12, 0x16),
            text_primary: Color32::from_rgb(0xE7, 0xEA, 0xF0),
            text_muted: Color32::from_rgb(0x9A, 0xA4, 0xB2),
            accent: Color32::from_rgb(0x34, 0x78, 0xF6),
            accent_hover: Color32::from_rgb(0x55, 0x8B, 0xF7),
            accent_text: Color32::WHITE,
            ok: Color32::from_rgb(0x22, 0xB9, 0x83),
            warn: Color32::from_rgb(0xFF, 0xB8, 0x6C),
            danger: Color32::from_rgb(0xFF, 0x5B, 0x61),
            mono_highlight: Color32::from_rgb(0xA8, 0xC7, 0xFF),
            badge_bg: Color32::from_rgb(0x20, 0x2E, 0x49),
        },
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes, family) in [
        (
            "Inter",
            include_bytes!("../assets/fonts/Inter.ttf").as_slice(),
            egui::FontFamily::Proportional,
        ),
        (
            "IBM Plex Mono",
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf").as_slice(),
            egui::FontFamily::Monospace,
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.to_owned());
    }
    ctx.set_fonts(fonts);
}

pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) -> Palette {
    let resolved = mode.resolved(ctx);
    let p = palette_for(resolved);
    let mut visuals = match resolved {
        ThemeMode::System => egui::Visuals::light(),
        ThemeMode::OpenVmsLight => egui::Visuals::light(),
        ThemeMode::DraculaDark => egui::Visuals::dark(),
    };

    visuals.window_fill = p.canvas_bg;
    visuals.panel_fill = p.canvas_bg;
    visuals.extreme_bg_color = p.input_bg;
    visuals.faint_bg_color = p.sidebar_bg;
    visuals.code_bg_color = p.badge_bg;
    visuals.override_text_color = Some(p.text_primary);
    visuals.selection.bg_fill = p.badge_bg;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
    visuals.hyperlink_color = p.accent;

    let radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = p.card_bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.card_border);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text_primary);

    visuals.widgets.inactive.bg_fill = p.sidebar_bg;
    visuals.widgets.inactive.weak_bg_fill = p.sidebar_bg;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.card_border);
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text_primary);

    visuals.widgets.hovered.bg_fill = p.badge_bg;
    visuals.widgets.hovered.weak_bg_fill = p.badge_bg;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.2_f32, p.accent);
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, p.text_primary);

    visuals.widgets.active.bg_fill = p.badge_bg;
    visuals.widgets.active.weak_bg_fill = p.badge_bg;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.accent_hover);
    visuals.widgets.active.corner_radius = radius;
    // egui also uses this foreground for every strong label, including card headings.
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.text_primary);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    for (role, font) in [
        (egui::TextStyle::Small, FontId::proportional(12.0)),
        (egui::TextStyle::Body, FontId::proportional(14.0)),
        (egui::TextStyle::Button, FontId::proportional(14.0)),
        (egui::TextStyle::Heading, FontId::proportional(24.0)),
        (egui::TextStyle::Monospace, FontId::monospace(14.0)),
    ] {
        style.text_styles.insert(role, font);
    }
    style.spacing.interact_size.y = 30.0;
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.indent = 16.0;
    ctx.set_style(style);

    p
}

pub fn centered_text_edit(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .horizontal_align(egui::Align::Center)
        .vertical_align(egui::Align::Center)
}

pub fn placeholder_text(p: &Palette, text: impl Into<String>) -> RichText {
    let blend = |foreground: u8, background: u8| {
        (foreground as f32 * 0.64 + background as f32 * 0.36) as u8
    };
    let color = Color32::from_rgb(
        blend(p.text_muted.r(), p.canvas_bg.r()),
        blend(p.text_muted.g(), p.canvas_bg.g()),
        blend(p.text_muted.b(), p.canvas_bg.b()),
    );
    RichText::new(text).color(color)
}

pub fn session_action_button(ui: &mut egui::Ui, sign_in: bool) -> egui::Response {
    let label = if sign_in {
        "Iniciar sesión"
    } else {
        "Cerrar sesión"
    };
    let response = icon_button(ui, label);
    let glyph = if sign_in { "↪" } else { "↩" };
    ui.painter().text(
        response.rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(18.0),
        ui.visuals().text_color(),
    );
    response.on_hover_text(label)
}

pub fn status_indicator(ui: &mut egui::Ui, color: Color32, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 22.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    response.on_hover_text(label)
}

pub fn theme_button(ui: &mut egui::Ui, palette: &Palette, mode: ThemeMode) -> egui::Response {
    let mode = mode.resolved(ui.ctx());
    let label = match mode {
        ThemeMode::System | ThemeMode::OpenVmsLight => "Cambiar a tema oscuro",
        ThemeMode::DraculaDark => "Cambiar a tema claro",
    };
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let painter = ui.painter();
    let stroke = Stroke::new(1.5_f32, palette.text_primary);
    match mode {
        ThemeMode::System | ThemeMode::OpenVmsLight => {
            let points = (0..=20)
                .map(|step| {
                    let angle = -0.4 + step as f32 * 0.235;
                    center + 7.0 * Vec2::angled(angle)
                })
                .chain([center + Vec2::new(-2.0, 2.0)])
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        ThemeMode::DraculaDark => {
            painter.circle_stroke(center, 4.0, stroke);
            for ray in 0..8 {
                let direction = Vec2::angled(ray as f32 * std::f32::consts::FRAC_PI_4);
                painter.line_segment([center + 6.0 * direction, center + 8.0 * direction], stroke);
            }
        }
    }
    response
}

pub fn copy_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> egui::Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let stroke = Stroke::new(1.5_f32, palette.text_primary);
    let back_top = center + Vec2::new(-7.0, -7.0);
    ui.painter()
        .line_segment([back_top, back_top + Vec2::new(10.0, 0.0)], stroke);
    ui.painter()
        .line_segment([back_top, back_top + Vec2::new(0.0, 10.0)], stroke);
    let front = egui::Rect::from_min_size(center - Vec2::splat(3.0), Vec2::splat(10.0));
    ui.painter()
        .rect_stroke(front, 2, stroke, egui::StrokeKind::Inside);
    response
}

pub fn refresh_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> egui::Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let stroke = Stroke::new(1.5_f32, palette.text_primary);
    ui.painter().circle_stroke(center, 6.0, stroke);

    let tip = center + Vec2::angled(0.45) * 8.0;
    let wing_a = center + Vec2::angled(2.2) * 5.0;
    let wing_b = center + Vec2::angled(3.2) * 5.0;
    ui.painter().line_segment([wing_a, tip], stroke);
    ui.painter().line_segment([wing_b, tip], stroke);
    response
}

pub fn visibility_button(ui: &mut egui::Ui, visible: bool) -> egui::Response {
    let label = if visible {
        "Ocultar contraseña temporal"
    } else {
        "Mostrar contraseña temporal"
    };
    let response = ui.add(egui::Button::new("").min_size(Vec2::splat(34.0)));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let center = response.rect.center();
    let stroke = Stroke::new(1.5_f32, ui.visuals().widgets.inactive.fg_stroke.color);
    ui.painter().add(egui::Shape::ellipse_stroke(
        center,
        Vec2::new(7.0, 4.5),
        stroke,
    ));
    ui.painter().circle_filled(center, 2.0, stroke.color);
    if !visible {
        ui.painter().line_segment(
            [center + Vec2::new(-7.0, 7.0), center + Vec2::new(7.0, -7.0)],
            stroke,
        );
    }
    response.on_hover_text(label)
}

fn icon_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let response = ui.add(egui::Button::new("").min_size(Vec2::splat(32.0)));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response.on_hover_text(label)
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
    egui::Button::new(RichText::new(text.into()).color(p.accent_text).strong())
        .fill(p.danger)
        .stroke(Stroke::new(1.0_f32, p.danger))
        .corner_radius(CornerRadius::same(8))
}
