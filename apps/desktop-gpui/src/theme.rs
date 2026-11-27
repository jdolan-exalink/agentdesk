use gpui_kit::base::StyledExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: u32,
    pub panel: u32,
    pub raised: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub active: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub sidebar_bg: u32,
    pub sidebar_active: u32,
    pub sidebar_text: u32,
    pub sidebar_muted: u32,
    pub badge_ok_bg: u32,
    pub badge_ok_fg: u32,
    pub badge_off_bg: u32,
    pub badge_off_fg: u32,
    /// Fondo de la ventana (riel de navegación); el contenido va en `canvas`.
    pub win: u32,
    /// Texto terciario (pistas, cabeceras de tabla).
    pub faint: u32,
    /// Borde más marcado (inputs, separadores fuertes).
    pub line2: u32,
    pub warn_soft: u32,
    pub bad_soft: u32,
}

/// Tokens de la maqueta aprobada (docs/ui-mockup/agentdesk-maqueta.html, :root oscuro).
pub const DARK: Palette = Palette {
    canvas: 0x13161D,
    panel: 0x13161D,
    raised: 0x1A1E27,
    border: 0x222733,
    text: 0xE9ECF2,
    muted: 0x8C95A8,
    accent: 0x3B7BFF,
    active: 0x16233F,
    success: 0x2BC46A,
    warning: 0xF0A12B,
    danger: 0xF05252,
    sidebar_bg: 0x0D0F14,
    sidebar_active: 0x16233F,
    sidebar_text: 0x3B7BFF,
    sidebar_muted: 0x8C95A8,
    badge_ok_bg: 0x10261A,
    badge_ok_fg: 0x2BC46A,
    badge_off_bg: 0x1A1E27,
    badge_off_fg: 0x8C95A8,
    win: 0x0D0F14,
    faint: 0x5D6577,
    line2: 0x2C3241,
    warn_soft: 0x2A1F0B,
    bad_soft: 0x2D1214,
};

/// Tokens de la maqueta aprobada (:root claro): superficies neutras, acento único.
pub const LIGHT: Palette = Palette {
    canvas: 0xFFFFFF,
    panel: 0xFFFFFF,
    raised: 0xF1F3F7,
    border: 0xE5E8EE,
    text: 0x0E1117,
    muted: 0x667085,
    accent: 0x2563EB,
    active: 0xE8EFFE,
    success: 0x16A34A,
    warning: 0xC97A06,
    danger: 0xDC2626,
    sidebar_bg: 0xFBFCFD,
    sidebar_active: 0xE8EFFE,
    sidebar_text: 0x2563EB,
    sidebar_muted: 0x667085,
    badge_ok_bg: 0xE3F5E9,
    badge_ok_fg: 0x16A34A,
    badge_off_bg: 0xF1F3F7,
    badge_off_fg: 0x667085,
    win: 0xF6F7FA,
    faint: 0x98A2B3,
    line2: 0xD9DEE6,
    warn_soft: 0xFCF1DC,
    bad_soft: 0xFDE8E8,
};

pub fn bar_color(p: Palette, pct: f32) -> u32 {
    if pct >= 85.0 {
        p.danger
    } else if pct >= 65.0 {
        p.warning
    } else {
        p.success
    }
}

fn progress_row(title: String, detail: String, pct: f32, p: Palette) -> Div {
    let clamped = pct.clamp(0.0, 100.0);
    let fill = bar_color(p, clamped);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().text_xs().font_semibold().text_color(rgb(p.text)).child(title))
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(rgb(fill))
                        .child(format!("{clamped:.0}%")),
                ),
        )
        .child(div().text_xs().text_color(rgb(p.muted)).child(detail))
        .child(
            div()
                .w_full()
                .h(px(6.))
                .rounded_full()
                .bg(rgb(p.raised))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(clamped / 100.0))
                        .rounded_full()
                        .bg(rgb(fill)),
                ),
        )
}

