//! Piezas de interfaz compartidas del rediseño minimalista (maqueta aprobada
//! docs/ui-mockup/agentdesk-maqueta.html): botones de icono con tooltip y nombre
//! accesible, chips, pastillas de estado, selector segmentado y medidores.

use crate::theme::Palette;
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Familia monoespaciada para IDs, contraseñas y métricas.
pub const MONO: &str = "Consolas";

/// Constructor de tooltip de texto para `.tooltip(...)`.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let t: SharedString = text.into();
    move |window, cx| Tooltip::new(t.clone()).build(window, cx)
}

fn sized_icon_btn(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    size: f32,
    icon_size: f32,
    p: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(if size > 30. { 9. } else { 7. }))
        .cursor_pointer()
        .text_color(rgb(if active { p.accent } else { p.muted }))
        .when(active, |d| d.bg(rgb(p.active)))
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(rgb(p.raised)).text_color(rgb(p.text))
            }
        })
        .child(Icon::new(icon).size(px(icon_size)))
        .aria_label(label.clone())
        .tooltip(tip(label))
}

/// Botón de icono estándar (34 px) con tooltip y nombre accesible.
pub fn icon_btn(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    p: Palette,
) -> Stateful<Div> {
    sized_icon_btn(id, icon, label, active, 34., 18., p)
}

/// Botón de icono pequeño (28 px) para filas, tarjetas y barras secundarias.
pub fn icon_btn_sm(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    p: Palette,
) -> Stateful<Div> {
    sized_icon_btn(id, icon, label, active, 28., 16., p)
}

/// Botón del riel de navegación (40 px).
pub fn rail_btn(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    p: Palette,
) -> Stateful<Div> {
    sized_icon_btn(id, icon, label, active, 40., 19., p).rounded(px(10.))
}

/// Botón primario con texto (acciones de alto impacto: Conectar, Cortar...).
pub fn primary_btn(id: impl Into<ElementId>, icon: Option<IconName>, label: impl Into<SharedString>, p: Palette) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(38.))
        .px(px(14.))
        .rounded(px(9.))
        .cursor_pointer()
        .bg(rgb(p.accent))
        .text_color(rgb(0xFFFFFF))
        .text_sm()
        .font_semibold()
        .hover(|s| s.opacity(0.92))
        .when_some(icon, |d, i| d.child(Icon::new(i).size(px(17.))))
        .aria_label(label.clone())
        .child(label)
}

/// Botón secundario con borde y texto.
pub fn ghost_btn(id: impl Into<ElementId>, icon: Option<IconName>, label: impl Into<SharedString>, p: Palette) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(34.))
        .px_3()
        .rounded(px(9.))
        .border_1()
        .border_color(rgb(p.border))
        .cursor_pointer()
        .text_color(rgb(p.text))
        .text_sm()
        .font_medium()
        .hover(|s| s.bg(rgb(p.raised)))
        .when_some(icon, |d, i| d.child(Icon::new(i).size(px(16.))))
        .aria_label(label.clone())
        .child(label)
}

/// Chip conmutador (icono + texto opcional), estilo `.tog` de la maqueta.
pub fn toggle_chip(
    id: impl Into<ElementId>,
    icon: IconName,
    text: Option<&'static str>,
    tooltip: impl Into<SharedString>,
    on: bool,
    p: Palette,
) -> Stateful<Div> {
    let tooltip: SharedString = tooltip.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(7.))
        .h(px(34.))
        .px(px(10.))
        .rounded(px(9.))
        .border_1()
        .border_color(rgb(if on { p.active } else { p.border }))
        .bg(rgb(if on { p.active } else { p.canvas }))
        .text_color(rgb(if on { p.accent } else { p.muted }))
        .text_xs()
        .font_medium()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(p.line2)))
        .child(Icon::new(icon).size(px(16.)))
        .when_some(text, |d, t| d.child(t))
        .aria_label(tooltip.clone())
        .tooltip(tip(tooltip))
}

/// Pastilla de estado con punto (`.pill` de la maqueta).
pub fn status_pill(text: impl Into<SharedString>, fg: u32, bg: u32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_xs()
        .font_semibold()
        .child(div().size(px(7.)).rounded_full().bg(rgb(fg)))
        .child(text.into())
}

/// Punto de estado de 7 px.
pub fn dot(color: u32) -> Div {
    div().flex_none().size(px(7.)).rounded_full().bg(rgb(color))
}

