#![cfg_attr(windows, windows_subsystem = "windows")]

mod host;
#[path = "../../desktop-gpui/src/session_panel.rs"]
mod session_panel;

use agentdesk_protocol::{CreateSupportRequest, CreateSupportResponse};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use host::{start_embedded_host, HostEvent};
use session_panel::{FloatingPanelLaunch, FloatingSessionPanel, PanelEvent};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Copy)]
struct Palette {
    canvas: u32,
    panel: u32,
    raised: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    success: u32,
    danger: u32,
    header_bg: u32,
    header_btn: u32,
}

const LIGHT: Palette = Palette {
    canvas: 0xFFFFFF,
    panel: 0xF7F9FC,
    raised: 0xEEF2F7,
    border: 0xE2E8F0,
    text: 0x111827,
    muted: 0x64748B,
    accent: 0x1D4ED8,
    success: 0x16A34A,
    danger: 0xDC2626,
    header_bg: 0x0E2A8A,
    header_btn: 0x1E40AF,
};

const DARK: Palette = Palette {
    canvas: 0x0E131D,
    panel: 0x151B28,
    raised: 0x1D2536,
    border: 0x283247,
    text: 0xEDF2F7,
    muted: 0x94A3B8,
    accent: 0x3B82F6,
    success: 0x22B983,
    danger: 0xEF4444,
    header_bg: 0x0A1638,
    header_btn: 0x1D4ED8,
};

fn load_default_server_and_theme() -> (String, bool) {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local).join("AgentDesk").join("desktop_config.json");
        if let Ok(raw) = std::fs::read_to_string(p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}')) {
                let url = v
                    .get("server_url")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("https://agentdesk.dolan.com.ar")
                    .to_string();
                let dark = v.get("theme").and_then(|x| x.as_str()) != Some("light");
                return (url, dark);
            }
        }
    }
    ("https://agentdesk.dolan.com.ar".into(), false)
}

fn generate_password() -> String {
    const CHARSET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

fn apply_theme(cx: &mut App, mode: ThemeMode) {
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = "Inter".into();
        theme.font_size = px(14.);
    });
}

fn load_brand_logo_image(png_bytes: &[u8]) -> Arc<RenderImage> {
    let mut img_buf = image::load_from_memory(png_bytes)
        .expect("logo AgentDesk válido")
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    for px in img_buf.pixels_mut() {
        px.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
}

#[cfg(windows)]
fn apply_win32_window_branding(window: &mut Window, title: &str) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, SetClassLongPtrW, SetWindowTextW, GCLP_HICON, GCLP_HICONSM,
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, WM_SETICON,
    };
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return None;
    };
    let hwnd_val = h.hwnd.get() as isize;
    let hwnd = HWND(hwnd_val as *mut _);
    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(title_wide.as_ptr()));
        if let Ok(hmod) = GetModuleHandleW(None) {
            if let Ok(hbig) = LoadImageW(hmod, PCWSTR(1 as *const u16), IMAGE_ICON, 32, 32, LR_DEFAULTCOLOR) {
                let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(hbig.0 as isize));
                let _ = SetClassLongPtrW(hwnd, GCLP_HICON, hbig.0 as isize);
            }
            if let Ok(hsmall) = LoadImageW(hmod, PCWSTR(1 as *const u16), IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR) {
                let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(hsmall.0 as isize));
                let _ = SetClassLongPtrW(hwnd, GCLP_HICONSM, hsmall.0 as isize);
            }
        }
    }
    Some(hwnd_val)
}

