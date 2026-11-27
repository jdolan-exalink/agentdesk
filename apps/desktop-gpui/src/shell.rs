//! Estructura principal del rediseño: riel de iconos de 60 px, barra superior con
//! búsqueda central y Ctrl+K, paleta de comandos, menú "Nuevo", Inicio y
//! Dispositivos. Reaprovecha la lógica existente del cliente (conexión, grupos,
//! agenda, RMM) y solo cambia cómo se presenta.

use crate::config::{fmt_elapsed, normalize_conn_code, now_unix};
use crate::theme::{render_hardware_resources, Palette};
use crate::ui::*;
use crate::{Client, Page};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use uuid::Uuid;

gpui_kit::actions!(agentdesk, [TogglePalette, CloseOverlays]);

/// Filtro de la columna izquierda de Dispositivos.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DevFilter {
    All,
    Favorites,
    Recent,
    Group(Uuid),
}

/// Acción de un ítem de la paleta de comandos.
#[derive(Clone)]
pub enum PalAction {
    ConnectDevice(Uuid, String),
    ConnectId(String),
    History(usize),
    Wol(Uuid, String),
    CopyId,
    NewPassword,
    SendFiles,
    NewTicket,
    AddContact,
    AddComputer,
    NewGroup,
    Go(Page),
    ToggleTheme,
}

#[derive(Clone)]
pub struct PalItem {
    pub section: &'static str,
    pub icon: IconName,
    pub text: String,
    pub hint: String,
    pub action: PalAction,
}

fn device_icon(kind: &str) -> IconName {
    match kind.trim().to_ascii_lowercase().as_str() {
        "server" | "database" => IconName::Server,
        "laptop" | "notebook" => IconName::Laptop,
        "network" => IconName::Network,
        "cloud" => IconName::Globe,
        "camera" => IconName::Camera,
        _ => IconName::Monitor,
    }
}

fn os_tile(icon: IconName, size: f32, accent: bool, p: Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.26))
        .bg(rgb(if accent { p.active } else { p.raised }))
        .text_color(rgb(if accent { p.accent } else { p.muted }))
        .child(Icon::new(icon).size(px(size * 0.46)))
}

impl Page {
    /// Página del riel que se resalta para cada página (las secciones viejas
    /// quedaron agrupadas dentro de las 7 nuevas).
    pub fn rail_page(self) -> Page {
        match self {
            Page::Agenda => Page::Devices,
            Page::Sessions => Page::Support,
            Page::Automation => Page::Monitoring,
            Page::UsersMfa | Page::Ai => Page::Settings,
            other => other,
        }
    }
}