/// Etiqueta gris pequeña (`.tag`).
pub fn tag(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(px(2.))
        .rounded(px(6.))
        .bg(rgb(p.raised))
        .text_color(rgb(p.muted))
        .text_xs()
        .child(text.into())
}

/// Rótulo de sección con icono (`.lbl`).
pub fn label_row(icon: IconName, text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_xs()
        .font_medium()
        .text_color(rgb(p.muted))
        .child(Icon::new(icon).size(px(15.)))
        .child(text.into())
}

/// Encabezado de sección (`.sh h3`).
pub fn section_head(icon: IconName, text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .font_semibold()
        .text_color(rgb(p.muted))
        .child(Icon::new(icon).size(px(16.)))
        .child(text.into())
}

/// Tarjeta base (`.card`).
pub fn card(p: Palette) -> Div {
    div()
        .rounded(px(14.))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
}

/// Contenedor del selector segmentado (`.seg`).
pub fn seg(p: Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(10.))
        .bg(rgb(p.raised))
}

/// Opción del selector segmentado (solo icono, con tooltip).
pub fn seg_btn(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    on: bool,
    p: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(38.))
        .h(px(30.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_color(rgb(if on { p.text } else { p.muted }))
        .when(on, |d| d.bg(rgb(p.canvas)).shadow_sm())
        .hover(|s| s.text_color(rgb(p.text)))
        .child(Icon::new(icon).size(px(17.)))
        .aria_label(label.clone())
        .tooltip(tip(label))
}

/// Medidor horizontal con título y porcentaje (`.meter`).
pub fn meter(title: impl Into<SharedString>, pct: f32, p: Palette) -> Div {
    let v = pct.clamp(0.0, 100.0);
    let col = crate::theme::bar_color(p, v);
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .child(div().text_color(rgb(p.text)).child(title.into()))
                .child(div().font_family(MONO).text_color(rgb(p.muted)).child(format!("{v:.0}%"))),
        )
        .child(
            div()
                .h(px(6.))
                .w_full()
                .rounded_full()
                .bg(rgb(p.raised))
                .overflow_hidden()
                .child(div().h_full().w(relative(v / 100.0)).rounded_full().bg(rgb(col))),
        )
}

/// Mini barra de CPU de 70 px (`.mbar`).
pub fn mini_bar(pct: f32, p: Palette) -> Div {
    let v = pct.clamp(0.0, 100.0);
    div()
        .w(px(70.))
        .h(px(5.))
        .rounded_full()
        .bg(rgb(p.raised))
        .overflow_hidden()
        .child(div().h_full().w(relative(v / 100.0)).rounded_full().bg(rgb(crate::theme::bar_color(p, v))))
}

/// Separador vertical para barras de herramientas.
pub fn vsep(p: Palette) -> Div {
    div().flex_none().w(px(1.)).h(px(22.)).mx_1().bg(rgb(p.line2))
}

/// Ítem de menú emergente (icono + texto + atajo opcional), estilo `.pop .it`.
pub fn menu_item(
    id: impl Into<ElementId>,
    icon: IconName,
    text: impl Into<SharedString>,
    hint: Option<SharedString>,
    on: bool,
    p: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(34.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(if on { p.accent } else { p.text }))
        .hover(|s| s.bg(rgb(p.raised)))
        .child(Icon::new(icon).size(px(16.)))
        .child(div().flex_1().min_w_0().truncate().child(text.into()))
        .when_some(hint, |d, h| {
            d.child(div().font_family(MONO).text_xs().text_color(rgb(p.faint)).child(h))
        })
}

/// Encabezado de grupo dentro de un menú emergente (`.pop .hd`).
pub fn menu_head(text: &'static str, p: Palette) -> Div {
    div()
        .px(px(10.))
        .pt(px(6.))
        .pb(px(4.))
        .text_xs()
        .text_color(rgb(p.faint))
        .child(text.to_uppercase())
}

/// Superficie flotante (menús, paneles laterales del visor).
pub fn floating(p: Palette) -> Div {
    div()
        .rounded(px(12.))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .shadow_lg()
}

/// Agrupa un ID numérico en bloques de 3 ("482119730" → "482 119 730").
pub fn group_id(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.len() == 9 && clean.chars().all(|c| c.is_ascii_digit()) {
        format!("{} {} {}", &clean[0..3], &clean[3..6], &clean[6..9])
    } else {
        raw.to_string()
    }
}
