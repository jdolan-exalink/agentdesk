use crate::config::{
    detect_lan_ip, fmt_elapsed, install_host_service_elevated, now_unix, set_windows_autostart,
};
use crate::theme::{render_hardware_resources, Palette};
use crate::{action_button, page_heading, subtle_button, workspace_panel, Client};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

fn parse_hex_color(hex: &str) -> Option<u32> {
    let clean = hex.trim().strip_prefix('#')?;
    if clean.len() == 6 {
        u32::from_str_radix(clean, 16).ok()
    } else {
        None
    }
}

fn device_icon_name(icon_kind: &str) -> IconName {
    match icon_kind.trim().to_ascii_lowercase().as_str() {
        "server" | "workstation" => IconName::Cpu,
        "database" => IconName::PanelBottom,
        "network" => IconName::Network,
        "cloud" => IconName::Globe,
        "camera" => IconName::Eye,
        _ => IconName::Monitor,
    }
}

fn render_device_badge(icon_kind: &str, accent_color: &str, online: bool, p: Palette) -> Div {
    let custom = parse_hex_color(accent_color);
    let screen_bg = if online {
        custom.unwrap_or(p.accent)
    } else {
        p.canvas
    };
    let screen_border = if online {
        custom.unwrap_or(p.accent)
    } else {
        custom.unwrap_or(p.border)
    };
    let icon_fg = if online {
        0xFFFFFF
    } else {
        custom.unwrap_or(p.muted)
    };
    let led_bg = if online { 0x22C55E_u32 } else { p.muted };

    div()
        .relative()
        .w(px(28.))
        .h(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(rgb(screen_border))
        .bg(rgb(screen_bg))
        .child(
            Icon::new(device_icon_name(icon_kind))
                .size(px(13.))
                .text_color(rgb(icon_fg)),
        )
        .child(
            div()
                .absolute()
                .bottom(px(-3.))
                .right(px(-3.))
                .size(px(8.))
                .rounded_full()
                .border_1()
                .border_color(rgb(p.panel))
                .bg(rgb(led_bg)),
        )
}

fn render_tree_branch(is_last: bool, p: Palette) -> Div {
    div()
        .relative()
        .w(px(24.))
        .h(px(34.))
        .child(
            div()
                .absolute()
                .left(px(9.))
                .top_0()
                .w(px(1.))
                .bg(rgb(p.border))
                .when(is_last, |el| el.h(px(17.)))
                .when(!is_last, |el| el.bottom_0()),
        )
        .child(
            div()
                .absolute()
                .left(px(9.))
                .top(px(17.))
                .w(px(13.))
                .h(px(1.))
                .bg(rgb(p.border)),
        )
}

impl Client {
    pub fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let tab_sessions = self.home_tab_sessions;

        let mut root = div().flex().flex_col().gap_4();

        // Selector segmentado centrado estilo TeamViewer: ID de AgentDesk | Sesiones
        root = root.child(
            div()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .p_1()
                        .gap_1()
                        .rounded_lg()
                        .bg(rgb(p.raised))
                        .border_1()
                        .border_color(rgb(p.border))
                        .child(
                            div()
                                .id("tab-home-id")
                                .px_4()
                                .py(px(6.))
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(if !tab_sessions { p.canvas } else { p.raised }))
                                .text_color(rgb(if !tab_sessions { p.accent } else { p.muted }))
                                .text_xs()
                                .font_semibold()
                                .child("ID de AgentDesk")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.home_tab_sessions = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("tab-home-sess")
                                .px_4()
                                .py(px(6.))
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(if tab_sessions { p.canvas } else { p.raised }))
                                .text_color(rgb(if tab_sessions { p.accent } else { p.muted }))
                                .text_xs()
                                .font_semibold()
                                .child("Sesiones")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.home_tab_sessions = true;
                                    if this.auth.is_some() {
                                        this.load_sessions(cx);
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        );

        // Banner si hay auto-reconexión activa
        if let Some((target, _, attempts)) = &self.auto_reconnect {
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(p.warning))
                    .bg(rgb(p.panel))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.warning))
                            .child(format!(
                                "Auto-reconexión activa para {} (intento {}) — reintentando cada 4 s",
                                target.display_name(),
                                attempts
                            )),
                    )
                    .child(
                        subtle_button("Cancelar", "cancel-auto-reconn", p)
                            .id("cancel-auto-reconn")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.auto_reconnect = None;
                                this.connect_status = "Auto-reconexión cancelada".into();
                                cx.notify();
                            })),
                    ),
            );
        }

        // Banner de control remoto entrante
        if let Some(inc) = self.incoming_control.clone() {
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(p.danger))
                    .bg(rgb(p.panel))
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(rgb(p.danger))
                            .child(format!(
                                "Control remoto entrante activo: {} ({})",
                                inc.caller_name,
                                if inc.allow_control { "Control total" } else { "Solo ver" }
                            )),
                    )
                    .child(
                        div()
                            .id("cut-incoming")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(p.danger))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .child("Cortar control remoto")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = inc.stop_tx.send(true);
                                this.incoming_control = None;
                                this.local_access_status = "Control remoto cortado localmente".into();
                                cx.notify();
                            })),
                    ),
            );
        }

        if tab_sessions {
            let mut panel = div()
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded_xl()
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_base().font_semibold().child("Tus sesiones"))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .when(self.auth.is_some(), |this| {
                                    this.child(
                                        subtle_button("Actualizar", "home-sess-ref", p)
                                            .id("home-sess-ref")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.load_sessions(cx);
                                            })),
                                    )
                                })
                                .child(
                                    action_button("+ Nueva sesión", "home-sess-new", p)
                                        .id("home-sess-new")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.home_tab_sessions = false;
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
            if self.auth.is_none() {
                panel = panel.child(self.auth_required_panel("Sesiones", p, cx));
            } else if self.sessions.is_empty() {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .py_10()
                        .gap_2()
                        .child(div().text_base().font_semibold().child("Todavía no hay sesiones remotas"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child("Usá 'ID de AgentDesk' o 'Dispositivos' para iniciar soporte remoto."),
                        ),
                );
            } else {
                for s in self.sessions.clone() {
                    let sid = s.id;
                    panel = panel.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().font_semibold().child(format!(
                                        "Sesión {}",
                                        &sid.to_string()[..8]
                                    )))
                                    .child(div().text_xs().text_color(rgb(p.muted)).child(format!(
                                        "{} · inicio {}",
                                        s.status,
                                        s.created_at.format("%Y-%m-%d %H:%M")
                                    ))),
                            )
                            .when(s.status != "ended", |this| {
                                this.child(
                                    subtle_button("Terminar", format!("hs-term-{sid}"), p)
                                        .id(SharedString::from(format!("hs-term-{sid}")))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.terminate_server_session(sid, cx);
                                        })),
                                )
                            }),
                    );
                }
            }
            return root.child(panel);
        }

        // Vista principal "ID de AgentDesk": Tarjeta unificada de 2 columnas + toggle Acceso fácil
        root = root
            .child(
                div()
                    .flex()
                    .gap_6()
                    .p_6()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.canvas))
                    .child(self.render_share_card(p, cx).flex_1())
                    .child(div().w(px(1.)).bg(rgb(p.border)))
                    .child(self.render_connect_card(p, cx).flex_1()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id("toggle-easy-access")
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(p.text))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size_4()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(if self.show_easy_access || self.cfg.personal_password_set {
                                        p.accent
                                    } else {
                                        p.border
                                    }))
                                    .bg(rgb(if self.show_easy_access || self.cfg.personal_password_set {
                                        p.accent
                                    } else {
                                        p.canvas
                                    }))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_xs()
                                    .child(if self.show_easy_access || self.cfg.personal_password_set {
                                        "✓"
                                    } else {
                                        ""
                                    }),
                            )
                            .child("Otorgar Acceso fácil y contraseña personal a este dispositivo")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_easy_access = !this.show_easy_access;
                                cx.notify();
                            })),
                    )
                    .when(self.show_easy_access, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.panel))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(Input::new(&self.personal_pw_input).flex_1().mask_toggle())
                                        .child(
                                            action_button("Fijar contraseña", "home-set-ppw", p)
                                                .id("home-set-ppw")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.set_personal_password(true, Some(window), cx);
                                                })),
                                        )
                                        .when(self.cfg.personal_password_set, |b| {
                                            b.child(
                                                subtle_button("Quitar", "home-clr-ppw", p)
                                                    .id("home-clr-ppw")
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.set_personal_password(false, Some(window), cx);
                                                    })),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(Input::new(&self.owner_email_input).flex_1())
                                        .child(
                                            subtle_button("Asignar email propietario", "home-assign-owner", p)
                                                .id("home-assign-owner")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cfg.owner_email =
                                                        this.owner_email_input.read(cx).value().trim().to_string();
                                                    let _ = this.cfg.save(&this.config_path);
                                                    this.restart_host(cx);
                                                    this.connect_status =
                                                        "Email propietario guardado y re-registrado".into();
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    }),
            );
        root
    }

    fn render_share_card(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_base().font_semibold().child("Permitir el control remoto"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(p.panel))
                    .border_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(p.muted)).child("Tu ID"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_bold()
                                            .text_color(rgb(p.text))
                                            .child(self.local_connection_id.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("copy-local-id")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(32.))
                                            .rounded_md()
                                            .cursor_pointer()
                                            .text_color(rgb(p.muted))
                                            .hover(|s| s.bg(rgb(p.raised)).text_color(rgb(p.text)))
                                            .child(Icon::new(IconName::Copy).size(px(17.)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    this.local_connection_id.clone(),
                                                ));
                                                this.connect_status = "ID copiado al portapapeles".into();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(p.muted)).child("Contraseña"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_semibold()
                                            .text_color(rgb(p.text))
                                            .child(if self.show_local_password {
                                                self.local_password.clone()
                                            } else {
                                                "••••••••".into()
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .id("toggle-local-password")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(32.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .text_color(rgb(p.muted))
                                                    .hover(|s| s.bg(rgb(p.raised)).text_color(rgb(p.text)))
                                                    .child(Icon::new(if self.show_local_password {
                                                        IconName::EyeOff
                                                    } else {
                                                        IconName::Eye
                                                    }).size(px(17.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.show_local_password = !this.show_local_password;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("rotate-local-password")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(32.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .text_color(rgb(p.muted))
                                                    .hover(|s| s.bg(rgb(p.raised)).text_color(rgb(p.text)))
                                                    .child(Icon::new(IconName::RefreshCw).size(px(17.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.rotate_local_password(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("copy-local-password")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(32.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .text_color(rgb(p.muted))
                                                    .hover(|s| s.bg(rgb(p.raised)).text_color(rgb(p.text)))
                                                    .child(Icon::new(IconName::Copy).size(px(17.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                                            this.local_password.clone(),
                                                        ));
                                                        this.connect_status = "Contraseña copiada".into();
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(self.local_access_status.clone()),
            )
    }

    fn render_connect_card(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_base().font_semibold().child("Dispositivo de control remoto"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("toggle-partner-ctrl")
                            .cursor_pointer()
                            .text_xs()
                            .font_semibold()
                            .text_color(rgb(p.accent))
                            .child(if self.partner_allow_control {
                                "Control remoto ▾"
                            } else {
                                "Solo ver ▾"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.partner_allow_control = !this.partner_allow_control;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child("Contraseña aleatoria o fija"),
                    ),
            )
            .child(Input::new(&self.quick_id))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt_1()
                    .child(
                        action_button("Conectar", "connect-detached", p)
                            .id("connect-detached")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.initiate_connect_from_home(Some(window), cx)
                            })),
                    ),
            )
            .when(!self.connect_status.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.accent))
                        .child(self.connect_status.clone()),
                )
            })
    }

    pub fn render_devices(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let selected_dev = self
            .selected_device_id
            .and_then(|sid| self.devices.iter().find(|d| d.id == sid).cloned());

        let mut root = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when_some(selected_dev.clone(), |this, sel| {
                                let did = sel.id;
                                let h1 = sel.hostname.clone();
                                let h2 = sel.hostname.clone();
                                this.child(
                                    action_button("⇄ Conectar", "sel-dev-conn", p)
                                        .id("sel-dev-conn")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.start_managed_session(did, None, true, h1.clone(), cx);
                                        })),
                                )
                                .child(
                                    subtle_button("Solo ver", "sel-dev-view", p)
                                        .id("sel-dev-view")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.start_managed_session(did, None, false, h2.clone(), cx);
                                        })),
                                )
                                .child(
                                    subtle_button("Recursos / RMM", "sel-dev-rmm", p)
                                        .id("sel-dev-rmm")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(idx) = this.devices.iter().position(|d| d.id == did) {
                                                this.rmm_device_idx = idx;
                                            }
                                            this.page = crate::Page::Monitoring;
                                            this.run_rmm_diagnostic(did, "resources", cx);
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(selected_dev.is_none(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child("Seleccioná un equipo para ver sus detalles o conectar."),
                                )
                            }),
                    )
                    .when(self.auth.is_some(), |this| {
                        this.child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    subtle_button("Actualizar", "dev-refresh", p)
                                        .id("dev-refresh")
                                        .on_click(cx.listener(|this, _, _, cx| this.load_devices(cx))),
                                )
                                .child(
                                    subtle_button("+ Token", "dev-enroll", p)
                                        .id("dev-enroll")
                                        .on_click(cx.listener(|this, _, _, cx| this.create_enrollment(cx))),
                                )
                                .child(
                                    action_button(
                                        if self.show_add_device_form { "Cerrar" } else { "+ Añadir" },
                                        "dev-toggle-add",
                                        p,
                                    )
                                    .id("dev-toggle-add")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_add_device_form = !this.show_add_device_form;
                                        cx.notify();
                                    })),
                                ),
                        )
                    }),
            );

        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Dispositivos", p, cx));
        }

        if !self.enroll_cmd_out.is_empty() {
            root = root.child(
                workspace_panel("Comando de enrolamiento (Copiar)", p)
                    .child(div().text_xs().text_color(rgb(p.accent)).child(self.enroll_cmd_out.clone()))
                    .child(
                        subtle_button("Copiar comando", "copy-enroll-cmd", p)
                            .id("copy-enroll-cmd")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.enroll_cmd_out.clone(),
                                ));
                                this.server_status = "Comando de enrolamiento copiado".into();
                                cx.notify();
                            })),
                    ),
            );
        }

        if self.show_add_device_form {
            let group_idx = self.assign_group_idx;
            let groups = self.groups.clone();
            root = root.child(
                workspace_panel("Registrar / Asignar equipo por ID de conexión o Email", p)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.assign_conn_id).flex_1())
                            .child(Input::new(&self.assign_email).flex_1())
                            .child(Input::new(&self.assign_alias).flex_1()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                subtle_button(
                                    if group_idx == 0 {
                                        "Grupo: Sin grupo".to_string()
                                    } else {
                                        format!(
                                            "Grupo: {}",
                                            groups
                                                .get(group_idx - 1)
                                                .map(|g| g.name.as_str())
                                                .unwrap_or("?")
                                        )
                                    },
                                    "cycle-assign-grp",
                                    p,
                                )
                                .id("cycle-assign-grp")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let total = this.groups.len() + 1;
                                    this.assign_group_idx = (this.assign_group_idx + 1) % total;
                                    cx.notify();
                                })),
                            )
                            .child(
                                action_button("Registrar / Asignar", "assign-dev-go", p)
                                    .id("assign-dev-go")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.assign_device(window, cx)
                                    })),
                            ),
                    ),
            );
        }

        // Tabla izquierda estilo TeamViewer Remote + Drawer derecho deslizable de detalles
        let now = now_unix();
        let history = self.cfg.connection_history.clone();
        let devs = self.devices.clone();
        let groups = self.groups.clone();

        let mut table = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.canvas))
            .child(
                // Header columnas
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .text_xs()
                    .font_semibold()
                    .text_color(rgb(p.muted))
                    .child(div().flex_1().child("NOMBRE"))
                    .child(div().w(px(140.)).child("ID DE AGENTDESK"))
                    .child(div().w(px(110.)).child("ESTADO"))
                    .child(div().w(px(90.)).child("")),
            );

        // Sección 1: Últimas conexiones (con regla de 1 hora)
        table = table.child(
            div()
                .id("sec-recent-conns")
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .cursor_pointer()
                .text_xs()
                .font_semibold()
                .child(format!(
                    "{} Últimas conexiones ({})",
                    if self.devices_show_recent { "▾" } else { "▸" },
                    history.len()
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.devices_show_recent = !this.devices_show_recent;
                    cx.notify();
                })),
        );
        if self.devices_show_recent {
            for (idx, h) in history.into_iter().take(8).enumerate() {
                let trust = h.is_trust_valid(now);
                let elapsed = fmt_elapsed(now - h.last_connected_unix);
                let norm_h = crate::config::normalize_conn_code(&h.connection_id);
                let matched_dev = devs
                    .iter()
                    .find(|d| crate::config::normalize_conn_code(&d.public_code) == norm_h);
                let did_m = matched_dev.map(|d| d.id);
                let aid_m = did_m.and_then(|did| {
                    self.agenda
                        .iter()
                        .find(|a| a.device_id == Some(did))
                        .map(|a| a.id)
                });
                let host_ctx = h.hostname.clone();
                let code_ctx = h.connection_id.clone();
                let host_ctx2 = host_ctx.clone();
                let code_ctx2 = code_ctx.clone();
                table = table.child(
                    div()
                        .id(SharedString::from(format!("rc-row-{idx}")))
                        .flex()
                        .items_center()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .hover(|s| s.bg(rgb(p.raised)))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                this.grp_ctx_menu = None;
                                this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                    pos: ev.position,
                                    device_id: did_m,
                                    agenda_id: aid_m,
                                    history_idx: Some(idx),
                                    hostname: host_ctx.clone(),
                                    connection_id: code_ctx.clone(),
                                    submenu: None,
                                });
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .child(format!("{} · {}", h.hostname, elapsed)),
                        )
                        .child(div().w(px(140.)).text_xs().text_color(rgb(p.muted)).child(h.connection_id.clone()))
                        .child(
                            div().w(px(110.)).child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded_md()
                                    .bg(rgb(if trust { p.badge_ok_bg } else { p.badge_off_bg }))
                                    .text_color(rgb(if trust { p.badge_ok_fg } else { p.badge_off_fg }))
                                    .text_xs()
                                    .child(if trust { "<1h rápida" } else { "Pedirá pass" }),
                            ),
                        )
                        .child(
                            div()
                                .w(px(115.))
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .child(
                                    subtle_button("Conectar", format!("rc-tbl-{idx}"), p)
                                        .id(SharedString::from(format!("rc-tbl-{idx}")))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.reconnect_from_history(idx, Some(window), cx);
                                        })),
                                )
                                .child(
                                    subtle_button("⋮", format!("rc-menu-{idx}"), p)
                                        .id(SharedString::from(format!("rc-menu-{idx}")))
                                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                            this.grp_ctx_menu = None;
                                            this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                pos: ev.position(),
                                                device_id: did_m,
                                                agenda_id: aid_m,
                                                history_idx: Some(idx),
                                                hostname: host_ctx2.clone(),
                                                connection_id: code_ctx2.clone(),
                                                submenu: None,
                                            });
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
            }
        }

        // Sección 2: SOLO equipos agendados, asignados a usuario/grupo o conectados recientemente
        let managed_devs: Vec<_> = devs
            .iter()
            .filter(|d| {
                let norm_d = crate::config::normalize_conn_code(&d.public_code);
                self.agenda.iter().any(|a| a.device_id == Some(d.id))
                    || d.group_id.is_some()
                    || d.owner_email.as_deref().is_some_and(|e| !e.trim().is_empty())
                    || self
                        .cfg
                        .connection_history
                        .iter()
                        .any(|h| crate::config::normalize_conn_code(&h.connection_id) == norm_d)
            })
            .cloned()
            .collect();

        table = table.child(
            div()
                .id("sec-all-devs")
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .cursor_pointer()
                .text_xs()
                .font_semibold()
                .child(format!(
                    "{} Mis dispositivos agendados y recientes ({})",
                    if self.devices_show_all { "▾" } else { "▸" },
                    managed_devs.len()
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.devices_show_all = !this.devices_show_all;
                    cx.notify();
                })),
        );
        if self.devices_show_all {
            if managed_devs.is_empty() {
                table = table.child(
                    div()
                        .px_3()
                        .py_3()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child("Todavía no agendaste o asignaste equipos. Usá '+ Añadir' o conectate por ID."),
                );
            }
            for d in &managed_devs {
                let did = d.id;
                let online = d.status.to_lowercase().contains("online");
                let is_sel = self.selected_device_id == Some(did);
                let ag_entry = self.agenda.iter().find(|a| a.device_id == Some(did));
                let aid_opt = ag_entry.map(|a| a.id);
                let display_label = ag_entry
                    .map(|a| a.alias.clone())
                    .unwrap_or_else(|| d.hostname.clone());
                let h_btn = display_label.clone();
                let h_ctx = display_label.clone();
                let h_ctx2 = display_label.clone();
                let c_ctx = d.public_code.clone();
                let c_ctx2 = d.public_code.clone();
                table = table.child(
                    div()
                        .id(SharedString::from(format!("row-dev-{did}")))
                        .flex()
                        .items_center()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(if is_sel { p.active } else { p.canvas }))
                        .hover(|s| s.bg(rgb(p.raised)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_device_id = Some(did);
                            cx.notify();
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                this.selected_device_id = Some(did);
                                this.grp_ctx_menu = None;
                                this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                    pos: ev.position,
                                    device_id: Some(did),
                                    agenda_id: aid_opt,
                                    history_idx: None,
                                    hostname: h_ctx.clone(),
                                    connection_id: c_ctx.clone(),
                                    submenu: None,
                                });
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size_2().rounded_full().bg(rgb(if online { p.success } else { p.muted })))
                                .child(div().text_sm().font_medium().child(display_label)),
                        )
                        .child(div().w(px(140.)).text_xs().text_color(rgb(p.muted)).child(d.public_code.clone()))
                        .child(
                            div().w(px(110.)).child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded_md()
                                    .bg(rgb(if online { p.badge_ok_bg } else { p.badge_off_bg }))
                                    .text_color(rgb(if online { p.badge_ok_fg } else { p.badge_off_fg }))
                                    .text_xs()
                                    .child(if online { "En línea" } else { "Desconectado" }),
                            ),
                        )
                        .child(
                            div()
                                .w(px(115.))
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .child(
                                    action_button("Conectar", format!("dev-ctrl-{did}"), p)
                                        .id(SharedString::from(format!("dev-ctrl-{did}")))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.start_managed_session(did, None, true, h_btn.clone(), cx);
                                        })),
                                )
                                .child(
                                    subtle_button("⋮", format!("dev-menu-{did}"), p)
                                        .id(SharedString::from(format!("dev-menu-{did}")))
                                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                            this.selected_device_id = Some(did);
                                            this.grp_ctx_menu = None;
                                            this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                pos: ev.position(),
                                                device_id: Some(did),
                                                agenda_id: aid_opt,
                                                history_idx: None,
                                                hostname: h_ctx2.clone(),
                                                connection_id: c_ctx2.clone(),
                                                submenu: None,
                                            });
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
            }
        }

        // Sección 3: Grupos / carpetas desplegables
        for g in groups {
            let gid = g.id;
            let expanded = self.expanded_groups.contains(&gid);
            let group_devs: Vec<_> = devs.iter().filter(|d| d.group_id == Some(gid)).cloned().collect();
            table = table.child(
                div()
                    .id(SharedString::from(format!("grp-fold-{gid}")))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .cursor_pointer()
                    .text_xs()
                    .font_semibold()
                    .child(format!(
                        "{} 📁 {} ({})",
                        if expanded { "▾" } else { "▸" },
                        g.name,
                        group_devs.len()
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_groups.remove(&gid) {
                            this.expanded_groups.insert(gid);
                        }
                        cx.notify();
                    })),
            );
            if expanded {
                for d in group_devs {
                    let did = d.id;
                    let online = d.status.to_lowercase().contains("online");
                    let h_btn = d.hostname.clone();
                    let h_ctx = d.hostname.clone();
                    let h_ctx2 = d.hostname.clone();
                    let c_ctx = d.public_code.clone();
                    let c_ctx2 = d.public_code.clone();
                    let aid_opt = self.agenda.iter().find(|a| a.device_id == Some(did)).map(|a| a.id);
                    table = table.child(
                        div()
                            .id(SharedString::from(format!("gdev-{gid}-{did}")))
                            .flex()
                            .items_center()
                            .px_5()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .hover(|s| s.bg(rgb(p.raised)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_device_id = Some(did);
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.selected_device_id = Some(did);
                                    this.grp_ctx_menu = None;
                                    this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                        pos: ev.position,
                                        device_id: Some(did),
                                        agenda_id: aid_opt,
                                        history_idx: None,
                                        hostname: h_ctx.clone(),
                                        connection_id: c_ctx.clone(),
                                        submenu: None,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(div().flex_1().text_sm().child(d.hostname.clone()))
                            .child(div().w(px(140.)).text_xs().text_color(rgb(p.muted)).child(d.public_code.clone()))
                            .child(
                                div().w(px(110.)).child(
                                    div()
                                        .px_2()
                                        .py(px(2.))
                                        .rounded_md()
                                        .bg(rgb(if online { p.badge_ok_bg } else { p.badge_off_bg }))
                                        .text_color(rgb(if online { p.badge_ok_fg } else { p.badge_off_fg }))
                                        .text_xs()
                                        .child(if online { "En línea" } else { "Desconectado" }),
                                ),
                            )
                            .child(
                                div()
                                    .w(px(115.))
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_1()
                                    .child(
                                        subtle_button("Conectar", format!("gd-c-{did}"), p)
                                            .id(SharedString::from(format!("gd-c-{did}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_managed_session(did, None, true, h_btn.clone(), cx);
                                            })),
                                    )
                                    .child(
                                        subtle_button("⋮", format!("gd-m-{gid}-{did}"), p)
                                            .id(SharedString::from(format!("gd-m-{gid}-{did}")))
                                            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                                this.selected_device_id = Some(did);
                                                this.grp_ctx_menu = None;
                                                this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                    pos: ev.position(),
                                                    device_id: Some(did),
                                                    agenda_id: aid_opt,
                                                    history_idx: None,
                                                    hostname: h_ctx2.clone(),
                                                    connection_id: c_ctx2.clone(),
                                                    submenu: None,
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    );
                }
            }
        }

        // Split horizontal: tabla izquierda + panel derecho deslizable de detalles (Screenshot 5)
        let mut split = div().flex().gap_4().child(table);
        if let Some(d) = selected_dev {
            let did = d.id;
            let online = d.status.to_lowercase().contains("online");
            let h1 = d.hostname.clone();
            let h2 = d.hostname.clone();
            split = split.child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(330.))
                    .gap_3()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_base().font_bold().child(d.hostname.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(if online { p.badge_ok_fg } else { p.muted }))
                                            .child(if online { "● En línea" } else { "● Desconectado" }),
                                    ),
                            )
                            .child(
                                subtle_button("×", "close-dev-drawer", p)
                                    .id("close-dev-drawer")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.selected_device_id = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.canvas))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().font_semibold().child("Conectar a dispositivo"))
                                    .child(div().text_xs().text_color(rgb(p.muted)).child("Control remoto E2EE")),
                            )
                            .child(
                                action_button("Conectar", "drawer-conn", p)
                                    .id("drawer-conn")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_managed_session(did, None, true, h1.clone(), cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.canvas))
                            .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("OTROS MÉTODOS Y RMM"))
                            .child(
                                subtle_button("Solo ver pantalla", "drawer-view", p)
                                    .id("drawer-view")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_managed_session(did, None, false, h2.clone(), cx);
                                    })),
                            )
                            .child(
                                subtle_button("Ver recursos HW remotos (CPU/RAM/GPU)", "drawer-rmm", p)
                                    .id("drawer-rmm")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                                            this.rmm_device_idx = idx;
                                        }
                                        this.page = crate::Page::Monitoring;
                                        this.run_rmm_diagnostic(did, "resources", cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.canvas))
                            .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("INFORMACIÓN DEL DISPOSITIVO"))
                            .child(div().text_xs().child(format!("ID: {}", d.public_code)))
                            .child(div().text_xs().child(format!("Tipo: {} · {} ({})", d.kind, d.os_name, d.os_arch)))
                            .child(div().text_xs().child(format!("Pantalla: {}x{}", d.screen_width, d.screen_height)))
                            .child(div().text_xs().child(format!(
                                "Propietario: {}",
                                d.owner_email.as_deref().unwrap_or("Sin asignar")
                            )))
                            .child(div().text_xs().child(format!(
                                "Acceso fácil / Pass fija: {}",
                                if d.has_personal_password { "Sí" } else { "No" }
                            ))),
                    ),
            );
        }

        root.child(split)
    }

    pub fn render_support(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let mut root = div().flex().flex_col().gap_3().child(page_heading(
            "Chat y Centro de soporte",
            "Tickets de QuickSupport, asignación, resolución e historial.",
            p,
        ));
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Chat y soporte", p, cx));
        }

        if let Some((tid, code)) = self.resolving_ticket.clone() {
            let outcome = self.resolve_outcome.clone();
            root = root.child(
                workspace_panel(&format!("Resolver y cerrar {code}"), p)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .children(["resolved", "partial", "unresolved"].into_iter().map(|opt| {
                                let opt_s = opt.to_string();
                                subtle_button(opt, format!("out-{opt}"), p)
                                    .id(SharedString::from(format!("out-{opt}")))
                                    .when(outcome == opt, |b| b.bg(rgb(p.active)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.resolve_outcome = opt_s.clone();
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(Input::new(&self.resolve_summary_input))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                subtle_button("Cancelar", "cancel-res-tk", p)
                                    .id("cancel-res-tk")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resolving_ticket = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button("Confirmar cierre", "go-res-tk", p)
                                    .id("go-res-tk")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.resolve_ticket(tid, window, cx);
                                    })),
                            ),
                    ),
            );
        }

        // Vista dividida estilo Chat/Mensajes de TeamViewer (Screenshot 3)
        let tks = self.tickets.clone();
        let active_tk = self
            .selected_ticket_id
            .and_then(|sid| tks.iter().find(|t| t.id == sid).cloned())
            .or_else(|| tks.first().cloned());

        let mut left_list = div()
            .flex()
            .flex_col()
            .w(px(280.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.canvas))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().text_sm().font_semibold().child("Solicitudes y mensajes"))
                    .child(
                        subtle_button("Actualizar", "tk-ref-btn", p)
                            .id("tk-ref-btn")
                            .on_click(cx.listener(|this, _, _, cx| this.load_tickets(cx))),
                    ),
            );
        if tks.is_empty() {
            left_list = left_list.child(
                div().p_4().text_xs().text_color(rgb(p.muted)).child("Sin tickets en cola."),
            );
        } else {
            for t in tks {
                let tid = t.id;
                let is_sel = active_tk.as_ref().is_some_and(|a| a.id == tid);
                left_list = left_list.child(
                    div()
                        .id(SharedString::from(format!("tk-item-{tid}")))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(if is_sel { p.active } else { p.canvas }))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_ticket_id = Some(tid);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_xs().font_semibold().child(t.public_code))
                                .child(div().text_xs().text_color(rgb(p.muted)).child(t.status)),
                        )
                        .child(div().text_xs().child(t.requester_name)),
                );
            }
        }

        let mut right_detail = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel));
        if let Some(t) = active_tk {
            let tid = t.id;
            let code = t.public_code.clone();
            let did_opt = t.device_id;
            right_detail = right_detail
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pb_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_base().font_bold().child(format!("{} — {}", t.public_code, t.requester_name)))
                                .child(div().text_xs().text_color(rgb(p.muted)).child(format!(
                                    "Contacto: {} · Estado: {}",
                                    t.requester_contact, t.status
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .when(t.status == "queued", |this| {
                                    this.child(
                                        action_button("Reclamar", format!("claim-{tid}"), p)
                                            .id(SharedString::from(format!("claim-{tid}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.claim_ticket(tid, cx);
                                            })),
                                    )
                                })
                                .when_some(did_opt, |this, did| {
                                    let c = code.clone();
                                    this.child(
                                        action_button("Conectar", format!("tk-conn-{tid}"), p)
                                            .id(SharedString::from(format!("tk-conn-{tid}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_managed_session(did, Some(tid), true, c.clone(), cx);
                                            })),
                                    )
                                })
                                .when(t.status != "closed", |this| {
                                    let c = code.clone();
                                    this.child(
                                        subtle_button("Resolver", format!("res-{tid}"), p)
                                            .id(SharedString::from(format!("res-{tid}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.resolving_ticket = Some((tid, c.clone()));
                                                cx.notify();
                                            })),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(rgb(p.canvas))
                        .border_1()
                        .border_color(rgb(p.border))
                        .text_sm()
                        .child(t.description),
                );
        } else {
            right_detail = right_detail.child(
                div().text_sm().text_color(rgb(p.muted)).child("Seleccioná un ticket para atenderlo."),
            );
        }

        root.child(div().flex().gap_4().child(left_list).child(right_detail))
    }

    pub fn render_sessions(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let root = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(page_heading("Sesiones remotas", "Sesiones activas e históricas en el relay.", p))
                    .when(self.auth.is_some(), |this| {
                        this.child(
                            subtle_button("Actualizar", "sess-refresh", p)
                                .id("sess-refresh")
                                .on_click(cx.listener(|this, _, _, cx| this.load_sessions(cx))),
                        )
                    }),
            );
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Sesiones", p, cx));
        }

        let mut panel = workspace_panel("Listado de sesiones", p);
        if self.sessions.is_empty() {
            panel = panel.child(div().text_sm().text_color(rgb(p.muted)).child("Sin sesiones."));
        } else {
            let list = self.sessions.clone();
            for s in list {
                let sid = s.id;
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().font_semibold().child(format!(
                                    "Sesión {} [{}]",
                                    &sid.to_string()[..8],
                                    s.status
                                )))
                                .child(div().text_xs().text_color(rgb(p.muted)).child(format!(
                                    "Device {} · Control: {} · Creada {}",
                                    &s.device_id.to_string()[..8],
                                    s.allow_control,
                                    s.created_at.format("%Y-%m-%d %H:%M")
                                ))),
                        )
                        .when(s.status != "ended", |this| {
                            this.child(
                                subtle_button("Terminar", format!("term-{sid}"), p)
                                    .id(SharedString::from(format!("term-{sid}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.terminate_server_session(sid, cx);
                                    })),
                            )
                        }),
                );
            }
        }
        root.child(panel)
    }

    pub fn render_agenda(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let selected_gid = match self.oc_selection {
            Some(crate::OcSelection::Group(gid)) => Some(gid),
            Some(crate::OcSelection::Device(did)) => self
                .devices
                .iter()
                .find(|d| d.id == did)
                .and_then(|d| d.group_id),
            Some(crate::OcSelection::Agenda(aid)) => self
                .agenda
                .iter()
                .find(|a| a.id == aid)
                .and_then(|a| a.group_id),
            _ => None,
        };

        let mut root = div().flex().flex_col().gap_4().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(page_heading(
                    "Ordenadores y contactos",
                    "Conexiones recientes, grupos de equipos, contactos y compartición por permisos.",
                    p,
                ))
                .when(self.auth.is_some(), |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                action_button("+ Nuevo grupo", "oc-new-grp", p)
                                    .id("oc-new-grp")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_create_group_modal(window, cx);
                                    })),
                            )
                            .child(
                                subtle_button("+ Agregar ordenador", "oc-add-comp", p)
                                    .id("oc-add-comp")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_add_computer_modal(selected_gid, window, cx);
                                    })),
                            )
                            .child(
                                subtle_button("+ Añadir contacto", "oc-add-ct", p)
                                    .id("oc-add-ct")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_add_contact_modal(selected_gid, window, cx);
                                    })),
                            )
                            .child(
                                subtle_button(
                                    if self.only_favorites {
                                        "Favoritos"
                                    } else {
                                        "Todos"
                                    },
                                    "toggle-fav-filter",
                                    p,
                                )
                                .id("toggle-fav-filter")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.only_favorites = !this.only_favorites;
                                    cx.notify();
                                })),
                            ),
                    )
                }),
        );

        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Ordenadores y contactos", p, cx));
        }

        if !self.group_action_msg.is_empty() {
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .text_xs()
                    .child(self.group_action_msg.clone())
                    .child(
                        subtle_button("×", "oc-msg-close", p)
                            .id("oc-msg-close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.group_action_msg.clear();
                                cx.notify();
                            })),
                    ),
            );
        }

        // Toolbar de búsqueda tipo TeamViewer
        root = root.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .rounded_lg()
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .child(Icon::new(IconName::Search).size(px(15.)).text_color(rgb(p.muted)))
                .child(Input::new(&self.oc_filter_in).flex_1()),
        );

        let q = self.oc_filter_in.read(cx).value().trim().to_ascii_lowercase();
        let now = now_unix();
        let devs = self.devices.clone();
        let agenda = self.agenda.clone();
        let groups = self.groups.clone();
        let history = self.cfg.connection_history.clone();
        let only_fav = self.only_favorites;
        let current_sel = self.oc_selection.clone();

        let alias_for_dev = |did: uuid::Uuid| -> Option<&agentdesk_protocol::AddressBookEntryDto> {
            agenda.iter().find(|a| a.device_id == Some(did))
        };

        let mut tree = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .overflow_hidden();

        // 1. Conexiones recientes
        let recent_rows: Vec<(usize, crate::config::ConnectionHistoryEntry)> = history
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, h)| {
                q.is_empty()
                    || h.hostname.to_ascii_lowercase().contains(&q)
                    || h.connection_id.to_ascii_lowercase().contains(&q)
            })
            .take(8)
            .collect();

        tree = tree.child(
            div()
                .id("oc-sec-recent")
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.raised))
                .cursor_pointer()
                .text_xs()
                .font_semibold()
                .child(format!(
                    "{} Conexiones recientes ({})",
                    if self.oc_show_recent { "▾" } else { "▸" },
                    recent_rows.len()
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.oc_show_recent = !this.oc_show_recent;
                    cx.notify();
                })),
        );

        if self.oc_show_recent {
            if recent_rows.is_empty() {
                tree = tree.child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child("Sin conexiones recientes."),
                );
            } else {
                for (idx, h) in recent_rows {
                    let norm_h = crate::config::normalize_conn_code(&h.connection_id);
                    let matched_dev = devs
                        .iter()
                        .find(|d| crate::config::normalize_conn_code(&d.public_code) == norm_h);
                    let did_m = matched_dev.map(|d| d.id);
                    let aid_m = did_m.and_then(|did| agenda.iter().find(|a| a.device_id == Some(did)).map(|a| a.id));
                    let online = matched_dev
                        .map(|d| d.status.to_ascii_lowercase().contains("online"))
                        .unwrap_or(false);
                    let icon_kind = matched_dev
                        .map(|d| d.icon_kind.as_str())
                        .unwrap_or("desktop");
                    let accent_color = matched_dev
                        .map(|d| d.accent_color.as_str())
                        .unwrap_or("");
                    let is_sel = current_sel == Some(crate::OcSelection::Recent(idx));
                    let elapsed = fmt_elapsed(now - h.last_connected_unix);
                    let host_ctx = h.hostname.clone();
                    let code_ctx = h.connection_id.clone();
                    let host_ctx2 = host_ctx.clone();
                    let code_ctx2 = code_ctx.clone();

                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("oc-rec-{idx}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py(px(7.))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .when(is_sel, |d| {
                                d.bg(rgb(p.active))
                                    .border_l_2()
                                    .border_color(rgb(p.accent))
                            })
                            .hover(|s| s.bg(rgb(p.raised)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.oc_selection = Some(crate::OcSelection::Recent(idx));
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.oc_selection = Some(crate::OcSelection::Recent(idx));
                                    this.grp_ctx_menu = None;
                                    this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                        pos: ev.position,
                                        device_id: did_m,
                                        agenda_id: aid_m,
                                        history_idx: Some(idx),
                                        hostname: host_ctx.clone(),
                                        connection_id: code_ctx.clone(),
                                        submenu: None,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(render_device_badge(icon_kind, accent_color, online, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(rgb(if online || matched_dev.is_none() { p.text } else { p.muted }))
                                    .child(format!("{} · {}", h.hostname, elapsed)),
                            )
                            .child(div().text_xs().text_color(rgb(p.muted)).child(h.connection_id.clone()))
                            .child(
                                subtle_button("Conectar", SharedString::from(format!("oc-rc-btn-{idx}")), p)
                                    .id(SharedString::from(format!("oc-rc-btn-{idx}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.reconnect_from_history(idx, Some(window), cx);
                                    })),
                            )
                            .child(
                                subtle_button("⋮", SharedString::from(format!("oc-rc-m-{idx}")), p)
                                    .id(SharedString::from(format!("oc-rc-m-{idx}")))
                                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                        this.oc_selection = Some(crate::OcSelection::Recent(idx));
                                        this.grp_ctx_menu = None;
                                        this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                            pos: ev.position(),
                                            device_id: did_m,
                                            agenda_id: aid_m,
                                            history_idx: Some(idx),
                                            hostname: host_ctx2.clone(),
                                            connection_id: code_ctx2.clone(),
                                            submenu: None,
                                        });
                                        cx.notify();
                                    })),
                            ),
                    );
                }
            }
        }

        // 2. Mis dispositivos gestionados (sin grupo) + contactos sin grupo
        let unassigned_devs: Vec<_> = devs
            .iter()
            .filter(|d| d.group_id.is_none())
            .filter(|d| {
                let norm_d = crate::config::normalize_conn_code(&d.public_code);
                let ag_opt = alias_for_dev(d.id);
                let is_fav_or_ok = !only_fav || ag_opt.is_some_and(|a| a.is_favorite);
                let tracked = ag_opt.is_some()
                    || d.owner_email.as_deref().is_some_and(|e| !e.trim().is_empty())
                    || history
                        .iter()
                        .any(|h| crate::config::normalize_conn_code(&h.connection_id) == norm_d);
                let matches_q = q.is_empty()
                    || d.hostname.to_ascii_lowercase().contains(&q)
                    || d.public_code.to_ascii_lowercase().contains(&q)
                    || ag_opt.is_some_and(|a| {
                        a.alias.to_ascii_lowercase().contains(&q)
                            || a.client_company.to_ascii_lowercase().contains(&q)
                    });
                tracked && is_fav_or_ok && matches_q
            })
            .cloned()
            .collect();

        let unassigned_contacts: Vec<_> = agenda
            .iter()
            .filter(|a| a.group_id.is_none())
            .filter(|a| {
                a.device_id
                    .is_none_or(|did| !unassigned_devs.iter().any(|d| d.id == did))
            })
            .filter(|a| !only_fav || a.is_favorite)
            .filter(|a| {
                q.is_empty()
                    || a.alias.to_ascii_lowercase().contains(&q)
                    || a.client_company.to_ascii_lowercase().contains(&q)
                    || a.contact_name.to_ascii_lowercase().contains(&q)
            })
            .cloned()
            .collect();

        tree = tree.child(
            div()
                .id("oc-sec-managed")
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.raised))
                .cursor_pointer()
                .text_xs()
                .font_semibold()
                .child(format!(
                    "{} Mis dispositivos gestionados ({})",
                    if self.oc_show_managed { "▾" } else { "▸" },
                    unassigned_devs.len() + unassigned_contacts.len()
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.oc_show_managed = !this.oc_show_managed;
                    cx.notify();
                })),
        );

        if self.oc_show_managed {
            if unassigned_devs.is_empty() && unassigned_contacts.is_empty() {
                tree = tree.child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child("Sin dispositivos gestionados fuera de grupos."),
                );
            } else {
                for d in &unassigned_devs {
                    let did = d.id;
                    let ag_opt = alias_for_dev(did);
                    let aid_opt = ag_opt.map(|a| a.id);
                    let display_name = ag_opt
                        .map(|a| a.alias.clone())
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| d.hostname.clone());
                    let online = d.status.to_ascii_lowercase().contains("online");
                    let is_sel = current_sel == Some(crate::OcSelection::Device(did));
                    let h_btn = display_name.clone();
                    let h_ctx = display_name.clone();
                    let h_ctx2 = display_name.clone();
                    let c_ctx = d.public_code.clone();
                    let c_ctx2 = d.public_code.clone();

                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("oc-mdev-{did}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py(px(7.))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .when(is_sel, |el| {
                                el.bg(rgb(p.active))
                                    .border_l_2()
                                    .border_color(rgb(p.accent))
                            })
                            .hover(|s| s.bg(rgb(p.raised)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.oc_selection = Some(crate::OcSelection::Device(did));
                                this.selected_device_id = Some(did);
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.oc_selection = Some(crate::OcSelection::Device(did));
                                    this.selected_device_id = Some(did);
                                    this.grp_ctx_menu = None;
                                    this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                        pos: ev.position,
                                        device_id: Some(did),
                                        agenda_id: aid_opt,
                                        history_idx: None,
                                        hostname: h_ctx.clone(),
                                        connection_id: c_ctx.clone(),
                                        submenu: None,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(render_device_badge(&d.icon_kind, &d.accent_color, online, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(rgb(if online {
                                        parse_hex_color(&d.accent_color).unwrap_or(p.text)
                                    } else {
                                        p.muted
                                    }))
                                    .child(display_name),
                            )
                            .child(div().text_xs().text_color(rgb(p.muted)).child(d.public_code.clone()))
                            .child(
                                subtle_button("Conectar", SharedString::from(format!("oc-md-c-{did}")), p)
                                    .id(SharedString::from(format!("oc-md-c-{did}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_managed_session(did, None, true, h_btn.clone(), cx);
                                    })),
                            )
                            .child(
                                subtle_button("⋮", SharedString::from(format!("oc-md-m-{did}")), p)
                                    .id(SharedString::from(format!("oc-md-m-{did}")))
                                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                        this.oc_selection = Some(crate::OcSelection::Device(did));
                                        this.selected_device_id = Some(did);
                                        this.grp_ctx_menu = None;
                                        this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                            pos: ev.position(),
                                            device_id: Some(did),
                                            agenda_id: aid_opt,
                                            history_idx: None,
                                            hostname: h_ctx2.clone(),
                                            connection_id: c_ctx2.clone(),
                                            submenu: None,
                                        });
                                        cx.notify();
                                    })),
                            ),
                    );
                }

                for a in unassigned_contacts {
                    let aid = a.id;
                    let did_opt = a.device_id;
                    let is_sel = current_sel == Some(crate::OcSelection::Agenda(aid));
                    let alias = a.alias.clone();
                    let alias2 = alias.clone();
                    let code = a.device_public_code.clone().unwrap_or_default();
                    let code2 = code.clone();

                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("oc-mct-{aid}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py(px(7.))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .when(is_sel, |el| {
                                el.bg(rgb(p.active))
                                    .border_l_2()
                                    .border_color(rgb(p.accent))
                            })
                            .hover(|s| s.bg(rgb(p.raised)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                    this.grp_ctx_menu = None;
                                    this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                        pos: ev.position,
                                        device_id: did_opt,
                                        agenda_id: Some(aid),
                                        history_idx: None,
                                        hostname: alias.clone(),
                                        connection_id: code.clone(),
                                        submenu: None,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(Icon::new(IconName::UserRound).size(px(15.)).text_color(rgb(p.accent)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .child(format!("{}{}", if a.is_favorite { "★ " } else { "" }, a.alias)),
                            )
                            .child(div().text_xs().text_color(rgb(p.muted)).child(a.contact_name.clone()))
                            .child(
                                subtle_button("⋮", SharedString::from(format!("oc-mc-m-{aid}")), p)
                                    .id(SharedString::from(format!("oc-mc-m-{aid}")))
                                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                        this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                        this.grp_ctx_menu = None;
                                        this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                            pos: ev.position(),
                                            device_id: did_opt,
                                            agenda_id: Some(aid),
                                            history_idx: None,
                                            hostname: alias2.clone(),
                                            connection_id: code2.clone(),
                                            submenu: None,
                                        });
                                        cx.notify();
                                    })),
                            ),
                    );
                }
            }
        }

        // 3. Grupos de dispositivos (árbol desplegable estilo TeamViewer)
        tree = tree.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.raised))
                .text_xs()
                .font_semibold()
                .child(format!("▾ Grupos de dispositivos ({})", groups.len())),
        );

        if groups.is_empty() {
            tree = tree.child(
                div()
                    .px_4()
                    .py_3()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child("Aún no tenés grupos. Creá uno con '+ Nuevo grupo' para organizar y compartir equipos."),
            );
        } else {
            for g in &groups {
                let gid = g.id;
                let gname = g.name.clone();
                let gperm = g.my_permission.clone();
                let g_devs: Vec<_> = devs
                    .iter()
                    .filter(|d| d.group_id == Some(gid))
                    .filter(|d| {
                        let ag_opt = alias_for_dev(d.id);
                        (!only_fav || ag_opt.is_some_and(|a| a.is_favorite))
                            && (q.is_empty()
                                || g.name.to_ascii_lowercase().contains(&q)
                                || d.hostname.to_ascii_lowercase().contains(&q)
                                || d.public_code.to_ascii_lowercase().contains(&q)
                                || ag_opt.is_some_and(|a| a.alias.to_ascii_lowercase().contains(&q)))
                    })
                    .cloned()
                    .collect();

                let g_contacts: Vec<_> = agenda
                    .iter()
                    .filter(|a| a.group_id == Some(gid))
                    .filter(|a| {
                        a.device_id
                            .is_none_or(|did| !g_devs.iter().any(|d| d.id == did))
                    })
                    .filter(|a| !only_fav || a.is_favorite)
                    .filter(|a| {
                        q.is_empty()
                            || g.name.to_ascii_lowercase().contains(&q)
                            || a.alias.to_ascii_lowercase().contains(&q)
                            || a.contact_name.to_ascii_lowercase().contains(&q)
                    })
                    .cloned()
                    .collect();

                let grp_matches_q = q.is_empty()
                    || g.name.to_ascii_lowercase().contains(&q)
                    || !g_devs.is_empty()
                    || !g_contacts.is_empty();
                if !grp_matches_q {
                    continue;
                }

                let expanded = self.expanded_groups.contains(&gid) || !q.is_empty();
                let is_sel = current_sel == Some(crate::OcSelection::Group(gid));
                let online_count = g_devs
                    .iter()
                    .filter(|d| d.status.to_ascii_lowercase().contains("online"))
                    .count();
                let total_count = g_devs.len() + g_contacts.len();
                let shares_count = g.shares.len();
                let gname_ctx = gname.clone();
                let gperm_ctx = gperm.clone();
                let gname_ctx2 = gname.clone();
                let gperm_ctx2 = gperm.clone();

                tree = tree.child(
                    div()
                        .id(SharedString::from(format!("oc-grp-{gid}")))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .py(px(8.))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .when(is_sel, |el| {
                            el.bg(rgb(p.active))
                                .border_l_2()
                                .border_color(rgb(p.accent))
                        })
                        .hover(|s| s.bg(rgb(p.raised)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.oc_selection = Some(crate::OcSelection::Group(gid));
                            if !this.expanded_groups.remove(&gid) {
                                this.expanded_groups.insert(gid);
                            }
                            cx.notify();
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                this.oc_selection = Some(crate::OcSelection::Group(gid));
                                this.dev_ctx_menu = None;
                                this.grp_ctx_menu = Some(crate::GroupContextMenu {
                                    pos: ev.position,
                                    group_id: gid,
                                    group_name: gname_ctx.clone(),
                                    my_permission: gperm_ctx.clone(),
                                });
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(div().text_xs().text_color(rgb(p.muted)).child(if expanded { "▾" } else { "▸" }))
                                .child(div().text_sm().font_semibold().child(g.name.clone()))
                                .when(shares_count > 0, |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .px_2()
                                            .py(px(1.))
                                            .rounded_md()
                                            .bg(rgb(p.raised))
                                            .text_xs()
                                            .text_color(rgb(p.accent))
                                            .child(Icon::new(IconName::Globe).size(px(12.)))
                                            .child(format!("{shares_count}")),
                                    )
                                })
                                .when(g.my_permission != "full_control", |el| {
                                    el.child(
                                        div()
                                            .px_2()
                                            .py(px(1.))
                                            .rounded_md()
                                            .bg(rgb(p.badge_off_bg))
                                            .text_color(rgb(p.badge_off_fg))
                                            .text_xs()
                                            .child(if g.my_permission == "read_write" {
                                                "Lectura y escritura"
                                            } else {
                                                "Solo lectura"
                                            }),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child(format!("{online_count}/{total_count}")),
                                )
                                .child(
                                    subtle_button("⋮", SharedString::from(format!("oc-gm-{gid}")), p)
                                        .id(SharedString::from(format!("oc-gm-{gid}")))
                                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                            this.oc_selection = Some(crate::OcSelection::Group(gid));
                                            this.dev_ctx_menu = None;
                                            this.grp_ctx_menu = Some(crate::GroupContextMenu {
                                                pos: ev.position(),
                                                group_id: gid,
                                                group_name: gname_ctx2.clone(),
                                                my_permission: gperm_ctx2.clone(),
                                            });
                                            cx.notify();
                                        })),
                                ),
                        ),
                );

                if expanded {
                    if g_devs.is_empty() && g_contacts.is_empty() {
                        tree = tree.child(
                            div()
                                .pl_8()
                                .pr_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(p.border))
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child("Sin equipos en este grupo — clic derecho → Agregar ordenador"),
                        );
                    } else {
                        let total_children = g_devs.len() + g_contacts.len();
                        let mut child_idx = 0usize;
                        for d in g_devs {
                            let did = d.id;
                            let is_last = child_idx + 1 == total_children;
                            child_idx += 1;
                            let ag_opt = alias_for_dev(did);
                            let aid_opt = ag_opt.map(|a| a.id);
                            let display_name = ag_opt
                                .map(|a| a.alias.clone())
                                .filter(|s| !s.trim().is_empty())
                                .unwrap_or_else(|| d.hostname.clone());
                            let online = d.status.to_ascii_lowercase().contains("online");
                            let is_dsel = current_sel == Some(crate::OcSelection::Device(did));
                            let custom_rgb = parse_hex_color(&d.accent_color);
                            let h_btn = display_name.clone();
                            let h_ctx = display_name.clone();
                            let h_ctx2 = display_name.clone();
                            let c_ctx = d.public_code.clone();
                            let c_ctx2 = d.public_code.clone();

                            tree = tree.child(
                                div()
                                    .id(SharedString::from(format!("oc-gd-{gid}-{did}")))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_4()
                                    .pr_3()
                                    .py(px(2.))
                                    .border_b_1()
                                    .border_color(rgb(p.border))
                                    .when(is_dsel, |el| {
                                        el.bg(rgb(p.active))
                                            .border_l_2()
                                            .border_color(rgb(custom_rgb.unwrap_or(p.accent)))
                                    })
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.oc_selection = Some(crate::OcSelection::Device(did));
                                        this.selected_device_id = Some(did);
                                        cx.notify();
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            this.oc_selection = Some(crate::OcSelection::Device(did));
                                            this.selected_device_id = Some(did);
                                            this.grp_ctx_menu = None;
                                            this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                pos: ev.position,
                                                device_id: Some(did),
                                                agenda_id: aid_opt,
                                                history_idx: None,
                                                hostname: h_ctx.clone(),
                                                connection_id: c_ctx.clone(),
                                                submenu: None,
                                            });
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_tree_branch(is_last, p))
                                    .child(render_device_badge(&d.icon_kind, &d.accent_color, online, p))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .font_medium()
                                            .text_color(rgb(if online {
                                                custom_rgb.unwrap_or(p.text)
                                            } else {
                                                p.muted
                                            }))
                                            .child(display_name),
                                    )
                                    .child(div().text_xs().text_color(rgb(p.muted)).child(d.public_code.clone()))
                                    .child(
                                        subtle_button("Conectar", SharedString::from(format!("oc-gdc-{did}")), p)
                                            .id(SharedString::from(format!("oc-gdc-{did}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_managed_session(did, None, true, h_btn.clone(), cx);
                                            })),
                                    )
                                    .child(
                                        subtle_button("⋮", SharedString::from(format!("oc-gdm-{did}")), p)
                                            .id(SharedString::from(format!("oc-gdm-{did}")))
                                            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                                this.oc_selection = Some(crate::OcSelection::Device(did));
                                                this.selected_device_id = Some(did);
                                                this.grp_ctx_menu = None;
                                                this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                    pos: ev.position(),
                                                    device_id: Some(did),
                                                    agenda_id: aid_opt,
                                                    history_idx: None,
                                                    hostname: h_ctx2.clone(),
                                                    connection_id: c_ctx2.clone(),
                                                    submenu: None,
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            );
                        }

                        for a in g_contacts {
                            let aid = a.id;
                            let is_last = child_idx + 1 == total_children;
                            child_idx += 1;
                            let did_opt = a.device_id;
                            let is_asel = current_sel == Some(crate::OcSelection::Agenda(aid));
                            let alias = a.alias.clone();
                            let alias2 = alias.clone();
                            let code = a.device_public_code.clone().unwrap_or_default();
                            let code2 = code.clone();

                            tree = tree.child(
                                div()
                                    .id(SharedString::from(format!("oc-gct-{gid}-{aid}")))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_4()
                                    .pr_3()
                                    .py(px(2.))
                                    .border_b_1()
                                    .border_color(rgb(p.border))
                                    .when(is_asel, |el| {
                                        el.bg(rgb(p.active))
                                            .border_l_2()
                                            .border_color(rgb(p.accent))
                                    })
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                        cx.notify();
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                            this.grp_ctx_menu = None;
                                            this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                pos: ev.position,
                                                device_id: did_opt,
                                                agenda_id: Some(aid),
                                                history_idx: None,
                                                hostname: alias.clone(),
                                                connection_id: code.clone(),
                                                submenu: None,
                                            });
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_tree_branch(is_last, p))
                                    .child(
                                        div()
                                            .w(px(28.))
                                            .h(px(22.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(parse_hex_color(&a.accent_color).unwrap_or(p.border)))
                                            .bg(rgb(p.raised))
                                            .child(
                                                Icon::new(IconName::UserRound)
                                                    .size(px(13.))
                                                    .text_color(rgb(parse_hex_color(&a.accent_color).unwrap_or(p.accent))),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .child(format!("{}{}", if a.is_favorite { "★ " } else { "" }, a.alias)),
                                    )
                                    .child(div().text_xs().text_color(rgb(p.muted)).child(a.contact_name.clone()))
                                    .child(
                                        subtle_button("⋮", SharedString::from(format!("oc-gcm-{aid}")), p)
                                            .id(SharedString::from(format!("oc-gcm-{aid}")))
                                            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                                this.oc_selection = Some(crate::OcSelection::Agenda(aid));
                                                this.grp_ctx_menu = None;
                                                this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                                    pos: ev.position(),
                                                    device_id: did_opt,
                                                    agenda_id: Some(aid),
                                                    history_idx: None,
                                                    hostname: alias2.clone(),
                                                    connection_id: code2.clone(),
                                                    submenu: None,
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            );
                        }
                    }
                }
            }
        }

        // Panel derecho estilo TeamViewer (acciones rápidas + compartición)
        let mut inspector = div()
            .w(px(280.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel));

        match &current_sel {
            Some(crate::OcSelection::Group(gid)) => {
                let gid = *gid;
                if let Some(g) = groups.iter().find(|x| x.id == gid).cloned() {
                    let can_write = g.my_permission != "read_only";
                    let is_owner = g.my_permission == "full_control";
                    let perm_label = match g.my_permission.as_str() {
                        "full_control" => "Propietario (Control total)",
                        "read_write" => "Compartido (Lectura y escritura)",
                        _ => "Compartido (Solo lectura)",
                    };
                    inspector = inspector
                        .child(div().text_base().font_semibold().child(g.name.clone()))
                        .when(!g.description.trim().is_empty(), |el| {
                            el.child(div().text_xs().text_color(rgb(p.muted)).child(g.description.clone()))
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(format!("Propietario: {} · {} equipos", g.owner_email, g.device_count)),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(3.))
                                .rounded_md()
                                .bg(rgb(p.raised))
                                .text_xs()
                                .text_color(rgb(p.accent))
                                .child(perm_label),
                        )
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .when(can_write, |el| {
                            el.child(
                                subtle_button("Agregar ordenador", SharedString::from(format!("ins-ac-{gid}")), p)
                                    .id(SharedString::from(format!("ins-ac-{gid}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_add_computer_modal(Some(gid), window, cx);
                                    })),
                            )
                            .child(
                                subtle_button("Añadir contacto", SharedString::from(format!("ins-act-{gid}")), p)
                                    .id(SharedString::from(format!("ins-act-{gid}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_add_contact_modal(Some(gid), window, cx);
                                    })),
                            )
                            .child(
                                subtle_button("Renombrar / Propiedades", SharedString::from(format!("ins-ren-{gid}")), p)
                                    .id(SharedString::from(format!("ins-ren-{gid}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_edit_group_modal(gid, window, cx);
                                    })),
                            )
                        })
                        .when(is_owner, |el| {
                            el.child(
                                action_button("Compartir grupo", SharedString::from(format!("ins-sh-{gid}")), p)
                                    .id(SharedString::from(format!("ins-sh-{gid}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_share_group_modal(gid, window, cx);
                                    })),
                            )
                            .child(
                                subtle_button("Eliminar grupo", SharedString::from(format!("ins-del-{gid}")), p)
                                    .id(SharedString::from(format!("ins-del-{gid}")))
                                    .text_color(rgb(p.danger))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_group(gid, cx);
                                    })),
                            )
                        })
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("COMPARTIDO CON"));

                    if g.shares.is_empty() {
                        inspector = inspector.child(
                            div().text_xs().text_color(rgb(p.muted)).child("Solo visible para vos / administradores."),
                        );
                    } else {
                        for sh in g.shares {
                            let uid = sh.user_id;
                            let sp_txt = match sh.permission.as_str() {
                                "read_only" => "Solo lectura",
                                "read_write" => "Lectura/Escritura",
                                _ => "Control total",
                            };
                            inspector = inspector.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_1()
                                    .px_2()
                                    .py(px(4.))
                                    .rounded_md()
                                    .bg(rgb(p.raised))
                                    .text_xs()
                                    .child(format!("{} · {}", sh.user_email, sp_txt))
                                    .when(is_owner, |el| {
                                        el.child(
                                            subtle_button("×", SharedString::from(format!("ins-unsh-{gid}-{uid}")), p)
                                                .id(SharedString::from(format!("ins-unsh-{gid}-{uid}")))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.unshare_group(gid, uid, cx);
                                                })),
                                        )
                                    }),
                            );
                        }
                    }
                } else {
                    inspector = inspector.child(div().text_xs().text_color(rgb(p.muted)).child("Grupo no disponible."));
                }
            }
            Some(crate::OcSelection::Device(did)) => {
                let did = *did;
                if let Some(d) = devs.iter().find(|x| x.id == did).cloned() {
                    let ag_opt = alias_for_dev(did).cloned();
                    let aid_opt = ag_opt.as_ref().map(|a| a.id);
                    let display_name = ag_opt
                        .as_ref()
                        .map(|a| a.alias.clone())
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| d.hostname.clone());
                    let online = d.status.to_ascii_lowercase().contains("online");
                    let grp_name = d
                        .group_id
                        .and_then(|gid| groups.iter().find(|g| g.id == gid).map(|g| g.name.clone()))
                        .unwrap_or_else(|| "Sin grupo".into());
                    let h1 = display_name.clone();
                    let h2 = display_name.clone();
                    let h3 = display_name.clone();
                    let h4 = display_name.clone();
                    let h5 = display_name.clone();
                    let h_wol = display_name.clone();
                    let mac_label = if d.mac_addresses.is_empty() {
                        "Sin MAC registrada".to_string()
                    } else {
                        d.mac_addresses.join(", ")
                    };

                    inspector = inspector
                        .child(div().text_base().font_semibold().child(display_name))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .px_2()
                                        .py(px(2.))
                                        .rounded_md()
                                        .bg(rgb(if online { p.badge_ok_bg } else { p.badge_off_bg }))
                                        .text_color(rgb(if online { p.badge_ok_fg } else { p.badge_off_fg }))
                                        .text_xs()
                                        .child(if online { "En línea" } else { "Desconectado" }),
                                )
                                .child(div().text_xs().text_color(rgb(p.muted)).child(d.public_code.clone())),
                        )
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("SO: {} ({})", d.os_name, d.os_arch)))
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Grupo: {grp_name}")))
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("MAC: {mac_label}")))
                        .when_some(ag_opt.as_ref().filter(|a| !a.notes.trim().is_empty()), |el, a| {
                            el.child(div().text_xs().text_color(rgb(p.muted)).child(format!("Notas: {}", a.notes)))
                        })
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .child(
                            action_button("Control remoto", SharedString::from(format!("ins-ctrl-{did}")), p)
                                .id(SharedString::from(format!("ins-ctrl-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_managed_session(did, None, true, h1.clone(), cx);
                                })),
                        )
                        .child(
                            subtle_button("Encender (Wake-on-LAN)", SharedString::from(format!("ins-wol-{did}")), p)
                                .id(SharedString::from(format!("ins-wol-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.trigger_wol(did, h_wol.clone(), cx);
                                })),
                        )
                        .child(
                            subtle_button("Presentación (solo vista)", SharedString::from(format!("ins-view-{did}")), p)
                                .id(SharedString::from(format!("ins-view-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_managed_session(did, None, false, h2.clone(), cx);
                                })),
                        )
                        .child(
                            subtle_button("Enviar chat", SharedString::from(format!("ins-chat-{did}")), p)
                                .id(SharedString::from(format!("ins-chat-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_managed_session_with_panel(did, None, true, h3.clone(), Some("chat"), cx);
                                })),
                        )
                        .child(
                            subtle_button("Transferir archivos", SharedString::from(format!("ins-files-{did}")), p)
                                .id(SharedString::from(format!("ins-files-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_managed_session_with_panel(did, None, true, h4.clone(), Some("files"), cx);
                                })),
                        )
                        .child(
                            subtle_button("Diagnóstico / Terminal RMM", SharedString::from(format!("ins-rmm-{did}")), p)
                                .id(SharedString::from(format!("ins-rmm-{did}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                                        this.rmm_device_idx = idx;
                                    }
                                    this.page = crate::Page::Monitoring;
                                    cx.notify();
                                })),
                        )
                        .child(
                            subtle_button("Propiedades", SharedString::from(format!("ins-props-{did}")), p)
                                .id(SharedString::from(format!("ins-props-{did}")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_edit_device(Some(did), aid_opt, h5.clone(), window, cx);
                                })),
                        )
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("ICONO DEL EQUIPO"))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .children([
                                    ("desktop", "PC"),
                                    ("server", "Servidor"),
                                    ("laptop", "Portátil"),
                                    ("database", "DB"),
                                    ("network", "Red"),
                                    ("cloud", "Nube"),
                                    ("camera", "Cámara"),
                                ].into_iter().map(|(k, lbl)| {
                                    let ks = k.to_string();
                                    let col_now = d.accent_color.clone();
                                    let active = d.icon_kind == k;
                                    subtle_button(lbl, SharedString::from(format!("ins-ic-{did}-{k}")), p)
                                        .id(SharedString::from(format!("ins-ic-{did}-{k}")))
                                        .when(active, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_device_appearance(did, ks.clone(), col_now.clone(), cx);
                                        }))
                                })),
                        )
                        .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("COLOR"))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .children([
                                    ("", 0x3B82F6_u32, "Auto"),
                                    ("#3B82F6", 0x3B82F6_u32, "Azul"),
                                    ("#10B981", 0x10B981_u32, "Verde"),
                                    ("#8B5CF6", 0x8B5CF6_u32, "Violeta"),
                                    ("#F59E0B", 0xF59E0B_u32, "Ámbar"),
                                    ("#EF4444", 0xEF4444_u32, "Rojo"),
                                    ("#06B6D4", 0x06B6D4_u32, "Cian"),
                                ].into_iter().map(|(hex, swatch_rgb, lbl)| {
                                    let hs = hex.to_string();
                                    let ic_now = d.icon_kind.clone();
                                    let active = d.accent_color.eq_ignore_ascii_case(hex);
                                    div()
                                        .id(SharedString::from(format!("ins-col-{did}-{lbl}")))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .py(px(3.))
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(if active { p.accent } else { p.border }))
                                        .bg(rgb(if active { p.active } else { p.raised }))
                                        .cursor_pointer()
                                        .text_xs()
                                        .when(!hex.is_empty(), |el| {
                                            el.child(div().size(px(9.)).rounded_full().bg(rgb(swatch_rgb)))
                                        })
                                        .child(lbl)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_device_appearance(did, ic_now.clone(), hs.clone(), cx);
                                        }))
                                })),
                        );
                } else {
                    inspector = inspector.child(div().text_xs().text_color(rgb(p.muted)).child("Equipo no encontrado."));
                }
            }
            Some(crate::OcSelection::Agenda(aid)) => {
                let aid = *aid;
                if let Some(a) = agenda.iter().find(|x| x.id == aid).cloned() {
                    let did_opt = a.device_id;
                    let al = a.alias.clone();
                    let al2 = a.alias.clone();
                    inspector = inspector
                        .child(div().text_base().font_semibold().child(a.alias.clone()))
                        .when(!a.client_company.is_empty(), |el| {
                            el.child(div().text_xs().text_color(rgb(p.muted)).child(format!("Empresa: {}", a.client_company)))
                        })
                        .when(!a.contact_name.is_empty(), |el| {
                            el.child(div().text_xs().text_color(rgb(p.muted)).child(format!("Contacto: {}", a.contact_name)))
                        })
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Grupo: {}", a.group_name)))
                        .when(!a.notes.trim().is_empty(), |el| {
                            el.child(div().text_xs().text_color(rgb(p.muted)).child(format!("Notas: {}", a.notes)))
                        })
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .when_some(did_opt, |el, did| {
                            let al_c = al.clone();
                            el.child(
                                action_button("Conectar al equipo", SharedString::from(format!("ins-agc-{aid}")), p)
                                    .id(SharedString::from(format!("ins-agc-{aid}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_managed_session(did, None, true, al_c.clone(), cx);
                                    })),
                            )
                        })
                        .child(
                            subtle_button("Propiedades", SharedString::from(format!("ins-agp-{aid}")), p)
                                .id(SharedString::from(format!("ins-agp-{aid}")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_edit_device(did_opt, Some(aid), al2.clone(), window, cx);
                                })),
                        )
                        .child(
                            subtle_button("Eliminar contacto", SharedString::from(format!("ins-agd-{aid}")), p)
                                .id(SharedString::from(format!("ins-agd-{aid}")))
                                .text_color(rgb(p.danger))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.oc_selection = None;
                                    this.delete_agenda_entry(aid, cx);
                                })),
                        );
                } else {
                    inspector = inspector.child(div().text_xs().text_color(rgb(p.muted)).child("Contacto no encontrado."));
                }
            }
            Some(crate::OcSelection::Recent(idx)) => {
                let idx = *idx;
                if let Some(h) = history.get(idx).cloned() {
                    inspector = inspector
                        .child(div().text_base().font_semibold().child(h.hostname.clone()))
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("ID: {}", h.connection_id)))
                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Última conexión: {}", fmt_elapsed(now - h.last_connected_unix))))
                        .child(div().h(px(1.)).bg(rgb(p.border)))
                        .child(
                            action_button("Reconectar", SharedString::from(format!("ins-rec-{idx}")), p)
                                .id(SharedString::from(format!("ins-rec-{idx}")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.reconnect_from_history(idx, Some(window), cx);
                                })),
                        )
                        .child(
                            subtle_button("Quitar del historial", SharedString::from(format!("ins-recd-{idx}")), p)
                                .id(SharedString::from(format!("ins-recd-{idx}")))
                                .text_color(rgb(p.danger))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.oc_selection = None;
                                    this.remove_history_entry(idx, cx);
                                })),
                        );
                }
            }
            None => {
                let online_total = devs
                    .iter()
                    .filter(|d| d.status.to_ascii_lowercase().contains("online"))
                    .count();
                let shared_groups = groups.iter().filter(|g| !g.shares.is_empty()).count();
                inspector = inspector
                    .child(div().text_base().font_semibold().child("Resumen"))
                    .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Grupos visibles: {}", groups.len())))
                    .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Equipos en línea: {online_total} / {}", devs.len())))
                    .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Grupos compartidos: {shared_groups}")))
                    .child(div().h(px(1.)).bg(rgb(p.border)))
                    .child(div().text_xs().text_color(rgb(p.muted)).child("Hacé clic derecho sobre cualquier grupo o equipo para ver acciones rápidas, compartir o editar propiedades."));
            }
        }

        root.child(div().flex().items_start().gap_4().child(tree).child(inspector))
    }

    pub fn render_monitoring(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let mut root = div().flex().flex_col().gap_4().child(page_heading(
            "Monitoreo RMM y Terminal Remota",
            "Recursos HW (CPU/RAM/Disco/GPU), alertas con umbrales y terminal PowerShell/CMD auditable.",
            p,
        ));
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Monitoreo RMM", p, cx));
        }
        if self.devices.is_empty() {
            return root.child(
                workspace_panel("RMM", p)
                    .child(div().text_sm().text_color(rgb(p.muted)).child("No hay dispositivos registrados aún.")),
            );
        }
        if self.rmm_device_idx >= self.devices.len() {
            self.rmm_device_idx = 0;
        }
        let selected = self.devices[self.rmm_device_idx].clone();
        let did = selected.id;
        let tab = self.monitor_tab;

        root = root.child(
            workspace_panel(if tab == "metrics" { "Diagnósticos en vivo" } else { "Equipo" }, p)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            subtle_button(
                                format!("Equipo: {} ({}) [{}]", selected.hostname, selected.public_code, selected.status),
                                "rmm-next-dev",
                                p,
                            )
                            .id("rmm-next-dev")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.devices.is_empty() {
                                    this.rmm_device_idx = (this.rmm_device_idx + 1) % this.devices.len();
                                    this.load_device_alerts(cx);
                                    cx.notify();
                                }
                            })),
                        )
                        .children([
                            ("resources", "Recursos HW"),
                            ("metrics", "Métricas"),
                            ("inventory", "Inventario"),
                            ("services", "Servicios"),
                            ("events", "Eventos"),
                        ].into_iter().filter(|_| tab == "metrics").map(|(kind, label)| {
                            action_button(label, format!("diag-{kind}"), p)
                                .id(SharedString::from(format!("diag-{kind}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_rmm_diagnostic(did, kind, cx);
                                }))
                        })),
                )
                .when_some(self.rmm_hw_snapshot.clone().filter(|_| tab == "metrics"), |this, hw| {
                    this.child(render_hardware_resources(&hw, p))
                })
                .when(tab == "metrics" && !self.rmm_diag_output.is_empty(), |this| {
                    this.child(
                        div()
                            .id("rmm-diag-out")
                            .max_h(px(220.))
                            .overflow_y_scrollbar()
                            .p_2()
                            .rounded_md()
                            .bg(rgb(p.raised))
                            .text_xs()
                            .child(self.rmm_diag_output.clone()),
                    )
                }),
        );

        // Alertas del equipo + Umbrales
        let alerts = self.device_alerts.clone();
        let mut alerts_panel = workspace_panel("Alertas del dispositivo", p).child(
            subtle_button("Refrescar alertas", "ref-alerts", p)
                .id("ref-alerts")
                .on_click(cx.listener(|this, _, _, cx| this.load_device_alerts(cx))),
        );
        if alerts.is_empty() {
            alerts_panel = alerts_panel.child(div().text_xs().text_color(rgb(p.muted)).child("Sin alertas abiertas."));
        } else {
            for al in alerts {
                let al_id = al.id;
                alerts_panel = alerts_panel.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py_1()
                        .text_xs()
                        .child(format!(
                            "[{}] {} ({:.1}{} vs {:.1}{}) · {}",
                            al.severity, al.summary, al.current_value, al.unit, al.threshold, al.unit, al.status
                        ))
                        .when(al.status == "open", |this| {
                            this.child(
                                subtle_button("Reconocer", format!("ack-{al_id}"), p)
                                .id(SharedString::from(format!("ack-{al_id}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.acknowledge_alert(al_id, cx);
                                })),
                            )
                        }),
                );
            }
        }
        if tab == "alerts" {
            root = root.child(alerts_panel);
        }
        if tab != "terminal" {
            return root;
        }

        // Terminal Remota
        root.child(
            workspace_panel("Terminal Remota Auditable", p)
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            subtle_button(
                                format!("Shell: {}", self.rmm_shell),
                                "toggle-rmm-shell",
                                p,
                            )
                            .id("toggle-rmm-shell")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rmm_shell = if this.rmm_shell == "powershell" {
                                    "cmd".into()
                                } else {
                                    "powershell".into()
                                };
                                cx.notify();
                            })),
                        )
                        .child(Input::new(&self.rmm_command_in).flex_1())
                        .child(
                            action_button("Ejecutar", "run-rmm-cmd", p)
                                .id("run-rmm-cmd")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_rmm_command(did, cx);
                                })),
                        ),
                )
                .when(!self.rmm_cmd_output.is_empty(), |this| {
                    this.child(
                        div()
                            .id("rmm-cmd-out")
                            .max_h(px(220.))
                            .overflow_y_scrollbar()
                            .p_2()
                            .rounded_md()
                            .bg(rgb(p.raised))
                            .text_xs()
                            .child(self.rmm_cmd_output.clone()),
                    )
                }),
        )
    }

    pub fn render_automation(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let root = div().flex().flex_col().gap_4().child(page_heading(
            "Automatización y Calendario",
            "Agenda de mantenimientos y base de conocimiento.",
            p,
        ));
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Calendario", p, cx));
        }
        let mut cal_panel = workspace_panel("Calendario de mantenimientos", p)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(&self.cal_title_in).flex_1())
                    .child(Input::new(&self.cal_client_in).flex_1())
                    .child(
                        action_button("+ Programar (+2h)", "cal-create", p)
                            .id("cal-create")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_calendar_event(window, cx);
                            })),
                    ),
            );
        for ev in self.calendar.clone() {
            let eid = ev.id;
            cal_panel = cal_panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .text_xs()
                    .child(format!(
                        "{} — {} ({})",
                        ev.title,
                        ev.client_name,
                        ev.starts_at.format("%Y-%m-%d %H:%M")
                    ))
                    .child(
                        subtle_button("Eliminar", format!("cal-del-{eid}"), p)
                            .id(SharedString::from(format!("cal-del-{eid}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_calendar_event(eid, cx);
                            })),
                    ),
            );
        }
        let mut doc_panel = workspace_panel("Documentación técnica", p);
        for d in &self.docs {
            doc_panel = doc_panel.child(
                div()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().text_sm().font_semibold().child(d.title.clone()))
                    .child(div().text_xs().text_color(rgb(p.muted)).child(d.body_markdown.clone())),
            );
        }
        root.child(cal_panel).child(doc_panel)
    }

    pub fn render_reports(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let root = div().flex().flex_col().gap_4().child(page_heading(
            "Informes y Auditoría",
            "Exportación CSV de sesiones y registro append-only.",
            p,
        ));
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Informes", p, cx));
        }
        let exp = workspace_panel("Exportar reportes", p)
            .child(
                action_button("Descargar sesiones.csv", "export-csv", p)
                    .id("export-csv")
                    .on_click(cx.listener(|this, _, _, cx| this.export_sessions_csv(cx))),
            )
            .when(!self.report_status.is_empty(), |this| {
                this.child(div().text_xs().text_color(rgb(p.accent)).child(self.report_status.clone()))
            });

        let mut aud = workspace_panel("Eventos de auditoría recientes", p);
        for e in self.audit.iter().take(40) {
            aud = aud.child(
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(format!(
                        "{} · {} · {} ({})",
                        e.occurred_at.format("%Y-%m-%d %H:%M:%S"),
                        e.action,
                        e.resource_type,
                        e.outcome
                    )),
            );
        }
        root.child(exp).child(aud)
    }

    pub fn render_users_mfa(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let mut root = div().flex().flex_col().gap_4().child(page_heading(
            "Usuarios y MFA TOTP",
            "Administración de cuentas técnicas y segundo factor de autenticación.",
            p,
        ));
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("Usuarios y MFA", p, cx));
        }

        if self.is_admin() {
            let role = self.new_user_role.clone();
            root = root.child(
                workspace_panel("+ Crear usuario técnico", p)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.new_user_email_in).flex_1())
                            .child(Input::new(&self.new_user_name_in).flex_1())
                            .child(Input::new(&self.new_user_pw_in).flex_1().mask_toggle()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .children(["org_admin", "technician", "viewer"].into_iter().map(|r| {
                                        let rs = r.to_string();
                                        subtle_button(r, format!("nr-{r}"), p)
                                            .id(SharedString::from(format!("nr-{r}")))
                                            .when(role == r, |b| b.bg(rgb(p.active)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.new_user_role = rs.clone();
                                                cx.notify();
                                            }))
                                    })),
                            )
                            .child(
                                action_button("Crear usuario", "create-usr-btn", p)
                                    .id("create-usr-btn")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.create_user(window, cx);
                                    })),
                            ),
                    ),
            );
            root = root.child(crate::users_table(&self.users, p));
        }

        // MFA TOTP setup + verify
        let mut mfa_panel = workspace_panel("Mi MFA TOTP (Autenticador 6 dígitos)", p).child(
            action_button("Generar secreto TOTP", "mfa-setup-btn", p)
                .id("mfa-setup-btn")
                .on_click(cx.listener(|this, _, _, cx| this.setup_mfa(cx))),
        );
        if let Some(ms) = &self.mfa_setup {
            mfa_panel = mfa_panel
                .child(div().text_xs().text_color(rgb(p.accent)).child(format!("Secreto Base32: {}", ms.secret_base32)))
                .child(div().text_xs().text_color(rgb(p.muted)).child(ms.otpauth_uri.clone()))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Input::new(&self.mfa_verify_in).w(px(180.)))
                        .child(
                            action_button("Activar MFA", "mfa-verify-btn", p)
                                .id("mfa-verify-btn")
                                .on_click(cx.listener(|this, _, _, cx| this.verify_mfa(cx))),
                        ),
                );
        }
        root.child(mfa_panel)
    }

    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let lan = detect_lan_ip();
        let sec = self.settings_section;

        let nav_sec = |id: &'static str, icon: IconName, label: &'static str| {
            let active = sec == id;
            div()
                .id(SharedString::from(format!("set-nav-{id}")))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(7.))
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(if active { p.active } else { p.panel }))
                .text_color(rgb(if active { p.accent } else { p.text }))
                .hover(|s| s.bg(rgb(p.raised)))
                .text_xs()
                .font_semibold()
                .child(Icon::new(icon).size(px(15.)))
                .child(label)
        };

        let row_item = |icon: IconName, title: &'static str, desc: &'static str, right: Div| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(Icon::new(icon).size(px(18.)).text_color(rgb(p.muted)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().font_semibold().child(title))
                                .child(div().text_xs().text_color(rgb(p.muted)).child(desc)),
                        ),
                )
                .child(right)
        };

        let left_nav = div()
            .flex()
            .flex_col()
            .w(px(210.))
            .gap_1()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .child(div().px_3().py_1().text_xs().font_bold().text_color(rgb(p.muted)).child("PERFIL"))
            .child(nav_sec("account", IconName::CircleUser, "Cuenta").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "account";
                cx.notify();
            })))
            .child(nav_sec("auth", IconName::ShieldCheck, "Autenticación y MFA").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "auth";
                cx.notify();
            })))
            .when(self.auth.is_some(), |d| {
                d.child(nav_sec("users", IconName::Users, "Usuarios y roles").on_click(cx.listener(|this, _, _, cx| {
                    this.settings_section = "users";
                    this.load_users(cx);
                    cx.notify();
                })))
            })
            .child(div().px_3().pt_3().pb_1().text_xs().font_bold().text_color(rgb(p.muted)).child("DISPOSITIVO"))
            .child(nav_sec("general", IconName::Monitor, "General y Windows").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "general";
                cx.notify();
            })))
            .child(nav_sec("network", IconName::Globe, "Red y Servidor").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "network";
                cx.notify();
            })))
            .child(nav_sec("ai", IconName::Sparkles, "AgentDesk AI").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "ai";
                if this.auth.is_some() {
                    this.load_ai_settings(cx);
                }
                cx.notify();
            })))
            .child(div().px_3().pt_3().pb_1().text_xs().font_bold().text_color(rgb(p.muted)).child("APLICACIÓN"))
            .child(nav_sec("about", IconName::Info, "Acerca de").on_click(cx.listener(|this, _, _, cx| {
                this.settings_section = "about";
                cx.notify();
            })));

        let right_content = match sec {
            "users" => self.render_users_mfa(cx),
            "ai" => self.render_ai_settings_full(cx),
            "about" => workspace_panel("Acerca de", p)
                .child(row_item(
                    IconName::Info,
                    "Versión",
                    "Versión instalada de este cliente.",
                    div().child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(concat!("AgentDesk v", env!("CARGO_PKG_VERSION"))),
                    ),
                ))
                .child(row_item(
                    IconName::Server,
                    "Servidor",
                    "Servidor central configurado.",
                    div().child(div().text_xs().text_color(rgb(p.muted)).child(self.base_url(cx))),
                ))
                .child(row_item(
                    IconName::Monitor,
                    "ID de este equipo",
                    "Identificador para recibir soporte.",
                    div().child(div().text_sm().font_semibold().child(self.local_connection_id.clone())),
                )),
            "auth" => {
                let mut mfa_panel = workspace_panel("Autenticación y MFA TOTP", p)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child("Protegé tu cuenta técnica con verificación en dos pasos (TOTP 6 dígitos)."),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(self.auth.is_some(), |d| {
                                d.child(
                                    action_button("Generar secreto TOTP", "set-mfa-setup", p)
                                        .id("set-mfa-setup")
                                        .on_click(cx.listener(|this, _, _, cx| this.setup_mfa(cx))),
                                )
                                .child(
                                    subtle_button("Cambiar contraseña", "set-auth-pw", p)
                                        .id("set-auth-pw")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.change_pw_open = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(self.auth.is_none(), |d| {
                                d.child(
                                    action_button("Iniciar sesión", "set-auth-login", p)
                                        .id("set-auth-login")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.login_open = true;
                                            cx.notify();
                                        })),
                                )
                            }),
                    );
                if let Some(ms) = &self.mfa_setup {
                    mfa_panel = mfa_panel
                        .child(div().text_xs().text_color(rgb(p.accent)).child(format!("Secreto Base32: {}", ms.secret_base32)))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(Input::new(&self.mfa_verify_in).w(px(180.)))
                                .child(
                                    action_button("Activar MFA", "set-mfa-verify", p)
                                        .id("set-mfa-verify")
                                        .on_click(cx.listener(|this, _, _, cx| this.verify_mfa(cx))),
                                ),
                        );
                }
                mfa_panel
            }
            "general" => workspace_panel("General, Tema e Inicio con Windows", p)
                .child(row_item(
                    IconName::SunMoon,
                    "Apariencia Claro / Oscuro",
                    "Cambiá entre tema claro estilo TeamViewer o modo oscuro.",
                    div().child(
                        subtle_button(
                            if self.dark { "Modo actual: Oscuro" } else { "Modo actual: Claro" },
                            "set-theme-btn",
                            p,
                        )
                        .id("set-theme-btn")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dark = !this.dark;
                            this.cfg.set_dark(this.dark);
                            let _ = this.cfg.save(&this.config_path);
                            crate::apply_theme(
                                cx,
                                if this.dark {
                                    gpui_kit::component::ThemeMode::Dark
                                } else {
                                    gpui_kit::component::ThemeMode::Light
                                },
                            );
                            cx.notify();
                        })),
                    ),
                ))
                .child(row_item(
                    IconName::RefreshCw,
                    "Inicio automático con Windows",
                    "Inicia AgentDesk minimizado en systray al iniciar sesión.",
                    div().child(
                        subtle_button(
                            if self.cfg.start_with_windows { "Activado" } else { "Activar" },
                            "toggle-win-autostart",
                            p,
                        )
                        .id("toggle-win-autostart")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.cfg.start_with_windows;
                            match set_windows_autostart(next) {
                                Ok(()) => {
                                    this.cfg.start_with_windows = next;
                                    let _ = this.cfg.save(&this.config_path);
                                    this.server_status = "Inicio automático actualizado".into();
                                }
                                Err(e) => this.server_status = format!("Error autostart: {e}"),
                            }
                            cx.notify();
                        })),
                    ),
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pt_2()
                        .child(div().text_xs().font_semibold().child("Servicio Pre-Login (Session 0 — acceso con bloqueo/UAC)"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(Input::new(&self.service_enroll_token_in).flex_1())
                                .child(
                                    action_button("Instalar AgentDeskHost", "inst-host-svc", p)
                                        .id("inst-host-svc")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let tok = this.service_enroll_token_in.read(cx).value().to_string();
                                            let srv = this.base_url(cx);
                                            match install_host_service_elevated(&srv, &tok) {
                                                Ok(m) => this.server_status = m,
                                                Err(e) => this.server_status = format!("Error servicio: {e}"),
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                ),
            "network" => workspace_panel("Red y Servidor AgentDesk", p)
                .child(div().text_xs().text_color(rgb(p.muted)).child("Dirección URL del servidor central (LAN, WAN o Localhost)."))
                .child(Input::new(&self.server_url).w(px(460.)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            action_button("Guardar y reconectar", "save-srv-url", p)
                                .id("save-srv-url")
                                .on_click(cx.listener(|this, _, _, cx| this.save_server_url(cx))),
                        )
                        .child(
                            subtle_button("Verificar conexión", "chk-srv", p)
                                .id("chk-srv")
                                .on_click(cx.listener(|this, _, _, cx| this.check_server(cx))),
                        )
                        .child(
                            subtle_button("Usar Localhost", "use-local-srv", p)
                                .id("use-local-srv")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_server_url_preset("http://127.0.0.1:8080", window, cx);
                                })),
                        )
                        .when_some(lan, |this, ip| {
                            let url = format!("http://{ip}:8080");
                            this.child(
                                subtle_button(format!("Usar LAN ({ip})"), "use-lan-srv", p)
                                    .id("use-lan-srv")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_server_url_preset(&url, window, cx);
                                    })),
                            )
                        }),
                )
                .child(div().text_xs().text_color(rgb(p.muted)).child(self.server_status.clone())),
            _ => {
                // Cuenta (Captura 1)
                let (name_val, email_val, org_val, mfa_val) = if let Some(a) = &self.auth {
                    let mfa_on = self
                        .users
                        .iter()
                        .find(|u| u.id == a.user.id)
                        .is_some_and(|u| u.mfa_enabled);
                    (
                        a.user.display_name.clone(),
                        a.user.email.clone(),
                        format!("{} ({})", a.user.organization_name, a.user.role),
                        if mfa_on { "Activo" } else { "Inactivo" },
                    )
                } else {
                    ("Sin sesión".into(), "No autenticado".into(), "Local".into(), "No aplica")
                };
                workspace_panel("Cuenta", p)
                    .child(row_item(
                        IconName::CircleUser,
                        "Imagen de perfil",
                        "La insignia ayuda a tus contactos a identificarte.",
                        div().child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(36.))
                                .rounded_full()
                                .bg(rgb(if self.auth.is_some() { 0xB91C1C } else { p.raised }))
                                .text_color(rgb(if self.auth.is_some() { 0xFFFFFF } else { p.text }))
                                .text_xs()
                                .font_bold()
                                .child(self.account_initials()),
                        ),
                    ))
                    .child(row_item(
                        IconName::UserRound,
                        "Nombre y organización",
                        "Nombre visible y empresa asignada a tu cuenta.",
                        div().child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(div().text_sm().font_semibold().child(name_val))
                                .child(div().text_xs().text_color(rgb(p.muted)).child(org_val)),
                        ),
                    ))
                    .child(row_item(
                        IconName::Inbox,
                        "Correo electrónico",
                        "Dirección asignada a tu cuenta AgentDesk.",
                        div().child(div().text_sm().child(email_val)),
                    ))
                    .child(row_item(
                        IconName::KeyRound,
                        "Contraseña y seguridad",
                        "Contraseña personal y estado de MFA.",
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(p.muted)).child(format!("MFA: {mfa_val}")))
                            .when(self.auth.is_some(), |d| {
                                d.child(
                                    subtle_button("Cambiar contraseña", "set-ch-pw", p)
                                        .id("set-ch-pw")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.change_pw_open = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(self.auth.is_none(), |d| {
                                d.child(
                                    action_button("Iniciar sesión", "set-go-login", p)
                                        .id("set-go-login")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.login_open = true;
                                            cx.notify();
                                        })),
                                )
                            }),
                    ))
                    .child(row_item(
                        IconName::Monitor,
                        "ID de dispositivo local",
                        "Identificador asignado a esta estación AgentDesk.",
                        div().child(div().text_sm().font_semibold().child(self.local_connection_id.clone())),
                    ))
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(page_heading(
                "Configuración",
                "Establecé los ajustes de tu cuenta AgentDesk, seguridad, servidor y servicio Windows.",
                p,
            ))
            .child(div().flex().items_start().gap_4().child(left_nav).child(div().flex_1().min_w_0().child(right_content)))
    }

    pub fn render_ai_settings_full(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let subtab = self.ai_subtab;
        let mut root = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(page_heading(
                "Automatización e información · AgentDesk AI",
                "ChatGPT OAuth (Device Code), selector dinámico de modelos e información inteligente de sesiones.",
                p,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        subtle_button("ChatGPT OAuth y Modelos", "ai-tab-oauth", p)
                            .id("ai-tab-oauth")
                            .when(subtab == "oauth", |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ai_subtab = "oauth";
                                cx.notify();
                            })),
                    )
                    .child(
                        subtle_button("Información de sesión (IA)", "ai-tab-insights", p)
                            .id("ai-tab-insights")
                            .when(subtab == "insights", |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ai_subtab = "insights";
                                cx.notify();
                            })),
                    ),
            );
        if self.auth.is_none() {
            return root.child(self.auth_required_panel("AgentDesk AI", p, cx));
        }

        if subtab == "insights" {
            let total_sess = self.sessions.len();
            let resolved_tickets = self.tickets.iter().filter(|t| t.status == "resolved" || t.status == "closed").count();
            return root
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .rounded_xl()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.panel))
                                .child(div().text_sm().font_semibold().child("Principales tipos de problemas"))
                                .child(div().text_xs().text_color(rgb(p.muted)).child("Resumen de categorías en tus sesiones de soporte."))
                                .child(
                                    div()
                                        .py_4()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_2()
                                        .child(Icon::new(IconName::Sparkles).size(px(28.)).text_color(rgb(p.accent)))
                                        .child(div().text_xs().text_color(rgb(p.muted)).child(
                                            if self.tickets.is_empty() {
                                                "Aún no hay categorías registradas.".to_string()
                                            } else {
                                                format!("{} tickets totales · {} resueltos", self.tickets.len(), resolved_tickets)
                                            },
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .rounded_xl()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.panel))
                                .child(div().text_sm().font_semibold().child("Aplicaciones y equipos atendidos"))
                                .child(div().text_xs().text_color(rgb(p.muted)).child("Actividad reciente durante sesiones remotas."))
                                .child(
                                    div()
                                        .py_4()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_2()
                                        .child(Icon::new(IconName::Cpu).size(px(28.)).text_color(rgb(p.accent)))
                                        .child(div().text_xs().text_color(rgb(p.muted)).child(
                                            if total_sess == 0 {
                                                "Aún no hay datos de sesiones.".to_string()
                                            } else {
                                                format!("{total_sess} sesiones remotas auditadas")
                                            },
                                        )),
                                ),
                        ),
                )
                .child(
                    workspace_panel("Resúmenes de sesión", p).child(
                        if self.sessions.is_empty() {
                            div()
                                .py_4()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child("Todavía no hay resúmenes generados. Iniciá una sesión remota o abrí el resumen IA desde la barra superior del visor.")
                        } else {
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(self.sessions.iter().take(6).map(|s| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(p.border))
                                        .text_xs()
                                        .child(format!("Sesión {} · {}", s.id, s.status))
                                        .child(div().text_color(rgb(p.muted)).child(s.created_at.to_string()))
                                }))
                        },
                    ),
                );
        }

        let connected = self
            .ai_settings
            .as_ref()
            .is_some_and(|s| s.oauth_connected);
        let prov = self.ai_provider.clone();
        let active_model = if self.ai_selected_model.trim().is_empty() {
            self.ai_settings
                .as_ref()
                .map(|s| s.model.clone())
                .unwrap_or_else(|| "gpt-6-luna".into())
        } else {
            self.ai_selected_model.clone()
        };
        let active_effort = if self.ai_reasoning_effort.trim().is_empty() {
            "high".to_string()
        } else {
            self.ai_reasoning_effort.clone()
        };
        let dev_opt = self.ai_device_idx;
        let dev_label = if dev_opt == 0 {
            "Equipo contexto: Ninguno".to_string()
        } else {
            self.devices
                .get(dev_opt - 1)
                .map(|d| format!("Equipo: {} ({})", d.hostname, d.public_code))
                .unwrap_or_else(|| "Equipo contexto: Ninguno".into())
        };

        let show_models_picker = (prov == "chat_gpt_o_auth" && connected)
            || (prov == "openai_api" && self.ai_settings.as_ref().is_some_and(|s| s.has_api_key))
            || prov == "ollama_local";
        let device_code_opt = self.oauth_device_code.clone();

        root = root.child(
            workspace_panel("Proveedor IA y ChatGPT OAuth (Device Code estilo OpenCode)", p)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .children([
                            ("chat_gpt_o_auth", "ChatGPT OAuth"),
                            ("openai_api", "OpenAI API"),
                            ("ollama_local", "Ollama Local"),
                            ("disabled", "Deshabilitado"),
                        ].into_iter().map(|(v, l)| {
                            let vs = v.to_string();
                            subtle_button(l, format!("aip-{v}"), p)
                                .id(SharedString::from(format!("aip-{v}")))
                                .when(prov == v, |b| b.bg(rgb(p.active)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ai_provider = vs.clone();
                                    cx.notify();
                                }))
                        }))
                        .when(prov == "chat_gpt_o_auth", |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py(px(3.))
                                    .rounded_full()
                                    .bg(rgb(if connected { p.badge_ok_bg } else { p.badge_off_bg }))
                                    .text_color(rgb(if connected { p.badge_ok_fg } else { p.badge_off_fg }))
                                    .text_xs()
                                    .font_semibold()
                                    .child(if connected { "● Conectado" } else { "○ Sin vincular" }),
                            )
                        }),
                )
                .when(prov != "chat_gpt_o_auth", |this| {
                    this.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.ai_model_in).w(px(180.)))
                            .child(Input::new(&self.ai_base_url_in).flex_1())
                            .child(Input::new(&self.ai_api_key_in).flex_1().mask_toggle()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            action_button("Guardar configuración IA", "save-ai-cfg", p)
                                .id("save-ai-cfg")
                                .on_click(cx.listener(|this, _, _, cx| this.save_ai_settings(cx))),
                        )
                        .when(prov == "chat_gpt_o_auth" && !connected, |this| {
                            this.child(
                                action_button("Vincular ChatGPT (OAuth)", "connect-chatgpt", p)
                                    .id("connect-chatgpt")
                                    .on_click(cx.listener(|this, _, _, cx| this.connect_chatgpt(cx))),
                            )
                        })
                        .when(prov == "chat_gpt_o_auth" && connected, |this| {
                            this.child(
                                subtle_button("Listar modelos", "ref-ai-models", p)
                                    .id("ref-ai-models")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_ai_models(cx))),
                            )
                            .child(
                                subtle_button("Desconectar OAuth", "disc-chatgpt", p)
                                    .id("disc-chatgpt")
                                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_chatgpt(cx))),
                            )
                        })
                        .child(
                            subtle_button(dev_label, "ai-pick-dev", p)
                                .id("ai-pick-dev")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let n = this.devices.len() + 1;
                                    this.ai_device_idx = (this.ai_device_idx + 1) % n;
                                    this.start_new_ai_session(cx);
                                    this.load_ai_chat_sessions(cx);
                                })),
                        )
                        .child(
                            subtle_button(
                                if self.ai_include_diag { "Diagnóstico RMM: incluido" } else { "Diagnóstico RMM: no" },
                                "ai-tog-diag",
                                p,
                            )
                            .id("ai-tog-diag")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ai_include_diag = !this.ai_include_diag;
                                cx.notify();
                            })),
                        ),
                )
                .when_some(device_code_opt, |this, (code, verify_url)| {
                    let c_copy = code.clone();
                    let u_open = verify_url.clone();
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(p.accent))
                            .bg(rgb(p.active))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_xs().font_semibold().child("Código de vinculación OpenAI (sin usar puerto 1455):"))
                                    .child(div().text_2xl().font_bold().text_color(rgb(p.accent)).child(code))
                                    .child(div().text_xs().text_color(rgb(p.muted)).child("Ya se copió al portapapeles. Pegalo en la pestaña auth.openai.com/codex/device.")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        subtle_button("Copiar código", "oauth-copy-code", p)
                                            .id("oauth-copy-code")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(c_copy.clone()));
                                                this.ai_status = "Código copiado al portapapeles".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        action_button("Abrir auth.openai.com/codex/device", "oauth-reopen", p)
                                            .id("oauth-reopen")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.open_url(&u_open);
                                            })),
                                    ),
                            ),
                    )
                })
                .when(prov == "chat_gpt_o_auth" && !connected, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pt_1()
                            .child(Input::new(&self.ai_callback_paste_in).flex_1())
                            .child(
                                subtle_button("Completar manual", "oauth-manual-go", p)
                                    .id("oauth-manual-go")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.complete_oauth_manual(window, cx);
                                    })),
                            ),
                    )
                })
                .when(show_models_picker, |this| {
                    let models = self.ai_models_info.clone();
                    let cur_model_for_eff = active_model.clone();
                    this.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(rgb(p.muted))
                                    .child(if models.is_empty() {
                                        "Cargando modelos disponibles…".to_string()
                                    } else {
                                        format!(
                                            "Modelos disponibles ({}) — por defecto: {} ({})",
                                            models.len(),
                                            active_model,
                                            active_effort
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .children(models.into_iter().map(|(id, label, _)| {
                                        let m_click = id.clone();
                                        let is_active = id == active_model;
                                        subtle_button(label, format!("pick-mod-{id}"), p)
                                            .id(SharedString::from(format!("pick-mod-{id}")))
                                            .when(is_active, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.select_ai_model(m_click.clone(), None, Some(window), cx);
                                            }))
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap_2()
                                    .pt_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(rgb(p.muted))
                                            .child("Esfuerzo de razonamiento por defecto:"),
                                    )
                                    .children(["low", "medium", "high", "xhigh"].into_iter().map(|eff| {
                                        let e_click = eff.to_string();
                                        let m_keep = cur_model_for_eff.clone();
                                        let is_eff = eff == active_effort;
                                        subtle_button(eff.to_uppercase(), format!("pick-eff-{eff}"), p)
                                            .id(SharedString::from(format!("pick-eff-{eff}")))
                                            .when(is_eff, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.select_ai_model(m_keep.clone(), Some(e_click.clone()), Some(window), cx);
                                            }))
                                    })),
                            ),
                    )
                })
                .child(div().text_xs().text_color(rgb(p.muted)).child(self.ai_status.clone())),
        );

        let proposed = self.ai_proposed_actions.clone();
        if !proposed.is_empty() {
            let mut prop_panel = workspace_panel("Acciones propuestas (requieren aprobación)", p);
            for (idx, act) in proposed.into_iter().enumerate() {
                let target_id = self
                    .ai_device_idx
                    .checked_sub(1)
                    .and_then(|i| self.devices.get(i))
                    .map(|d| d.id);
                prop_panel = prop_panel.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py_1()
                        .text_xs()
                        .child(format!("{} · {}", act.command, act.rationale))
                        .when_some(target_id, |this, dev_id| {
                            this.child(
                                action_button("Aprobar y ejecutar", format!("ai-exec-{idx}"), p)
                                    .id(SharedString::from(format!("ai-exec-{idx}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                    this.execute_ai_action(
                                        dev_id,
                                        act.shell.clone(),
                                        act.command.clone(),
                                        act.rationale.clone(),
                                        cx,
                                    );
                                })),
                            )
                        }),
                );
            }
            root = root.child(prop_panel);
        }
        root
    }

    pub fn auth_required_panel(
        &self,
        section: &'static str,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        workspace_panel(section, p)
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child("Iniciá sesión desde tu menú de usuario (arriba a la derecha) para usar esta sección."),
            )
            .child(
                action_button("Iniciar sesión", "open-login-from-panel", p)
                    .id("open-login-from-panel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.login_open = true;
                        this.login_error.clear();
                        cx.notify();
                    })),
            )
    }

    pub fn set_server_url_preset(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.server_url = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx).default_value(url.to_string())
        });
        self.save_server_url(cx);
    }
}
