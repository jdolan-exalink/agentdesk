#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod host;
mod pages;
mod session_panel;
mod shell;
mod theme;
mod ui;
mod viewer;

use agentdesk_protocol::{
    AddressBookEntryDto, AiChatMessageDto, AiChatRequest, AiChatResponse,
    AiChatSessionSummaryDto, AiExecuteActionRequest, AiProposedActionDto, AiSettingsDto,
    AuditEventDto, AuthResponse, AuthUserInfo, CalendarEventDto, ChangePasswordRequest,
    ClientLookupResponse, CreateCalendarEventRequest, CreateEnrollmentResponse,
    CreateSessionRequest, CreateSessionResponse, CreateUserRequest, DeviceDto, DeviceGroupDto,
    DirectConnectRequest, DirectConnectResponse, DocPageDto, LoginRequest, MfaSetupResponse,
    MfaVerifyRequest, ResolveTicketRequest, RmmCommandDto, RunCommandRequest, RunDiagnosticRequest,
    SessionDto, SetPersonalPasswordRequest, SupportTicketDto, UpdateAiSettingsRequest,
    UpsertAddressBookRequest, UserDto,
};
use config::{normalize_conn_code, normalize_server_url, now_unix, DesktopConfig, SavedWindowBounds};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DeviceAlertDto {
    pub id: uuid::Uuid,
    pub severity: String,
    pub status: String,
    pub summary: String,
    pub current_value: f64,
    pub threshold: f64,
    pub unit: String,
}
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Sizable, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use host::{runtime, start_embedded_host, HostEvent};
use session_panel::{FloatingPanelLaunch, FloatingSessionPanel, PanelEvent};
use rand::Rng;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::{extract_resources, Palette, DARK, LIGHT};
use tokio::sync::watch;
use uuid::Uuid;
use viewer::{ReconnectTarget, RemoteSession, ViewerEndedNotice, ViewerLaunch};

fn startup_log(message: &str) {
    let mut log_dirs = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        log_dirs.push(
            std::path::PathBuf::from(local_app_data)
                .join("AgentDesk")
                .join("logs"),
        );
    }
    log_dirs.push(std::env::temp_dir().join("AgentDesk").join("logs"));
    for log_dir in log_dirs {
        if std::fs::create_dir_all(&log_dir).is_err() {
            continue;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("desktop-gpui-startup.log"))
        {
            use std::io::Write;
            let _ = writeln!(file, "{message}");
            return;
        }
    }
}

pub(crate) fn apply_theme(cx: &mut App, mode: ThemeMode) {
    Theme::change(mode, None, cx);
    let pal = if matches!(mode, ThemeMode::Dark) { DARK } else { LIGHT };
    Theme::update(cx, |theme| {
        theme.font_family = "Inter".into();
        theme.font_size = px(14.);
        theme.mono_font_family = ui::MONO.into();
        // Alinear los componentes (inputs, botones, foco) con los tokens de la maqueta.
        let c = &mut theme.colors;
        c.primary = rgb(pal.accent).into();
        c.primary_hover = rgb(pal.accent).into();
        c.primary_active = rgb(pal.accent).into();
        c.primary_foreground = rgb(0xFFFFFF).into();
        c.ring = rgb(pal.accent).into();
        c.caret = rgb(pal.accent).into();
        c.background = rgb(pal.canvas).into();
        c.foreground = rgb(pal.text).into();
        c.border = rgb(pal.border).into();
        c.input = rgb(pal.line2).into();
        c.muted = rgb(pal.raised).into();
        c.muted_foreground = rgb(pal.muted).into();
        c.popover = rgb(pal.panel).into();
        c.popover_foreground = rgb(pal.text).into();
        c.accent = rgb(pal.raised).into();
        c.accent_foreground = rgb(pal.text).into();
        c.selection = rgba((pal.accent << 8) | 0x40).into();
        c.success = rgb(pal.success).into();
        c.warning = rgb(pal.warning).into();
        c.danger = rgb(pal.danger).into();
    });
}

/// Icono del systray del cliente usando el logo oficial azul de AgentDesk (32x32).
fn build_client_tray_rgba() -> Vec<u8> {
    image::load_from_memory(include_bytes!("../../../logos/agentdesk-icono.png"))
        .expect("logo AgentDesk válido")
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
        .into_raw()
}