pub fn extract_resources(v: &serde_json::Value) -> Option<serde_json::Value> {
    let candidates = [
        Some(v),
        v.get("data"),
        v.get("data").and_then(|d| d.get("resources")),
        v.get("resources"),
        v.get("result"),
        v.get("result").and_then(|d| d.get("resources")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.get("cpu").is_some() && c.get("ram").is_some() {
            return Some(c.clone());
        }
    }
    None
}

pub fn render_hardware_resources(hw: &serde_json::Value, p: Palette) -> Div {
    let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("-")
            .to_string()
    };

    let host = s(hw, "hostname");
    let os = s(hw, "os_caption");
    let up = f(hw, "uptime_hours");

    let mut root = div().flex().flex_col().gap_3().child(
        div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(format!("{host} · {os} · uptime {up:.1} h")),
    );

    if let Some(cpu) = hw.get("cpu") {
        root = root.child(progress_row(
            "Procesador".into(),
            format!(
                "{} · {} núcleos / {} hilos",
                s(cpu, "name"),
                f(cpu, "cores") as i64,
                f(cpu, "logical_processors") as i64
            ),
            f(cpu, "load_pct") as f32,
            p,
        ));
    }

    if let Some(ram) = hw.get("ram") {
        root = root.child(progress_row(
            "Memoria RAM".into(),
            format!(
                "{:.1} GB usados de {:.1} GB ({:.1} GB libres)",
                f(ram, "used_gb"),
                f(ram, "total_gb"),
                f(ram, "free_gb")
            ),
            f(ram, "load_pct") as f32,
            p,
        ));
    }

    if let Some(disks) = hw.get("disks").and_then(|d| d.as_array()) {
        for d in disks {
            root = root.child(progress_row(
                format!("Disco {} ({})", s(d, "drive"), s(d, "label")),
                format!(
                    "{:.0} GB libres de {:.0} GB · estado: {}",
                    f(d, "free_gb"),
                    f(d, "total_gb"),
                    s(d, "status")
                ),
                f(d, "used_pct") as f32,
                p,
            ));
        }
    }

    if let Some(gpus) = hw.get("gpus").and_then(|d| d.as_array()) {
        for g in gpus {
            let vram = g
                .get("vram_gb")
                .and_then(|x| x.as_f64())
                .map(|v| format!("{v:.1} GB VRAM"))
                .unwrap_or_else(|| "VRAM n/d".into());
            let detail = format!("{} · driver {} · {}", vram, s(g, "driver"), s(g, "status"));
            match g.get("load_pct").and_then(|x| x.as_f64()) {
                Some(load) => {
                    root = root.child(progress_row(
                        format!("GPU {}", s(g, "name")),
                        detail,
                        load as f32,
                        p,
                    ));
                }
                None => {
                    root = root.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(rgb(p.text))
                                    .child(format!("GPU {}", s(g, "name"))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child(format!("{detail} · carga n/d")),
                            ),
                    );
                }
            }
        }
    }

    if let Some(users) = hw.get("windows_users").and_then(|d| d.as_array()) {
        let names: Vec<String> = users
            .iter()
            .map(|u| {
                let en = u.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
                format!("{}{}", s(u, "name"), if en { "" } else { " (deshab.)" })
            })
            .collect();
        root = root.child(
            div()
                .pt_2()
                .border_t_1()
                .border_color(rgb(p.border))
                .text_xs()
                .text_color(rgb(p.muted))
                .child(format!("Usuarios Windows: {}", names.join(", "))),
        );
    }

    if let Some(sess) = hw.get("windows_sessions").and_then(|d| d.as_array()) {
        root = root.when(!sess.is_empty(), |this| {
            let txt: Vec<String> = sess
                .iter()
                .map(|se| {
                    format!(
                        "#{} {} ({})",
                        s(se, "id"),
                        s(se, "username"),
                        s(se, "state")
                    )
                })
                .collect();
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(format!("Sesiones WTS: {}", txt.join(" · "))),
            )
        });
    }

    root
}