#[cfg(windows)]
fn spawn_quicksupport_tray(window_handle: isize) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostMessageW, SetForegroundWindow, ShowWindow,
        TranslateMessage, MSG, SW_RESTORE, WM_CLOSE,
    };
    let rgba = image::load_from_memory(include_bytes!("../../../logos/agentdesk-icono.png"))
        .expect("logo AgentDesk válido")
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
        .into_raw();

    std::thread::spawn(move || {
        let menu = Menu::new();
        let item_open = MenuItem::new("Abrir AgentDesk QuickSupport", true, None);
        let item_quit = MenuItem::new("Salir", true, None);
        let _ = menu.append(&item_open);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_quit);
        let id_open = item_open.id().clone();
        let id_quit = item_quit.id().clone();

        let Ok(icon) = tray_icon::Icon::from_rgba(rgba, 32, 32) else {
            return;
        };
        let Ok(_tray) = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("AgentDesk QuickSupport (azul)")
            .with_icon(icon)
            .build()
        else {
            return;
        };

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let hwnd = HWND(window_handle as *mut _);
            if event.id == id_quit {
                unsafe { let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)); }
            } else if event.id == id_open {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }));

        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(
                event,
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                    | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
            ) {
                let hwnd = HWND(window_handle as *mut _);
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }));

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(windows)]
fn focus_existing_window(title: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn acquire_single_instance() -> Result<Option<windows::Win32::Foundation::HANDLE>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "Local\\AgentDesk.QuickSupport.Instance"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
        .map_err(|e| format!("CreateMutexW error: {e}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { let _ = CloseHandle(handle); }
        focus_existing_window("AgentDesk QuickSupport");
        return Ok(None);
    }
    Ok(Some(handle))
}

#[derive(Clone)]
struct IncomingSession {
    caller: String,
    allow_control: bool,
    stop_tx: watch::Sender<bool>,
}

struct QuickSupportApp {
    dark: bool,
    brand_logo: Arc<RenderImage>,
    main_hwnd: Option<isize>,
    incoming_panel_tx: Option<Sender<PanelEvent>>,
    show_server_bar: bool,
    server_url_in: Entity<InputState>,
    req_name_in: Entity<InputState>,
    req_contact_in: Entity<InputState>,
    req_desc_in: Entity<InputState>,

    host_stop: Option<watch::Sender<bool>>,
    host_rx: Receiver<HostEvent>,
    host_tx: Sender<HostEvent>,
    device_id: Option<Uuid>,
    device_token: Option<String>,
    connection_id: String,
    password: String,
    show_password: bool,
    status_text: String,
    feedback_text: String,
    incoming: Option<IncomingSession>,

    ticket_code: Option<String>,
    ticket_status: String,
    sending_ticket: bool,
}

impl QuickSupportApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (srv, dark) = load_default_server_and_theme();
        apply_theme(cx, if dark { ThemeMode::Dark } else { ThemeMode::Light });

        let host_name = hostname::get()
            .ok()
            .and_then(|s| s.into_string().ok())
            .unwrap_or_else(|| "Usuario Windows".into());

        let server_url_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://agentdesk.dolan.com.ar")
                .default_value(srv)
        });
        let req_name_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Tu nombre")
                .default_value(host_name)
        });
        let req_contact_in = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Email o teléfono de contacto")
        });
        let req_desc_in = cx.new(|cx| {
            InputState::new(window, cx).placeholder("¿En qué podemos ayudarte hoy?")
        });

        let (host_tx, host_rx) = mpsc::channel();
        let mut app = Self {
            dark,
            brand_logo: load_brand_logo_image(include_bytes!("../../../logos/agentdesk-icono.png")),
            main_hwnd: None,
            incoming_panel_tx: None,
            show_server_bar: false,
            server_url_in,
            req_name_in,
            req_contact_in,
            req_desc_in,
            host_stop: None,
            host_rx,
            host_tx,
            device_id: None,
            device_token: None,
            connection_id: "Asignando…".into(),
            password: generate_password(),
            show_password: true,
            status_text: "Conectando con AgentDesk Server…".into(),
            feedback_text: String::new(),
            incoming: None,
            ticket_code: None,
            ticket_status: String::new(),
            sending_ticket: false,
        };
        app.restart_host(cx);

        cx.subscribe(&app.req_desc_in, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) && !this.sending_ticket {
                this.submit_support_ticket(cx);
            }
        })
        .detach();
        cx.subscribe(&app.server_url_in, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.restart_host(cx);
                cx.notify();
            }
        })
        .detach();

        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep = cx
                    .update(|_, cx| {
                        let Some(ent) = weak.upgrade() else { return false };
                        ent.update(cx, |this, cx| this.poll_host(cx));
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            })
            .detach();

        app
    }

    fn palette(&self) -> Palette {
        if self.dark { DARK } else { LIGHT }
    }

    fn base_url(&self, cx: &App) -> String {
        let raw = self.server_url_in.read(cx).value().trim().trim_end_matches('/').to_string();
        if raw.is_empty() {
            "https://agentdesk.dolan.com.ar".into()
        } else if raw.starts_with("http://") || raw.starts_with("https://") {
            raw
        } else {
            format!("http://{raw}")
        }
    }

    fn restart_host(&mut self, cx: &mut Context<Self>) {
        if let Some(stop) = self.host_stop.take() {
            let _ = stop.send(true);
        }
        let srv = self.base_url(cx);
        let pw = self.password.clone();
        self.host_stop = Some(start_embedded_host(srv, pw, None, self.host_tx.clone()));
    }

    fn poll_host(&mut self, cx: &mut Context<Self>) {
        let mut dirty = false;
        while let Ok(ev) = self.host_rx.try_recv() {
            dirty = true;
            match ev {
                HostEvent::Status(s) => self.status_text = s,
                HostEvent::Registered {
                    device_id,
                    connection_id,
                    device_token,
                    password,
                } => {
                    self.device_id = Some(device_id);
                    self.device_token = Some(device_token);
                    self.connection_id = connection_id;
                    self.password = password;
                    self.status_text = "Listo para recibir soporte (conexión segura)".into();
                }
                HostEvent::PasswordRotated(npw) => {
                    self.password = npw;
                    self.feedback_text = "Contraseña temporal rotada".into();
                }
                HostEvent::IncomingStarted {
                    caller_name,
                    allow_control,
                    stop_tx,
                    chat_tx,
                } => {
                    self.incoming = Some(IncomingSession {
                        caller: caller_name.clone(),
                        allow_control,
                        stop_tx: stop_tx.clone(),
                    });
                    self.status_text = format!("Técnico conectado: {caller_name}");
                    let (ptx, prx) = mpsc::channel();
                    self.incoming_panel_tx = Some(ptx);
                    FloatingSessionPanel::open(
                        FloatingPanelLaunch {
                            caller_name,
                            allow_control,
                            connection_id: self.connection_id.clone(),
                            dark: self.dark,
                            logo: self.brand_logo.clone(),
                            stop_tx: stop_tx.clone(),
                            stop_rx: stop_tx.subscribe(),
                            chat_tx,
                            incoming_rx: prx,
                        },
                        cx,
                    );
                    #[cfg(windows)]
                    if let Some(hwnd) = self.main_hwnd {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                        unsafe {
                            let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                        }
                    }
                }
                HostEvent::IncomingChat { sender, text } => {
                    if let Some(ptx) = &self.incoming_panel_tx {
                        let _ = ptx.send(PanelEvent::Chat { sender, text });
                    }
                }
                HostEvent::IncomingToast(msg) => {
                    if let Some(ptx) = &self.incoming_panel_tx {
                        let _ = ptx.send(PanelEvent::Toast(msg));
                    }
                }
                HostEvent::IncomingEnded => {
                    self.incoming = None;
                    if let Some(ptx) = self.incoming_panel_tx.take() {
                        let _ = ptx.send(PanelEvent::Close);
                    }
                    self.status_text = "Sesión finalizada — listo para soporte".into();
                    #[cfg(windows)]
                    if let Some(hwnd) = self.main_hwnd {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            SetForegroundWindow, ShowWindow, SW_RESTORE,
                        };
                        unsafe {
                            let _ = ShowWindow(HWND(hwnd as *mut _), SW_RESTORE);
                            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                        }
                    }
                }
            }
        }
        if dirty {
            cx.notify();
        }
    }

    fn rotate_password(&mut self, cx: &mut Context<Self>) {
        let (Some(did), Some(tok)) = (self.device_id, self.device_token.clone()) else {
            self.feedback_text = "Esperando registro del servidor…".into();
            cx.notify();
            return;
        };
        host::rotate_password_async(
            self.base_url(cx),
            did,
            tok,
            generate_password(),
            self.host_tx.clone(),
        );
    }

    fn submit_support_ticket(&mut self, cx: &mut Context<Self>) {
        let name = self.req_name_in.read(cx).value().trim().to_string();
        let contact = self.req_contact_in.read(cx).value().trim().to_string();
        let raw_desc = self.req_desc_in.read(cx).value().trim().to_string();
        if name.is_empty() || contact.is_empty() || raw_desc.is_empty() {
            self.ticket_status = "Completá tu nombre, contacto y descripción.".into();
            cx.notify();
            return;
        }
        let desc = format!(
            "{raw_desc}\n[QuickSupport ID: {} | Pass temporal: {}]",
            self.connection_id, self.password
        );
        let base = self.base_url(cx);
        self.sending_ticket = true;
        self.ticket_status = "Enviando solicitud a la cola…".into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/support/requests"))
                        .json(&CreateSupportRequest {
                            requester_name: name,
                            requester_contact: contact,
                            company_name: None,
                            category: Some("QuickSupport".into()),
                            description: desc,
                            consent_accepted: true,
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<CreateSupportResponse>().map_err(|e| e.to_string())
                    } else {
                        Err(format!("HTTP {}", r.status()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.sending_ticket = false;
                match res {
                    Ok(sr) => {
                        this.ticket_code = Some(sr.public_code.clone());
                        this.ticket_status = format!("Solicitud {} en cola — aguardando técnico", sr.public_code);
                    }
                    Err(e) => {
                        this.ticket_status = format!("Error enviando solicitud: {e}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for QuickSupportApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(p.canvas))
            .text_color(rgb(p.text))
            .child(
                // Header navy estilo TeamViewer QuickSupport
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(50.))
                    .px_3()
                    .bg(rgb(p.header_bg))
                    .text_color(rgb(0xFFFFFF))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                img(self.brand_logo.clone())
                                    .size(px(30.))
                                    .rounded_lg(),
                            )
                            .child(div().text_sm().font_bold().child("AgentDesk QuickSupport")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("qs-srv-toggle")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(34.))
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(p.header_btn)))
                                    .child(Icon::new(IconName::Settings).size(px(19.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_server_bar = !this.show_server_bar;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("qs-theme-toggle")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(34.))
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(p.header_btn)))
                                    .child(Icon::new(IconName::SunMoon).size(px(19.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dark = !this.dark;
                                        apply_theme(
                                            cx,
                                            if this.dark { ThemeMode::Dark } else { ThemeMode::Light },
                                        );
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("qs-scroll-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .gap_3()
                    .when(self.show_server_bar, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.panel))
                                .child(Input::new(&self.server_url_in).flex_1())
                                .child(
                                    div()
                                        .id("qs-srv-apply")
                                        .px_3()
                                        .py(px(6.))
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(rgb(p.accent))
                                        .text_color(rgb(0xFFFFFF))
                                        .text_xs()
                                        .font_semibold()
                                        .child("Conectar")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.restart_host(cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .when_some(self.incoming.clone(), |this, inc| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.danger))
                                .bg(rgb(p.panel))
                                .child(div().text_xs().font_semibold().text_color(rgb(p.danger)).child(format!(
                                    "Sesión activa: {} ({})",
                                    inc.caller,
                                    if inc.allow_control { "Control" } else { "Solo ver" }
                                )))
                                .child(
                                    div()
                                        .id("qs-cut-inc")
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(rgb(p.danger))
                                        .text_color(rgb(0xFFFFFF))
                                        .text_xs()
                                        .child("Desconectar")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let _ = inc.stop_tx.send(true);
                                            this.incoming = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        // Tarjeta ID y contraseña estilo TeamViewer QuickSupport
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.panel))
                            .child(div().text_sm().font_semibold().child("Permitir soporte remoto"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child("Compartí este ID y contraseña con tu técnico."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(p.canvas))
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("Tu ID"))
                                            .child(div().text_xl().font_bold().child(self.connection_id.clone())),
                                    )
                                    .child(
                                        div()
                                            .id("qs-copy-id")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(30.))
                                            .rounded_md()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgb(p.raised)))
                                            .child(Icon::new(IconName::Copy).size(px(16.)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    this.connection_id.clone(),
                                                ));
                                                this.feedback_text = "ID copiado".into();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(p.canvas))
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("Contraseña"))
                                            .child(div().text_lg().font_semibold().child(
                                                if self.show_password {
                                                    self.password.clone()
                                                } else {
                                                    "••••••••".into()
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .id("qs-eye")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(30.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(rgb(p.raised)))
                                                    .child(Icon::new(if self.show_password {
                                                        IconName::EyeOff
                                                    } else {
                                                        IconName::Eye
                                                    }).size(px(16.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.show_password = !this.show_password;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("qs-rot")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(30.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(rgb(p.raised)))
                                                    .child(Icon::new(IconName::RefreshCw).size(px(16.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.rotate_password(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("qs-copy-pw")
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .size(px(30.))
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(rgb(p.raised)))
                                                    .child(Icon::new(IconName::Copy).size(px(16.)))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                                            this.password.clone(),
                                                        ));
                                                        this.feedback_text = "Contraseña copiada".into();
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            )
                            .when(!self.feedback_text.is_empty(), |this| {
                                this.child(
                                    div().text_xs().text_color(rgb(p.accent)).child(self.feedback_text.clone()),
                                )
                            }),
                    )
                    .child(
                        // Solicitar soporte en cola (Ticket QuickSupport)
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.panel))
                            .child(div().text_sm().font_semibold().child("Solicitar asistencia a un técnico"))
                            .child(Input::new(&self.req_name_in))
                            .child(Input::new(&self.req_contact_in))
                            .child(Input::new(&self.req_desc_in))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .pt_1()
                                    .child(
                                        div()
                                            .id("qs-send-ticket")
                                            .px_4()
                                            .py_2()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .bg(rgb(p.accent))
                                            .text_color(rgb(0xFFFFFF))
                                            .text_xs()
                                            .font_semibold()
                                            .child(if self.sending_ticket {
                                                "Enviando…"
                                            } else {
                                                "Enviar solicitud a cola"
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if !this.sending_ticket {
                                                    this.submit_support_ticket(cx);
                                                }
                                            })),
                                    )
                                    .when_some(self.ticket_code.clone(), |this, code| {
                                        this.child(
                                            div().text_xs().font_bold().text_color(rgb(p.success)).child(code),
                                        )
                                    }),
                            )
                            .when(!self.ticket_status.is_empty(), |this| {
                                this.child(div().text_xs().text_color(rgb(p.muted)).child(self.ticket_status.clone()))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h_7()
                    .px_4()
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(div().size_2().rounded_full().bg(rgb(
                        if self.device_id.is_some() { p.success } else { p.muted },
                    )))
                    .child(self.status_text.clone()),
            )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SavedWindowBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    maximized: bool,
}

fn qs_bounds_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("AgentDesk").join("quicksupport_window.json")
}

fn restore_qs_window_bounds() -> WindowBounds {
    if let Ok(raw) = std::fs::read_to_string(qs_bounds_path()) {
        if let Ok(sb) = serde_json::from_str::<SavedWindowBounds>(raw.trim_start_matches('\u{feff}')) {
            if sb.width >= 340.0
                && sb.height >= 420.0
                && sb.x > -3000.0
                && sb.y > -3000.0
                && sb.x < 8000.0
                && sb.y < 5000.0
            {
                let b = Bounds {
                    origin: point(px(sb.x), px(sb.y)),
                    size: size(px(sb.width), px(sb.height)),
                };
                return if sb.maximized {
                    WindowBounds::Maximized(b)
                } else {
                    WindowBounds::Windowed(b)
                };
            }
        }
    }
    WindowBounds::Windowed(Bounds {
        origin: point(px(180.), px(90.)),
        size: size(px(390.), px(500.)),
    })
}

fn save_qs_window_bounds(window: &Window) {
    let (bounds, maximized) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    let x: f32 = bounds.origin.x.into();
    let y: f32 = bounds.origin.y.into();
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    if width < 220.0 || height < 220.0 || x <= -10000.0 || y <= -10000.0 {
        return;
    }
    let path = qs_bounds_path();
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let sb = SavedWindowBounds {
        x,
        y,
        width,
        height,
        maximized,
    };
    if let Ok(json) = serde_json::to_string_pretty(&sb) {
        let _ = std::fs::write(path, json);
    }
}

fn main() {
    #[cfg(windows)]
    let _guard = match acquire_single_instance() {
        Ok(Some(h)) => h,
        _ => return,
    };

    let init_bounds = restore_qs_window_bounds();

    application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.text_system()
                .add_fonts(vec![Cow::Borrowed(
                    include_bytes!("../../desktop/assets/fonts/Inter.ttf").as_slice(),
                )])
                .expect("no se pudo cargar la fuente Inter");
            let (_, dark) = load_default_server_and_theme();
            apply_theme(cx, if dark { ThemeMode::Dark } else { ThemeMode::Light });

            open_window(
                WindowOptions {
                    window_bounds: Some(init_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some("AgentDesk QuickSupport".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let app = cx.new(|cx| QuickSupportApp::new(window, cx));
                    #[cfg(windows)]
                    if let Some(hwnd) = apply_win32_window_branding(window, "AgentDesk QuickSupport") {
                        let _ = app.update(cx, |this, _| {
                            this.main_hwnd = Some(hwnd);
                        });
                        spawn_quicksupport_tray(hwnd);
                    }
                    window.on_window_should_close(cx, |window, _| {
                        save_qs_window_bounds(window);
                        true
                    });
                    app
                },
            )
            .expect("no se pudo abrir AgentDesk QuickSupport");
        });
}