pub(crate) fn load_brand_logo_image(png_bytes: &[u8]) -> Arc<RenderImage> {
    let mut img_buf = image::load_from_memory(png_bytes)
        .expect("logo AgentDesk válido")
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    // GPUI espera orden de canales BGRA en RenderImage
    for px in img_buf.pixels_mut() {
        px.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
}

#[cfg(windows)]
pub(crate) fn apply_win32_window_branding(window: &mut Window, title: &str) -> Option<isize> {
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
fn spawn_client_tray(
    window_handle: isize,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    allow_quit: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostMessageW, SetForegroundWindow, ShowWindow,
        TranslateMessage, MSG, SW_RESTORE, WM_CLOSE,
    };

    std::thread::spawn(move || {
        let mut built = None;
        let mut last_err = String::new();
        for attempt in 0..5 {
            let menu = Menu::new();
            let item_open = MenuItem::new("Abrir AgentDesk", true, None);
            let item_quit = MenuItem::new("Salir completamente", true, None);
            let _ = menu.append(&item_open);
            let _ = menu.append(&PredefinedMenuItem::separator());
            let _ = menu.append(&item_quit);
            let id_open = item_open.id().clone();
            let id_quit = item_quit.id().clone();

            let Ok(icon) = tray_icon::Icon::from_rgba(build_client_tray_rgba(), 32, 32) else {
                startup_log("tray: icon creation failed");
                return;
            };
            match TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("AgentDesk — Cliente remoto")
                .with_icon(icon)
                .build()
            {
                Ok(tray) => {
                    built = Some((tray, id_open, id_quit));
                    break;
                }
                Err(error) => {
                    last_err = error.to_string();
                    if attempt < 4 {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
        let Some((_tray, id_open, id_quit)) = built else {
            startup_log(&format!("tray: registration failed after retries: {last_err}"));
            return;
        };
        ready.store(true, Ordering::Relaxed);
        startup_log("tray: registered");

        let menu_allow_quit = allow_quit.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let hwnd = HWND(window_handle as *mut _);
            if event.id == id_quit {
                menu_allow_quit.store(true, Ordering::Relaxed);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
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
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let hwnd = HWND(window_handle as *mut _);
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }));

        unsafe {
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Devices,
    Support,
    Sessions,
    Agenda,
    Monitoring,
    Automation,
    Ai,
    Reports,
    UsersMfa,
    Settings,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::Home => "Inicio",
            Self::Devices => "Dispositivos",
            Self::Support => "Soporte",
            Self::Sessions => "Historial de sesiones",
            Self::Agenda => "Ordenadores y contactos",
            Self::Monitoring => "Monitoreo",
            Self::Automation => "Automatización",
            Self::Ai => "AgentDesk AI",
            Self::Reports => "Informes",
            Self::UsersMfa => "Usuarios y MFA",
            Self::Settings => "Ajustes",
        }
    }
}

#[derive(Clone)]
pub struct IncomingControl {
    pub caller_name: String,
    pub allow_control: bool,
    pub stop_tx: watch::Sender<bool>,
}

fn generate_access_password() -> String {
    const CHARSET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

#[derive(Clone)]
pub struct DeviceContextMenu {
    pub pos: Point<Pixels>,
    pub device_id: Option<Uuid>,
    pub agenda_id: Option<Uuid>,
    pub history_idx: Option<usize>,
    pub hostname: String,
    pub connection_id: String,
    pub submenu: Option<&'static str>,
}

#[derive(Clone)]
pub struct GroupContextMenu {
    pub pos: Point<Pixels>,
    pub group_id: Uuid,
    pub group_name: String,
    pub my_permission: String,
}

#[derive(Clone)]
pub enum GroupModalMode {
    Create,
    Edit(Uuid),
}

#[derive(Clone)]
pub struct ConnectAuthPopup {
    pub connection_id: String,
    pub hostname: String,
    pub allow_control: bool,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub enum OcSelection {
    Group(Uuid),
    Device(Uuid),
    Agenda(Uuid),
    Recent(usize),
}

pub struct Client {
    pub cfg: DesktopConfig,
    pub config_path: std::path::PathBuf,
    pub main_hwnd: Option<isize>,
    pub incoming_panel_tx: Option<Sender<PanelEvent>>,
    pub connect_auth_popup: Option<ConnectAuthPopup>,
    pub connect_auth_pw_input: Entity<InputState>,
    pub page: Page,
    pub dark: bool,
    pub brand_logo: Arc<RenderImage>,
    pub sidebar_compact: bool,
    pub show_ai: bool,
    pub show_notifications: bool,
    pub settings_section: &'static str,
    pub ai_subtab: &'static str,
    pub oauth_device_code: Option<(String, String)>,
    pub home_tab_sessions: bool,
    pub show_easy_access: bool,
    pub selected_device_id: Option<Uuid>,
    pub devices_show_recent: bool,
    pub devices_show_all: bool,
    pub expanded_groups: HashSet<Uuid>,
    pub show_add_device_form: bool,
    pub selected_ticket_id: Option<Uuid>,
    pub dev_ctx_menu: Option<DeviceContextMenu>,
    pub grp_ctx_menu: Option<GroupContextMenu>,
    pub editing_device: Option<(Option<Uuid>, Option<Uuid>, String)>,
    pub edit_icon_kind: String,
    pub edit_accent_color: String,
    pub group_modal: Option<GroupModalMode>,
    pub sharing_group_id: Option<Uuid>,
    pub add_computer_modal_group: Option<Option<Uuid>>,
    pub add_contact_modal_group: Option<Option<Uuid>>,
    pub oc_selection: Option<OcSelection>,
    pub oc_show_recent: bool,
    pub oc_show_managed: bool,
    pub groups_initialized: bool,
    pub group_action_msg: String,
    pub oc_filter_in: Entity<InputState>,

    // Inputs
    pub global_search: Entity<InputState>,
    pub quick_id: Entity<InputState>,
    pub quick_password: Entity<InputState>,
    pub personal_pw_input: Entity<InputState>,
    pub owner_email_input: Entity<InputState>,
    pub server_url: Entity<InputState>,
    pub service_enroll_token_in: Entity<InputState>,
    pub login_email: Entity<InputState>,
    pub login_password: Entity<InputState>,
    pub login_mfa: Entity<InputState>,
    pub cur_pw_in: Entity<InputState>,
    pub new_pw_in: Entity<InputState>,
    pub new_pw_confirm_in: Entity<InputState>,
    pub ai_prompt: Entity<InputState>,
    pub ai_model_in: Entity<InputState>,
    pub ai_base_url_in: Entity<InputState>,
    pub ai_api_key_in: Entity<InputState>,

    // Devices / Assign / Enroll
    pub assign_conn_id: Entity<InputState>,
    pub assign_email: Entity<InputState>,
    pub assign_alias: Entity<InputState>,
    pub assign_group_idx: usize,
    pub enroll_cmd_out: String,

    // Agenda / Groups
    pub only_favorites: bool,
    pub agenda_alias_in: Entity<InputState>,
    pub agenda_company_in: Entity<InputState>,
    pub agenda_contact_in: Entity<InputState>,
    pub agenda_notes_in: Entity<InputState>,
    pub new_agenda_device_idx: usize,
    pub new_agenda_group_idx: usize,
    pub group_name_in: Entity<InputState>,
    pub group_desc_in: Entity<InputState>,
    pub share_email_in: Entity<InputState>,
    pub share_perm: String,

    // Tickets / Calendar / Users / RMM
    pub resolving_ticket: Option<(Uuid, String)>,
    pub resolve_outcome: String,
    pub resolve_summary_input: Entity<InputState>,
    pub rmm_device_idx: usize,
    pub rmm_shell: String,
    pub rmm_command_in: Entity<InputState>,
    pub rmm_hw_snapshot: Option<serde_json::Value>,
    pub rmm_diag_output: String,
    pub rmm_cmd_output: String,
    pub device_alerts: Vec<DeviceAlertDto>,
    pub cal_title_in: Entity<InputState>,
    pub cal_client_in: Entity<InputState>,
    pub new_user_email_in: Entity<InputState>,
    pub new_user_name_in: Entity<InputState>,
    pub new_user_pw_in: Entity<InputState>,
    pub new_user_role: String,
    pub mfa_setup: Option<MfaSetupResponse>,
    pub mfa_verify_in: Entity<InputState>,

    // Local Embedded Host
    pub host_stop: Option<watch::Sender<bool>>,
    pub host_rx: Receiver<HostEvent>,
    pub host_tx: Sender<HostEvent>,
    pub local_device_id: Option<Uuid>,
    pub local_device_token: Option<String>,
    pub local_connection_id: String,
    pub local_password: String,
    pub local_access_status: String,
    pub show_local_password: bool,
    pub incoming_control: Option<IncomingControl>,
    pub partner_allow_control: bool,

    // Auto-reconnect & Viewer lifecycle
    pub viewer_end_rx: Receiver<ViewerEndedNotice>,
    pub viewer_end_tx: Sender<ViewerEndedNotice>,
    pub active_viewer_count: usize,
    pub reconnect_prompt: Option<(String, ReconnectTarget)>,
    pub auto_reconnect: Option<(ReconnectTarget, Instant, u32)>,

    // Auth & Overlays
    pub login_open: bool,
    pub login_error: String,
    pub mfa_required: bool,
    pub account_menu_open: bool,
    pub change_pw_open: bool,
    pub change_pw_msg: String,
    pub session_restore_in_flight: bool,
    pub last_session_restore_try: Option<Instant>,

    // AI Copilot
    pub ai_provider: String,
    pub ai_autonomy: String,
    pub ai_redact_pii: bool,
    pub ai_device_idx: usize,
    pub ai_include_diag: bool,
    pub ai_settings: Option<AiSettingsDto>,
    pub ai_models: Vec<String>,
    pub ai_models_info: Vec<(String, String, Vec<String>)>,
    pub ai_models_loaded: bool,
    pub ai_selected_model: String,
    pub ai_reasoning_effort: String,
    pub ai_chat_model: String,
    pub ai_chat_effort: String,
    pub ai_chat_model_picker_open: bool,
    pub ai_chat_session_id: Option<Uuid>,
    pub ai_chat_sessions: Vec<AiChatSessionSummaryDto>,
    pub ai_chat_messages: Vec<AiChatMessageDto>,
    pub ai_callback_paste_in: Entity<InputState>,
    pub ai_status: String,
    pub ai_reply: String,
    pub ai_proposed_actions: Vec<AiProposedActionDto>,
    pub ai_busy: bool,

    // Loaded server state
    pub server_status: String,
    pub connect_status: String,
    pub report_status: String,
    pub auth: Option<AuthResponse>,
    pub devices: Vec<DeviceDto>,
    pub agenda: Vec<AddressBookEntryDto>,
    pub groups: Vec<DeviceGroupDto>,
    pub tickets: Vec<SupportTicketDto>,
    pub sessions: Vec<SessionDto>,
    pub calendar: Vec<CalendarEventDto>,
    pub docs: Vec<DocPageDto>,
    pub audit: Vec<AuditEventDto>,
    pub users: Vec<UserDto>,

    // Rediseño: paleta de comandos, menú "Nuevo", filtros y pestañas
    pub focus_handle: FocusHandle,
    pub palette_open: bool,
    pub palette_query: Entity<InputState>,
    pub palette_sel: usize,
    pub plus_menu_open: bool,
    pub devices_filter: shell::DevFilter,
    pub drawer_tab: &'static str,
    pub monitor_tab: &'static str,
    /// 0 = control, 1 = solo ver, 2 = archivos.
    pub connect_mode: u8,
    /// Panel que abrirá el próximo visor (p. ej. "files" en modo archivos).
    pub pending_viewer_panel: Option<&'static str>,
}

impl Client {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (cfg, config_path) = DesktopConfig::load();
        let dark = cfg.is_dark();
        apply_theme(cx, if dark { ThemeMode::Dark } else { ThemeMode::Light });

        let server_val = cfg.server_url.clone();
        let last_email = cfg.last_email.clone();
        let owner_val = cfg.owner_email.clone();

        let mk = |window: &mut Window, cx: &mut Context<Self>, ph: &'static str, val: &str, mask: bool| {
            let v = val.to_string();
            cx.new(|cx| {
                let mut st = InputState::new(window, cx).placeholder(ph);
                if !v.is_empty() {
                    st = st.default_value(v);
                }
                if mask {
                    st = st.masked(true);
                }
                st
            })
        };

        let (host_tx, host_rx) = mpsc::channel();
        let (viewer_end_tx, viewer_end_rx) = mpsc::channel();

        let mut client = Self {
            cfg,
            config_path,
            main_hwnd: None,
            incoming_panel_tx: None,
            connect_auth_popup: None,
            connect_auth_pw_input: mk(window, cx, "Contraseña del equipo remoto", "", true),
            page: Page::Home,
            dark,
            brand_logo: load_brand_logo_image(include_bytes!("../../../logos/agentdesk-icono.png")),
            sidebar_compact: false,
            show_ai: false,
            show_notifications: false,
            settings_section: "account",
            ai_subtab: "oauth",
            oauth_device_code: None,
            home_tab_sessions: false,
            show_easy_access: false,
            selected_device_id: None,
            devices_show_recent: true,
            devices_show_all: true,
            expanded_groups: HashSet::new(),
            show_add_device_form: false,
            selected_ticket_id: None,
            dev_ctx_menu: None,
            grp_ctx_menu: None,
            editing_device: None,
            edit_icon_kind: "desktop".into(),
            edit_accent_color: String::new(),
            group_modal: None,
            sharing_group_id: None,
            add_computer_modal_group: None,
            add_contact_modal_group: None,
            oc_selection: None,
            oc_show_recent: true,
            oc_show_managed: true,
            groups_initialized: false,
            group_action_msg: String::new(),
            oc_filter_in: mk(window, cx, "Buscar ordenadores, grupos o contactos…", "", false),
            global_search: mk(window, cx, "Buscar ID, equipo, grupo o historial…", "", false),
            quick_id: mk(window, cx, "ID del asociado", "", false),
            quick_password: mk(window, cx, "Contraseña aleatoria o fija", "", true),
            personal_pw_input: mk(window, cx, "Nueva contraseña personal (mín. 4)", "", true),
            owner_email_input: mk(window, cx, "Email propietario (ej: tecnico@empresa.com)", &owner_val, false),
            server_url: mk(window, cx, "https://agentdesk.dolan.com.ar", &server_val, false),
            service_enroll_token_in: mk(window, cx, "Token enrolamiento para AgentDeskHost", "", false),
            login_email: mk(window, cx, "Correo electrónico", &last_email, false),
            login_password: mk(window, cx, "Contraseña", "", true),
            login_mfa: mk(window, cx, "Código MFA 6 dígitos", "", false),
            cur_pw_in: mk(window, cx, "Contraseña actual", "", true),
            new_pw_in: mk(window, cx, "Nueva contraseña (mín. 6)", "", true),
            new_pw_confirm_in: mk(window, cx, "Repetir nueva contraseña", "", true),
            ai_prompt: mk(window, cx, "Preguntá sobre un equipo o un problema…", "", false),
            ai_model_in: mk(window, cx, "gpt-6-luna", "gpt-6-luna", false),
            ai_base_url_in: mk(window, cx, "https://api.openai.com/v1", "https://api.openai.com/v1", false),
            ai_api_key_in: mk(window, cx, "API Key (opcional si usás OAuth/Ollama)", "", true),
            ai_callback_paste_in: mk(window, cx, "Pegá aquí la URL de callback http://localhost:1455/auth/callback?code=… (opcional)", "", false),
            assign_conn_id: mk(window, cx, "ID de conexión (9 dígitos)", "", false),
            assign_email: mk(window, cx, "Email usuario registrado (opcional)", "", false),
            assign_alias: mk(window, cx, "Alias agenda (opcional)", "", false),
            assign_group_idx: 0,
            enroll_cmd_out: String::new(),
            only_favorites: false,
            agenda_alias_in: mk(window, cx, "Alias / PC", "", false),
            agenda_company_in: mk(window, cx, "Empresa", "", false),
            agenda_contact_in: mk(window, cx, "Contacto", "", false),
            agenda_notes_in: mk(window, cx, "Notas (horario, detalles)", "", false),
            new_agenda_device_idx: 0,
            new_agenda_group_idx: 0,
            group_name_in: mk(window, cx, "Nombre del grupo", "", false),
            group_desc_in: mk(window, cx, "Descripción", "", false),
            share_email_in: mk(window, cx, "usuario@empresa.com", "", false),
            share_perm: "read_write".into(),
            resolving_ticket: None,
            resolve_outcome: "resolved".into(),
            resolve_summary_input: mk(window, cx, "Resumen de resolución del incidente…", "", false),
            rmm_device_idx: 0,
            rmm_shell: "powershell".into(),
            rmm_command_in: mk(window, cx, "Get-Service AgentDesk* | Format-Table -AutoSize", "", false),
            rmm_hw_snapshot: None,
            rmm_diag_output: String::new(),
            rmm_cmd_output: String::new(),
            device_alerts: Vec::new(),
            cal_title_in: mk(window, cx, "Título mantenimiento", "", false),
            cal_client_in: mk(window, cx, "Cliente", "", false),
            new_user_email_in: mk(window, cx, "nuevo@empresa.com", "", false),
            new_user_name_in: mk(window, cx, "Nombre visible", "", false),
            new_user_pw_in: mk(window, cx, "Contraseña inicial", "", true),
            new_user_role: "technician".into(),
            mfa_setup: None,
            mfa_verify_in: mk(window, cx, "Código TOTP 6 dígitos", "", false),
            host_stop: None,
            host_rx,
            host_tx,
            local_device_id: None,
            local_device_token: None,
            local_connection_id: "Asignando…".into(),
            local_password: generate_access_password(),
            local_access_status: "Registrando este equipo…".into(),
            show_local_password: true,
            incoming_control: None,
            partner_allow_control: true,
            viewer_end_rx,
            viewer_end_tx,
            active_viewer_count: 0,
            reconnect_prompt: None,
            auto_reconnect: None,
            login_open: false,
            login_error: String::new(),
            mfa_required: false,
            account_menu_open: false,
            change_pw_open: false,
            change_pw_msg: String::new(),
            session_restore_in_flight: false,
            last_session_restore_try: Some(Instant::now()),
            ai_provider: "chat_gpt_o_auth".into(),
            ai_autonomy: "human_approved_exec".into(),
            ai_redact_pii: true,
            ai_device_idx: 0,
            ai_include_diag: true,
            ai_settings: None,
            ai_models: Vec::new(),
            ai_models_info: Vec::new(),
            ai_models_loaded: false,
            ai_selected_model: "gpt-6-luna".into(),
            ai_reasoning_effort: "high".into(),
            ai_chat_model: "gpt-6-luna".into(),
            ai_chat_effort: "high".into(),
            ai_chat_model_picker_open: false,
            ai_chat_session_id: None,
            ai_chat_sessions: Vec::new(),
            ai_chat_messages: Vec::new(),
            ai_status: "Sin proveedor conectado — configurá ChatGPT OAuth, OpenAI o Ollama.".into(),
            ai_reply: String::new(),
            ai_proposed_actions: Vec::new(),
            ai_busy: false,
            server_status: "Verificando servidor…".into(),
            connect_status: String::new(),
            report_status: String::new(),
            auth: None,
            devices: Vec::new(),
            agenda: Vec::new(),
            groups: Vec::new(),
            tickets: Vec::new(),
            sessions: Vec::new(),
            calendar: Vec::new(),
            docs: Vec::new(),
            audit: Vec::new(),
            users: Vec::new(),
            focus_handle: cx.focus_handle(),
            palette_open: false,
            palette_query: mk(window, cx, "Buscá un equipo, un ID o una acción", "", false),
            palette_sel: 0,
            plus_menu_open: false,
            devices_filter: shell::DevFilter::All,
            drawer_tab: "general",
            monitor_tab: "metrics",
            connect_mode: 0,
            pending_viewer_panel: None,
        };

        window.focus(&client.focus_handle, cx);
        cx.subscribe_in(&client.palette_query, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.run_palette_selected(window, cx),
            InputEvent::Change => {
                this.palette_sel = 0;
                cx.notify();
            }
            _ => {}
        })
        .detach();

        client.check_server(cx);
        client.restart_host(cx);

        for inp in [&client.login_email, &client.login_password, &client.login_mfa] {
            cx.subscribe(inp, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.login(cx);
                }
            })
            .detach();
        }
        client.bind_quick_connect_enter(&client.quick_id, window, cx);
        client.bind_quick_connect_enter(&client.quick_password, window, cx);
        client.bind_global_search_enter(window, cx);
        client.bind_connect_auth_pw_enter(window, cx);
        for inp in [&client.cur_pw_in, &client.new_pw_in, &client.new_pw_confirm_in] {
            client.bind_change_password_enter(inp, window, cx);
        }
        cx.subscribe_in(&client.personal_pw_input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.set_personal_password(true, Some(window), cx);
            }
        })
        .detach();
        client.bind_ai_prompt_enter(&client.ai_prompt, window, cx);

        // Poll host events, viewer disconnect notices, and auto-reconnect every 250ms
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep = cx
                    .update(|_, cx| {
                        let Some(ent) = weak.upgrade() else { return false };
                        ent.update(cx, |this, cx| this.poll_background(cx));
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            })
            .detach();

        client
    }

    pub fn palette(&self) -> Palette {
        if self.dark { DARK } else { LIGHT }
    }

    fn bind_quick_connect_enter(
        &self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.initiate_connect_from_home(Some(window), cx);
            }
        })
        .detach();
    }

    fn bind_global_search_enter(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            &self.global_search,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.handle_search_enter(window, cx);
                }
            },
        )
        .detach();
    }

    fn bind_connect_auth_pw_enter(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            &self.connect_auth_pw_input,
            window,
            |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.submit_connect_auth_popup(cx);
                }
            },
        )
        .detach();
    }

    fn bind_ai_prompt_enter(
        &self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.ask_ai(window, cx);
            }
        })
        .detach();
    }

    pub fn is_admin(&self) -> bool {
        self.auth.as_ref().is_some_and(|a| {
            let r = a.user.role.to_ascii_lowercase();
            r.contains("admin")
        })
    }

    fn account_initials(&self) -> String {
        let Some(auth) = &self.auth else { return String::new() };
        let parts: Vec<_> = auth.user.display_name.split_whitespace().collect();
        let init: String = parts
            .first()
            .into_iter()
            .chain(parts.last().filter(|_| parts.len() > 1))
            .filter_map(|p| p.chars().find(|c| c.is_alphanumeric()))
            .collect::<String>()
            .to_uppercase();
        if init.is_empty() { "U".into() } else { init }
    }

    pub fn base_url(&self, cx: &App) -> String {
        normalize_server_url(&self.server_url.read(cx).value())
    }

    pub fn restart_host(&mut self, cx: &App) {
        if let Some(prev) = self.host_stop.take() {
            let _ = prev.send(true);
        }
        let srv = self.base_url(cx);
        let owner = {
            let t = self.cfg.owner_email.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        if self.local_password.is_empty() {
            self.local_password = generate_access_password();
        }
        self.host_stop = Some(start_embedded_host(
            srv,
            self.local_password.clone(),
            owner,
            self.host_tx.clone(),
        ));
    }

    fn poll_background(&mut self, cx: &mut Context<Self>) {
        let mut dirty = false;
        while let Ok(ev) = self.host_rx.try_recv() {
            dirty = true;
            match ev {
                HostEvent::Status(s) => self.local_access_status = s,
                HostEvent::Registered {
                    device_id,
                    connection_id,
                    device_token,
                    password,
                } => {
                    self.local_device_id = Some(device_id);
                    self.local_connection_id = connection_id.clone();
                    self.local_device_token = Some(device_token);
                    self.local_password = password;
                    self.local_access_status = format!("En línea (ID: {connection_id})");
                }
                HostEvent::PasswordRotated(pw) => {
                    self.local_password = pw;
                    self.local_access_status = "Contraseña aleatoria renovada".into();
                }
                HostEvent::IncomingStarted {
                    caller_name,
                    allow_control,
                    stop_tx,
                    chat_tx,
                } => {
                    // The password was single-use and is already dead; a new one appears when
                    // the session ends (never on screen while someone is watching it).
                    self.local_password.clear();
                    self.incoming_control = Some(IncomingControl {
                        caller_name: caller_name.clone(),
                        allow_control,
                        stop_tx: stop_tx.clone(),
                    });
                    self.local_access_status = format!("Sesión entrante activa con {caller_name}");
                    let (ptx, prx) = mpsc::channel();
                    self.incoming_panel_tx = Some(ptx);
                    FloatingSessionPanel::open(
                        FloatingPanelLaunch {
                            caller_name,
                            allow_control,
                            connection_id: self.local_connection_id.clone(),
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
                    self.incoming_control = None;
                    if let Some(ptx) = self.incoming_panel_tx.take() {
                        let _ = ptx.send(PanelEvent::Close);
                    }
                    self.local_access_status = "Sesión entrante finalizada".into();
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

        while let Ok(notice) = self.viewer_end_rx.try_recv() {
            dirty = true;
            self.active_viewer_count = self.active_viewer_count.saturating_sub(1);
            if let Some(target) = notice.target {
                if notice.reboot_pending {
                    self.auto_reconnect = Some((
                        target,
                        Instant::now() + Duration::from_secs(20),
                        0,
                    ));
                    self.connect_status =
                        "Reinicio remoto detectado: auto-reconexión programada".into();
                } else if self.auto_reconnect.is_none() {
                    self.reconnect_prompt = Some((notice.reason, target));
                }
            }
        }

        if let Some((target, last, attempts)) = self.auto_reconnect.clone() {
            if self.active_viewer_count > 0 {
                self.auto_reconnect = None;
                self.connect_status = "Auto-reconexión completada".into();
                dirty = true;
            } else if Instant::now() >= last && last.elapsed() >= Duration::from_secs(4) {
                if attempts >= 150 {
                    self.auto_reconnect = None;
                    self.connect_status =
                        "Auto-reconexión cancelada tras 10 minutos sin respuesta".into();
                } else {
                    self.auto_reconnect = Some((target.clone(), Instant::now(), attempts + 1));
                    self.execute_reconnect_target(target, cx);
                }
                dirty = true;
            }
        }

        if self.auth.is_none()
            && !self.session_restore_in_flight
            && self
                .cfg
                .saved_auth_for_server(&self.base_url(cx))
                .is_some()
            && self
                .last_session_restore_try
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            self.restore_saved_session(cx);
        }

        if dirty {
            cx.notify();
        }
    }

    fn execute_reconnect_target(&mut self, target: ReconnectTarget, cx: &mut Context<Self>) {
        match target {
            ReconnectTarget::Direct {
                connection_id,
                reconnect_token,
                allow_control,
                ..
            } => {
                self.connect_direct(connection_id, String::new(), reconnect_token, allow_control, cx);
            }
            ReconnectTarget::Managed {
                device_id,
                label,
                allow_control,
            } => {
                self.start_managed_session(device_id, None, allow_control, label, cx);
            }
        }
    }

    pub fn save_server_url(&mut self, cx: &mut Context<Self>) {
        self.cfg.server_url = self.base_url(cx);
        let _ = self.cfg.save(&self.config_path);
        self.check_server(cx);
        self.restart_host(cx);
    }

    pub fn check_server(&mut self, cx: &mut Context<Self>) {
        let base = self.base_url(cx);
        if self.auth.is_none() {
            self.server_status = format!("Consultando {base}…");
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(6))
                        .build()
                        .map_err(|e| e.to_string())?
                        .get(format!("{base}/health/ready"))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(format!("Servidor conectado ({base})"))
                    } else {
                        Err(format!("HTTP {}", r.status()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(msg) => {
                        if let Some(auth) = &this.auth {
                            this.server_status = format!("Conectado como {}", auth.user.display_name);
                        } else if this
                            .cfg
                            .saved_auth_for_server(&this.base_url(cx))
                            .is_some()
                        {
                            this.restore_saved_session(cx);
                        } else {
                            this.server_status = msg;
                        }
                    }
                    Err(e) => {
                        if this.auth.is_none() {
                            this.server_status = format!("Sin conexión: {e}");
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn restore_saved_session(&mut self, cx: &mut Context<Self>) {
        if self.auth.is_some() || self.session_restore_in_flight {
            return;
        }
        let base = self.base_url(cx);
        let Some(saved_tok) = self.cfg.saved_auth_for_server(&base) else {
            return;
        };
        self.session_restore_in_flight = true;
        self.last_session_restore_try = Some(Instant::now());
        self.server_status = "Restaurando sesión…".into();
        cx.notify();

        let tok_req = saved_tok.clone();
        let base_req = base.clone();
        cx.spawn(async move |this, cx| {
            let res: Result<Option<AuthUserInfo>, String> = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(8))
                        .build()
                        .map_err(|e| e.to_string())?
                        .get(format!("{base_req}/api/v1/auth/me"))
                        .bearer_auth(&tok_req)
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<AuthUserInfo>().map(Some).map_err(|e| e.to_string())
                    } else if r.status() == reqwest::StatusCode::UNAUTHORIZED
                        || r.status() == reqwest::StatusCode::FORBIDDEN
                    {
                        Ok(None)
                    } else {
                        Err(format!("HTTP {}", r.status()))
                    }
                })
                .await;

            this.update(cx, |this, cx| {
                this.session_restore_in_flight = false;
                match res {
                    Ok(Some(user)) => {
                        if this.auth.is_none() {
                            this.auth = Some(AuthResponse {
                                token: saved_tok,
                                expires_at: chrono::Utc::now() + chrono::Duration::days(30),
                                user: user.clone(),
                            });
                            this.cfg.last_email = user.email.clone();
                            this.server_status = format!("Conectado como {}", user.display_name);
                            this.login_open = false;
                            this.login_error.clear();
                            this.mfa_required = false;
                            this.account_menu_open = false;
                            this.refresh_all(cx);
                        }
                    }
                    Ok(None) => {
                        this.cfg.clear_saved_auth();
                        let _ = this.cfg.save(&this.config_path);
                        if this.auth.is_none() {
                            this.server_status = format!("Servidor conectado ({base}) · Iniciá sesión");
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn rotate_local_password(&mut self, cx: &mut Context<Self>) {
        let (Some(did), Some(tok)) = (self.local_device_id, self.local_device_token.clone()) else {
            self.local_access_status = "Esperando registro del equipo…".into();
            cx.notify();
            return;
        };
        let npw = generate_access_password();
        host::rotate_password_async(self.base_url(cx), did, tok, npw, self.host_tx.clone());
    }

    pub fn set_personal_password(
        &mut self,
        set: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let (Some(did), Some(tok)) = (self.local_device_id, self.local_device_token.clone()) else {
            self.connect_status = "El equipo todavía no está registrado".into();
            cx.notify();
            return;
        };
        let pw = self.personal_pw_input.read(cx).value().trim().to_string();
        if set && pw.len() < 4 {
            self.connect_status = "La contraseña fija debe tener al menos 4 caracteres".into();
            cx.notify();
            return;
        }
        if let Some(w) = window {
            self.personal_pw_input = cx.new(|cx| {
                InputState::new(w, cx)
                    .placeholder("Nueva contraseña personal (mín. 4)")
                    .masked(true)
            });
        }
        let base = self.base_url(cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/clients/personal-password"))
                        .header("x-device-token", tok)
                        .json(&SetPersonalPasswordRequest {
                            device_id: did,
                            personal_password: if set { Some(pw) } else { None },
                        })
                        .send()
                        .map_err(|e| e.to_string())?
                        .error_for_status()
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.cfg.personal_password_set = set;
                        let _ = this.cfg.save(&this.config_path);
                        this.connect_status = if set {
                            "Contraseña personal fija guardada".into()
                        } else {
                            "Contraseña personal fija eliminada".into()
                        };
                    }
                    Err(e) => this.connect_status = format!("Error contraseña fija: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn request_remote_connection(&mut self, cx: &mut Context<Self>) {
        self.initiate_connect_from_home(None, cx);
    }

    pub fn initiate_connect_from_home(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let cid = self.quick_id.read(cx).value().trim().to_string();
        if cid.is_empty() {
            self.connect_status = "Ingresá el ID del asociado.".into();
            cx.notify();
            return;
        }
        let allow = self.partner_allow_control;
        self.initiate_connect_by_id(cid, allow, window, cx);
    }

    pub fn initiate_connect_by_id(
        &mut self,
        raw_id: String,
        allow_control: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let norm = normalize_conn_code(&raw_id);
        if norm.is_empty() {
            self.connect_status = "Ingresá un ID válido.".into();
            cx.notify();
            return;
        }

        // Si tenemos un grant de reconexión (< 1h) guardado en historial, conectar directo sin popup.
        let now = now_unix();
        if let Some(h) = self
            .cfg
            .connection_history
            .iter()
            .find(|h| normalize_conn_code(&h.connection_id) == norm && h.is_trust_valid(now))
            .cloned()
        {
            self.connect_direct(h.connection_id, String::new(), h.reconnect_token, allow_control, cx);
            return;
        }

        if let Some(w) = window {
            self.connect_auth_pw_input =
                cx.new(|cx| InputState::new(w, cx).placeholder("Contraseña del asociado").masked(true));
            self.bind_connect_auth_pw_enter(w, cx);
        }

        let base = self.base_url(cx);
        let q_id = raw_id.clone();
        self.connect_status = format!("Verificando disponibilidad de {raw_id}…");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(8))
                        .build()
                        .map_err(|e| e.to_string())?
                        .get(format!("{base}/api/v1/clients/lookup"))
                        .query(&[("connection_id", q_id.as_str())])
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<ClientLookupResponse>().map_err(|e| e.to_string())
                    } else {
                        Err(format!("Error HTTP {} al consultar equipo", r.status()))
                    }
                })
                .await;

            this.update(cx, |this, cx| {
                match res {
                    Ok(info) if !info.exists => {
                        this.connect_status =
                            format!("No se encontró ningún equipo registrado con ID {raw_id}.");
                    }
                    Ok(info) if !info.online => {
                        this.connect_status = format!(
                            "El equipo {} ({}) no está conectado al servidor.",
                            info.hostname, info.connection_id
                        );
                    }
                    Ok(info) => {
                        this.connect_status.clear();
                        this.connect_auth_popup = Some(ConnectAuthPopup {
                            connection_id: info.connection_id,
                            hostname: info.hostname,
                            allow_control,
                            error: None,
                            busy: false,
                        });
                    }
                    Err(e) => {
                        if let Some(pop) = &mut this.connect_auth_popup {
                            pop.busy = false;
                            pop.error = Some(e.clone());
                        }
                        this.connect_status = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn submit_connect_auth_popup(&mut self, cx: &mut Context<Self>) {
        let Some(mut pop) = self.connect_auth_popup.clone() else {
            return;
        };
        let pw = self.connect_auth_pw_input.read(cx).value().trim().to_string();
        if pw.is_empty() {
            pop.error = Some("Ingresá la contraseña para conectar.".into());
            self.connect_auth_popup = Some(pop);
            cx.notify();
            return;
        }
        pop.busy = true;
        pop.error = None;
        self.connect_auth_popup = Some(pop.clone());
        self.connect_direct(pop.connection_id, pw, None, pop.allow_control, cx);
    }

    pub fn handle_search_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.global_search.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return;
        }
        let q = raw.to_lowercase();
        let qn = normalize_conn_code(&raw).to_lowercase();

        // 1) Buscar coincidencia exacta o única en dispositivos gestionados
        let dev_matches: Vec<_> = self
            .devices
            .iter()
            .filter(|d| {
                d.hostname.to_lowercase() == q
                    || normalize_conn_code(&d.public_code).to_lowercase() == qn
                    || d.hostname.to_lowercase().contains(&q)
                    || (!qn.is_empty() && normalize_conn_code(&d.public_code).to_lowercase().contains(&qn))
            })
            .cloned()
            .collect();

        let exact_dev = dev_matches
            .iter()
            .find(|d| {
                d.hostname.to_lowercase() == q
                    || normalize_conn_code(&d.public_code).to_lowercase() == qn
            })
            .cloned()
            .or_else(|| if dev_matches.len() == 1 { Some(dev_matches[0].clone()) } else { None });

        if let Some(d) = exact_dev {
            self.global_search = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
            });
            self.bind_global_search_enter(window, cx);
            if self.auth.is_some() && d.status == "online" {
                self.start_managed_session(d.id, None, true, d.hostname, cx);
            } else {
                self.initiate_connect_by_id(d.public_code, self.partner_allow_control, Some(window), cx);
            }
            return;
        }

        // 2) Buscar coincidencia en historial reciente
        if let Some(idx) = self.cfg.connection_history.iter().position(|h| {
            h.hostname.to_lowercase() == q
                || normalize_conn_code(&h.connection_id).to_lowercase() == qn
        }) {
            self.global_search = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
            });
            self.bind_global_search_enter(window, cx);
            self.reconnect_from_history(idx, Some(window), cx);
            return;
        }

        // 3) Si parece un ID (>= 6 caracteres alfanuméricos), consultar online y pedir contraseña en popup
        if qn.len() >= 6 {
            self.global_search = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
            });
            self.bind_global_search_enter(window, cx);
            self.initiate_connect_by_id(raw, self.partner_allow_control, Some(window), cx);
        }
    }

    pub fn reconnect_from_history(
        &mut self,
        idx: usize,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(h) = self.cfg.connection_history.get(idx).cloned() else {
            return;
        };
        if h.is_trust_valid(now_unix()) {
            self.connect_direct(h.connection_id, String::new(), h.reconnect_token, true, cx);
        } else {
            self.initiate_connect_by_id(h.connection_id, true, window, cx);
        }
    }

    pub fn connect_direct(
        &mut self,
        connection_id: String,
        password: String,
        reconnect_token: Option<String>,
        allow_control: bool,
        cx: &mut Context<Self>,
    ) {
        let base = self.base_url(cx);
        let caller = self
            .auth
            .as_ref()
            .map(|a| a.user.display_name.clone())
            .or_else(|| Some(format!("Equipo {}", self.local_connection_id)));
        self.connect_status = format!("Conectando a {connection_id}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(12))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/sessions/connect-by-id"))
                        .json(&DirectConnectRequest {
                            connection_id,
                            password,
                            reconnect_token,
                            caller_name: caller,
                            allow_control,
                            allow_clipboard: true,
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<DirectConnectResponse>().map_err(|e| e.to_string())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(dr) => {
                        this.connect_auth_popup = None;
                        this.cfg.record_history(
                            dr.target_connection_id.clone(),
                            dr.target_hostname.clone(),
                            now_unix(),
                            dr.reconnect_token.clone(),
                            dr.reconnect_expires_at.map(|t| t.timestamp()),
                        );
                        let _ = this.cfg.save(&this.config_path);
                        let ws_base = ws_url_base(&this.base_url(cx));
                        let ws_url = format!(
                            "{ws_base}{}?role=viewer&token={}",
                            dr.relay_ws_path, dr.viewer_relay_token
                        );
                        this.open_viewer(
                            dr.session_id,
                            ws_url,
                            allow_control,
                            format!("{} ({})", dr.target_hostname, dr.target_connection_id),
                            Some(ReconnectTarget::Direct {
                                connection_id: dr.target_connection_id,
                                hostname: dr.target_hostname,
                                reconnect_token: dr.reconnect_token,
                                allow_control,
                            }),
                            cx,
                        );
                        this.connect_status.clear();
                    }
                    Err(e) => {
                        if let Some(pop) = &mut this.connect_auth_popup {
                            pop.busy = false;
                            pop.error = Some(e.clone());
                        }
                        this.connect_status = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn start_managed_session(
        &mut self,
        device_id: Uuid,
        ticket_id: Option<Uuid>,
        allow_control: bool,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.start_managed_session_with_panel(device_id, ticket_id, allow_control, label, None, cx);
    }

    pub fn start_managed_session_with_panel(
        &mut self,
        device_id: Uuid,
        ticket_id: Option<Uuid>,
        allow_control: bool,
        label: String,
        initial_panel: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        let Some(auth) = &self.auth else {
            self.login_open = true;
            cx.notify();
            return;
        };
        let base = self.base_url(cx);
        let token = auth.token.clone();
        self.connect_status = format!("Iniciando sesión en {label}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(12))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/sessions"))
                        .bearer_auth(token)
                        .json(&CreateSessionRequest {
                            device_id,
                            ticket_id,
                            allow_control,
                            allow_clipboard: true,
                            allow_files: true,
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<CreateSessionResponse>().map_err(|e| e.to_string())
                    } else {
                        Err(format!("HTTP {}", r.status()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(sr) => {
                        let ws_base = ws_url_base(&this.base_url(cx));
                        let ws_url = format!(
                            "{ws_base}{}?role=viewer&token={}",
                            sr.relay_ws_path, sr.viewer_relay_token
                        );
                        this.open_viewer_with_panel(
                            sr.session.id,
                            ws_url,
                            allow_control,
                            label.clone(),
                            Some(ReconnectTarget::Managed {
                                device_id,
                                label,
                                allow_control,
                            }),
                            initial_panel,
                            cx,
                        );
                        this.connect_status.clear();
                    }
                    Err(e) => {
                        this.connect_status = format!("No se pudo iniciar sesión: {e}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_viewer(
        &mut self,
        session_id: Uuid,
        ws_url: String,
        allow_control: bool,
        label: String,
        target: Option<ReconnectTarget>,
        cx: &mut Context<Self>,
    ) {
        let panel = self.pending_viewer_panel.take();
        self.open_viewer_with_panel(session_id, ws_url, allow_control, label, target, panel, cx);
    }

    fn open_viewer_with_panel(
        &mut self,
        session_id: Uuid,
        ws_url: String,
        allow_control: bool,
        label: String,
        target: Option<ReconnectTarget>,
        initial_panel: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        self.active_viewer_count += 1;
        let label_title = label.clone();
        let launch = ViewerLaunch {
            session_id,
            ws_url,
            allow_control,
            label,
            target,
            server_url: self.base_url(cx),
            auth_token: self.auth.as_ref().map(|a| a.token.clone()),
            dark: self.dark,
            initial_panel: initial_panel.map(str::to_string),
            on_ended: self.viewer_end_tx.clone(),
        };
        let mut launch_opt = Some(launch);
        let _ = open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(100.), px(70.)),
                    size: size(px(1340.), px(840.)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{label_title} — AgentDesk").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                #[cfg(windows)]
                let _ = apply_win32_window_branding(window, &format!("{label_title} — AgentDesk"));
                let l = launch_opt.take().expect("single viewer init");
                cx.new(|cx| RemoteSession::new(l, window, cx))
            },
        );
    }

    pub fn login(&mut self, cx: &mut Context<Self>) {
        let base = self.base_url(cx);
        let email = self.login_email.read(cx).value().trim().to_string();
        let pw = self.login_password.read(cx).value().to_string();
        let mfa = self.login_mfa.read(cx).value().trim().to_string();
        self.login_error = "Autenticando…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(12))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/auth/login"))
                        .json(&LoginRequest {
                            email: email.clone(),
                            password: pw,
                            totp_code: if mfa.is_empty() { None } else { Some(mfa) },
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<AuthResponse>().map_err(|e| e.to_string())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(auth) => {
                        this.cfg.last_email = auth.user.email.clone();
                        this.cfg
                            .set_saved_auth(auth.token.clone(), this.base_url(cx));
                        let _ = this.cfg.save(&this.config_path);
                        this.server_status = format!("Conectado como {}", auth.user.display_name);
                        this.auth = Some(auth);
                        this.login_open = false;
                        this.login_error.clear();
                        this.mfa_required = false;
                        this.account_menu_open = false;
                        this.refresh_all(cx);
                    }
                    Err(e) if e == "mfa_code_required" => {
                        this.mfa_required = true;
                        this.login_error = "Ingresá el código TOTP de 6 dígitos.".into();
                    }
                    Err(e) => {
                        this.login_error = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn bind_change_password_enter(
        &self,
        inp: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(inp, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.change_password(window, cx);
            }
        })
        .detach();
    }

    pub fn change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let cur = self.cur_pw_in.read(cx).value().to_string();
        let next = self.new_pw_in.read(cx).value().to_string();
        let conf = self.new_pw_confirm_in.read(cx).value().to_string();
        if next.trim().len() < 6 {
            self.change_pw_msg = "La nueva contraseña debe tener al menos 6 caracteres".into();
            cx.notify();
            return;
        }
        if next != conf {
            self.change_pw_msg = "Las contraseñas nuevas no coinciden".into();
            cx.notify();
            return;
        }
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.change_pw_msg = "Guardando…".into();
        self.cur_pw_in = cx.new(|cx| InputState::new(window, cx).placeholder("Contraseña actual").masked(true));
        self.new_pw_in = cx.new(|cx| InputState::new(window, cx).placeholder("Nueva contraseña (mín. 6)").masked(true));
        self.new_pw_confirm_in = cx.new(|cx| InputState::new(window, cx).placeholder("Repetir nueva contraseña").masked(true));
        self.bind_change_password_enter(&self.cur_pw_in, window, cx);
        self.bind_change_password_enter(&self.new_pw_in, window, cx);
        self.bind_change_password_enter(&self.new_pw_confirm_in, window, cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/auth/change-password"))
                        .bearer_auth(tok)
                        .json(&ChangePasswordRequest {
                            current_password: cur,
                            new_password: next,
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        Err(r.text().unwrap_or_else(|_| "Error".into()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.change_pw_open = false;
                        this.change_pw_msg.clear();
                        this.server_status = "Contraseña actualizada".into();
                    }
                    Err(e) => this.change_pw_msg = e,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let base = self.base_url(cx);
        let tok = self.auth.as_ref().map(|a| a.token.clone());
        self.cfg.clear_saved_auth();
        let _ = self.cfg.save(&self.config_path);
        self.auth = None;
        self.devices.clear();
        self.agenda.clear();
        self.groups.clear();
        self.tickets.clear();
        self.sessions.clear();
        self.users.clear();
        self.ai_settings = None;
        self.ai_models.clear();
        self.ai_models_info.clear();
        self.ai_models_loaded = false;
        self.ai_chat_sessions.clear();
        self.ai_chat_messages.clear();
        self.ai_chat_session_id = None;
        self.dev_ctx_menu = None;
        self.grp_ctx_menu = None;
        self.group_modal = None;
        self.sharing_group_id = None;
        self.add_computer_modal_group = None;
        self.add_contact_modal_group = None;
        self.oc_selection = None;
        self.groups_initialized = false;
        self.group_action_msg.clear();
        self.account_menu_open = false;
        self.server_status = "Sesión cerrada".into();
        cx.notify();
        if let Some(tok) = tok {
            runtime().spawn(async move {
                let _ = reqwest::Client::new()
                    .post(format!("{base}/api/v1/auth/logout"))
                    .bearer_auth(tok)
                    .send()
                    .await;
            });
        }
    }

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.load_devices(cx);
        self.load_agenda_groups(cx);
        self.load_tickets(cx);
        self.load_sessions(cx);
        self.load_calendar_docs(cx);
        self.load_audit(cx);
        self.load_ai_settings(cx);
        if self.is_admin() {
            self.load_users(cx);
        }
    }

    pub fn load_devices(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/devices"))
                        .bearer_auth(tok)
                        .send()
                        .map_err(|e| e.to_string())?
                        .error_for_status()
                        .map_err(|e| e.to_string())?
                        .json::<Vec<DeviceDto>>()
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(d) = res {
                    this.devices = d;
                    this.load_device_alerts(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn create_enrollment(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/enrollments"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({
                            "label": "GPUI Enroll",
                            "max_uses": 25,
                            "expires_hours": 168
                        }))
                        .send()
                        .map_err(|e| e.to_string())?
                        .json::<CreateEnrollmentResponse>()
                        .map_err(|e| e.to_string())
                        .map(|er| format!(
                            "agentdesk-host.exe --server {base} --enroll-token {}",
                            er.enrollment_token
                        ))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(cmd) = res {
                    this.enroll_cmd_out = cmd;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn assign_device(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let cid = self.assign_conn_id.read(cx).value().trim().to_string();
        if cid.is_empty() {
            self.group_action_msg = "Ingresá un ID de conexión válido.".into();
            cx.notify();
            return;
        }
        let email = self.assign_email.read(cx).value().trim().to_string();
        let alias = self.assign_alias.read(cx).value().trim().to_string();
        let gid = if self.assign_group_idx > 0 {
            self.groups.get(self.assign_group_idx - 1).map(|g| g.id)
        } else {
            None
        };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.assign_conn_id = cx.new(|cx| InputState::new(window, cx).placeholder("ID de conexión (9 dígitos)"));
        self.assign_email = cx.new(|cx| InputState::new(window, cx).placeholder("Email usuario registrado (opcional)"));
        self.assign_alias = cx.new(|cx| InputState::new(window, cx).placeholder("Alias (opcional)"));
        self.add_computer_modal_group = None;
        if let Some(g) = gid {
            self.expanded_groups.insert(g);
        }

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/devices/assign"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({
                            "connection_id": cid,
                            "owner_email": if email.is_empty() { None } else { Some(email) },
                            "group_id": gid,
                            "alias": if alias.is_empty() { None } else { Some(alias) },
                        }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.server_status = "Ordenador agregado al grupo".into();
                        this.group_action_msg = "Ordenador agregado correctamente.".into();
                        this.refresh_all(cx);
                    }
                    Err(e) => {
                        this.server_status = format!("Error asignando equipo: {e}");
                        this.group_action_msg = format!("Error: {e}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_agenda_groups(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let c = reqwest::blocking::Client::new();
                    let a = c.get(format!("{base}/api/v1/address-book")).bearer_auth(&tok).send().ok()?.json::<Vec<AddressBookEntryDto>>().ok()?;
                    let g = c.get(format!("{base}/api/v1/groups")).bearer_auth(&tok).send().ok()?.json::<Vec<DeviceGroupDto>>().ok()?;
                    Some((a, g))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some((a, g)) = res {
                    if !this.groups_initialized && !g.is_empty() {
                        for grp in &g {
                            this.expanded_groups.insert(grp.id);
                        }
                        this.groups_initialized = true;
                    }
                    this.agenda = a;
                    this.groups = g;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn open_create_group_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.grp_ctx_menu = None;
        self.dev_ctx_menu = None;
        self.group_action_msg.clear();
        self.group_name_in = cx.new(|cx| InputState::new(window, cx).placeholder("Nombre del grupo (ej: Cliente ACME)"));
        self.group_desc_in = cx.new(|cx| InputState::new(window, cx).placeholder("Descripción (opcional)"));
        self.group_modal = Some(GroupModalMode::Create);
        cx.notify();
    }

    pub fn open_edit_group_modal(&mut self, gid: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.grp_ctx_menu = None;
        self.dev_ctx_menu = None;
        self.group_action_msg.clear();
        let (name, desc) = self
            .groups
            .iter()
            .find(|g| g.id == gid)
            .map(|g| (g.name.clone(), g.description.clone()))
            .unwrap_or_default();
        self.group_name_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Nombre del grupo")
                .default_value(name)
        });
        self.group_desc_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Descripción (opcional)")
                .default_value(desc)
        });
        self.group_modal = Some(GroupModalMode::Edit(gid));
        cx.notify();
    }

    pub fn open_share_group_modal(&mut self, gid: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.grp_ctx_menu = None;
        self.dev_ctx_menu = None;
        self.group_action_msg.clear();
        self.share_email_in = cx.new(|cx| InputState::new(window, cx).placeholder("usuario@empresa.com"));
        self.sharing_group_id = Some(gid);
        if self.users.is_empty() {
            self.load_users(cx);
        }
        cx.notify();
    }

    pub fn open_add_computer_modal(
        &mut self,
        gid: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grp_ctx_menu = None;
        self.dev_ctx_menu = None;
        self.group_action_msg.clear();
        self.assign_conn_id = cx.new(|cx| InputState::new(window, cx).placeholder("ID de conexión (ej: 123 456 789)"));
        self.assign_email = cx.new(|cx| InputState::new(window, cx).placeholder("Email usuario registrado (opcional)"));
        self.assign_alias = cx.new(|cx| InputState::new(window, cx).placeholder("Alias en el grupo (opcional)"));
        self.assign_group_idx = gid
            .and_then(|id| self.groups.iter().position(|g| g.id == id).map(|i| i + 1))
            .unwrap_or(0);
        self.add_computer_modal_group = Some(gid);
        cx.notify();
    }

    pub fn open_add_contact_modal(
        &mut self,
        gid: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grp_ctx_menu = None;
        self.dev_ctx_menu = None;
        self.group_action_msg.clear();
        self.agenda_alias_in = cx.new(|cx| InputState::new(window, cx).placeholder("Alias / Nombre del contacto o PC"));
        self.agenda_company_in = cx.new(|cx| InputState::new(window, cx).placeholder("Empresa (opcional)"));
        self.agenda_contact_in = cx.new(|cx| InputState::new(window, cx).placeholder("Persona / Email / Teléfono"));
        self.agenda_notes_in = cx.new(|cx| InputState::new(window, cx).placeholder("Notas (opcional)"));
        self.new_agenda_device_idx = 0;
        self.new_agenda_group_idx = gid
            .and_then(|id| self.groups.iter().position(|g| g.id == id).map(|i| i + 1))
            .unwrap_or(0);
        self.add_contact_modal_group = Some(gid);
        cx.notify();
    }

    pub fn create_agenda_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let alias = self.agenda_alias_in.read(cx).value().trim().to_string();
        if alias.is_empty() {
            self.group_action_msg = "Ingresá un alias o nombre para el contacto.".into();
            cx.notify();
            return;
        }
        let company = self.agenda_company_in.read(cx).value().trim().to_string();
        let contact = self.agenda_contact_in.read(cx).value().trim().to_string();
        let notes = self.agenda_notes_in.read(cx).value().trim().to_string();
        let device_id = self.new_agenda_device_idx.checked_sub(1).and_then(|i| self.devices.get(i)).map(|d| d.id);
        let group_id = self.new_agenda_group_idx.checked_sub(1).and_then(|i| self.groups.get(i)).map(|g| g.id);

        self.agenda_alias_in = cx.new(|cx| InputState::new(window, cx).placeholder("Alias / PC"));
        self.agenda_company_in = cx.new(|cx| InputState::new(window, cx).placeholder("Empresa"));
        self.agenda_contact_in = cx.new(|cx| InputState::new(window, cx).placeholder("Contacto"));
        self.agenda_notes_in = cx.new(|cx| InputState::new(window, cx).placeholder("Notas (horario, detalles)"));
        self.add_contact_modal_group = None;
        if let Some(g) = group_id {
            self.expanded_groups.insert(g);
        }

        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/address-book"))
                        .bearer_auth(tok)
                        .json(&UpsertAddressBookRequest {
                            device_id,
                            group_id,
                            alias,
                            client_company: company,
                            contact_name: contact,
                            contact_email: String::new(),
                            group_name: "General".into(),
                            is_favorite: false,
                            notes,
                            scheduled_next_at: None,
                            icon_kind: "desktop".into(),
                            accent_color: String::new(),
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => this.group_action_msg = "Contacto guardado.".into(),
                    Err(e) => this.group_action_msg = format!("Error guardando contacto: {e}"),
                }
                this.load_agenda_groups(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn delete_agenda_entry(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .delete(format!("{base}/api/v1/address-book/{id}"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_agenda_groups(cx)).ok();
        })
        .detach();
    }

    pub fn move_device_to_group(&mut self, device_id: Uuid, group_id: Option<Uuid>, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .patch(format!("{base}/api/v1/devices/{device_id}/group"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({ "group_id": group_id }))
                        .send();
                })
                .await;
            this.update(cx, |this, cx| {
                this.server_status = "Grupo actualizado".into();
                this.load_devices(cx);
                this.load_agenda_groups(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn remove_history_entry(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.cfg.connection_history.len() {
            self.cfg.connection_history.remove(idx);
            let _ = self.cfg.save(&self.config_path);
            cx.notify();
        }
    }

    pub fn start_edit_device(
        &mut self,
        device_id: Option<Uuid>,
        agenda_id: Option<Uuid>,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = agenda_id
            .and_then(|aid| self.agenda.iter().find(|a| a.id == aid).cloned())
            .or_else(|| {
                device_id.and_then(|did| self.agenda.iter().find(|a| a.device_id == Some(did)).cloned())
            });
        let alias = existing.as_ref().map(|a| a.alias.clone()).unwrap_or_else(|| label.clone());
        let company = existing.as_ref().map(|a| a.client_company.clone()).unwrap_or_default();
        let contact = existing.as_ref().map(|a| a.contact_name.clone()).unwrap_or_default();
        let notes = existing.as_ref().map(|a| a.notes.clone()).unwrap_or_default();
        let current_gid = existing
            .as_ref()
            .and_then(|a| a.group_id)
            .or_else(|| {
                device_id.and_then(|did| {
                    self.devices
                        .iter()
                        .find(|d| d.id == did)
                        .and_then(|d| d.group_id)
                })
            });
        let dev_ref = device_id.and_then(|did| self.devices.iter().find(|d| d.id == did));
        let cur_icon = dev_ref
            .map(|d| d.icon_kind.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|a| a.icon_kind.clone())
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| "desktop".into());
        let cur_color = dev_ref
            .map(|d| d.accent_color.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.as_ref().map(|a| a.accent_color.clone()))
            .unwrap_or_default();
        self.edit_icon_kind = cur_icon;
        self.edit_accent_color = cur_color;
        self.new_agenda_group_idx = current_gid
            .and_then(|gid| self.groups.iter().position(|g| g.id == gid).map(|i| i + 1))
            .unwrap_or(0);
        self.agenda_alias_in = cx.new(|cx| InputState::new(window, cx).placeholder("Alias / Nombre").default_value(alias));
        self.agenda_company_in = cx.new(|cx| InputState::new(window, cx).placeholder("Empresa").default_value(company));
        self.agenda_contact_in = cx.new(|cx| InputState::new(window, cx).placeholder("Contacto / Teléfono / Email").default_value(contact));
        self.agenda_notes_in = cx.new(|cx| InputState::new(window, cx).placeholder("Notas / Descripción").default_value(notes));
        self.editing_device = Some((device_id, existing.map(|a| a.id).or(agenda_id), label));
        cx.notify();
    }

    pub fn trigger_wol(&mut self, device_id: Uuid, hostname: String, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else {
            self.connect_status = "Iniciá sesión para enviar Wake-on-LAN.".into();
            cx.notify();
            return;
        };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.connect_status = format!("Enviando paquete mágico Wake-on-LAN a {hostname}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res: Result<agentdesk_protocol::WolResponse, String> = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/devices/{device_id}/wol"))
                        .bearer_auth(tok)
                        .json(&agentdesk_protocol::WolRequest { mac_address: None })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if !r.status().is_success() {
                        let txt = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&txt)
                            .ok()
                            .and_then(|v| v.get("error").and_then(|s| s.as_str()).map(str::to_string))
                            .unwrap_or(txt);
                        return Err(msg);
                    }
                    r.json().map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(resp) => {
                        this.connect_status = resp.message;
                    }
                    Err(e) => {
                        this.connect_status = format!("Wake-on-LAN: {e}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn update_device_appearance(
        &mut self,
        device_id: Uuid,
        icon_kind: String,
        accent_color: String,
        cx: &mut Context<Self>,
    ) {
        let Some(auth) = &self.auth else { return };
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == device_id) {
            d.icon_kind = icon_kind.clone();
            d.accent_color = accent_color.clone();
        }
        for a in self.agenda.iter_mut().filter(|a| a.device_id == Some(device_id)) {
            a.icon_kind = icon_kind.clone();
            a.accent_color = accent_color.clone();
        }
        cx.notify();
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .patch(format!("{base}/api/v1/devices/{device_id}/appearance"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({
                            "icon_kind": icon_kind,
                            "accent_color": accent_color,
                        }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        Err(format!("HTTP {}", r.status()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Err(e) = res {
                    this.group_action_msg = format!("Error guardando apariencia: {e}");
                    this.load_devices(cx);
                } else {
                    this.group_action_msg = "Apariencia del equipo actualizada.".into();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn save_device_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let Some((device_id, agenda_id, _)) = self.editing_device.take() else { return };
        let alias = self.agenda_alias_in.read(cx).value().trim().to_string();
        if alias.is_empty() {
            cx.notify();
            return;
        }
        let company = self.agenda_company_in.read(cx).value().trim().to_string();
        let contact = self.agenda_contact_in.read(cx).value().trim().to_string();
        let notes = self.agenda_notes_in.read(cx).value().trim().to_string();
        let icon_kind = if self.edit_icon_kind.trim().is_empty() {
            "desktop".to_string()
        } else {
            self.edit_icon_kind.clone()
        };
        let accent_color = self.edit_accent_color.clone();
        let group_id = self
            .new_agenda_group_idx
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .map(|g| g.id);
        let group_name = group_id
            .and_then(|gid| self.groups.iter().find(|g| g.id == gid).map(|g| g.name.clone()))
            .unwrap_or_else(|| "General".into());
        self.agenda_alias_in = cx.new(|cx| InputState::new(window, cx).placeholder("Alias / PC"));
        self.agenda_company_in = cx.new(|cx| InputState::new(window, cx).placeholder("Empresa"));
        self.agenda_contact_in = cx.new(|cx| InputState::new(window, cx).placeholder("Contacto"));
        self.agenda_notes_in = cx.new(|cx| InputState::new(window, cx).placeholder("Notas (horario, detalles)"));

        let base = self.base_url(cx);
        let tok = auth.token.clone();
        let payload = UpsertAddressBookRequest {
            device_id,
            group_id,
            alias,
            client_company: company,
            contact_name: contact,
            contact_email: String::new(),
            group_name,
            is_favorite: true,
            notes,
            scheduled_next_at: None,
            icon_kind: icon_kind.clone(),
            accent_color: accent_color.clone(),
        };
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let c = reqwest::blocking::Client::new();
                    if let Some(did) = device_id {
                        let _ = c
                            .patch(format!("{base}/api/v1/devices/{did}/group"))
                            .bearer_auth(&tok)
                            .json(&serde_json::json!({ "group_id": group_id }))
                            .send();
                        let _ = c
                            .patch(format!("{base}/api/v1/devices/{did}/appearance"))
                            .bearer_auth(&tok)
                            .json(&serde_json::json!({
                                "icon_kind": icon_kind,
                                "accent_color": accent_color,
                            }))
                            .send();
                    }
                    if let Some(aid) = agenda_id {
                        let _ = c.patch(format!("{base}/api/v1/address-book/{aid}")).bearer_auth(&tok).json(&payload).send();
                    } else {
                        let _ = c.post(format!("{base}/api/v1/address-book")).bearer_auth(&tok).json(&payload).send();
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.load_agenda_groups(cx);
                this.load_devices(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let name = self.group_name_in.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.group_action_msg = "Escribí el nombre del grupo.".into();
            cx.notify();
            return;
        }
        let desc = self.group_desc_in.read(cx).value().trim().to_string();
        self.group_name_in = cx.new(|cx| InputState::new(window, cx).placeholder("Nombre del grupo"));
        self.group_desc_in = cx.new(|cx| InputState::new(window, cx).placeholder("Descripción"));
        self.group_modal = None;
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/groups"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({ "name": name, "description": desc }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<DeviceGroupDto>().map_err(|e| e.to_string())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(grp) => {
                        this.expanded_groups.insert(grp.id);
                        this.oc_selection = Some(OcSelection::Group(grp.id));
                        this.group_action_msg = format!("Grupo «{}» creado.", grp.name);
                        this.load_agenda_groups(cx);
                    }
                    Err(e) => {
                        this.group_action_msg = format!("Error creando grupo: {e}");
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn update_group(&mut self, gid: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let name = self.group_name_in.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.group_action_msg = "El nombre del grupo no puede estar vacío.".into();
            cx.notify();
            return;
        }
        let desc = self.group_desc_in.read(cx).value().trim().to_string();
        self.group_name_in = cx.new(|cx| InputState::new(window, cx).placeholder("Nombre del grupo"));
        self.group_desc_in = cx.new(|cx| InputState::new(window, cx).placeholder("Descripción"));
        self.group_modal = None;
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .patch(format!("{base}/api/v1/groups/{gid}"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({ "name": name, "description": desc }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        r.json::<DeviceGroupDto>().map_err(|e| e.to_string())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(grp) => {
                        this.group_action_msg = format!("Grupo actualizado: «{}».", grp.name);
                        this.load_agenda_groups(cx);
                        this.load_devices(cx);
                    }
                    Err(e) => {
                        this.group_action_msg = format!("Error actualizando grupo: {e}");
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn delete_group(&mut self, gid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        if self.oc_selection == Some(OcSelection::Group(gid)) {
            self.oc_selection = None;
        }
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .delete(format!("{base}/api/v1/groups/{gid}"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| {
                this.group_action_msg = "Grupo eliminado.".into();
                this.load_agenda_groups(cx);
                this.load_devices(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn share_group(&mut self, gid: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let email = self.share_email_in.read(cx).value().trim().to_string();
        if email.is_empty() {
            self.group_action_msg = "Escribí el correo del usuario a compartir.".into();
            cx.notify();
            return;
        }
        let perm = self.share_perm.clone();
        self.share_email_in = cx.new(|cx| InputState::new(window, cx).placeholder("usuario@empresa.com"));
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        let email_ok = email.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/groups/{gid}/shares"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({ "user_email": email, "permission": perm }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        let body = r.text().unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                            .unwrap_or(body);
                        Err(msg)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.group_action_msg = format!("Grupo compartido con {email_ok}.");
                        this.load_agenda_groups(cx);
                    }
                    Err(e) => {
                        this.group_action_msg = format!("Error compartiendo grupo: {e}");
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn unshare_group(&mut self, gid: Uuid, uid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .delete(format!("{base}/api/v1/groups/{gid}/shares/{uid}"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_agenda_groups(cx)).ok();
        })
        .detach();
    }

    pub fn load_tickets(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/tickets"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<SupportTicketDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(t) = res {
                    this.tickets = t;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn claim_ticket(&mut self, tid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/tickets/{tid}/claim"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_tickets(cx)).ok();
        })
        .detach();
    }

    pub fn resolve_ticket(&mut self, tid: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let summary = self.resolve_summary_input.read(cx).value().trim().to_string();
        let outcome = self.resolve_outcome.clone();
        self.resolving_ticket = None;
        self.resolve_summary_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Resumen de resolución del incidente…"));
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/tickets/{tid}/resolve"))
                        .bearer_auth(tok)
                        .json(&ResolveTicketRequest {
                            outcome,
                            resolution_summary: if summary.is_empty() {
                                "Resuelto desde cliente desktop".into()
                            } else {
                                summary
                            },
                        })
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_tickets(cx)).ok();
        })
        .detach();
    }

    pub fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/sessions"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<SessionDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(s) = res {
                    this.sessions = s;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn terminate_server_session(&mut self, sid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/sessions/{sid}/terminate"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_sessions(cx)).ok();
        })
        .detach();
    }

    pub fn run_rmm_diagnostic(&mut self, did: Uuid, kind: &str, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        let k = kind.to_string();
        self.rmm_diag_output = format!("Consultando {kind}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(20))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/devices/{did}/diagnostics"))
                        .bearer_auth(tok)
                        .json(&RunDiagnosticRequest { kind: k })
                        .send()
                        .map_err(|e| e.to_string())?
                        .json::<serde_json::Value>()
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        if let Some(hw) = extract_resources(&v) {
                            this.rmm_hw_snapshot = Some(hw);
                        }
                        this.rmm_diag_output = serde_json::to_string_pretty(&v).unwrap_or_default();
                    }
                    Err(e) => this.rmm_diag_output = format!("Error: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_device_alerts(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let Some(dev) = self.devices.get(self.rmm_device_idx) else { return };
        let did = dev.id;
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/devices/{did}/alerts"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<DeviceAlertDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(a) = res {
                    this.device_alerts = a;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn acknowledge_alert(&mut self, al_id: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/alerts/{al_id}/acknowledge"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_device_alerts(cx)).ok();
        })
        .detach();
    }

    pub fn run_rmm_command(&mut self, did: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let cmd = self.rmm_command_in.read(cx).value().trim().to_string();
        if cmd.is_empty() {
            return;
        }
        let shell = self.rmm_shell.clone();
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.rmm_cmd_output = format!("Ejecutando {cmd}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(25))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/devices/{did}/command"))
                        .bearer_auth(tok)
                        .json(&RunCommandRequest {
                            shell,
                            command: cmd,
                            timeout_secs: 20,
                        })
                        .send()
                        .map_err(|e| e.to_string())?
                        .json::<RmmCommandDto>()
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(c) => {
                        this.rmm_cmd_output = format!(
                            "$ {} ({} ms, exit={})\n{}{}",
                            c.command,
                            c.duration_ms,
                            c.exit_code,
                            c.stdout,
                            if c.stderr.is_empty() {
                                String::new()
                            } else {
                                format!("\n[stderr]\n{}", c.stderr)
                            }
                        );
                    }
                    Err(e) => this.rmm_cmd_output = format!("Error: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_calendar_docs(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let c = reqwest::blocking::Client::new();
                    let cal = c.get(format!("{base}/api/v1/calendar")).bearer_auth(&tok).send().ok()?.json::<Vec<CalendarEventDto>>().ok()?;
                    let docs = c.get(format!("{base}/api/v1/docs")).send().ok()?.json::<Vec<DocPageDto>>().ok()?;
                    Some((cal, docs))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some((cal, docs)) = res {
                    this.calendar = cal;
                    this.docs = docs;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn create_calendar_event(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let title = self.cal_title_in.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let client_name = self.cal_client_in.read(cx).value().trim().to_string();
        self.cal_title_in = cx.new(|cx| InputState::new(window, cx).placeholder("Título mantenimiento"));
        self.cal_client_in = cx.new(|cx| InputState::new(window, cx).placeholder("Cliente"));
        let start = chrono::Utc::now() + chrono::Duration::hours(2);
        let end = start + chrono::Duration::minutes(60);
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/calendar"))
                        .bearer_auth(tok)
                        .json(&CreateCalendarEventRequest {
                            device_id: None,
                            title,
                            client_name,
                            starts_at: start,
                            ends_at: end,
                            notes: String::new(),
                        })
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_calendar_docs(cx)).ok();
        })
        .detach();
    }

    pub fn delete_calendar_event(&mut self, eid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .delete(format!("{base}/api/v1/calendar/{eid}"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_calendar_docs(cx)).ok();
        })
        .detach();
    }

    pub fn export_sessions_csv(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.report_status = "Exportando CSV…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let txt = reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/reports/sessions.csv"))
                        .bearer_auth(tok)
                        .send()
                        .map_err(|e| e.to_string())?
                        .text()
                        .map_err(|e| e.to_string())?;
                    let dir = std::env::var("USERPROFILE")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| std::env::temp_dir())
                        .join("Downloads")
                        .join("AgentDesk");
                    let _ = std::fs::create_dir_all(&dir);
                    let path = dir.join("agentdesk-sessions.csv");
                    std::fs::write(&path, txt).map_err(|e| e.to_string())?;
                    Ok::<_, String>(path.display().to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.report_status = match res {
                    Ok(p) => format!("CSV guardado en {p}"),
                    Err(e) => format!("Error CSV: {e}"),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_audit(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/audit"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<AuditEventDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(a) = res {
                    this.audit = a;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_users(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/users"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<UserDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(u) = res {
                    this.users = u;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn create_user(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let email = self.new_user_email_in.read(cx).value().trim().to_string();
        let display_name = self.new_user_name_in.read(cx).value().trim().to_string();
        let password = self.new_user_pw_in.read(cx).value().to_string();
        if email.is_empty() || password.len() < 6 {
            return;
        }
        let role = self.new_user_role.clone();
        self.new_user_email_in = cx.new(|cx| InputState::new(window, cx).placeholder("nuevo@empresa.com"));
        self.new_user_name_in = cx.new(|cx| InputState::new(window, cx).placeholder("Nombre visible"));
        self.new_user_pw_in = cx.new(|cx| InputState::new(window, cx).placeholder("Contraseña inicial").masked(true));
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/users"))
                        .bearer_auth(tok)
                        .json(&CreateUserRequest {
                            email,
                            display_name,
                            password,
                            role,
                        })
                        .send();
                })
                .await;
            this.update(cx, |this, cx| this.load_users(cx)).ok();
        })
        .detach();
    }

    pub fn setup_mfa(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/auth/mfa/setup"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<MfaSetupResponse>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                this.mfa_setup = res;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn verify_mfa(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let code = self.mfa_verify_in.read(cx).value().trim().to_string();
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let ok = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/auth/mfa/verify"))
                        .bearer_auth(tok)
                        .json(&MfaVerifyRequest { code })
                        .send()
                        .map(|r| r.status().is_success())
                        .unwrap_or(false)
                })
                .await;
            this.update(cx, |this, cx| {
                this.server_status = if ok {
                    "MFA TOTP activado".into()
                } else {
                    "Código MFA inválido".into()
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn is_ai_connected(&self) -> bool {
        let Some(_) = &self.auth else {
            return false;
        };
        let Some(s) = &self.ai_settings else {
            return false;
        };
        match s.provider.as_str() {
            "chat_gpt_o_auth" => s.oauth_connected,
            "openai_api" => s.has_api_key,
            "ollama_local" => true,
            _ => false,
        }
    }

    pub fn load_ai_settings(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/ai/settings"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<AiSettingsDto>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(s) = res {
                    this.ai_provider = s.provider.clone();
                    this.ai_autonomy = s.autonomy_level.clone();
                    this.ai_redact_pii = s.redact_pii;
                    if !s.model.trim().is_empty() {
                        this.ai_selected_model = s.model.clone();
                        this.ai_chat_model = s.model.clone();
                    }
                    if !s.reasoning_effort.trim().is_empty() {
                        this.ai_reasoning_effort = s.reasoning_effort.clone();
                        this.ai_chat_effort = s.reasoning_effort.clone();
                    }
                    let should_list = match s.provider.as_str() {
                        "chat_gpt_o_auth" => s.oauth_connected,
                        "openai_api" => s.has_api_key,
                        "ollama_local" => true,
                        _ => false,
                    };
                    this.ai_status = if should_list {
                        format!("Conectado ({}) · {} ({})", s.provider, s.model, s.reasoning_effort)
                    } else {
                        "No estás conectado a ningún proveedor de IA.".into()
                    };
                    this.ai_settings = Some(s);
                    if should_list {
                        this.load_ai_models(cx);
                        this.load_ai_chat_sessions(cx);
                    } else {
                        this.ai_models.clear();
                        this.ai_models_info.clear();
                        this.ai_models_loaded = false;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_ai_models(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(15))
                        .build()
                        .ok()?
                        .get(format!("{base}/api/v1/ai/models"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<serde_json::Value>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(v) = res {
                    let mut ids = Vec::new();
                    let mut infos = Vec::new();
                    if let Some(arr) = v.get("models").and_then(|m| m.as_array()) {
                        for item in arr {
                            let Some(id) = item.get("id").and_then(|x| x.as_str()) else {
                                continue;
                            };
                            let label = item
                                .get("label")
                                .and_then(|x| x.as_str())
                                .unwrap_or(id)
                                .to_string();
                            let efforts = item
                                .get("reasoning_efforts")
                                .and_then(|x| x.as_array())
                                .map(|ea| {
                                    ea.iter()
                                        .filter_map(|e| e.as_str().map(str::to_string))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_else(|| {
                                    vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()]
                                });
                            ids.push(id.to_string());
                            infos.push((id.to_string(), label, efforts));
                        }
                    }
                    this.ai_models = ids;
                    this.ai_models_info = infos;
                    this.ai_models_loaded = true;
                    if let Some(sel) = v.get("selected_model").and_then(|x| x.as_str()) {
                        this.ai_selected_model = sel.to_string();
                        this.ai_chat_model = sel.to_string();
                        if let Some(st) = &mut this.ai_settings {
                            st.model = sel.to_string();
                        }
                    }
                    if let Some(eff) = v.get("reasoning_effort").and_then(|x| x.as_str()) {
                        this.ai_reasoning_effort = eff.to_string();
                        this.ai_chat_effort = eff.to_string();
                        if let Some(st) = &mut this.ai_settings {
                            st.reasoning_effort = eff.to_string();
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn select_ai_model(
        &mut self,
        model: String,
        effort: Option<String>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if !model.trim().is_empty() {
            self.ai_selected_model = model.clone();
            self.ai_chat_model = model.clone();
            if let Some(w) = window {
                self.ai_model_in = cx.new(|cx| {
                    InputState::new(w, cx)
                        .placeholder("gpt-6-luna")
                        .default_value(model.clone())
                });
            }
            if let Some(st) = &mut self.ai_settings {
                st.model = model;
            }
        }
        if let Some(eff) = effort {
            self.ai_reasoning_effort = eff.clone();
            self.ai_chat_effort = eff.clone();
            if let Some(st) = &mut self.ai_settings {
                st.reasoning_effort = eff;
            }
        }
        self.save_ai_settings(cx);
    }

    pub fn save_ai_settings(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        let key_val = self.ai_api_key_in.read(cx).value().trim().to_string();
        let typed_model = self.ai_model_in.read(cx).value().trim().to_string();
        let chosen_model = if self.ai_provider == "chat_gpt_o_auth" || !self.ai_selected_model.trim().is_empty() {
            if self.ai_selected_model.trim().is_empty() {
                typed_model
            } else {
                self.ai_selected_model.clone()
            }
        } else {
            typed_model
        };
        let payload = UpdateAiSettingsRequest {
            provider: self.ai_provider.clone(),
            model: chosen_model,
            base_url: self.ai_base_url_in.read(cx).value().trim().to_string(),
            api_key: if key_val.is_empty() { None } else { Some(key_val) },
            oauth_client_id: None,
            oauth_auth_url: None,
            oauth_token_url: None,
            oauth_manual_token: None,
            autonomy_level: self.ai_autonomy.clone(),
            redact_pii: self.ai_redact_pii,
            reasoning_effort: self.ai_reasoning_effort.clone(),
        };
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .put(format!("{base}/api/v1/ai/settings"))
                        .bearer_auth(tok)
                        .json(&payload)
                        .send()
                        .ok()?
                        .json::<AiSettingsDto>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(s) = res {
                    this.ai_selected_model = s.model.clone();
                    this.ai_reasoning_effort = s.reasoning_effort.clone();
                    this.ai_chat_model = s.model.clone();
                    this.ai_chat_effort = s.reasoning_effort.clone();
                    this.ai_status = format!("Configuración guardada · {} ({})", s.model, s.reasoning_effort);
                    this.ai_settings = Some(s);
                    this.load_ai_models(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn load_ai_chat_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        let did_opt = self
            .ai_device_idx
            .checked_sub(1)
            .and_then(|i| self.devices.get(i))
            .map(|d| d.id);
        cx.spawn(async move |this, cx| {
            let url = match did_opt {
                Some(did) => format!("{base}/api/v1/ai/sessions?device_id={did}"),
                None => format!("{base}/api/v1/ai/sessions"),
            };
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(url)
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<AiChatSessionSummaryDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(list) = res {
                    this.ai_chat_sessions = list;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn open_ai_chat_session(&mut self, sid: Uuid, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.ai_chat_session_id = Some(sid);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .get(format!("{base}/api/v1/ai/sessions/{sid}/messages"))
                        .bearer_auth(tok)
                        .send()
                        .ok()?
                        .json::<Vec<AiChatMessageDto>>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(msgs) = res {
                    if let Some(last) = msgs.last() {
                        this.ai_reply = last.reply.clone();
                        this.ai_proposed_actions = last.proposed_actions.clone();
                        if !last.model_used.is_empty() {
                            this.ai_chat_model = last.model_used.clone();
                        }
                        if !last.reasoning_effort.is_empty() {
                            this.ai_chat_effort = last.reasoning_effort.clone();
                        }
                    }
                    this.ai_chat_messages = msgs;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn start_new_ai_session(&mut self, cx: &mut Context<Self>) {
        self.ai_chat_session_id = None;
        self.ai_chat_messages.clear();
        self.ai_reply.clear();
        self.ai_proposed_actions.clear();
        cx.notify();
    }

    pub fn disconnect_chatgpt(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    let _ = reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/ai/oauth/disconnect"))
                        .bearer_auth(tok)
                        .send();
                })
                .await;
            this.update(cx, |this, cx| {
                this.ai_models.clear();
                this.ai_models_info.clear();
                this.ai_models_loaded = false;
                if let Some(st) = &mut this.ai_settings {
                    st.oauth_connected = false;
                }
                this.ai_status = "No estás conectado a ningún proveedor de IA.".into();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn complete_oauth_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let cb = self.ai_callback_paste_in.read(cx).value().trim().to_string();
        if cb.is_empty() {
            return;
        }
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.ai_callback_paste_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Pegá aquí la URL de callback http://localhost:1455/auth/callback?code=… (opcional)")
        });
        self.ai_status = "Completando intercambio OAuth…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(15))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/ai/oauth/complete"))
                        .bearer_auth(tok)
                        .json(&serde_json::json!({ "callback_url": cb }))
                        .send()
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        Err(r.text().unwrap_or_else(|_| "Error OAuth".into()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.ai_status = "ChatGPT OAuth conectado. Cargando modelos…".into();
                        this.load_ai_settings(cx);
                    }
                    Err(e) => {
                        this.ai_status = format!("Error completando OAuth: {e}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn connect_chatgpt(&mut self, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.oauth_device_code = None;
        self.ai_status = "Iniciando Device Code OAuth (OpenCode · sin conflicto de puertos)…".into();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    let resp = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(15))
                        .build()
                        .ok()?
                        .post(format!("{base}/api/v1/ai/oauth/device/start"))
                        .bearer_auth(tok.clone())
                        .send()
                        .ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let v = resp.json::<serde_json::Value>().ok()?;
                    let code = v.get("user_code").and_then(|x| x.as_str())?.to_string();
                    let url = v.get("verify_url").and_then(|x| x.as_str())?.to_string();
                    Some((code, url, tok, base))
                })
                .await;

            let opened = this
                .update(cx, |this, cx| {
                    if let Some((code, url, _, _)) = &started {
                        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                        cx.open_url(url);
                        this.oauth_device_code = Some((code.clone(), url.clone()));
                        this.ai_status = format!(
                            "Código {code} copiado al portapapeles. Pegalo en auth.openai.com/codex/device…"
                        );
                        cx.notify();
                        true
                    } else {
                        this.ai_status = "No se pudo iniciar ChatGPT Device OAuth.".into();
                        cx.notify();
                        false
                    }
                })
                .unwrap_or(false);
            if !opened {
                return;
            }
            let Some((_, _, tok, base)) = started else { return };
            let connected = cx
                .background_executor()
                .spawn(async move {
                    let c = reqwest::blocking::Client::new();
                    for _ in 0..180 {
                        std::thread::sleep(Duration::from_secs(2));
                        if let Ok(resp) = c
                            .get(format!("{base}/api/v1/ai/settings"))
                            .bearer_auth(&tok)
                            .send()
                        {
                            if let Ok(s) = resp.json::<AiSettingsDto>() {
                                if s.oauth_connected {
                                    return true;
                                }
                            }
                        }
                    }
                    false
                })
                .await;

            this.update(cx, |this, cx| {
                this.oauth_device_code = None;
                if connected {
                    this.ai_status = "ChatGPT OAuth conectado. Listando modelos…".into();
                    this.load_ai_settings(cx);
                } else {
                    this.ai_status = "Tiempo agotado esperando confirmación en auth.openai.com/codex/device.".into();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn ask_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = &self.auth else {
            self.ai_status = "Iniciá sesión y conectá un proveedor de IA.".into();
            cx.notify();
            return;
        };
        if !self.is_ai_connected() {
            self.ai_status = "No estás conectado a ningún proveedor de IA.".into();
            cx.notify();
            return;
        }
        let prompt = self.ai_prompt.read(cx).value().trim().to_string();
        if prompt.is_empty() || self.ai_busy {
            return;
        }
        let device_id = self
            .ai_device_idx
            .checked_sub(1)
            .and_then(|i| self.devices.get(i))
            .map(|d| d.id);
        let include_diagnostics = self.ai_include_diag && device_id.is_some();
        let active_model = if self.ai_chat_model.trim().is_empty() {
            self.ai_selected_model.clone()
        } else {
            self.ai_chat_model.clone()
        };
        let active_effort = if self.ai_chat_effort.trim().is_empty() {
            self.ai_reasoning_effort.clone()
        } else {
            self.ai_chat_effort.clone()
        };
        let chat_session_id = self.ai_chat_session_id;
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        self.ai_prompt = cx.new(|cx| InputState::new(window, cx).placeholder("Preguntá sobre un equipo o un problema…"));
        self.bind_ai_prompt_enter(&self.ai_prompt, window, cx);
        self.ai_busy = true;
        self.ai_reply.clear();
        cx.notify();
        let prompt_sent = prompt.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(90))
                        .build()
                        .map_err(|e| e.to_string())?
                        .post(format!("{base}/api/v1/ai/chat"))
                        .bearer_auth(tok)
                        .json(&AiChatRequest {
                            device_id,
                            ticket_id: None,
                            prompt,
                            include_diagnostics,
                            include_screenshot: false,
                            chat_session_id,
                            model: if active_model.is_empty() { None } else { Some(active_model) },
                            reasoning_effort: if active_effort.is_empty() { None } else { Some(active_effort) },
                        })
                        .send()
                        .map_err(|e| e.to_string())?;
                    if !r.status().is_success() {
                        let txt = r.text().unwrap_or_default();
                        return Err(if txt.is_empty() { "Error HTTP".into() } else { txt });
                    }
                    r.json::<AiChatResponse>().map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.ai_busy = false;
                match res {
                    Ok(a) => {
                        let sid = a.chat_session_id.unwrap_or_else(Uuid::new_v4);
                        this.ai_chat_session_id = Some(sid);
                        this.ai_reply = a.reply.clone();
                        this.ai_proposed_actions = a.proposed_actions.clone();
                        this.ai_chat_messages.push(AiChatMessageDto {
                            id: Uuid::new_v4(),
                            chat_session_id: sid,
                            device_id,
                            prompt: prompt_sent,
                            reply: a.reply,
                            model_used: a.model_used.clone(),
                            reasoning_effort: a.reasoning_effort.clone(),
                            proposed_actions: a.proposed_actions,
                            created_at: chrono::Utc::now(),
                        });
                        this.ai_status = format!("{} · {} ({})", a.provider_used, a.model_used, a.reasoning_effort);
                        this.load_ai_chat_sessions(cx);
                    }
                    Err(e) => this.ai_status = format!("Error IA: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn execute_ai_action(
        &mut self,
        device_id: Uuid,
        shell: String,
        command: String,
        rationale: String,
        cx: &mut Context<Self>,
    ) {
        let Some(auth) = &self.auth else { return };
        let base = self.base_url(cx);
        let tok = auth.token.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    reqwest::blocking::Client::new()
                        .post(format!("{base}/api/v1/ai/execute-action"))
                        .bearer_auth(tok)
                        .json(&AiExecuteActionRequest {
                            device_id,
                            shell,
                            command,
                            rationale,
                        })
                        .send()
                        .ok()?
                        .json::<RmmCommandDto>()
                        .ok()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(c) = res {
                    this.ai_reply.push_str(&format!("\n\n[Ejecutado exit={}]\n{}", c.exit_code, c.stdout));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_notifications_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let open_tickets: Vec<_> = self
            .tickets
            .iter()
            .filter(|t| t.status != "resolved" && t.status != "closed")
            .take(8)
            .cloned()
            .collect();
        let alerts = self.device_alerts.clone();
        let has_items = !alerts.is_empty() || !open_tickets.is_empty();

        div()
            .flex()
            .flex_col()
            .w(px(300.))
            .h_full()
            .p_4()
            .gap_3()
            .border_l_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child("No leídas"))
                    .child(
                        div()
                            .id("close-notif-panel")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(p.muted))
                            .hover(|s| s.bg(rgb(p.raised)))
                            .child(Icon::new(IconName::X).size(px(16.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_notifications = false;
                                cx.notify();
                            })),
                    ),
            )
            .when(!has_items, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_4()
                        .rounded_xl()
                        .bg(rgb(p.canvas))
                        .border_1()
                        .border_color(rgb(p.border))
                        .child(Icon::new(IconName::Inbox).size(px(34.)).text_color(rgb(p.muted)))
                        .child(div().text_sm().font_semibold().child("Todavía no hay notificaciones"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child("Las alertas y solicitudes de los últimos 30 días aparecerán aquí."),
                        ),
                )
            })
            .when(has_items, |d| {
                d.child(
                    div()
                        .id("notif-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .gap_2()
                        .overflow_y_scrollbar()
                        .children(alerts.into_iter().map(|al| {
                            div()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.canvas))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().font_semibold().text_color(rgb(p.danger)).child(al.severity.to_uppercase()))
                                .child(div().text_xs().child(al.summary))
                        }))
                        .children(open_tickets.into_iter().map(|t| {
                            let tid = t.id;
                            div()
                                .id(SharedString::from(format!("nf-tk-{tid}")))
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.canvas))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(p.raised)))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().font_semibold().text_color(rgb(p.accent)).child(format!("Ticket {}", t.public_code)))
                                .child(div().text_xs().child(format!("{} · {}", t.requester_name, t.description)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected_ticket_id = Some(tid);
                                    this.page = Page::Support;
                                    this.show_notifications = false;
                                    cx.notify();
                                }))
                        })),
                )
            })
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> Div {
        match self.page {
            Page::Home => self.render_home_v2(cx),
            Page::Devices | Page::Agenda => self.render_devices_v2(cx),
            Page::Support => self.render_support(cx),
            Page::Sessions => self.render_sessions(cx),
            Page::Monitoring => self.render_monitoring(cx),
            Page::Automation => self.render_automation(cx),
            Page::Reports => self.render_reports(cx),
            Page::Ai => {
                self.settings_section = "ai";
                self.page = Page::Settings;
                self.render_settings(cx)
            }
            Page::UsersMfa => {
                self.settings_section = "users";
                self.page = Page::Settings;
                self.render_settings(cx)
            }
            Page::Settings => self.render_settings(cx),
        }
    }

    fn render_ai_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let connected = self.is_ai_connected();
        let dev_idx = self.ai_device_idx;
        let dev_label = if dev_idx == 0 {
            "Equipo: General".to_string()
        } else {
            self.devices
                .get(dev_idx - 1)
                .map(|d| format!("Equipo: {}", d.hostname))
                .unwrap_or_else(|| "Equipo: General".into())
        };
        let active_model = if self.ai_chat_model.trim().is_empty() {
            self.ai_selected_model.clone()
        } else {
            self.ai_chat_model.clone()
        };
        let active_effort = if self.ai_chat_effort.trim().is_empty() {
            self.ai_reasoning_effort.clone()
        } else {
            self.ai_chat_effort.clone()
        };
        let current_efforts = self
            .ai_models_info
            .iter()
            .find(|(id, _, _)| id == &active_model)
            .map(|(_, _, e)| e.clone())
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()]);

        div()
            .flex()
            .flex_col()
            .w(px(360.))
            .h_full()
            .p_4()
            .gap_3()
            .border_l_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().font_semibold().child("AgentDesk AI"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(connected, |d| {
                                d.child(
                                    subtle_button("+ Nueva sesión", "ai-new-sess", p)
                                        .id("ai-new-sess")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_new_ai_session(cx);
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("close-ai-panel")
                                    .p_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_color(rgb(p.muted))
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .child(Icon::new(IconName::X).size(px(16.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_ai = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(!connected, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_5()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.canvas))
                        .child(Icon::new(IconName::BotOff).size(px(36.)).text_color(rgb(p.muted)))
                        .child(div().text_sm().font_semibold().child("No estás conectado a un proveedor de IA"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child("Conectate con ChatGPT OAuth, OpenAI API u Ollama para consultar con IA, elegir modelos (Sol / Luna) y guardar historial por equipo."),
                        )
                        .child(
                            action_button("Conectar con un proveedor", "ai-go-connect", p)
                                .id("ai-go-connect")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.auth.is_none() {
                                        this.login_open = true;
                                    } else {
                                        this.page = Page::Ai;
                                        this.ai_subtab = "oauth";
                                    }
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(connected, |d| {
                let sessions = self.ai_chat_sessions.clone();
                let msgs = self.ai_chat_messages.clone();
                let active_sid = self.ai_chat_session_id;
                let models_info = self.ai_models_info.clone();
                let picker_open = self.ai_chat_model_picker_open;

                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            subtle_button(dev_label, "chat-pick-dev", p)
                                .id("chat-pick-dev")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let n = this.devices.len() + 1;
                                    this.ai_device_idx = (this.ai_device_idx + 1) % n;
                                    this.start_new_ai_session(cx);
                                    this.load_ai_chat_sessions(cx);
                                })),
                        )
                        .child(
                            subtle_button(
                                if self.ai_include_diag { "Con RMM" } else { "Sin RMM" },
                                "chat-tog-rmm",
                                p,
                            )
                            .id("chat-tog-rmm")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ai_include_diag = !this.ai_include_diag;
                                cx.notify();
                            })),
                        ),
                )
                .when(!sessions.is_empty(), |d2| {
                    d2.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .children(sessions.into_iter().take(4).map(|s| {
                                let sid = s.chat_session_id;
                                let is_cur = active_sid == Some(sid);
                                let lbl = if s.title.is_empty() {
                                    "Sesión".to_string()
                                } else {
                                    s.title.chars().take(20).collect()
                                };
                                subtle_button(lbl, format!("csess-{sid}"), p)
                                    .id(SharedString::from(format!("csess-{sid}")))
                                    .when(is_cur, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_ai_chat_session(sid, cx);
                                    }))
                            })),
                    )
                })
                .child(
                    div()
                        .id("ai-chat-msgs-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.canvas))
                        .overflow_y_scrollbar()
                        .text_sm()
                        .when(msgs.is_empty() && !self.ai_busy, |d2| {
                            d2.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child("Iniciá una consulta para diagnosticar o resolver problemas en el equipo."),
                            )
                        })
                        .children(msgs.into_iter().map(|m| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .pb_2()
                                .border_b_1()
                                .border_color(rgb(p.border))
                                .child(
                                    div()
                                        .p_2()
                                        .rounded_md()
                                        .bg(rgb(p.active))
                                        .text_xs()
                                        .child(format!("Tú: {}", m.prompt)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child(format!("{} ({})", m.model_used, m.reasoning_effort)),
                                )
                                .child(div().text_sm().child(m.reply))
                        }))
                        .when(self.ai_busy, |d2| {
                            d2.child(div().text_xs().text_color(rgb(p.accent)).child("Consultando con IA…"))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(
                            subtle_button(active_model.to_string(), "chat-model-tog", p)
                                .id("chat-model-tog")
                                .when(picker_open, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.ai_chat_model_picker_open = !this.ai_chat_model_picker_open;
                                    if this.ai_chat_model_picker_open && this.ai_models_info.is_empty() {
                                        this.load_ai_models(cx);
                                    }
                                    cx.notify();
                                })),
                        )
                        .children(current_efforts.into_iter().map(|eff| {
                            let e_click = eff.clone();
                            let is_eff = eff == active_effort;
                            subtle_button(eff.clone(), format!("ceff-{eff}"), p)
                                .id(SharedString::from(format!("ceff-{eff}")))
                                .when(is_eff, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ai_chat_effort = e_click.clone();
                                    cx.notify();
                                }))
                        })),
                )
                .when(picker_open, |d2| {
                    d2.child(
                        div()
                            .id("chat-model-list")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .max_h(px(140.))
                            .overflow_y_scrollbar()
                            .p_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.canvas))
                            .when(models_info.is_empty(), |m| {
                                m.child(div().text_xs().text_color(rgb(p.muted)).child("Cargando modelos…"))
                            })
                            .children(models_info.into_iter().map(|(id, label, _)| {
                                let id_c = id.clone();
                                let is_cur = id == active_model;
                                div()
                                    .id(SharedString::from(format!("cm-pick-{id}")))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(if is_cur { p.active } else { p.canvas }))
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .text_xs()
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.ai_chat_model = id_c.clone();
                                        this.ai_chat_model_picker_open = false;
                                        cx.notify();
                                    }))
                            })),
                    )
                })
                .child(div().text_xs().text_color(rgb(p.muted)).child(self.ai_status.clone()))
                .child(Input::new(&self.ai_prompt))
                .child(
                    action_button(
                        if self.ai_busy { "Consultando con IA…" } else { "Consultar con IA" },
                        "ask-ai",
                        p,
                    )
                    .id("ask-ai")
                    .on_click(cx.listener(|this, _, window, cx| this.ask_ai(window, cx))),
                )
            })
    }

    fn render_search_dropdown(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let raw = self.global_search.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return None;
        }
        let q = raw.to_lowercase();
        let qn = q.replace(' ', "");
        let digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();

        let mut list = div()
            .absolute()
            .top_12()
            .left(px(260.))
            .w(px(460.))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .shadow_lg()
            .text_color(rgb(p.text));

        let mut count = 0usize;
        if digits.len() >= 6 {
            let d_fill = digits.clone();
            list = list.child(
                div()
                    .id("srch-fill-id")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.raised)))
                    .text_xs()
                    .child(format!("Conectar al ID {d_fill}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.global_search = cx.new(|cx| {
                            InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
                        });
                        this.bind_global_search_enter(window, cx);
                        this.initiate_connect_by_id(d_fill.clone(), this.partner_allow_control, Some(window), cx);
                    })),
            );
            count += 1;
        }

        for (idx, h) in self.cfg.connection_history.clone().into_iter().enumerate() {
            if count >= 8 { break; }
            if h.hostname.to_lowercase().contains(&q) || normalize_conn_code(&h.connection_id).to_lowercase().contains(&qn) {
                list = list.child(
                    div()
                        .id(SharedString::from(format!("srch-hist-{idx}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.raised)))
                        .text_xs()
                        .child(format!("Historial: {} ({})", h.hostname, h.connection_id))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.global_search = cx.new(|cx| {
                                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
                            });
                            this.bind_global_search_enter(window, cx);
                            this.reconnect_from_history(idx, Some(window), cx);
                        })),
                );
                count += 1;
            }
        }

        for d in self.devices.clone() {
            if count >= 8 { break; }
            if d.hostname.to_lowercase().contains(&q) || normalize_conn_code(&d.public_code).to_lowercase().contains(&qn) {
                let did = d.id;
                let host = d.hostname.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("srch-dev-{did}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.raised)))
                        .text_xs()
                        .child(format!("Equipo: {} ({}) [{}]", d.hostname, d.public_code, d.status))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.global_search = cx.new(|cx| {
                                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
                            });
                            this.bind_global_search_enter(window, cx);
                            this.start_managed_session(did, None, true, host.clone(), cx);
                        })),
                );
                count += 1;
            }
        }

        for g in self.groups.clone() {
            if count >= 8 { break; }
            if g.name.to_lowercase().contains(&q) {
                let gid = g.id;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("srch-grp-{gid}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.raised)))
                        .text_xs()
                        .child(format!("Grupo: {} ({} equipos)", g.name, g.device_count))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.global_search = cx.new(|cx| {
                                InputState::new(window, cx).placeholder("Buscar ID, equipo, grupo o historial…")
                            });
                            this.bind_global_search_enter(window, cx);
                            this.page = Page::Agenda;
                            cx.notify();
                        })),
                );
                count += 1;
            }
        }

        if count == 0 {
            list = list.child(div().text_xs().text_color(rgb(p.muted)).child("Sin coincidencias"));
        }
        Some(list)
    }

    fn render_connect_auth_popup(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let pop = self.connect_auth_popup.clone()?;
        Some(
            div()
                .id("connect-auth-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .id("connect-auth-dialog")
                        .on_click(cx.listener(|_, _, _, _| {}))
                        .w(px(410.))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .p_5()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .shadow_2xl()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    img(ImageSource::Render(self.brand_logo.clone()))
                                        .size(px(34.))
                                        .rounded_md(),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_bold()
                                                .child("Autenticación de AgentDesk"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(p.muted))
                                                .child(format!(
                                                    "{} ({})",
                                                    pop.hostname, pop.connection_id
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.text))
                                .child("Introducí la contraseña que aparece en el equipo de tu asociado:"),
                        )
                        .child(Input::new(&self.connect_auth_pw_input).mask_toggle())
                        .when_some(pop.error.clone(), |b, err| {
                            b.child(div().text_xs().text_color(rgb(p.danger)).child(err))
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("Cancelar", "conn-auth-cancel", p)
                                        .id("conn-auth-cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.connect_auth_popup = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button(
                                        if pop.busy {
                                            "Conectando…"
                                        } else {
                                            "Iniciar sesión"
                                        },
                                        "conn-auth-submit",
                                        p,
                                    )
                                    .id("conn-auth-submit")
                                    .on_click(cx.listener({
                                        let busy = pop.busy;
                                        move |this, _, _, cx| {
                                            if !busy {
                                                this.submit_connect_auth_popup(cx);
                                            }
                                        }
                                    })),
                                ),
                        ),
                ),
        )
    }

    fn render_login_dialog(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(400.))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .child(div().font_semibold().child("Iniciar sesión"))
                    .child(Input::new(&self.login_email))
                    .child(Input::new(&self.login_password).mask_toggle())
                    .when(self.mfa_required, |this| this.child(Input::new(&self.login_mfa)))
                    .when(!self.login_error.is_empty(), |this| {
                        this.child(div().text_xs().text_color(rgb(p.warning)).child(self.login_error.clone()))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                subtle_button("Cancelar", "cancel-login", p)
                                    .id("cancel-login")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.login_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button("Entrar", "submit-login", p)
                                    .id("submit-login")
                                    .on_click(cx.listener(|this, _, _, cx| this.login(cx))),
                            ),
                    ),
            )
    }

    fn render_change_pw_dialog(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(400.))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .child(div().font_semibold().child("Cambiar contraseña"))
                    .child(Input::new(&self.cur_pw_in).mask_toggle())
                    .child(Input::new(&self.new_pw_in).mask_toggle())
                    .child(Input::new(&self.new_pw_confirm_in).mask_toggle())
                    .when(!self.change_pw_msg.is_empty(), |this| {
                        this.child(div().text_xs().text_color(rgb(p.warning)).child(self.change_pw_msg.clone()))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                subtle_button("Cancelar", "cancel-cpw", p)
                                    .id("cancel-cpw")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.change_pw_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button("Guardar", "submit-cpw", p)
                                    .id("submit-cpw")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.change_password(window, cx);
                                    })),
                            ),
                    ),
            )
    }

    fn render_reconnect_dialog(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let (reason, target) = self.reconnect_prompt.clone()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(440.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(div().font_semibold().child("Conexión interrumpida"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(p.muted))
                                .child(format!(
                                    "Se detectó desconexión de {} ({reason}). ¿Querés activar auto-reconexión cuando el equipo vuelva a iniciar?",
                                    target.display_name()
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("No", "reconn-no", p)
                                        .id("reconn-no")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reconnect_prompt = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button("Sí, auto-reconectar", "reconn-yes", p)
                                        .id("reconn-yes")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.auto_reconnect = Some((
                                                target.clone(),
                                                Instant::now() + Duration::from_secs(10),
                                                0,
                                            ));
                                            this.reconnect_prompt = None;
                                            this.connect_status = "Auto-reconexión activada…".into();
                                            cx.notify();
                                        })),
                                ),
                        ),
                ),
        )
    }
}