impl Client {
    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        self.cfg.set_dark(self.dark);
        let _ = self.cfg.save(&self.config_path);
        crate::apply_theme(
            cx,
            if self.dark {
                gpui_kit::component::ThemeMode::Dark
            } else {
                gpui_kit::component::ThemeMode::Light
            },
        );
        cx.notify();
    }

    pub fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.palette_open = false;
        self.plus_menu_open = false;
        match page {
            Page::Support if self.auth.is_some() => self.load_tickets(cx),
            Page::Sessions if self.auth.is_some() => self.load_sessions(cx),
            Page::Monitoring if self.auth.is_some() => self.load_device_alerts(cx),
            Page::Automation if self.auth.is_some() => self.load_calendar_docs(cx),
            Page::Reports if self.auth.is_some() => self.load_audit(cx),
            Page::UsersMfa if self.auth.is_some() => self.load_users(cx),
            Page::Agenda if self.auth.is_some() => self.load_agenda_groups(cx),
            _ => {}
        }
        cx.notify();
    }

    pub fn on_toggle_palette(&mut self, _: &TogglePalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
        } else {
            self.open_palette(window, cx);
        }
        cx.notify();
    }

    pub fn on_close_overlays(&mut self, _: &CloseOverlays, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open || self.plus_menu_open || self.account_menu_open {
            self.palette_open = false;
            self.plus_menu_open = false;
            self.account_menu_open = false;
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_sel = 0;
        self.plus_menu_open = false;
        self.palette_query.update(cx, |st, cx| {
            st.set_value("", window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Riel de navegación
    // ------------------------------------------------------------------

    pub fn render_rail(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let cur = self.page.rail_page();
        let has_open_tickets = self
            .tickets
            .iter()
            .any(|t| t.status != "resolved" && t.status != "closed");
        let nav = [
            (Page::Home, IconName::House, "Inicio"),
            (Page::Devices, IconName::Monitor, "Dispositivos"),
            (Page::Support, IconName::LifeBuoy, "Soporte"),
            (Page::Monitoring, IconName::Activity, "Monitoreo"),
            (Page::Reports, IconName::ChartNoAxesColumn, "Informes"),
        ];

        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap_1()
            .w(px(60.))
            .h_full()
            .pt(px(10.))
            .pb(px(12.))
            .bg(rgb(p.win))
            .child(
                img(self.brand_logo.clone())
                    .size(px(30.))
                    .rounded(px(8.))
                    .mb(px(14.)),
            )
            .children(nav.into_iter().map(|(page, icon, label)| {
                rail_btn(SharedString::from(format!("rail-{label}")), icon, label, cur == page, p)
                    .when(page == Page::Support && has_open_tickets, |d| {
                        d.relative().child(
                            div()
                                .absolute()
                                .top(px(8.))
                                .right(px(8.))
                                .size(px(9.))
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(p.win))
                                .bg(rgb(p.danger)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
            }))
            .child(div().flex_1())
            .child(
                rail_btn("rail-ai", IconName::Sparkles, "AgentDesk AI", self.show_ai, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.show_ai = !this.show_ai;
                        if this.show_ai {
                            this.show_notifications = false;
                            if this.auth.is_some() {
                                this.load_ai_settings(cx);
                                this.load_ai_chat_sessions(cx);
                            }
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                rail_btn("rail-settings", IconName::Settings, "Ajustes", cur == Page::Settings, p)
                    .on_click(cx.listener(|this, _, _, cx| this.go(Page::Settings, cx))),
            )
            .child({
                let logged = self.auth.is_some();
                let label: SharedString = match &self.auth {
                    Some(a) => format!("{} · cuenta, contraseña, cerrar sesión", a.user.display_name).into(),
                    None => "Cuenta · iniciar sesión".into(),
                };
                div()
                    .id("rail-avatar")
                    .relative()
                    .mt(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(32.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(rgb(if logged { p.accent } else { p.raised }))
                    .text_color(rgb(if logged { 0xFFFFFF } else { p.muted }))
                    .text_xs()
                    .font_semibold()
                    .child(if logged {
                        self.account_initials().into_any_element()
                    } else {
                        Icon::new(IconName::UserRound).size(px(16.)).into_any_element()
                    })
                    .child(
                        div()
                            .absolute()
                            .bottom(px(-1.))
                            .right(px(-1.))
                            .size(px(9.))
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(p.win))
                            .bg(rgb(if logged { p.success } else { p.faint })),
                    )
                    .aria_label(label.clone())
                    .tooltip(tip(label))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.account_menu_open = !this.account_menu_open;
                        this.plus_menu_open = false;
                        cx.notify();
                    }))
            })
    }

    // ------------------------------------------------------------------
    // Barra superior
    // ------------------------------------------------------------------

    fn page_title(&self) -> &'static str {
        match self.page.rail_page() {
            Page::Home => "Inicio",
            Page::Devices => "Dispositivos",
            Page::Support => "Soporte",
            Page::Monitoring => "Monitoreo",
            Page::Reports => "Informes",
            _ => "Ajustes",
        }
    }

    /// Selector de icono propio de cada página agrupada (pestañas).
    fn render_page_tabs(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let p = self.palette();
        let tabs: Vec<(Page, &'static str, IconName, &'static str)> = match self.page.rail_page() {
            Page::Support => vec![
                (Page::Support, "", IconName::Inbox, "Tickets y chat"),
                (Page::Sessions, "", IconName::RotateCcwClock, "Historial de sesiones"),
            ],
            Page::Monitoring => vec![
                (Page::Monitoring, "metrics", IconName::Activity, "Métricas y diagnósticos"),
                (Page::Monitoring, "alerts", IconName::Bell, "Alertas y umbrales"),
                (Page::Automation, "", IconName::Workflow, "Automatización y calendario"),
                (Page::Monitoring, "terminal", IconName::SquareTerminal, "Terminal remota"),
            ],
            _ => return None,
        };
        let cur_page = self.page;
        let cur_tab = self.monitor_tab;
        Some(seg(p).children(tabs.into_iter().enumerate().map(|(i, (page, sub, icon, label))| {
            let on = cur_page == page && (sub.is_empty() || cur_tab == sub);
            seg_btn(SharedString::from(format!("ptab-{i}")), icon, label, on, p).on_click(cx.listener(
                move |this, _, _, cx| {
                    if !sub.is_empty() {
                        this.monitor_tab = sub;
                    }
                    this.go(page, cx);
                },
            ))
        })))
    }

    pub fn render_topbar(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let show_notif = self.show_notifications;
        let has_unread = !self.device_alerts.is_empty()
            || self.tickets.iter().any(|t| t.status != "resolved" && t.status != "closed");
        let tabs = self.render_page_tabs(cx);

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(56.))
            .px(px(20.))
            .child(
                div()
                    .min_w(px(140.))
                    .text_size(px(15.))
                    .font_semibold()
                    .child(self.page_title()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("topbar-search")
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(420.))
                    .h(px(34.))
                    .pl(px(10.))
                    .pr(px(6.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.raised))
                    .text_color(rgb(p.faint))
                    .child(Icon::new(IconName::Search).size(px(16.)))
                    .child(
                        Input::new(&self.global_search)
                            .appearance(false)
                            .flex_1()
                            .text_sm()
                            .aria_label("Buscar equipos, IDs o acciones"),
                    )
                    .child(
                        div()
                            .id("topbar-ctrlk")
                            .flex_none()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgb(p.line2))
                            .font_family(MONO)
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .cursor_pointer()
                            .child("Ctrl K")
                            .tooltip(tip("Paleta de comandos"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
                    ),
            )
            .child(div().flex_1())
            .when_some(tabs, |d, t| d.child(t))
            .child(
                icon_btn("topbar-bell", IconName::Bell, "Notificaciones", show_notif, p)
                    .relative()
                    .when(has_unread, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(7.))
                                .right(px(7.))
                                .size(px(7.))
                                .rounded_full()
                                .bg(rgb(p.danger)),
                        )
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_notifications = !this.show_notifications;
                        if this.show_notifications {
                            this.show_ai = false;
                        }
                        cx.notify();
                    })),
            )
            .child(
                icon_btn(
                    "topbar-plus",
                    IconName::Plus,
                    "Nuevo: equipo, contacto, grupo o ticket",
                    self.plus_menu_open,
                    p,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.plus_menu_open = !this.plus_menu_open;
                    this.account_menu_open = false;
                    cx.notify();
                })),
            )
    }

    pub fn render_plus_menu(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if !self.plus_menu_open {
            return None;
        }
        let logged = self.auth.is_some();
        let item = |id: &'static str, icon: IconName, text: &'static str| menu_item(id, icon, text, None, false, p);
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("plus-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.plus_menu_open = false;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    floating(p)
                        .id("plus-menu")
                        .absolute()
                        .top(px(52.))
                        .right(px(20.))
                        .w(px(250.))
                        .p(px(6.))
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(menu_head("Nuevo", p))
                        .child(item("plus-computer", IconName::Monitor, "Agregar equipo").on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.plus_menu_open = false;
                                if this.auth.is_none() {
                                    this.login_open = true;
                                } else {
                                    this.open_add_computer_modal(None, window, cx);
                                }
                                cx.notify();
                            },
                        )))
                        .child(item("plus-contact", IconName::UserPlus, "Agregar contacto").on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.plus_menu_open = false;
                                if this.auth.is_none() {
                                    this.login_open = true;
                                } else {
                                    this.open_add_contact_modal(None, window, cx);
                                }
                                cx.notify();
                            },
                        )))
                        .child(item("plus-group", IconName::FolderPlus, "Nuevo grupo").on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.plus_menu_open = false;
                                if this.auth.is_none() {
                                    this.login_open = true;
                                } else {
                                    this.open_create_group_modal(window, cx);
                                }
                                cx.notify();
                            },
                        )))
                        .child(item("plus-assign", IconName::Hash, "Asignar equipo por ID o email").on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.plus_menu_open = false;
                                this.show_add_device_form = true;
                                this.devices_filter = DevFilter::All;
                                this.go(Page::Devices, cx);
                            }),
                        ))
                        .child(item("plus-enroll", IconName::KeyRound, "Token de enrolamiento").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.plus_menu_open = false;
                                if this.auth.is_none() {
                                    this.login_open = true;
                                    cx.notify();
                                } else {
                                    this.create_enrollment(cx);
                                    this.go(Page::Devices, cx);
                                }
                            },
                        )))
                        .child(item("plus-ticket", IconName::Ticket, "Tickets de soporte").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.plus_menu_open = false;
                                this.go(Page::Support, cx);
                            },
                        )))
                        .when(!logged, |d| {
                            d.child(
                                div()
                                    .px(px(10.))
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(p.faint))
                                    .child("Iniciá sesión para gestionar equipos y grupos."),
                            )
                        }),
                ),
        )
    }

    // ------------------------------------------------------------------
    // Paleta de comandos (Ctrl+K)
    // ------------------------------------------------------------------

    pub fn palette_items(&self, cx: &App) -> Vec<PalItem> {
        let q = self.palette_query.read(cx).value().trim().to_lowercase();
        let qn = normalize_conn_code(&q).to_lowercase();
        let mut out = Vec::new();
        let matches = |t: &str, h: &str| {
            q.is_empty()
                || t.to_lowercase().contains(&q)
                || h.to_lowercase().contains(&q)
                || (!qn.is_empty() && normalize_conn_code(h).to_lowercase().contains(&qn))
        };

        // Equipos gestionados
        for d in &self.devices {
            let online = d.status.to_lowercase().contains("online");
            let (text, action, icon) = if online {
                (
                    format!("Conectar a {}", d.hostname),
                    PalAction::ConnectDevice(d.id, d.hostname.clone()),
                    device_icon(&d.icon_kind),
                )
            } else {
                (
                    format!("Encender {}", d.hostname),
                    PalAction::Wol(d.id, d.hostname.clone()),
                    IconName::Power,
                )
            };
            let hint = if online { group_id(&d.public_code) } else { "Wake-on-LAN".to_string() };
            if matches(&text, &d.public_code) {
                out.push(PalItem { section: "Equipos", icon, text, hint, action });
            }
        }
        // Historial reciente que no esté entre los gestionados
        for (idx, h) in self.cfg.connection_history.iter().enumerate().take(10) {
            let norm = normalize_conn_code(&h.connection_id);
            if self.devices.iter().any(|d| normalize_conn_code(&d.public_code) == norm) {
                continue;
            }
            let text = format!("Conectar a {}", h.hostname);
            if matches(&text, &h.connection_id) {
                out.push(PalItem {
                    section: "Equipos",
                    icon: IconName::RotateCcwClock,
                    text,
                    hint: group_id(&h.connection_id),
                    action: PalAction::History(idx),
                });
            }
        }
        // ID tecleado directamente
        if qn.len() >= 6 && qn.chars().all(|c| c.is_ascii_digit()) && out.is_empty() {
            out.push(PalItem {
                section: "Equipos",
                icon: IconName::Cast,
                text: format!("Conectar al ID {}", group_id(&qn.to_uppercase())),
                hint: "Enter".into(),
                action: PalAction::ConnectId(qn.to_uppercase()),
            });
        }

        let actions = [
            (IconName::Copy, "Copiar mi ID", group_id(&self.local_connection_id), PalAction::CopyId),
            (IconName::RefreshCw, "Nueva contraseña aleatoria", String::new(), PalAction::NewPassword),
            (IconName::FolderUp, "Enviar archivos a un equipo", String::new(), PalAction::SendFiles),
            (IconName::Ticket, "Nuevo ticket", String::new(), PalAction::NewTicket),
            (IconName::Monitor, "Agregar equipo", String::new(), PalAction::AddComputer),
            (IconName::UserPlus, "Agregar contacto", String::new(), PalAction::AddContact),
            (IconName::FolderPlus, "Nuevo grupo", String::new(), PalAction::NewGroup),
        ];
        for (icon, text, hint, action) in actions {
            if matches(text, &hint) {
                out.push(PalItem { section: "Acciones", icon, text: text.into(), hint, action });
            }
        }
        let nav = [
            (IconName::House, "Inicio", PalAction::Go(Page::Home)),
            (IconName::Monitor, "Dispositivos", PalAction::Go(Page::Devices)),
            (IconName::BookUser, "Ordenadores y contactos", PalAction::Go(Page::Agenda)),
            (IconName::LifeBuoy, "Soporte · tickets", PalAction::Go(Page::Support)),
            (IconName::RotateCcwClock, "Soporte · historial de sesiones", PalAction::Go(Page::Sessions)),
            (IconName::Activity, "Monitoreo · alertas", PalAction::Go(Page::Monitoring)),
            (IconName::SquareTerminal, "Terminal remota", PalAction::Go(Page::Monitoring)),
            (IconName::Workflow, "Automatización y calendario", PalAction::Go(Page::Automation)),
            (IconName::ChartNoAxesColumn, "Informes", PalAction::Go(Page::Reports)),
            (IconName::Settings, "Ajustes · servidor", PalAction::Go(Page::Settings)),
            (IconName::Users, "Ajustes · usuarios y MFA", PalAction::Go(Page::UsersMfa)),
            (if self.dark { IconName::Sun } else { IconName::Moon }, "Cambiar tema", PalAction::ToggleTheme),
        ];
        for (icon, text, action) in nav {
            if matches(text, "") {
                out.push(PalItem { section: "Ir a", icon, text: text.into(), hint: String::new(), action });
            }
        }
        out
    }

    pub fn run_palette_action(&mut self, action: PalAction, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        match action {
            PalAction::ConnectDevice(did, host) => self.start_managed_session(did, None, true, host, cx),
            PalAction::ConnectId(id) => self.initiate_connect_by_id(id, true, Some(window), cx),
            PalAction::History(idx) => self.reconnect_from_history(idx, Some(window), cx),
            PalAction::Wol(did, host) => self.trigger_wol(did, host, cx),
            PalAction::CopyId => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.local_connection_id.clone()));
                self.connect_status = "ID copiado al portapapeles".into();
            }
            PalAction::NewPassword => self.rotate_local_password(cx),
            PalAction::SendFiles => {
                self.connect_mode = 2;
                self.partner_allow_control = true;
                self.go(Page::Home, cx);
                self.quick_id.update(cx, |st, cx| st.focus(window, cx));
            }
            PalAction::NewTicket => self.go(Page::Support, cx),
            PalAction::AddComputer => {
                if self.auth.is_none() {
                    self.login_open = true;
                } else {
                    self.open_add_computer_modal(None, window, cx);
                }
            }
            PalAction::AddContact => {
                if self.auth.is_none() {
                    self.login_open = true;
                } else {
                    self.open_add_contact_modal(None, window, cx);
                }
            }
            PalAction::NewGroup => {
                if self.auth.is_none() {
                    self.login_open = true;
                } else {
                    self.open_create_group_modal(window, cx);
                }
            }
            PalAction::Go(page) => {
                if page == Page::Monitoring {
                    self.monitor_tab = "metrics";
                }
                self.go(page, cx)
            }
            PalAction::ToggleTheme => self.toggle_theme(cx),
        }
        cx.notify();
    }

    pub fn run_palette_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items(cx);
        if let Some(it) = items.get(self.palette_sel.min(items.len().saturating_sub(1))).cloned() {
            self.run_palette_action(it.action, window, cx);
        }
    }

    pub fn render_command_palette(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if !self.palette_open {
            return None;
        }
        let items = self.palette_items(cx);
        let sel = self.palette_sel.min(items.len().saturating_sub(1));
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .max_h(px(380.))
            .overflow_y_scrollbar()
            .p(px(6.));
        let mut last_section = "";
        for (i, it) in items.iter().enumerate() {
            if it.section != last_section {
                last_section = it.section;
                list = list.child(
                    div()
                        .px(px(12.))
                        .pt(px(8.))
                        .pb(px(4.))
                        .text_xs()
                        .text_color(rgb(p.faint))
                        .child(it.section.to_uppercase()),
                );
            }
            let action = it.action.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pal-it-{i}")))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(38.))
                    .px(px(12.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_sm()
                    .when(i == sel, |d| d.bg(rgb(p.raised)))
                    .hover(|s| s.bg(rgb(p.raised)))
                    .child(Icon::new(it.icon).size(px(17.)).text_color(rgb(p.muted)))
                    .child(div().flex_1().min_w_0().truncate().child(it.text.clone()))
                    .child(div().font_family(MONO).text_xs().text_color(rgb(p.faint)).child(it.hint.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_palette_action(action.clone(), window, cx);
                    })),
            );
        }
        if items.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py_4()
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child("Sin resultados. Escribí un ID de 9 dígitos para conectar."),
            );
        }
        let total = items.len();
        Some(div().absolute().inset_0().child(
            div()
                .id("palette-scrim")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(90.))
                .bg(rgba(0x0A0C1252))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("palette")
                        .w(px(560.))
                        .rounded(px(14.))
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .shadow_lg()
                        .overflow_hidden()
                        .aria_label("Paleta de comandos")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                            match ev.keystroke.key.as_str() {
                                "down" => {
                                    if total > 0 {
                                        this.palette_sel = (this.palette_sel + 1).min(total - 1);
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "up" => {
                                    this.palette_sel = this.palette_sel.saturating_sub(1);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "escape" => {
                                    this.palette_open = false;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                _ => {}
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .h(px(52.))
                                .px(px(16.))
                                .border_b_1()
                                .border_color(rgb(p.border))
                                .child(Icon::new(IconName::Search).size(px(18.)).text_color(rgb(p.muted)))
                                .child(
                                    Input::new(&self.palette_query)
                                        .appearance(false)
                                        .flex_1()
                                        .text_size(px(15.))
                                        .aria_label("Buscar"),
                                )
                                .child(div().font_family(MONO).text_xs().text_color(rgb(p.faint)).child("Esc")),
                        )
                        .child(list),
                ),
        ))
    }

    // ------------------------------------------------------------------
    // Inicio
    // ------------------------------------------------------------------

    pub fn render_home_v2(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let host = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "este equipo".into())
            .to_uppercase();
        let registered = self.local_device_id.is_some();
        let pw_text = if self.show_local_password {
            self.local_password.clone()
        } else {
            "••••••••".into()
        };
        let easy = self.show_easy_access || self.cfg.personal_password_set;
        let owner_set = !self.cfg.owner_email.trim().is_empty();
        let mode = self.connect_mode;

        let mut root = div().flex().flex_col().gap_4();

        if let Some((target, _, attempts)) = &self.auto_reconnect {
            root = root.child(
                card(p)
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_color(rgb(p.warning))
                    .bg(rgb(p.warn_soft))
                    .child(Icon::new(IconName::RotateCcw).size(px(17.)).text_color(rgb(p.warning)))
                    .child(div().flex_1().text_sm().text_color(rgb(p.text)).child(format!(
                        "Reconectando con {} (intento {}). Se reintenta cada 4 s.",
                        target.display_name(),
                        attempts
                    )))
                    .child(ghost_btn("cancel-auto-reconn", None, "Cancelar", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.auto_reconnect = None;
                            this.connect_status = "Reconexión automática cancelada".into();
                            cx.notify();
                        },
                    ))),
            );
        }

        // --- Tarjeta "Este equipo" ---
        let me_card = card(p)
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(24.))
            .py(px(22.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(label_row(IconName::MonitorSmartphone, format!("Este equipo · {host}"), p))
                    .child(if registered {
                        status_pill("Listo para recibir", p.success, p.badge_ok_bg)
                    } else {
                        status_pill("Registrando…", p.warning, p.warn_soft)
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(42.))
                            .font_medium()
                            .text_color(rgb(p.text))
                            .child(group_id(&self.local_connection_id)),
                    )
                    .child(icon_btn("copy-local-id", IconName::Copy, "Copiar ID", false, p).on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(this.local_connection_id.clone()));
                            this.connect_status = "ID copiado al portapapeles".into();
                            cx.notify();
                        },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .mr_2()
                            .font_family(MONO)
                            .text_size(px(20.))
                            .text_color(rgb(p.text))
                            .child(pw_text),
                    )
                    .child(
                        icon_btn_sm(
                            "toggle-local-password",
                            if self.show_local_password { IconName::EyeOff } else { IconName::Eye },
                            if self.show_local_password { "Ocultar contraseña" } else { "Mostrar contraseña" },
                            false,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_local_password = !this.show_local_password;
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_btn_sm("rotate-local-password", IconName::RefreshCw, "Nueva contraseña aleatoria", false, p)
                            .on_click(cx.listener(|this, _, _, cx| this.rotate_local_password(cx))),
                    )
                    .child(
                        icon_btn_sm("copy-local-password", IconName::Copy, "Copiar contraseña", false, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(this.local_password.clone()));
                                this.connect_status = "Contraseña copiada".into();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        icon_btn_sm(
                            "personal-password",
                            IconName::KeyRound,
                            "Fijar contraseña personal",
                            self.show_easy_access,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_easy_access = !this.show_easy_access;
                            cx.notify();
                        })),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(p.border)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        toggle_chip(
                            "chip-allow-control",
                            IconName::Hand,
                            Some("Control"),
                            if self.incoming_control.is_some() {
                                "Control remoto entrante activo"
                            } else {
                                "Se permite el control remoto con ID y contraseña"
                            },
                            registered,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connect_status = if this.local_device_id.is_some() {
                                "Este equipo acepta control remoto con su ID y contraseña.".into()
                            } else {
                                "El equipo todavía se está registrando en el servidor.".into()
                            };
                            cx.notify();
                        })),
                    )
                    .child(
                        toggle_chip(
                            "chip-easy-access",
                            IconName::Zap,
                            Some("Acceso fácil"),
                            "Acceso fácil y contraseña personal para tu cuenta",
                            easy,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_easy_access = !this.show_easy_access;
                            cx.notify();
                        })),
                    )
                    .child(
                        toggle_chip(
                            "chip-owner",
                            IconName::UserRoundCheck,
                            None,
                            if owner_set {
                                format!("Email propietario: {}", self.cfg.owner_email)
                            } else {
                                "Asignar email propietario".to_string()
                            },
                            owner_set,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_easy_access = !this.show_easy_access;
                            cx.notify();
                        })),
                    )
                    .child(
                        toggle_chip(
                            "chip-prelogin",
                            IconName::ShieldCheck,
                            None,
                            "Servicio pre-login (UAC y pantalla de bloqueo) · se instala en Ajustes",
                            false,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_section = "general";
                            this.go(Page::Settings, cx);
                        })),
                    )
                    .child(
                        toggle_chip(
                            "chip-autostart",
                            IconName::Power,
                            None,
                            if self.cfg.start_with_windows {
                                "Inicia con Windows (activado)"
                            } else {
                                "Iniciar con Windows"
                            },
                            self.cfg.start_with_windows,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.cfg.start_with_windows;
                            match crate::config::set_windows_autostart(next) {
                                Ok(()) => {
                                    this.cfg.start_with_windows = next;
                                    let _ = this.cfg.save(&this.config_path);
                                    this.connect_status = if next {
                                        "AgentDesk iniciará con Windows".into()
                                    } else {
                                        "Inicio con Windows desactivado".into()
                                    };
                                }
                                Err(e) => this.connect_status = format!("No se pudo cambiar el inicio: {e}"),
                            }
                            cx.notify();
                        })),
                    ),
            )
            .when(self.show_easy_access, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded(px(10.))
                        .bg(rgb(p.raised))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(Input::new(&self.personal_pw_input).flex_1().mask_toggle())
                                .child(
                                    icon_btn_sm("home-set-ppw", IconName::Check, "Fijar contraseña personal", false, p)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.set_personal_password(true, Some(window), cx);
                                        })),
                                )
                                .when(self.cfg.personal_password_set, |b| {
                                    b.child(
                                        icon_btn_sm("home-clr-ppw", IconName::X, "Quitar contraseña personal", false, p)
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
                                    icon_btn_sm("home-assign-owner", IconName::UserRoundCheck, "Guardar email propietario", false, p)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cfg.owner_email =
                                                this.owner_email_input.read(cx).value().trim().to_string();
                                            let _ = this.cfg.save(&this.config_path);
                                            this.restart_host(cx);
                                            this.connect_status = "Email propietario guardado y re-registrado".into();
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .child(div().text_xs().text_color(rgb(p.faint)).child(self.local_access_status.clone()));

        // --- Tarjeta "Conectar a otro equipo" ---
        let connect_card = card(p)
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(24.))
            .py(px(22.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgb(p.active), 0.),
                linear_color_stop(rgb(p.panel), 0.7),
            ))
            .child(label_row(IconName::Cast, "Conectar a otro equipo", p))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(64.))
                    .pl(px(18.))
                    .pr(px(8.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(p.line2))
                    .bg(rgb(p.canvas))
                    .child(
                        Input::new(&self.quick_id)
                            .appearance(false)
                            .flex_1()
                            .font_family(MONO)
                            .text_size(px(24.))
                            .aria_label("ID del asociado"),
                    )
                    .child(
                        div()
                            .id("connect-go")
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(48.))
                            .rounded(px(10.))
                            .cursor_pointer()
                            .bg(rgb(p.accent))
                            .text_color(rgb(0xFFFFFF))
                            .hover(|s| s.opacity(0.92))
                            .child(Icon::new(IconName::ArrowRight).size(px(22.)))
                            .aria_label("Conectar")
                            .tooltip(tip("Conectar (Enter)"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pending_viewer_panel = if this.connect_mode == 2 { Some("files") } else { None };
                                this.initiate_connect_from_home(Some(window), cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        seg(p)
                            .child(
                                seg_btn("mode-control", IconName::MousePointer2, "Control remoto", mode == 0, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.connect_mode = 0;
                                        this.partner_allow_control = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                seg_btn("mode-view", IconName::Eye, "Solo ver", mode == 1, p).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.connect_mode = 1;
                                        this.partner_allow_control = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                seg_btn("mode-files", IconName::FolderUp, "Transferir archivos", mode == 2, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.connect_mode = 2;
                                        this.partner_allow_control = true;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                icon_btn_sm("home-wol", IconName::Power, "Encender con Wake-on-LAN", false, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let raw = this.quick_id.read(cx).value().to_string();
                                        let norm = normalize_conn_code(&raw);
                                        if let Some(d) = this
                                            .devices
                                            .iter()
                                            .find(|d| !norm.is_empty() && normalize_conn_code(&d.public_code) == norm)
                                            .cloned()
                                        {
                                            this.trigger_wol(d.id, d.hostname, cx);
                                        } else {
                                            this.connect_status = "Wake-on-LAN: escribí el ID de un equipo gestionado o usá Dispositivos.".into();
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                icon_btn_sm("home-history", IconName::RotateCcwClock, "Historial de conexiones", false, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.devices_filter = DevFilter::Recent;
                                        this.go(Page::Devices, cx);
                                    })),
                            ),
                    ),
            )
            .child(div().text_xs().text_color(rgb(p.faint)).child(if self.connect_status.is_empty() {
                "Conexión segura E2EE · P2P directo si es posible, relay si no".to_string()
            } else {
                self.connect_status.clone()
            }));

        root = root.child(div().mt_1().flex().gap_4().child(me_card).child(connect_card));

        // --- Recientes ---
        let now = now_unix();
        let history: Vec<_> = self.cfg.connection_history.iter().take(4).cloned().collect();
        root = root.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mt_2()
                .child(section_head(IconName::Clock, "Recientes", p))
                .child(
                    icon_btn_sm("recent-all", IconName::List, "Ver todo el historial", false, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.devices_filter = DevFilter::Recent;
                            this.go(Page::Devices, cx);
                        }),
                    ),
                ),
        );
        if history.is_empty() {
            root = root.child(
                card(p)
                    .px_4()
                    .py_4()
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child("Todavía no te conectaste a otros equipos. Escribí un ID arriba para empezar."),
            );
        } else {
            let mut tiles = div().flex().gap_3();
            for (idx, h) in history.into_iter().enumerate() {
                let norm = normalize_conn_code(&h.connection_id);
                let dev = self.devices.iter().find(|d| normalize_conn_code(&d.public_code) == norm).cloned();
                let online = dev.as_ref().is_some_and(|d| d.status.to_lowercase().contains("online"));
                let icon = dev.as_ref().map(|d| device_icon(&d.icon_kind)).unwrap_or(IconName::Monitor);
                let trust = h.is_trust_valid(now);
                let gname = SharedString::from(format!("tile-{idx}"));
                let cid_v = h.connection_id.clone();
                let cid_f = h.connection_id.clone();
                tiles = tiles.child(
                    div()
                        .id(SharedString::from(format!("recent-tile-{idx}")))
                        .group(gname.clone())
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .p(px(14.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .cursor_pointer()
                        .hover(|s| s.border_color(rgb(p.line2)).shadow_sm())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reconnect_from_history(idx, Some(window), cx);
                        }))
                        .child(os_tile(icon, 36., false, p))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().text_sm().font_semibold().truncate().child(h.hostname.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child(dot(if online { p.success } else { p.faint }))
                                        .child(div().font_family(MONO).child(group_id(&h.connection_id)))
                                        .child(div().text_color(rgb(p.faint)).child(if trust {
                                            "· reconexión rápida".to_string()
                                        } else {
                                            format!("· {}", fmt_elapsed(now - h.last_connected_unix))
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(10.))
                                .right(px(10.))
                                .flex()
                                .gap(px(2.))
                                .opacity(0.)
                                .group_hover(gname, |s| s.opacity(1.))
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rt-ctl-{idx}")), IconName::MousePointer2, "Controlar", false, p)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.reconnect_from_history(idx, Some(window), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rt-view-{idx}")), IconName::Eye, "Solo ver", false, p)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.initiate_connect_by_id(cid_v.clone(), false, Some(window), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rt-files-{idx}")), IconName::FolderUp, "Archivos", false, p)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.pending_viewer_panel = Some("files");
                                            this.initiate_connect_by_id(cid_f.clone(), true, Some(window), cx);
                                        })),
                                ),
                        ),
                );
            }
            for _ in self.cfg.connection_history.len().min(4)..4 {
                tiles = tiles.child(div().flex_1());
            }
            root = root.child(tiles);
        }

        // --- Sesiones activas ---
        let live_server: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.status != "ended" && s.status != "closed")
            .take(4)
            .cloned()
            .collect();
        if self.active_viewer_count > 0 || !live_server.is_empty() {
            root = root.child(div().mt_2().child(section_head(IconName::Radio, "Sesiones activas", p)));
            if self.active_viewer_count > 0 {
                root = root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgb(p.border))
                        .child(dot(p.success))
                        .child(div().text_sm().font_semibold().child(format!(
                            "{} {} abiert{} en este equipo",
                            self.active_viewer_count,
                            if self.active_viewer_count == 1 { "sesión" } else { "sesiones" },
                            if self.active_viewer_count == 1 { "a" } else { "as" }
                        )))
                        .child(div().flex_1())
                        .child(tag("Visor", p)),
                );
            }
            for s in live_server {
                let sid = s.id;
                let dev_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == s.device_id)
                    .map(|d| d.hostname.clone())
                    .unwrap_or_else(|| format!("Sesión {}", &sid.to_string()[..8]));
                root = root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgb(p.border))
                        .child(dot(p.success))
                        .child(div().text_sm().font_semibold().child(dev_name))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(s.created_at.format("%H:%M").to_string()),
                        )
                        .child(tag(if s.allow_control { "Control" } else { "Solo ver" }, p))
                        .child(div().flex_1())
                        .child(
                            icon_btn_sm(SharedString::from(format!("hs-term-{sid}")), IconName::Square, "Terminar sesión", false, p)
                                .on_click(cx.listener(move |this, _, _, cx| this.terminate_server_session(sid, cx))),
                        ),
                );
            }
        }
        root
    }

    /// Aviso flotante abajo a la derecha cuando alguien controla este equipo.
    pub fn render_incoming_banner(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let inc = self.incoming_control.clone()?;
        Some(
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(16.))
                .flex()
                .items_center()
                .gap_3()
                .pl(px(14.))
                .pr(px(10.))
                .py(px(10.))
                .rounded(px(12.))
                .bg(rgb(p.text))
                .text_color(rgb(p.canvas))
                .shadow_lg()
                .text_sm()
                .child(Icon::new(if inc.allow_control { IconName::MousePointer2 } else { IconName::Eye }).size(px(17.)))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().font_semibold().child(inc.caller_name.clone()))
                        .child(if inc.allow_control {
                            "está controlando este equipo"
                        } else {
                            "está viendo este equipo"
                        }),
                )
                .child(
                    div()
                        .id("cut-incoming")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .bg(rgb(p.danger))
                        .text_color(rgb(0xFFFFFF))
                        .text_xs()
                        .font_semibold()
                        .child(Icon::new(IconName::Hand).size(px(15.)))
                        .child("Cortar")
                        .aria_label("Cortar el control remoto entrante")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = inc.stop_tx.send(true);
                            this.incoming_control = None;
                            this.local_access_status = "Control remoto cortado localmente".into();
                            cx.notify();
                        })),
                ),
        )
    }

    // ------------------------------------------------------------------
    // Dispositivos
    // ------------------------------------------------------------------

    fn render_groups_column(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let filter = self.devices_filter;
        let on_agenda = self.page == Page::Agenda;
        let fav_count = self.agenda.iter().filter(|a| a.is_favorite).count();
        let row = |id: SharedString, icon: IconName, text: String, count: String, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(9.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_sm()
                .text_color(rgb(if on { p.text } else { p.muted }))
                .when(on, |d| d.bg(rgb(p.raised)).font_medium())
                .hover(|s| s.bg(rgb(p.raised)))
                .child(Icon::new(icon).size(px(16.)))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .child(div().text_xs().text_color(rgb(p.faint)).child(count))
        };
        let sec = |text: &'static str, btn: Option<Stateful<Div>>| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(10.))
                .pt(px(14.))
                .pb(px(6.))
                .text_xs()
                .text_color(rgb(p.faint))
                .child(text.to_uppercase())
                .when_some(btn, |d, b| d.child(b))
        };

        let mut col = div()
            .id("groups-col")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(190.))
            .h_full()
            .px(px(10.))
            .py(px(6.))
            .border_r_1()
            .border_color(rgb(p.border))
            .overflow_y_scrollbar()
            .child(
                row("grp-all".into(), IconName::Layers, "Todos".into(), self.devices.len().to_string(), !on_agenda && filter == DevFilter::All)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.devices_filter = DevFilter::All;
                        this.go(Page::Devices, cx);
                    })),
            )
            .child(
                row("grp-fav".into(), IconName::Star, "Favoritos".into(), fav_count.to_string(), !on_agenda && filter == DevFilter::Favorites)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.devices_filter = DevFilter::Favorites;
                        this.go(Page::Devices, cx);
                    })),
            )
            .child(
                row(
                    "grp-recent".into(),
                    IconName::RotateCcwClock,
                    "Recientes".into(),
                    self.cfg.connection_history.len().to_string(),
                    !on_agenda && filter == DevFilter::Recent,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.devices_filter = DevFilter::Recent;
                    this.go(Page::Devices, cx);
                })),
            );

        let (mine, shared): (Vec<_>, Vec<_>) = self
            .groups
            .clone()
            .into_iter()
            .partition(|g| g.my_permission == "owner" || g.my_permission.is_empty());
        let group_row = |this: &mut Self, g: crate::DeviceGroupDto, icon: IconName, cx: &mut Context<Self>| {
            let gid = g.id;
            let gname = g.name.clone();
            let perm = g.my_permission.clone();
            let on = !on_agenda && filter == DevFilter::Group(gid);
            let _ = this;
            row(SharedString::from(format!("grp-{gid}")), icon, g.name.clone(), g.device_count.to_string(), on)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.devices_filter = DevFilter::Group(gid);
                    this.go(Page::Devices, cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.dev_ctx_menu = None;
                        this.grp_ctx_menu = Some(crate::GroupContextMenu {
                            pos: ev.position,
                            group_id: gid,
                            group_name: gname.clone(),
                            my_permission: perm.clone(),
                        });
                        cx.notify();
                    }),
                )
        };
        for g in mine {
            let r = group_row(self, g, IconName::Folder, cx);
            col = col.child(r);
        }

        col = col
            .child(sec(
                "Contactos",
                Some(
                    icon_btn_sm("grp-add-contact", IconName::UserPlus, "Agregar contacto", false, p).on_click(cx.listener(
                        |this, _, window, cx| {
                            if this.auth.is_none() {
                                this.login_open = true;
                                cx.notify();
                            } else {
                                this.open_add_contact_modal(None, window, cx);
                            }
                        },
                    )),
                ),
            ))
            .child(
                row(
                    "grp-agenda".into(),
                    IconName::BookUser,
                    "Ordenadores y contactos".into(),
                    self.agenda.len().to_string(),
                    on_agenda,
                )
                .on_click(cx.listener(|this, _, _, cx| this.go(Page::Agenda, cx))),
            );
        for a in self.agenda.iter().filter(|a| !a.contact_name.trim().is_empty()).take(6).cloned().collect::<Vec<_>>() {
            let online = a.device_status.as_deref().is_some_and(|s| s.to_lowercase().contains("online"));
            let did = a.device_id;
            col = col.child(
                div()
                    .id(SharedString::from(format!("grp-contact-{}", a.id)))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(32.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .hover(|s| s.bg(rgb(p.raised)))
                    .child(Icon::new(IconName::UserRound).size(px(16.)))
                    .child(div().flex_1().min_w_0().truncate().child(a.contact_name.clone()))
                    .child(dot(if online { p.success } else { p.faint }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(did) = did {
                            this.selected_device_id = Some(did);
                            this.devices_filter = DevFilter::All;
                            this.go(Page::Devices, cx);
                        } else {
                            this.go(Page::Agenda, cx);
                        }
                    })),
            );
        }

        col = col.child(sec(
            "Compartidos",
            Some(
                icon_btn_sm("grp-new", IconName::FolderPlus, "Nuevo grupo", false, p).on_click(cx.listener(
                    |this, _, window, cx| {
                        if this.auth.is_none() {
                            this.login_open = true;
                            cx.notify();
                        } else {
                            this.open_create_group_modal(window, cx);
                        }
                    },
                )),
            ),
        ));
        if shared.is_empty() {
            col = col.child(
                div()
                    .px(px(10.))
                    .py_1()
                    .text_xs()
                    .text_color(rgb(p.faint))
                    .child("Nadie compartió grupos con vos."),
            );
        }
        for g in shared {
            let r = group_row(self, g, IconName::Users, cx);
            col = col.child(r);
        }
        div().h_full().child(col)
    }

    pub fn render_devices_v2(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let groups_col = self.render_groups_column(cx);

        let content: AnyElement = if self.page == Page::Agenda {
            div()
                .id("agenda-scroll")
                .flex_1()
                .min_w_0()
                .h_full()
                .px(px(20.))
                .py_2()
                .overflow_y_scrollbar()
                .child(self.render_agenda(cx))
                .into_any_element()
        } else if self.auth.is_none() && self.devices_filter != DevFilter::Recent {
            div()
                .flex_1()
                .min_w_0()
                .px(px(20.))
                .py_2()
                .child(self.auth_required_panel("Dispositivos", p, cx))
                .into_any_element()
        } else {
            self.render_device_list(cx).into_any_element()
        };

        let drawer = if self.page == Page::Devices {
            self.render_device_drawer(cx)
        } else {
            None
        };

        div()
            .flex()
            .size_full()
            .min_h_0()
            .child(groups_col)
            .child(content)
            .when_some(drawer, |d, dr| d.child(dr))
    }

    fn render_device_list(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let now = now_unix();
        let filter = self.devices_filter;
        let compact = self.selected_device_id.is_some_and(|sid| self.devices.iter().any(|d| d.id == sid));

        let mut list = div().flex().flex_col().flex_1().min_w_0().h_full();

        // Barra de herramientas
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px(px(20.))
                .pt_1()
                .pb(px(10.))
                .child(div().text_sm().font_semibold().child(match filter {
                    DevFilter::All => "Todos los equipos".to_string(),
                    DevFilter::Favorites => "Favoritos".to_string(),
                    DevFilter::Recent => "Conexiones recientes".to_string(),
                    DevFilter::Group(gid) => self
                        .groups
                        .iter()
                        .find(|g| g.id == gid)
                        .map(|g| g.name.clone())
                        .unwrap_or_else(|| "Grupo".into()),
                }))
                .child(div().flex_1())
                .when(self.auth.is_some(), |d| {
                    d.child(
                        icon_btn_sm("dev-assign", IconName::Hash, "Asignar equipo por ID o email", self.show_add_device_form, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_add_device_form = !this.show_add_device_form;
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_btn_sm("dev-enroll", IconName::KeyRound, "Generar token de enrolamiento", false, p)
                            .on_click(cx.listener(|this, _, _, cx| this.create_enrollment(cx))),
                    )
                    .child(
                        icon_btn_sm("dev-refresh", IconName::RefreshCw, "Actualizar", false, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_devices(cx);
                                this.load_agenda_groups(cx);
                            })),
                    )
                }),
        );

        if !self.enroll_cmd_out.is_empty() {
            list = list.child(
                div().px(px(20.)).pb_3().child(
                    card(p)
                        .flex()
                        .items_center()
                        .gap_3()
                        .p_3()
                        .child(Icon::new(IconName::KeyRound).size(px(16.)).text_color(rgb(p.accent)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(MONO)
                                .text_xs()
                                .text_color(rgb(p.text))
                                .child(self.enroll_cmd_out.clone()),
                        )
                        .child(
                            icon_btn_sm("copy-enroll-cmd", IconName::Copy, "Copiar comando de enrolamiento", false, p)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(this.enroll_cmd_out.clone()));
                                    this.server_status = "Comando de enrolamiento copiado".into();
                                    cx.notify();
                                })),
                        ),
                ),
            );
        }

        if self.show_add_device_form && self.auth.is_some() {
            let group_idx = self.assign_group_idx;
            let gname = if group_idx == 0 {
                "Sin grupo".to_string()
            } else {
                self.groups.get(group_idx - 1).map(|g| g.name.clone()).unwrap_or_else(|| "?".into())
            };
            list = list.child(
                div().px(px(20.)).pb_3().child(
                    card(p)
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .child(label_row(IconName::Hash, "Registrar o asignar un equipo por ID de conexión o email", p))
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
                                .gap_2()
                                .child(
                                    ghost_btn("cycle-assign-grp", Some(IconName::Folder), format!("Grupo: {gname}"), p)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let total = this.groups.len() + 1;
                                            this.assign_group_idx = (this.assign_group_idx + 1) % total;
                                            cx.notify();
                                        })),
                                )
                                .child(div().flex_1())
                                .child(
                                    primary_btn("assign-dev-go", Some(IconName::Check), "Asignar", p)
                                        .on_click(cx.listener(|this, _, window, cx| this.assign_device(window, cx))),
                                ),
                        ),
                ),
            );
        }

        // Cabecera de tabla
        let head = |t: &'static str, w: Option<f32>| {
            let d = div().text_xs().font_medium().text_color(rgb(p.faint)).child(t);
            match w {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1().min_w_0(),
            }
        };
        let mut table = div()
            .id("dev-table")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(20.))
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(head("Equipo", None))
                    .child(head("ID", Some(104.)))
                    .child(head("Estado", Some(104.)))
                    .when(!compact, |d| d.child(head("Grupo", Some(110.))).child(head("CPU", Some(76.))))
                    .child(head("", Some(122.))),
            );

        if filter == DevFilter::Recent {
            let history = self.cfg.connection_history.clone();
            if history.is_empty() {
                table = table.child(
                    div().px(px(20.)).py_4().text_sm().text_color(rgb(p.muted)).child("Sin conexiones recientes."),
                );
            }
            for (idx, h) in history.into_iter().enumerate() {
                let norm = normalize_conn_code(&h.connection_id);
                let matched = self.devices.iter().find(|d| normalize_conn_code(&d.public_code) == norm).cloned();
                let did_m = matched.as_ref().map(|d| d.id);
                let aid_m = did_m.and_then(|did| self.agenda.iter().find(|a| a.device_id == Some(did)).map(|a| a.id));
                let online = matched.as_ref().is_some_and(|d| d.status.to_lowercase().contains("online"));
                let trust = h.is_trust_valid(now);
                let gname = SharedString::from(format!("rrow-{idx}"));
                let host_ctx = h.hostname.clone();
                let code_ctx = h.connection_id.clone();
                let host_ctx2 = host_ctx.clone();
                let code_ctx2 = code_ctx.clone();
                let cid_v = h.connection_id.clone();
                table = table.child(
                    div()
                        .id(SharedString::from(format!("rc-row-{idx}")))
                        .group(gname.clone())
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(52.))
                        .px(px(20.))
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
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(os_tile(IconName::RotateCcwClock, 30., false, p))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .child(div().text_sm().font_semibold().truncate().child(h.hostname.clone()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(p.muted))
                                                .child(fmt_elapsed(now - h.last_connected_unix)),
                                        ),
                                ),
                        )
                        .child(div().w(px(104.)).flex_none().font_family(MONO).text_sm().child(group_id(&h.connection_id)))
                        .child(
                            div()
                                .w(px(104.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_sm()
                                .child(dot(if online { p.success } else { p.faint }))
                                .child(if online { "En línea" } else { "Desconocido" }),
                        )
                        .when(!compact, |r| {
                            r.child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .child(tag(if trust { "Reconexión < 1 h" } else { "Pedirá contraseña" }, p)),
                            )
                            .child(div().w(px(76.)).flex_none().text_color(rgb(p.faint)).child("—"))
                        })
                        .child(
                            div()
                                .w(px(122.))
                                .flex_none()
                                .flex()
                                .justify_end()
                                .gap(px(2.))
                                .opacity(0.)
                                .group_hover(gname, |s| s.opacity(1.))
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rc-ctl-{idx}")), IconName::MousePointer2, "Controlar", false, p)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.reconnect_from_history(idx, Some(window), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rc-view-{idx}")), IconName::Eye, "Solo ver", false, p)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.initiate_connect_by_id(cid_v.clone(), false, Some(window), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("rc-menu-{idx}")), IconName::Ellipsis, "Más: propiedades, agenda, quitar", false, p)
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
            return list.child(table);
        }

        let fav_ids: Vec<Uuid> = self.agenda.iter().filter(|a| a.is_favorite).filter_map(|a| a.device_id).collect();
        let devs: Vec<_> = self
            .devices
            .iter()
            .filter(|d| match filter {
                DevFilter::All | DevFilter::Recent => true,
                DevFilter::Favorites => fav_ids.contains(&d.id),
                DevFilter::Group(gid) => d.group_id == Some(gid),
            })
            .cloned()
            .collect();
        if devs.is_empty() {
            table = table.child(
                div()
                    .px(px(20.))
                    .py_4()
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(match filter {
                        DevFilter::Favorites => "Marcá equipos como favoritos desde su menú para verlos acá.",
                        DevFilter::Group(_) => "Este grupo todavía no tiene equipos.",
                        _ => "Todavía no hay equipos. Usá + para agregar uno o conectate por ID.",
                    }),
            );
        }
        let rmm_dev = self.devices.get(self.rmm_device_idx).map(|d| d.id);
        let cpu_now = self
            .rmm_hw_snapshot
            .as_ref()
            .and_then(|hw| hw.get("cpu"))
            .and_then(|c| c.get("load_pct"))
            .and_then(|v| v.as_f64());
        for d in devs {
            let did = d.id;
            let online = d.status.to_lowercase().contains("online");
            let is_sel = self.selected_device_id == Some(did);
            let ag = self.agenda.iter().find(|a| a.device_id == Some(did)).cloned();
            let aid = ag.as_ref().map(|a| a.id);
            let label = ag.as_ref().map(|a| a.alias.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| d.hostname.clone());
            let owner = d
                .owner_email
                .clone()
                .filter(|e| !e.trim().is_empty())
                .or_else(|| ag.as_ref().map(|a| a.contact_name.clone()).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| format!("{} · {}", d.os_name, d.os_arch));
            let gname_tag = d
                .group_id
                .and_then(|g| self.groups.iter().find(|x| x.id == g))
                .map(|g| g.name.clone());
            let seen = if online {
                "En línea".to_string()
            } else {
                let secs = (chrono::Utc::now() - d.last_heartbeat_at).num_seconds();
                fmt_elapsed(secs).replacen("hace", "Hace", 1)
            };
            let cpu = if rmm_dev == Some(did) && online { cpu_now } else { None };
            let rowg = SharedString::from(format!("drow-{did}"));
            let (h1, h2, h3, h4, h5) = (label.clone(), label.clone(), label.clone(), label.clone(), label.clone());
            let code1 = d.public_code.clone();
            let code2 = d.public_code.clone();
            table = table.child(
                div()
                    .id(SharedString::from(format!("row-dev-{did}")))
                    .group(rowg.clone())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(52.))
                    .px(px(20.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .cursor_pointer()
                    .when(is_sel, |s| s.bg(rgb(p.active)))
                    .when(!is_sel, |s| s.hover(|s| s.bg(rgb(p.raised))))
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
                                agenda_id: aid,
                                history_idx: None,
                                hostname: h1.clone(),
                                connection_id: code1.clone(),
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
                            .gap(px(10.))
                            .child(os_tile(device_icon(&d.icon_kind), 30., false, p))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(div().text_sm().font_semibold().truncate().child(label.clone()))
                                    .child(div().text_xs().text_color(rgb(p.muted)).truncate().child(owner)),
                            ),
                    )
                    .child(div().w(px(104.)).flex_none().font_family(MONO).text_sm().child(group_id(&d.public_code)))
                    .child(
                        div()
                            .w(px(104.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_sm()
                            .child(dot(if online { p.success } else { p.faint }))
                            .child(seen),
                    )
                    .when(!compact, |r| r.child(
                        div()
                            .w(px(110.))
                            .flex_none()
                            .min_w_0()
                            .when_some(gname_tag, |x, g| x.child(tag(g, p)))
                            .when(d.group_id.is_none(), |x| x.child(div().text_color(rgb(p.faint)).child("—"))),
                    ).child(
                        div()
                            .w(px(76.))
                            .flex_none()
                            .when_some(cpu, |x, c| x.child(mini_bar(c as f32, p)))
                            .when(cpu.is_none(), |x| x.child(div().text_color(rgb(p.faint)).child("—"))),
                    ))
                    .child(
                        div()
                            .w(px(122.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .gap(px(2.))
                            .when(!is_sel, |x| x.opacity(0.).group_hover(rowg, |s| s.opacity(1.)))
                            .when(online, |x| {
                                x.child(
                                    icon_btn_sm(SharedString::from(format!("dev-ctrl-{did}")), IconName::MousePointer2, "Controlar", false, p)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.start_managed_session(did, None, true, h2.clone(), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("dev-view-{did}")), IconName::Eye, "Solo ver", false, p)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.start_managed_session(did, None, false, h3.clone(), cx);
                                        })),
                                )
                                .child(
                                    icon_btn_sm(SharedString::from(format!("dev-files-{did}")), IconName::FolderUp, "Archivos", false, p)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.start_managed_session_with_panel(did, None, true, h4.clone(), Some("files"), cx);
                                        })),
                                )
                            })
                            .when(!online, |x| {
                                x.child(
                                    icon_btn_sm(SharedString::from(format!("dev-wol-{did}")), IconName::Power, "Encender (Wake-on-LAN)", false, p)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trigger_wol(did, h5.clone(), cx);
                                        })),
                                )
                            })
                            .child(
                                icon_btn_sm(
                                    SharedString::from(format!("dev-menu-{did}")),
                                    IconName::Ellipsis,
                                    "Más: propiedades, mover, compartir, eliminar",
                                    false,
                                    p,
                                )
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.selected_device_id = Some(did);
                                    this.grp_ctx_menu = None;
                                    this.dev_ctx_menu = Some(crate::DeviceContextMenu {
                                        pos: ev.position(),
                                        device_id: Some(did),
                                        agenda_id: aid,
                                        history_idx: None,
                                        hostname: label.clone(),
                                        connection_id: code2.clone(),
                                        submenu: None,
                                    });
                                    cx.notify();
                                })),
                            ),
                    ),
            );
        }
        list.child(table)
    }

    fn render_device_drawer(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let p = self.palette();
        let d = self
            .selected_device_id
            .and_then(|sid| self.devices.iter().find(|d| d.id == sid).cloned())?;
        let did = d.id;
        let online = d.status.to_lowercase().contains("online");
        let ag = self.agenda.iter().find(|a| a.device_id == Some(did)).cloned();
        let aid = ag.as_ref().map(|a| a.id);
        let label = ag.as_ref().map(|a| a.alias.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| d.hostname.clone());
        let tab = self.drawer_tab;
        let rmm_is_this = self.devices.get(self.rmm_device_idx).map(|x| x.id) == Some(did);

        let tabs: [(&'static str, IconName, &'static str); 8] = [
            ("general", IconName::Info, "General"),
            ("hardware", IconName::Cpu, "Hardware"),
            ("metrics", IconName::Activity, "Rendimiento"),
            ("inventory", IconName::Package, "Software"),
            ("services", IconName::Cog, "Servicios"),
            ("terminal", IconName::SquareTerminal, "Terminal"),
            ("history", IconName::RotateCcwClock, "Historial y eventos"),
            ("ai", IconName::Sparkles, "Diagnóstico IA"),
        ];

        let mut body = div()
            .id("drawer-body")
            .flex()
            .flex_col()
            .gap(px(14.))
            .flex_1()
            .min_h_0()
            .px(px(18.))
            .py(px(16.))
            .overflow_y_scrollbar();

        let kv = |k: &'static str, v: String| {
            div()
                .flex()
                .gap(px(10.))
                .text_xs()
                .child(div().w(px(100.)).flex_none().text_color(rgb(p.muted)).child(k))
                .child(div().flex_1().min_w_0().text_color(rgb(p.text)).child(v))
        };

        match tab {
            "general" => {
                body = body
                    .child(kv("Sistema", format!("{} ({})", d.os_name, d.os_arch)))
                    .child(kv("Tipo", d.kind.clone()))
                    .child(kv(
                        "Propietario",
                        d.owner_email.clone().filter(|e| !e.is_empty()).unwrap_or_else(|| "Sin asignar".into()),
                    ))
                    .child(kv("IP local", if d.lan_ips.is_empty() { "—".into() } else { d.lan_ips.join(", ") }))
                    .child(kv("MAC", if d.mac_addresses.is_empty() { "—".into() } else { d.mac_addresses.join(", ") }))
                    .child(kv(
                        "Grupo",
                        d.group_id
                            .and_then(|g| self.groups.iter().find(|x| x.id == g))
                            .map(|g| g.name.clone())
                            .unwrap_or_else(|| "Sin grupo".into()),
                    ))
                    .child(kv("Pantalla", format!("{}×{}", d.screen_width, d.screen_height)))
                    .child(kv("Agente", d.agent_version.clone()))
                    .child(kv("Acceso fácil", if d.has_personal_password { "Sí".into() } else { "No".into() }))
                    .when_some(ag.clone(), |b, a| {
                        b.when(!a.client_company.is_empty(), |b| b.child(kv("Empresa", a.client_company.clone())))
                            .when(!a.notes.is_empty(), |b| b.child(kv("Notas", a.notes.clone())))
                    });
                if rmm_is_this {
                    if let Some(hw) = self.rmm_hw_snapshot.clone() {
                        let f = |k: &str| hw.get(k).and_then(|x| x.get("load_pct")).and_then(|v| v.as_f64());
                        if let Some(c) = f("cpu") {
                            body = body.child(meter("CPU", c as f32, p));
                        }
                        if let Some(r) = f("ram") {
                            body = body.child(meter("RAM", r as f32, p));
                        }
                    }
                }
            }
            "hardware" | "metrics" | "inventory" | "services" => {
                let kind = match tab {
                    "hardware" => "resources",
                    other => other,
                };
                body = body.child(
                    ghost_btn(
                        SharedString::from(format!("drawer-run-{tab}")),
                        Some(IconName::RefreshCw),
                        if online { "Consultar ahora" } else { "Equipo desconectado" },
                        p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                            this.rmm_device_idx = idx;
                        }
                        this.run_rmm_diagnostic(did, kind, cx);
                    })),
                );
                if rmm_is_this {
                    if tab == "hardware" {
                        if let Some(hw) = self.rmm_hw_snapshot.clone() {
                            body = body.child(render_hardware_resources(&hw, p));
                        }
                    } else if !self.rmm_diag_output.is_empty() {
                        body = body.child(
                            div()
                                .p_2()
                                .rounded(px(8.))
                                .bg(rgb(p.raised))
                                .font_family(MONO)
                                .text_xs()
                                .child(self.rmm_diag_output.clone()),
                        );
                    }
                }
            }
            "terminal" => {
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                seg(p)
                                    .child(
                                        seg_btn("drawer-shell-ps", IconName::SquareTerminal, "PowerShell", self.rmm_shell == "powershell", p)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.rmm_shell = "powershell".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        seg_btn("drawer-shell-cmd", IconName::Hash, "CMD", self.rmm_shell == "cmd", p)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.rmm_shell = "cmd".into();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(div().text_xs().text_color(rgb(p.muted)).child("Comandos auditados")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.rmm_command_in).flex_1().font_family(MONO))
                            .child(
                                icon_btn("drawer-run-cmd", IconName::CornerDownLeft, "Ejecutar", false, p).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                                            this.rmm_device_idx = idx;
                                        }
                                        this.run_rmm_command(did, cx);
                                    },
                                )),
                            ),
                    )
                    .when(rmm_is_this && !self.rmm_cmd_output.is_empty(), |b| {
                        b.child(
                            div()
                                .p_2()
                                .rounded(px(8.))
                                .bg(rgb(0x0E1117))
                                .text_color(rgb(0xD4D4D4))
                                .font_family(MONO)
                                .text_xs()
                                .child(self.rmm_cmd_output.clone()),
                        )
                    });
            }
            "history" => {
                let norm = normalize_conn_code(&d.public_code);
                let now = now_unix();
                let hist: Vec<_> = self
                    .cfg
                    .connection_history
                    .iter()
                    .filter(|h| normalize_conn_code(&h.connection_id) == norm)
                    .cloned()
                    .collect();
                if hist.is_empty() {
                    body = body.child(div().text_xs().text_color(rgb(p.muted)).child("Sin conexiones desde este equipo."));
                }
                for h in hist {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .child(Icon::new(IconName::MousePointer2).size(px(14.)).text_color(rgb(p.accent)))
                            .child(div().flex_1().child("Sesión de control"))
                            .child(div().text_color(rgb(p.faint)).child(fmt_elapsed(now - h.last_connected_unix))),
                    );
                }
                body = body.child(
                    ghost_btn("drawer-events", Some(IconName::FileText), "Eventos del sistema", p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                                this.rmm_device_idx = idx;
                            }
                            this.run_rmm_diagnostic(did, "events", cx);
                        },
                    )),
                );
                if rmm_is_this && !self.rmm_diag_output.is_empty() {
                    body = body.child(
                        div()
                            .p_2()
                            .rounded(px(8.))
                            .bg(rgb(p.raised))
                            .font_family(MONO)
                            .text_xs()
                            .child(self.rmm_diag_output.clone()),
                    );
                }
            }
            _ => {
                body = body
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.muted))
                            .child("Abrí el copiloto con este equipo seleccionado para diagnosticar problemas con datos redactados."),
                    )
                    .child(
                        primary_btn("drawer-ai", Some(IconName::Sparkles), "Diagnosticar con IA", p).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(idx) = this.devices.iter().position(|x| x.id == did) {
                                    this.ai_device_idx = idx + 1;
                                }
                                this.show_ai = true;
                                this.show_notifications = false;
                                if this.auth.is_some() {
                                    this.load_ai_settings(cx);
                                    this.load_ai_chat_sessions(cx);
                                }
                                cx.notify();
                            },
                        )),
                    );
            }
        }

        let (l1, l2, l3, l4, l5) = (label.clone(), label.clone(), label.clone(), label.clone(), label.clone());
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(320.))
                .h_full()
                .border_l_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.canvas))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px(px(18.))
                        .pt(px(16.))
                        .pb(px(12.))
                        .child(os_tile(device_icon(&d.icon_kind), 42., true, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().font_semibold().truncate().child(label.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child(dot(if online { p.success } else { p.faint }))
                                        .child(div().font_family(MONO).child(group_id(&d.public_code))),
                                ),
                        )
                        .child(
                            icon_btn_sm("drawer-edit", IconName::Pencil, "Editar propiedades", false, p).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.start_edit_device(Some(did), aid, l1.clone(), window, cx);
                                },
                            )),
                        )
                        .child(
                            icon_btn_sm("drawer-close", IconName::X, "Cerrar panel", false, p).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.selected_device_id = None;
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(2.))
                        .px(px(12.))
                        .pb(px(10.))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .children(tabs.into_iter().map(|(key, icon, text)| {
                            icon_btn(SharedString::from(format!("dtab-{key}")), icon, text, tab == key, p)
                                .size(px(32.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.drawer_tab = key;
                                    cx.notify();
                                }))
                        })),
                )
                .child(body)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px(px(18.))
                        .py(px(14.))
                        .border_t_1()
                        .border_color(rgb(p.border))
                        .when(online, |b| {
                            b.child(
                                primary_btn("drawer-conn", Some(IconName::MousePointer2), "Conectar", p)
                                    .flex_1()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_managed_session(did, None, true, l2.clone(), cx);
                                    })),
                            )
                            .child(
                                icon_btn("drawer-view", IconName::Eye, "Solo ver", false, p).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.start_managed_session(did, None, false, l3.clone(), cx);
                                    },
                                )),
                            )
                            .child(
                                icon_btn("drawer-files", IconName::FolderUp, "Archivos", false, p).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.start_managed_session_with_panel(did, None, true, l4.clone(), Some("files"), cx);
                                    },
                                )),
                            )
                            .child(
                                icon_btn("drawer-term", IconName::SquareTerminal, "Terminal", tab == "terminal", p).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.drawer_tab = "terminal";
                                        cx.notify();
                                    }),
                                ),
                            )
                        })
                        .when(!online, |b| {
                            b.child(
                                primary_btn("drawer-wol", Some(IconName::Power), "Encender", p)
                                    .flex_1()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.trigger_wol(did, l5.clone(), cx);
                                    })),
                            )
                        }),
                ),
        )
    }
}