impl Render for Client {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let show_ai = self.show_ai;
        let show_notif = self.show_notifications;
        let full_bleed = self.page.rail_page() == Page::Devices;
        let search_pop = self.render_search_dropdown(p, cx);
        let acct_pop = self.render_account_menu_overlay(p, cx);
        let reconn_pop = self.render_reconnect_dialog(p, cx);
        let ctx_pop = self.render_device_context_menu(p, cx);
        let grp_ctx_pop = self.render_group_context_menu(p, cx);
        let edit_pop = self.render_edit_device_dialog(p, cx);
        let group_modal_pop = self.render_group_modal(p, cx);
        let share_group_pop = self.render_share_group_modal(p, cx);
        let add_comp_pop = self.render_add_computer_modal(p, cx);
        let add_contact_pop = self.render_add_contact_modal(p, cx);
        let connect_auth_pop = self.render_connect_auth_popup(p, cx);
        let plus_pop = self.render_plus_menu(p, cx);
        let palette_pop = self.render_command_palette(p, cx);
        let incoming_pop = self.render_incoming_banner(p, cx);
        let rail = self.render_rail(cx);
        let topbar = self.render_topbar(cx);
        let page = self.render_page(cx);

        div()
            .id("client-root")
            .track_focus(&self.focus_handle)
            .key_context("AgentDeskClient")
            .on_action(cx.listener(Self::on_toggle_palette))
            .on_action(cx.listener(Self::on_close_overlays))
            .flex()
            .size_full()
            .relative()
            .bg(rgb(p.win))
            .text_color(rgb(p.text))
            .child(rail)
            .child(
                // Superficie principal redondeada junto al riel
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .mt(px(6.))
                    .rounded_tl(px(14.))
                    .border_t_1()
                    .border_l_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.canvas))
                    .overflow_hidden()
                    .child(topbar)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(if full_bleed {
                                div().flex_1().min_w_0().h_full().child(page).into_any_element()
                            } else {
                                div()
                                    .id("page-scroll")
                                    .flex_1()
                                    .min_w_0()
                                    .pt_1()
                                    .px(px(28.))
                                    .pb(px(28.))
                                    .overflow_y_scrollbar()
                                    .child(page)
                                    .into_any_element()
                            })
                            .when(show_ai, |this| this.child(self.render_ai_panel(cx)))
                            .when(show_notif, |this| this.child(self.render_notifications_panel(cx))),
                    ),
            )
            .when_some(incoming_pop, |this, pop| this.child(pop))
            .when_some(search_pop, |this, pop| this.child(pop))
            .when_some(plus_pop, |this, pop| this.child(pop))
            .when_some(acct_pop, |this, pop| this.child(pop))
            .when_some(ctx_pop, |this, pop| this.child(pop))
            .when_some(grp_ctx_pop, |this, pop| this.child(pop))
            .when_some(edit_pop, |this, pop| this.child(pop))
            .when_some(group_modal_pop, |this, pop| this.child(pop))
            .when_some(share_group_pop, |this, pop| this.child(pop))
            .when_some(add_comp_pop, |this, pop| this.child(pop))
            .when_some(add_contact_pop, |this, pop| this.child(pop))
            .when(self.login_open, |this| this.child(self.render_login_dialog(p, cx)))
            .when(self.change_pw_open, |this| this.child(self.render_change_pw_dialog(p, cx)))
            .when_some(connect_auth_pop, |this, pop| this.child(pop))
            .when_some(reconn_pop, |this, pop| this.child(pop))
            .when_some(palette_pop, |this, pop| this.child(pop))
    }
}

impl Client {
    fn render_account_menu_overlay(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.account_menu_open {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("account-menu-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.account_menu_open = false;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.account_menu_open = false;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("account-menu-card")
                        .absolute()
                        .bottom(px(12.))
                        .left(px(66.))
                        .w(px(260.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .shadow_lg()
                        .text_color(rgb(p.text))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .when_some(self.auth.as_ref(), |d, a| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .pb_2()
                                    .child(div().font_semibold().child(a.user.display_name.clone()))
                                    .child(div().text_xs().text_color(rgb(p.muted)).child(a.user.email.clone()))
                                    .child(div().text_xs().text_color(rgb(p.muted)).child(format!("{} · {}", a.user.organization_name, a.user.role))),
                            )
                        })
                        .when(self.auth.is_none(), |d| {
                            d.child(div().text_xs().text_color(rgb(p.muted)).child("Sin sesión iniciada"))
                                .child(
                                    action_button("Iniciar sesión", "dd-login", p)
                                        .id("dd-login")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.account_menu_open = false;
                                            this.login_open = true;
                                            this.login_error.clear();
                                            cx.notify();
                                        })),
                                )
                        })
                        .child(
                            div()
                                .id("dd-theme")
                                .px_2()
                                .py(px(6.))
                                .rounded_md()
                                .border_t_1()
                                .border_color(rgb(p.border))
                                .text_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(p.raised)))
                                .child(if self.dark {
                                    "Cambiar a tema claro"
                                } else {
                                    "Cambiar a tema oscuro"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dark = !this.dark;
                                    this.cfg.set_dark(this.dark);
                                    let _ = this.cfg.save(&this.config_path);
                                    apply_theme(
                                        cx,
                                        if this.dark { ThemeMode::Dark } else { ThemeMode::Light },
                                    );
                                    this.account_menu_open = false;
                                    cx.notify();
                                })),
                        )
                        .when(self.auth.is_some(), |d| {
                            d.child(
                                div()
                                    .id("dd-changepw")
                                    .px_2()
                                    .py(px(6.))
                                    .rounded_md()
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .child("Cambiar contraseña")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account_menu_open = false;
                                        this.change_pw_open = true;
                                        this.change_pw_msg.clear();
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            div()
                                .id("dd-settings")
                                .px_2()
                                .py(px(6.))
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(p.raised)))
                                .child("Configuración")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.account_menu_open = false;
                                    this.page = Page::Settings;
                                    cx.notify();
                                })),
                        )
                        .when(self.auth.is_some(), |d| {
                            d.child(
                                div()
                                    .id("dd-logout")
                                    .px_2()
                                    .py(px(6.))
                                    .rounded_md()
                                    .border_t_1()
                                    .border_color(rgb(p.border))
                                    .text_sm()
                                    .text_color(rgb(p.danger))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(p.raised)))
                                    .child("Cerrar sesión")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.logout(cx)
                                    })),
                            )
                        }),
                ),
        )
    }

    fn render_device_context_menu(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.dev_ctx_menu.clone()?;
        let x: f32 = menu.pos.x.into();
        let y: f32 = menu.pos.y.into();
        let left_px = x.clamp(16.0, 720.0);
        let top_px = y.clamp(48.0, 340.0);
        let did_opt = menu.device_id;
        let aid_opt = menu.agenda_id;
        let hidx_opt = menu.history_idx;
        let host = menu.hostname.clone();
        let conn_code = menu.connection_id.clone();

        let item = |id: &'static str, icon: IconName, label: &'static str, danger: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py(px(6.))
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(if danger { p.danger } else { p.text }))
                .hover(|s| s.bg(rgb(p.raised)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(icon).size(px(15.)).text_color(rgb(if danger { p.danger } else { p.muted })))
                        .child(label),
                )
        };

        let mut sub_div: Option<Div> = None;
        if menu.submenu == Some("move") {
            let mut move_box = div()
                .w(px(220.))
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .shadow_lg()
                .child(
                    div()
                        .id("ctx-mv-none")
                        .px_3()
                        .py(px(6.))
                        .rounded_md()
                        .cursor_pointer()
                        .text_xs()
                        .hover(|s| s.bg(rgb(p.raised)))
                        .child("Mis dispositivos gestionados (Sin grupo)")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dev_ctx_menu = None;
                            if let Some(did) = did_opt {
                                this.move_device_to_group(did, None, cx);
                            }
                            cx.notify();
                        })),
                );
            for g in self.groups.iter().filter(|g| g.my_permission != "read_only").cloned() {
                let gid = g.id;
                let gname = g.name.clone();
                move_box = move_box.child(
                    div()
                        .id(SharedString::from(format!("ctx-mv-{gid}")))
                        .px_3()
                        .py(px(6.))
                        .rounded_md()
                        .cursor_pointer()
                        .text_xs()
                        .hover(|s| s.bg(rgb(p.raised)))
                        .child(gname)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dev_ctx_menu = None;
                            this.expanded_groups.insert(gid);
                            if let Some(did) = did_opt {
                                this.move_device_to_group(did, Some(gid), cx);
                            }
                            cx.notify();
                        })),
                );
            }
            sub_div = Some(move_box);
        }

        let h1 = host.clone();
        let h2 = host.clone();
        let h3 = host.clone();
        let h4 = host.clone();
        let h_edit = host.clone();
        let h_msg = host.clone();
        let h_wol = host.clone();
        let c_pw = conn_code.clone();
        let c_conf = conn_code.clone();

        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("dev-ctx-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dev_ctx_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.dev_ctx_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(left_px))
                        .top(px(top_px))
                        .flex()
                        .items_start()
                        .gap_1()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .w(px(250.))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.panel))
                                .shadow_lg()
                                .child(item("ctx-conn-confirm", IconName::MousePointer2, "Control remoto (pedir confirmación)", false).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.dev_ctx_menu = None;
                                        if let Some(did) = did_opt {
                                            this.start_managed_session_with_panel(did, None, true, h1.clone(), None, cx);
                                        } else if let Some(idx) = hidx_opt {
                                            this.reconnect_from_history(idx, Some(window), cx);
                                        } else {
                                            this.connect_status = format!("Solicitud pendiente en {c_conf}");
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .child(item("ctx-conn-pw", IconName::KeyRound, "Control remoto (mediante contraseña)", false).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.dev_ctx_menu = None;
                                        let code = if c_pw.trim().is_empty() { h2.clone() } else { c_pw.clone() };
                                        let code_fill = code.clone();
                                        this.quick_id = cx.new(|cx| {
                                            InputState::new(window, cx)
                                                .placeholder("ID de equipo (ej: 123 456 789)")
                                                .default_value(code_fill)
                                        });
                                        this.bind_quick_connect_enter(&this.quick_id, window, cx);
                                        this.initiate_connect_by_id(code, true, Some(window), cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(item("ctx-conn-view", IconName::Eye, "Presentación (solo vista)", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.dev_ctx_menu = None;
                                        if let Some(did) = did_opt {
                                            this.start_managed_session_with_panel(did, None, false, h3.clone(), None, cx);
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                                .child(item("ctx-msg", IconName::MessageSquare, "Enviar un mensaje de chat", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.dev_ctx_menu = None;
                                        if let Some(did) = did_opt {
                                            this.start_managed_session_with_panel(did, None, true, h_msg.clone(), Some("chat"), cx);
                                        } else {
                                            this.page = Page::Support;
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .child(item("ctx-files", IconName::FolderUp, "Transferencia de archivos", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.dev_ctx_menu = None;
                                        if let Some(did) = did_opt {
                                            this.start_managed_session_with_panel(did, None, true, h4.clone(), Some("files"), cx);
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .when(did_opt.is_some(), |this| {
                                    let did = did_opt.unwrap();
                                    let h_w = h_wol.clone();
                                    this.child(item("ctx-wol", IconName::Power, "Encender (Wake-on-LAN)", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.dev_ctx_menu = None;
                                            this.trigger_wol(did, h_w.clone(), cx);
                                        }),
                                    ))
                                    .child(item("ctx-rmm", IconName::Activity, "Diagnóstico y Terminal (RMM)", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.dev_ctx_menu = None;
                                            if let Some(idx) = this.devices.iter().position(|d| d.id == did) {
                                                this.rmm_device_idx = idx;
                                            }
                                            this.page = Page::Monitoring;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        item("ctx-move", IconName::Folder, "Mover a grupo", false)
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("›"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(m) = &mut this.dev_ctx_menu {
                                                    m.submenu = if m.submenu == Some("move") { None } else { Some("move") };
                                                }
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                                .child(item("ctx-props", IconName::Settings, "Propiedades", false).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.dev_ctx_menu = None;
                                        this.start_edit_device(did_opt, aid_opt, h_edit.clone(), window, cx);
                                    }),
                                ))
                                .child(item("ctx-delete", IconName::Trash, "Eliminar", true).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.dev_ctx_menu = None;
                                        if let Some(aid) = aid_opt {
                                            this.delete_agenda_entry(aid, cx);
                                        } else if let Some(idx) = hidx_opt {
                                            this.remove_history_entry(idx, cx);
                                        } else if let Some(did) = did_opt {
                                            if let Some(ag) = this.agenda.iter().find(|a| a.device_id == Some(did)).map(|a| a.id) {
                                                this.delete_agenda_entry(ag, cx);
                                            } else {
                                                this.move_device_to_group(did, None, cx);
                                            }
                                        }
                                        cx.notify();
                                    },
                                ))),
                        )
                        .when_some(sub_div, |this, sub| this.child(sub)),
                ),
        )
    }

    fn render_group_context_menu(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.grp_ctx_menu.clone()?;
        let x: f32 = menu.pos.x.into();
        let y: f32 = menu.pos.y.into();
        let left_px = x.clamp(16.0, 740.0);
        let top_px = y.clamp(48.0, 380.0);
        let gid = menu.group_id;
        let can_write = menu.my_permission != "read_only";
        let is_owner = menu.my_permission == "full_control";

        let item = |id: &'static str, icon: IconName, label: &'static str, danger: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(6.))
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(if danger { p.danger } else { p.text }))
                .hover(|s| s.bg(rgb(p.raised)))
                .child(Icon::new(icon).size(px(15.)).text_color(rgb(if danger { p.danger } else { p.muted })))
                .child(label)
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("grp-ctx-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.grp_ctx_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.grp_ctx_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(left_px))
                        .top(px(top_px))
                        .w(px(230.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .px_3()
                                .py(px(4.))
                                .text_xs()
                                .font_semibold()
                                .text_color(rgb(p.muted))
                                .child(format!(
                                    "{} · {}",
                                    menu.group_name,
                                    match menu.my_permission.as_str() {
                                        "full_control" => "Propietario",
                                        "read_write" => "Lectura y escritura",
                                        _ => "Solo lectura",
                                    }
                                )),
                        )
                        .when(can_write, |this| {
                            this.child(item("gctx-add-comp", IconName::Monitor, "Agregar ordenador", false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.open_add_computer_modal(Some(gid), window, cx);
                                }),
                            ))
                            .child(item("gctx-add-contact", IconName::UserPlus, "Añadir contacto", false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.open_add_contact_modal(Some(gid), window, cx);
                                }),
                            ))
                        })
                        .when(is_owner, |this| {
                            this.child(item("gctx-share", IconName::Users, "Compartir grupo", false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.open_share_group_modal(gid, window, cx);
                                }),
                            ))
                        })
                        .when(can_write, |this| {
                            this.child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                                .child(item("gctx-rename", IconName::Pencil, "Renombrar", false).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.open_edit_group_modal(gid, window, cx);
                                    }),
                                ))
                                .child(item("gctx-props", IconName::Settings, "Propiedades", false).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.open_edit_group_modal(gid, window, cx);
                                    }),
                                ))
                        })
                        .when(!can_write, |this| {
                            this.child(item("gctx-inspect", IconName::Eye, "Ver detalles", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.grp_ctx_menu = None;
                                    this.oc_selection = Some(OcSelection::Group(gid));
                                    cx.notify();
                                }),
                            ))
                        })
                        .when(is_owner, |this| {
                            this.child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                                .child(item("gctx-delete", IconName::Trash, "Eliminar grupo", true).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.grp_ctx_menu = None;
                                        this.delete_group(gid, cx);
                                    }),
                                ))
                        }),
                ),
        )
    }

    fn render_edit_device_dialog(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let (did_opt, _, label) = self.editing_device.clone()?;
        let grp_label = self
            .new_agenda_group_idx
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .map(|g| format!("Grupo: {}", g.name))
            .unwrap_or_else(|| "Grupo: Sin grupo (General)".into());
        let cur_icon = self.edit_icon_kind.clone();
        let cur_color = self.edit_accent_color.clone();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(480.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(div().font_semibold().child(format!("Propiedades: {label}")))
                        .when_some(did_opt.and_then(|did| self.devices.iter().find(|d| d.id == did).cloned()), |this, dev| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(p.raised))
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child(format!("ID: {} · {} ({}) · {}", dev.public_code, dev.os_name, dev.os_arch, dev.status)),
                            )
                        })
                        .child(Input::new(&self.agenda_alias_in))
                        .child(Input::new(&self.agenda_company_in))
                        .child(Input::new(&self.agenda_contact_in))
                        .child(Input::new(&self.agenda_notes_in))
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
                                    ("database", "Base de datos"),
                                    ("network", "Red / Router"),
                                    ("cloud", "Nube"),
                                    ("camera", "Cámara"),
                                ].into_iter().map(|(k, lbl)| {
                                    let ks = k.to_string();
                                    subtle_button(lbl, SharedString::from(format!("ed-ic-{k}")), p)
                                        .id(SharedString::from(format!("ed-ic-{k}")))
                                        .when(cur_icon == k, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit_icon_kind = ks.clone();
                                            cx.notify();
                                        }))
                                })),
                        )
                        .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("COLOR IDENTIFICADOR"))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .children([
                                    ("", 0x3B82F6_u32, "Auto"),
                                    ("#3B82F6", 0x3B82F6_u32, "Azul"),
                                    ("#10B981", 0x10B981_u32, "Verde"),
                                    ("#8B5CF6", 0x8B5CF6_u32, "Violeta"),
                                    ("#F59E0B", 0xF59E0B_u32, "Ámbar"),
                                    ("#EF4444", 0xEF4444_u32, "Rojo"),
                                    ("#06B6D4", 0x06B6D4_u32, "Cian"),
                                    ("#EC4899", 0xEC4899_u32, "Rosa"),
                                ].into_iter().map(|(hex, preview_rgb, lbl)| {
                                    let hs = hex.to_string();
                                    let active = cur_color.eq_ignore_ascii_case(hex);
                                    div()
                                        .id(SharedString::from(format!("ed-col-{lbl}")))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(if active { p.accent } else { p.border }))
                                        .bg(rgb(if active { p.active } else { p.raised }))
                                        .cursor_pointer()
                                        .text_xs()
                                        .when(!hex.is_empty(), |el| {
                                            el.child(div().size(px(10.)).rounded_full().bg(rgb(preview_rgb)))
                                        })
                                        .child(lbl)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit_accent_color = hs.clone();
                                            cx.notify();
                                        }))
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    subtle_button(grp_label, "edit-dev-pick-grp", p)
                                        .id("edit-dev-pick-grp")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let n = this.groups.len() + 1;
                                            this.new_agenda_group_idx = (this.new_agenda_group_idx + 1) % n;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("Cancelar", "cancel-edit-dev", p)
                                        .id("cancel-edit-dev")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.editing_device = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button("Guardar", "save-edit-dev", p)
                                        .id("save-edit-dev")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_device_edit(window, cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_group_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let mode = self.group_modal.clone()?;
        let (title, cta) = match &mode {
            GroupModalMode::Create => ("Nuevo grupo de ordenadores", "Crear grupo"),
            GroupModalMode::Edit(_) => ("Propiedades del grupo", "Guardar cambios"),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(420.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(div().text_base().font_semibold().child(title))
                        .child(div().text_xs().text_color(rgb(p.muted)).child("Organizá equipos por cliente, sucursal o equipo de soporte."))
                        .child(Input::new(&self.group_name_in))
                        .child(Input::new(&self.group_desc_in))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("Cancelar", "grp-modal-cancel", p)
                                        .id("grp-modal-cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.group_modal = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button(cta, "grp-modal-save", p)
                                        .id("grp-modal-save")
                                        .on_click(cx.listener(move |this, _, window, cx| match &mode {
                                            GroupModalMode::Create => this.create_group(window, cx),
                                            GroupModalMode::Edit(gid) => this.update_group(*gid, window, cx),
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_share_group_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let gid = self.sharing_group_id?;
        let grp = self.groups.iter().find(|g| g.id == gid).cloned()?;
        let share_perm = self.share_perm.clone();
        let my_email = self
            .auth
            .as_ref()
            .map(|a| a.user.email.to_ascii_lowercase())
            .unwrap_or_default();
        let user_chips: Vec<_> = self
            .users
            .iter()
            .filter(|u| u.email.to_ascii_lowercase() != my_email)
            .take(6)
            .cloned()
            .collect();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(500.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_base().font_semibold().child(format!("Compartir grupo: {}", grp.name)))
                                .child(
                                    subtle_button("Cerrar", "share-modal-close", p)
                                        .id("share-modal-close")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sharing_group_id = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(!user_chips.is_empty(), |this| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(user_chips.into_iter().map(|u| {
                                        let em = u.email.clone();
                                        subtle_button(format!("{} ({})", u.display_name, u.email), SharedString::from(format!("pick-u-{}", u.id)), p)
                                            .id(SharedString::from(format!("pick-u-{}", u.id)))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.share_email_in = cx.new(|cx| {
                                                    InputState::new(window, cx)
                                                        .placeholder("usuario@empresa.com")
                                                        .default_value(em.clone())
                                                });
                                                cx.notify();
                                            }))
                                    })),
                            )
                        })
                        .child(Input::new(&self.share_email_in))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .children([
                                            ("read_only", "Solo lectura"),
                                            ("read_write", "Lectura y escritura"),
                                            ("full_control", "Control total"),
                                        ].into_iter().map(|(perm, label)| {
                                            let ps = perm.to_string();
                                            subtle_button(label, SharedString::from(format!("shperm-{perm}")), p)
                                                .id(SharedString::from(format!("shperm-{perm}")))
                                                .when(share_perm == perm, |b| b.bg(rgb(p.active)).border_color(rgb(p.accent)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.share_perm = ps.clone();
                                                    cx.notify();
                                                }))
                                        })),
                                )
                                .child(
                                    action_button("Compartir", "share-modal-submit", p)
                                        .id("share-modal-submit")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.share_group(gid, window, cx);
                                        })),
                                ),
                        )
                        .when(!self.group_action_msg.is_empty(), |this| {
                            this.child(div().text_xs().text_color(rgb(p.accent)).child(self.group_action_msg.clone()))
                        })
                        .child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                        .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("USUARIOS CON ACCESO"))
                        .child(
                            if grp.shares.is_empty() {
                                div().text_xs().text_color(rgb(p.muted)).child("Todavía no está compartido con otros usuarios.")
                            } else {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .children(grp.shares.into_iter().map(|sh| {
                                        let uid = sh.user_id;
                                        let perm_label = match sh.permission.as_str() {
                                            "read_only" => "Solo lectura",
                                            "read_write" => "Lectura y escritura",
                                            _ => "Control total",
                                        };
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .bg(rgb(p.raised))
                                            .text_xs()
                                            .child(format!("{} · {}", sh.user_email, perm_label))
                                            .child(
                                                subtle_button("Quitar", SharedString::from(format!("unsh-m-{uid}")), p)
                                                    .id(SharedString::from(format!("unsh-m-{uid}")))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.unshare_group(gid, uid, cx);
                                                    })),
                                            )
                                    }))
                            },
                        ),
                ),
        )
    }

    fn render_add_computer_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let _ = self.add_computer_modal_group?;
        let target_gid = self
            .assign_group_idx
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .map(|g| g.id);
        let grp_label = target_gid
            .and_then(|id| self.groups.iter().find(|g| g.id == id).map(|g| g.name.clone()))
            .unwrap_or_else(|| "Mis dispositivos gestionados (Sin grupo)".into());
        let candidates: Vec<_> = self
            .devices
            .iter()
            .filter(|d| d.group_id != target_gid)
            .take(8)
            .cloned()
            .collect();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(480.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(div().text_base().font_semibold().child("Agregar ordenador"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_xs().text_color(rgb(p.muted)).child("Grupo destino:"))
                                .child(
                                    subtle_button(grp_label, "addc-pick-grp", p)
                                        .id("addc-pick-grp")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let n = this.groups.len() + 1;
                                            this.assign_group_idx = (this.assign_group_idx + 1) % n;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(Input::new(&self.assign_conn_id))
                        .child(Input::new(&self.assign_alias))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("Cancelar", "addc-cancel", p)
                                        .id("addc-cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.add_computer_modal_group = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button("Agregar por ID", "addc-submit", p)
                                        .id("addc-submit")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.assign_device(window, cx);
                                        })),
                                ),
                        )
                        .when(!candidates.is_empty(), |this| {
                            this.child(div().my_1().h(px(1.)).bg(rgb(p.border)))
                                .child(div().text_xs().font_semibold().text_color(rgb(p.muted)).child("O MOVÉ UN EQUIPO EXISTENTE AL GRUPO"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .max_h(px(190.))
                                        .overflow_y_scrollbar()
                                        .children(candidates.into_iter().map(|d| {
                                            let did = d.id;
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .px_3()
                                                .py(px(6.))
                                                .rounded_md()
                                                .bg(rgb(p.raised))
                                                .text_xs()
                                                .child(format!("{} · {} ({})", d.hostname, d.public_code, d.status))
                                                .child(
                                                    action_button("Agregar", SharedString::from(format!("mv-cand-{did}")), p)
                                                        .id(SharedString::from(format!("mv-cand-{did}")))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.add_computer_modal_group = None;
                                                            if let Some(g) = target_gid {
                                                                this.expanded_groups.insert(g);
                                                            }
                                                            this.move_device_to_group(did, target_gid, cx);
                                                        })),
                                                )
                                        })),
                                )
                        }),
                ),
        )
    }

    fn render_add_contact_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let _ = self.add_contact_modal_group?;
        let grp_label = self
            .new_agenda_group_idx
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "Sin grupo (General)".into());
        let dev_label = self
            .new_agenda_device_idx
            .checked_sub(1)
            .and_then(|i| self.devices.get(i))
            .map(|d| format!("{} ({})", d.hostname, d.public_code))
            .unwrap_or_else(|| "Sin equipo vinculado".into());

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(460.))
                        .p_5()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .child(div().text_base().font_semibold().child("Añadir contacto o equipo agendado"))
                        .child(Input::new(&self.agenda_alias_in))
                        .child(Input::new(&self.agenda_company_in))
                        .child(Input::new(&self.agenda_contact_in))
                        .child(Input::new(&self.agenda_notes_in))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    subtle_button(format!("Grupo: {grp_label}"), "addct-pick-grp", p)
                                        .id("addct-pick-grp")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let n = this.groups.len() + 1;
                                            this.new_agenda_group_idx = (this.new_agenda_group_idx + 1) % n;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    subtle_button(format!("Equipo: {dev_label}"), "addct-pick-dev", p)
                                        .id("addct-pick-dev")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let n = this.devices.len() + 1;
                                            this.new_agenda_device_idx = (this.new_agenda_device_idx + 1) % n;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    subtle_button("Cancelar", "addct-cancel", p)
                                        .id("addct-cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.add_contact_modal_group = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action_button("Guardar contacto", "addct-submit", p)
                                        .id("addct-submit")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.create_agenda_entry(window, cx);
                                        })),
                                ),
                        ),
                ),
        )
    }
}

fn ws_url_base(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

/// Encabezado de página: el título ya está en la barra superior, así que aquí
/// solo queda la descripción breve (o nada).
pub fn page_heading(_title: &'static str, subtitle: &'static str, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .when(!subtitle.is_empty(), |this| {
            this.child(div().text_sm().text_color(rgb(p.muted)).child(subtitle))
        })
}

pub fn workspace_panel(title: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded(px(14.))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .child(div().text_sm().font_semibold().child(title.to_string()))
}

pub fn action_button(
    label: impl Into<SharedString>,
    _id: impl Into<SharedString>,
    p: Palette,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(34.))
        .px_3()
        .rounded(px(9.))
        .cursor_pointer()
        .bg(rgb(p.accent))
        .text_color(rgb(0xFFFFFF))
        .text_sm()
        .font_medium()
        .hover(|s| s.opacity(0.92))
        .child(label.into())
}

pub fn subtle_button(
    label: impl Into<SharedString>,
    _id: impl Into<SharedString>,
    p: Palette,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px_3()
        .rounded(px(8.))
        .cursor_pointer()
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.canvas))
        .text_color(rgb(p.text))
        .text_xs()
        .font_medium()
        .hover(|s| s.bg(rgb(p.raised)))
        .child(label.into())
}
pub fn users_table(users: &[UserDto], p: Palette) -> Div {
    let mut panel = workspace_panel("Usuarios de la organización", p);
    if users.is_empty() {
        return panel.child(
            div()
                .py_4()
                .text_sm()
                .text_color(rgb(p.muted))
                .child("No hay usuarios disponibles."),
        );
    }
    for u in users {
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .child(Icon::new(IconName::CircleUser).small().text_color(rgb(p.muted)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(u.display_name.clone())
                        .child(div().text_xs().text_color(rgb(p.muted)).child(u.email.clone())),
                )
                .child(div().text_xs().text_color(rgb(p.muted)).child(u.role.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(if u.mfa_enabled { p.success } else { p.muted }))
                        .child(if u.mfa_enabled { "MFA activo" } else { "MFA inactivo" }),
                ),
        );
    }
    panel
}

#[cfg(windows)]
fn acquire_single_instance() -> Result<Option<windows::Win32::Foundation::HANDLE>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "Local\\AgentDesk.Desktop.Instance"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
        .map_err(|error| format!("No se pudo bloquear la instancia del cliente: {error}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        focus_existing_window("AgentDesk");
        return Ok(None);
    }
    Ok(Some(handle))
}

fn extract_saved_bounds(window: &Window) -> Option<SavedWindowBounds> {
    let wb = window.window_bounds();
    let (bounds, maximized) = match wb {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    let x: f32 = bounds.origin.x.into();
    let y: f32 = bounds.origin.y.into();
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    if width < 240.0 || height < 180.0 || x <= -10000.0 || y <= -10000.0 {
        return None;
    }
    Some(SavedWindowBounds {
        x,
        y,
        width,
        height,
        maximized,
    })
}

fn restore_window_bounds(
    saved: Option<SavedWindowBounds>,
    def_origin: (f32, f32),
    def_size: (f32, f32),
    min_size: (f32, f32),
) -> WindowBounds {
    if let Some(sb) = saved {
        if sb.width >= min_size.0
            && sb.height >= min_size.1
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
    WindowBounds::Windowed(Bounds {
        origin: point(px(def_origin.0), px(def_origin.1)),
        size: size(px(def_size.0), px(def_size.1)),
    })
}

fn main() {
    startup_log("process started");
    let previous_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        startup_log(&format!("panic: {info}"));
        previous_panic_hook(info);
    }));

    #[cfg(windows)]
    let _instance_guard = match acquire_single_instance() {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            startup_log("another AgentDesk instance owns mutex");
            return;
        }
        Err(e) => {
            startup_log(&e);
            return;
        }
    };

    let (init_cfg, _) = DesktopConfig::load();
    let init_bounds = restore_window_bounds(
        init_cfg.window_bounds,
        (110., 70.),
        (1040., 660.),
        (640., 460.),
    );

    application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.text_system()
                .add_fonts(vec![Cow::Borrowed(
                    include_bytes!("../../desktop/assets/fonts/Inter.ttf").as_slice(),
                )])
                .expect("no se pudo cargar la fuente Inter");
            apply_theme(cx, if init_cfg.is_dark() { ThemeMode::Dark } else { ThemeMode::Light });
            cx.bind_keys([
                KeyBinding::new("ctrl-k", shell::TogglePalette, None),
                KeyBinding::new("escape", shell::CloseOverlays, Some("AgentDeskClient")),
            ]);
            open_window(
                WindowOptions {
                    window_bounds: Some(init_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some("AgentDesk".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let root = cx.new(|cx| Client::new(window, cx));
                    let weak_root = root.downgrade();
                    #[cfg(windows)]
                    {
                        use std::sync::{
                            atomic::{AtomicBool, Ordering},
                            Arc,
                        };
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

                        let allow_quit = Arc::new(AtomicBool::new(false));
                        let tray_ready = Arc::new(AtomicBool::new(false));
                        if let Some(hwnd) = apply_win32_window_branding(window, "AgentDesk") {
                            let _ = root.update(cx, |this, _| {
                                this.main_hwnd = Some(hwnd);
                            });
                            spawn_client_tray(hwnd, tray_ready.clone(), allow_quit.clone());
                            window.on_window_should_close(cx, move |window, cx| {
                                if let Some(sb) = extract_saved_bounds(window) {
                                    let _ = weak_root.update(cx, |this, _| {
                                        this.cfg.window_bounds = Some(sb);
                                        let _ = this.cfg.save(&this.config_path);
                                    });
                                }
                                if allow_quit.load(Ordering::Relaxed)
                                    || !tray_ready.load(Ordering::Relaxed)
                                {
                                    return true;
                                }
                                unsafe {
                                    let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                                }
                                false
                            });
                        }
                    }
                    #[cfg(not(windows))]
                    window.on_window_should_close(cx, move |window, cx| {
                        if let Some(sb) = extract_saved_bounds(window) {
                            let _ = weak_root.update(cx, |this, _| {
                                this.cfg.window_bounds = Some(sb);
                                let _ = this.cfg.save(&this.config_path);
                            });
                        }
                        true
                    });
                    root
                },
            )
            .expect("no se pudo abrir AgentDesk");
        });
}
