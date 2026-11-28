#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod panels;
mod theme;

use agentdesk_capture::win::{
    clipboard_sequence_number, current_process_private_mb, enumerate_monitors, inject_input_event,
    inject_special_combo, read_clipboard_files, read_clipboard_text, write_clipboard_files,
    write_clipboard_text, WallpaperBlackoutGuard, WindowsCapturer,
};
use agentdesk_capture::{
    encode_frame, enumerate_printers, print_file_to_printer, spawn_audio_capture_worker,
    spawn_capture_worker, AbrController, ScreenCapturer, ViewerFrameCanvas, WhiteboardOverlay,
};
use agentdesk_identity::DeviceIdentity;
use agentdesk_protocol::{
    AddressBookEntryDto, AgentToServerMessage, AiChatResponse, AiProposedActionDto, AiSettingsDto,
    AuthResponse, CalendarEventDto, ControlMessage, CreateEnrollmentResponse, CreateSessionRequest,
    CreateSessionResponse, DataPacket, DeviceDto, DeviceGroupDto, DirectConnectRequest,
    DirectConnectResponse, FileDropTarget, FrameEncoding, InputEvent, LoginRequest, MfaSetupResponse,
    MonitorInfo, MouseBtn, QuickRegisterRequest, QuickRegisterResponse, RmmCommandDto,
    RotatePasswordRequest, RunDiagnosticRequest, ServerToAgentMessage, SetPersonalPasswordRequest,
    SupportTicketDto, TicketCommentDto, UpsertAddressBookRequest, UserDto,
};
use agentdesk_transport::{
    enumerate_local_mac_and_ips, send_adaptive_packet, send_wol_magic_packets,
    start_host_p2p_handshake, store_received_file_for_target, verify_sha256_hex, P2pUdpChannel,
    RelayTunnel,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use config::{
    detect_lan_ip, install_host_service_elevated, normalize_conn_code, normalize_server_url,
    set_windows_autostart, ConnectionHistoryEntry, DesktopConfig,
};
use eframe::egui::{self, FontId, RichText};
use futures_util::{SinkExt, StreamExt};
use panels::{
    egui_key_to_vk, extract_resources, render_hardware_resources_ui, render_percent_trend,
};
use rand::Rng;
#[cfg(windows)]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use theme::{
    apply_theme, badge_frame, danger_button, primary_button, styled_card, Palette, ThemeMode,
};
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Connect,
    Agenda,
    Groups,
    Devices,
    Tickets,
    AiCopilot,
    Rmm,
    Calendar,
    UsersMfa,
    Viewer,
    Settings,
}

enum UiEvent {
    PeerStatus(String),
    TrayInitError(String),
    PeerReady,
    PeerRegistered {
        device_id: Uuid,
        connection_id: String,
        device_token: String,
        password: String,
    },
    PasswordRotated(String),
    IncomingControlStarted {
        caller_name: String,
        allow_control: bool,
        stop_tx: tokio::sync::watch::Sender<bool>,
    },
    IncomingControlEnded,
    LoginOk(AuthResponse),
    LoginErr(String),
    ConnectionTestResult {
        ok: bool,
        detail: String,
    },
    DataLoaded {
        devices: Vec<DeviceDto>,
        agenda: Vec<AddressBookEntryDto>,
        tickets: Vec<SupportTicketDto>,
    },
    DataLoadFailed(String),
    TicketCommentsLoaded(Uuid, Result<Vec<TicketCommentDto>, String>),
    TicketResolved(Uuid, Result<(), String>),
    EnrollmentCreated(String),
    Toast(String),
    SessionStarted {
        session_id: Uuid,
        ws_url: String,
        allow_control: bool,
        label: String,
    },
    RemoteFrame {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        kbps: f32,
    },
    RemoteRtt(u64),
    RemoteChat(String, String),
    RemoteMonitors {
        monitors: Vec<MonitorInfo>,
        active_index: usize,
    },
    RemoteClipboard(String),
    RemoteFileAck(Uuid, Uuid, String),
    FileTransferProgress(Uuid, Uuid, String),
    FileTransferFailed(Uuid, Uuid, String),
    SessionEnded(String),
    AiSettingsLoaded(AiSettingsDto),
    AiOAuthUrl(String),
    AiChatResult(AiChatResponse),
    AiActionExecuted(RmmCommandDto),
    RmmDiagnosticResult(serde_json::Value),
    DeviceTelemetryLoaded(Uuid, Result<Vec<serde_json::Value>, String>),
    DeviceAlertsLoaded(Uuid, Result<Vec<serde_json::Value>, String>),
    DeviceAlertAcknowledged(Uuid, Uuid, Result<(), String>),
    RmmAlertSettingsLoaded(Result<serde_json::Value, String>),
    RmmAlertSettingsSaved(Result<serde_json::Value, String>),
    DeviceDetailDiagnosticLoaded(Uuid, String, Result<serde_json::Value, String>),
    DeviceGroupUpdated(Uuid, Option<Uuid>, Result<(), String>),
    RmmCommandResult(RmmCommandDto),
    CalendarLoaded(Vec<CalendarEventDto>),
    UsersLoaded(Vec<UserDto>),
    MfaSetupResult(MfaSetupResponse),
    GroupsLoaded(Vec<DeviceGroupDto>),
    RemoteHardwareResources(serde_json::Value),
    RemoteRebootPending(u64, String),
    Tray(TrayAction),
    PasswordChanged(Result<(), String>),
    PersonalPasswordUpdated(bool, String),
    DirectConnected {
        connection_id: String,
        hostname: String,
        token: Option<String>,
        expires_unix: Option<i64>,
    },
    RefreshRequested,
}

#[derive(Debug, Clone, Copy)]
enum TrayAction {
    ShowWindow,
    CopyId,
    QuitApp,
}

#[derive(Debug, Clone)]
struct ReconnectTarget {
    connection_id: String,
    hostname: String,
    reconnect_token: Option<String>,
    allow_control: bool,
}

enum SearchAct {
    History(usize),
    Device(Uuid, String),
    Group,
    FillId(String),
}

struct ActiveViewer {
    session_id: Uuid,
    label: String,
    allow_control: bool,
    out_tx: tokio::sync::mpsc::UnboundedSender<DataPacket>,
    texture: Option<egui::TextureHandle>,
    frame_count: u64,
    fps_window_start: Instant,
    fps: f32,
    kbps: f32,
    rtt_ms: u64,
    jpeg_quality: u8,
    max_fps: u8,
    chat_input: String,
    chat_log: Vec<String>,
    last_mouse_norm: (f32, f32),
    monitors: Vec<MonitorInfo>,
    active_monitor: usize,
    file_path_input: String,
    file_status: String,
    active_file_transfer: Option<Uuid>,
    hw_resources: Option<serde_json::Value>,
    show_resources_panel: bool,
    confirm_reboot_open: bool,
    show_user_switch_popup: bool,
    reconnect_target: Option<ReconnectTarget>,
}

struct IncomingControl {
    caller_name: String,
    allow_control: bool,
    stop_tx: tokio::sync::watch::Sender<bool>,
}

struct AgentDeskApp {
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    brand_icon: egui::TextureHandle,
    ui_tx: Sender<UiEvent>,
    ui_rx: Receiver<UiEvent>,

    server_url: String,
    settings_server_url: String,
    config_path: PathBuf,
    theme_mode: ThemeMode,
    palette: Palette,
    conn_test_status: Option<(bool, String)>,

    // Embedded host (ID + Random Password)
    host_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    local_device_id: Option<Uuid>,
    local_device_token: Option<String>,
    local_connection_id: String,
    local_password: String,
    show_local_password: bool,
    peer_status: String,
    peer_ready: bool,
    incoming_control: Option<IncomingControl>,

    // Outbound direct connection form
    partner_id_input: String,
    partner_password_input: String,
    show_partner_password: bool,
    partner_allow_control: bool,
    connect_feedback: String,

    // Technician account auth (for Agenda / Devices / Tickets)
    email: String,
    password: String,
    mfa_login_code: String,
    auth: Option<AuthResponse>,
    status_msg: String,

    tab: NavTab,
    search: String,
    only_favorites: bool,

    devices: Vec<DeviceDto>,
    inventory_loading: bool,
    inventory_error: Option<String>,
    selected_device_id: Option<Uuid>,
    device_group_filter: Option<Uuid>,
    device_status_filter: String,
    devices_favorites_only: bool,
    device_detail_tab: String,
    device_telemetry_device: Option<Uuid>,
    device_telemetry: Vec<serde_json::Value>,
    device_telemetry_status: String,
    device_alerts_device: Option<Uuid>,
    device_alerts: Vec<serde_json::Value>,
    device_alerts_status: String,
    device_alerts_pending: HashSet<Uuid>,
    rmm_alert_threshold_inputs: [String; 6],
    rmm_alert_settings_status: String,
    rmm_alert_settings_loading: bool,
    rmm_alert_settings_saving: bool,
    device_detail_diagnostics: HashMap<Uuid, HashMap<String, serde_json::Value>>,
    device_detail_diagnostics_loading: HashSet<(Uuid, String)>,
    device_detail_diagnostics_error: HashMap<(Uuid, String), String>,
    device_group_update_pending: Option<Uuid>,
    device_group_update_status: String,
    agenda: Vec<AddressBookEntryDto>,
    tickets: Vec<SupportTicketDto>,
    ticket_status_filter: String,
    ticket_close_confirm: Option<(Uuid, String)>,
    ticket_resolution_outcome: String,
    ticket_resolution_summary: String,
    ticket_resolution_error: String,
    ticket_comments_open: Option<Uuid>,
    ticket_comments: HashMap<Uuid, Vec<TicketCommentDto>>,
    ticket_comments_error: HashMap<Uuid, String>,

    show_add_agenda: bool,
    new_alias: String,
    new_company: String,
    new_contact: String,
    new_group: String,
    new_notes: String,
    new_fav: bool,
    new_device_idx: usize,

    enroll_cmd_out: String,
    pending_deeplink_connect: Option<Uuid>,
    viewer: Option<ActiveViewer>,

    // AI Copilot state
    ai_settings: Option<AiSettingsDto>,
    ai_provider: String,
    ai_model: String,
    ai_base_url: String,
    ai_api_key: String,
    ai_oauth_client_id: String,
    ai_oauth_manual_token: String,
    ai_autonomy: String,
    ai_redact_pii: bool,
    ai_device_idx: usize,
    ai_ticket_context: Option<Uuid>,
    ai_resolution_draft: Option<(Uuid, String)>,
    ai_include_diag: bool,
    ai_include_shot: bool,
    ai_prompt: String,
    ai_messages: Vec<(String, String)>,
    ai_proposed_actions: Vec<AiProposedActionDto>,
    ai_busy: bool,
    ai_oauth_url: String,

    // RMM state
    rmm_device_idx: usize,
    rmm_shell: String,
    rmm_command: String,
    rmm_diag_output: String,
    rmm_cmd_output: String,

    // Calendar state
    calendar_events: Vec<CalendarEventDto>,
    cal_title: String,
    cal_client: String,
    cal_notes: String,
    cal_hours_from_now: String,
    cal_duration_mins: String,

    // Users & MFA state
    users: Vec<UserDto>,
    new_user_email: String,
    new_user_name: String,
    new_user_pw: String,
    new_user_role: String,
    mfa_setup: Option<MfaSetupResponse>,
    mfa_code_input: String,

    // TeamViewer-style client state
    connection_history: Vec<ConnectionHistoryEntry>,
    owner_email_input: String,
    personal_pw_input: String,
    personal_pw_set: bool,
    start_with_windows: bool,
    service_enroll_token: String,
    global_search: String,
    user_menu_open: bool,
    user_menu_rect: Option<egui::Rect>,
    search_rect: Option<egui::Rect>,
    login_modal_open: bool,
    change_pw_modal_open: bool,
    cur_pw_input: String,
    new_pw_input: String,
    new_pw_confirm: String,
    change_pw_msg: String,
    groups: Vec<DeviceGroupDto>,
    new_group_name: String,
    new_group_desc: String,
    share_email_map: HashMap<Uuid, String>,
    share_perm_map: HashMap<Uuid, String>,
    assign_conn_id: String,
    assign_email: String,
    assign_alias: String,
    assign_group_idx: usize,
    new_agenda_group_idx: usize,
    rmm_hw_snapshot: Option<serde_json::Value>,
    reconnect_prompt: Option<(String, ReconnectTarget)>,
    auto_reconnect: Option<(ReconnectTarget, Instant, u32)>,
    last_connect_target: Option<ReconnectTarget>,
    allow_quit: bool,
    tray_ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tray_window_handle: Option<isize>,
}

fn generate_random_password() -> String {
    const CHARSET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn desktop_identity_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("desktop_identity.dpapi")
}

fn transfer_file_over_session(
    path: PathBuf,
    out_tx: tokio::sync::mpsc::UnboundedSender<DataPacket>,
    ui_tx: Sender<UiEvent>,
    session_id: Uuid,
    transfer_id: Uuid,
) -> Result<(), String> {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "archivo.bin".into());
    let mut file =
        std::fs::File::open(&path).map_err(|error| format!("Error leyendo archivo: {error}"))?;
    let total = file
        .metadata()
        .map_err(|error| format!("No se pudo consultar el archivo: {error}"))?
        .len();

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Error calculando SHA-256: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let sha256 = hex::encode(hasher.finalize());
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("No se pudo reiniciar la lectura: {error}"))?;

    out_tx
        .send(DataPacket::Control(ControlMessage::FileOffer {
            transfer_id,
            filename: filename.clone(),
            size_bytes: total,
            sha256,
        }))
        .map_err(|_| "La sesión se cerró antes de enviar el archivo".to_string())?;

    let mut sent = 0_u64;
    let mut last_percent = u8::MAX;
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| format!("Error leyendo el archivo: {error}"))?;
        let eof = read == 0 || sent.saturating_add(read as u64) >= total;
        if read > 0 || eof {
            out_tx
                .send(DataPacket::Control(ControlMessage::FileChunk {
                    transfer_id,
                    offset: sent,
                    data: chunk[..read].to_vec(),
                    eof,
                }))
                .map_err(|_| "La sesión se cerró durante la transferencia".to_string())?;
        }
        sent = sent.saturating_add(read as u64);

        let percent = if total == 0 {
            100
        } else {
            ((sent.saturating_mul(100) / total).min(100)) as u8
        };
        if percent != last_percent {
            let _ = ui_tx.send(UiEvent::FileTransferProgress(
                session_id,
                transfer_id,
                format!("Enviando {filename}: {percent}%"),
            ));
            last_percent = percent;
        }
        if eof {
            break;
        }
    }
    Ok(())
}

impl AgentDeskApp {
    fn new(cc: &eframe::CreationContext<'_>, deeplink: Option<String>) -> Self {
        let (cfg, config_path) = DesktopConfig::load();
        let theme_mode = cfg.effective_theme();
        theme::install_fonts(&cc.egui_ctx);
        let palette = apply_theme(&cc.egui_ctx, theme_mode);
        let brand_icon = load_brand_icon(&cc.egui_ctx);

        let (ui_tx, ui_rx) = mpsc::channel();
        let pending_deeplink_connect = deeplink.and_then(|uri| {
            uri.strip_prefix("agentdesk://connect/")
                .and_then(|s| Uuid::parse_str(s.trim_end_matches('/')).ok())
        });

        let mut app = Self {
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
            http: reqwest::Client::new(),
            brand_icon,
            ui_tx,
            ui_rx,
            server_url: cfg.server_url.clone(),
            settings_server_url: cfg.server_url,
            config_path,
            theme_mode,
            palette,
            conn_test_status: None,
            host_shutdown_tx: None,
            local_device_id: None,
            local_device_token: None,
            local_connection_id: "...".into(),
            local_password: generate_random_password(),
            show_local_password: true,
            peer_status: "Registrando equipo...".into(),
            peer_ready: false,
            incoming_control: None,
            partner_id_input: String::new(),
            partner_password_input: String::new(),
            show_partner_password: false,
            partner_allow_control: true,
            connect_feedback: String::new(),
            email: cfg.last_email,
            password: String::new(),
            mfa_login_code: String::new(),
            auth: None,
            status_msg: String::new(),
            tab: NavTab::Connect,
            search: String::new(),
            only_favorites: false,
            devices: Vec::new(),
            inventory_loading: false,
            inventory_error: None,
            selected_device_id: None,
            device_group_filter: None,
            device_status_filter: "Todos los estados".into(),
            devices_favorites_only: false,
            device_detail_tab: "General".into(),
            device_telemetry_device: None,
            device_telemetry: Vec::new(),
            device_telemetry_status: String::new(),
            device_alerts_device: None,
            device_alerts: Vec::new(),
            device_alerts_status: String::new(),
            device_alerts_pending: HashSet::new(),
            rmm_alert_threshold_inputs: [
                "85".into(),
                "95".into(),
                "85".into(),
                "95".into(),
                "15".into(),
                "5".into(),
            ],
            rmm_alert_settings_status: String::new(),
            rmm_alert_settings_loading: false,
            rmm_alert_settings_saving: false,
            device_detail_diagnostics: HashMap::new(),
            device_detail_diagnostics_loading: HashSet::new(),
            device_detail_diagnostics_error: HashMap::new(),
            device_group_update_pending: None,
            device_group_update_status: String::new(),
            agenda: Vec::new(),
            tickets: Vec::new(),
            ticket_status_filter: "Abiertos".into(),
            ticket_close_confirm: None,
            ticket_resolution_outcome: "resolved".into(),
            ticket_resolution_summary: String::new(),
            ticket_resolution_error: String::new(),
            ticket_comments_open: None,
            ticket_comments: HashMap::new(),
            ticket_comments_error: HashMap::new(),
            show_add_agenda: false,
            new_alias: String::new(),
            new_company: String::new(),
            new_contact: String::new(),
            new_group: "General".into(),
            new_notes: String::new(),
            new_fav: true,
            new_device_idx: 0,
            enroll_cmd_out: String::new(),
            pending_deeplink_connect,
            viewer: None,
            ai_settings: None,
            ai_provider: "chat_gpt_o_auth".into(),
            ai_model: "gpt-4o-mini".into(),
            ai_base_url: "https://api.openai.com/v1".into(),
            ai_api_key: String::new(),
            ai_oauth_client_id: "agentdesk-desktop-client".into(),
            ai_oauth_manual_token: String::new(),
            ai_autonomy: "human_approved_exec".into(),
            ai_redact_pii: true,
            ai_device_idx: 0,
            ai_ticket_context: None,
            ai_resolution_draft: None,
            ai_include_diag: true,
            ai_include_shot: false,
            ai_prompt: String::new(),
            ai_messages: Vec::new(),
            ai_proposed_actions: Vec::new(),
            ai_busy: false,
            ai_oauth_url: String::new(),
            rmm_device_idx: 0,
            rmm_shell: "powershell".into(),
            rmm_command: String::new(),
            rmm_diag_output: String::new(),
            rmm_cmd_output: String::new(),
            calendar_events: Vec::new(),
            cal_title: String::new(),
            cal_client: String::new(),
            cal_notes: String::new(),
            cal_hours_from_now: "2".into(),
            cal_duration_mins: "60".into(),
            users: Vec::new(),
            new_user_email: String::new(),
            new_user_name: String::new(),
            new_user_pw: String::new(),
            new_user_role: "technician".into(),
            mfa_setup: None,
            mfa_code_input: String::new(),
            connection_history: cfg.connection_history,
            owner_email_input: cfg.owner_email,
            personal_pw_input: String::new(),
            personal_pw_set: cfg.personal_password_set,
            start_with_windows: cfg.start_with_windows,
            service_enroll_token: String::new(),
            global_search: String::new(),
            user_menu_open: false,
            user_menu_rect: None,
            search_rect: None,
            login_modal_open: false,
            change_pw_modal_open: false,
            cur_pw_input: String::new(),
            new_pw_input: String::new(),
            new_pw_confirm: String::new(),
            change_pw_msg: String::new(),
            groups: Vec::new(),
            new_group_name: String::new(),
            new_group_desc: String::new(),
            share_email_map: HashMap::new(),
            share_perm_map: HashMap::new(),
            assign_conn_id: String::new(),
            assign_email: String::new(),
            assign_alias: String::new(),
            assign_group_idx: 0,
            new_agenda_group_idx: 0,
            rmm_hw_snapshot: None,
            reconnect_prompt: None,
            auto_reconnect: None,
            last_connect_target: None,
            allow_quit: false,
            tray_ok: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tray_window_handle: None,
        };

        #[cfg(windows)]
        match cc.window_handle() {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::Win32(window) => {
                    app.tray_window_handle = Some(window.hwnd.get());
                    spawn_desktop_tray(
                        app.ui_tx.clone(),
                        app.tray_ok.clone(),
                        cc.egui_ctx.clone(),
                        window.hwnd.get(),
                    );
                }
                _ => {
                    let _ = app.ui_tx.send(UiEvent::TrayInitError(
                        "La ventana no usa un controlador Win32".into(),
                    ));
                }
            },
            Err(error) => {
                let _ = app.ui_tx.send(UiEvent::TrayInitError(format!(
                    "No se pudo obtener el controlador de ventana: {error}"
                )));
            }
        }
        app.start_embedded_host(cc.egui_ctx.clone());
        app
    }

    fn set_theme(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        self.theme_mode = mode;
        self.palette = apply_theme(ctx, mode);
        self.save_config(true);
    }

    fn save_config(&mut self, silent: bool) {
        self.server_url = normalize_server_url(&self.server_url);
        self.settings_server_url = self.server_url.clone();
        let mut cfg = DesktopConfig {
            server_url: self.server_url.clone(),
            last_email: self.email.clone(),
            dark_mode: matches!(self.theme_mode, ThemeMode::DraculaDark),
            theme: Some(self.theme_mode),
            connection_history: self.connection_history.clone(),
            owner_email: self.owner_email_input.trim().to_string(),
            personal_password_set: self.personal_pw_set,
            start_with_windows: self.start_with_windows,
        };
        cfg.set_theme(self.theme_mode);
        match cfg.save(&self.config_path) {
            Ok(()) => {
                if !silent {
                    self.status_msg =
                        format!("Configuración guardada en {}", self.config_path.display());
                }
            }
            Err(e) => {
                self.status_msg = format!("Error guardando configuración: {e}");
            }
        }
    }

    fn stop_embedded_host(&mut self) {
        if let Some(tx) = self.host_shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(inc) = self.incoming_control.take() {
            let _ = inc.stop_tx.send(true);
        }
    }

    fn start_embedded_host(&mut self, ctx: egui::Context) {
        self.stop_embedded_host();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        self.host_shutdown_tx = Some(stop_tx);

        let server = self.server_url.trim_end_matches('/').to_string();
        let ui_tx = self.ui_tx.clone();
        let initial_pw = self.local_password.clone();
        let owner_email = {
            let e = self.owner_email_input.trim().to_string();
            if e.is_empty() {
                None
            } else {
                Some(e)
            }
        };
        self.peer_ready = false;
        self.peer_status = format!("Conectando a {server}...");

        self.rt.spawn(async move {
            let identity = match DeviceIdentity::load_or_create(&desktop_identity_path()) {
                Ok(id) => id,
                Err(e) => {
                    let _ = ui_tx.send(UiEvent::PeerStatus(format!("Error identidad DPAPI: {e}")));
                    ctx.request_repaint();
                    return;
                }
            };

            let host_name = hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "Windows-Desktop".into());

            let (screen_w, screen_h) = {
                let cap = WindowsCapturer::new();
                cap.dimensions()
            };

            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .build()
                .unwrap_or_default();

            let mut access_pw = initial_pw;

            loop {
                if *stop_rx.borrow() {
                    break;
                }
                let (mac_addresses, lan_ips) = enumerate_local_mac_and_ips();

                let reg_res = http
                    .post(format!("{server}/api/v1/clients/register"))
                    .json(&QuickRegisterRequest {
                        ed25519_pubkey: identity.public_key_hex(),
                        hostname: host_name.clone(),
                        os_name: std::env::consts::OS.to_string(),
                        os_arch: std::env::consts::ARCH.to_string(),
                        agent_version: env!("CARGO_PKG_VERSION").to_string(),
                        screen_width: screen_w as i32,
                        screen_height: screen_h as i32,
                        private_mem_mb: current_process_private_mb(),
                        access_password: access_pw.clone(),
                        owner_email: owner_email.clone(),
                        personal_password: None,
                        mac_addresses,
                        lan_ips,
                    })
                    .send()
                    .await;

                let reg: QuickRegisterResponse = match reg_res {
                    Ok(r) if r.status().is_success() => match r.json().await {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            let _ = ui_tx.send(UiEvent::PeerStatus(format!("Error JSON registro: {e}")));
                            ctx.request_repaint();
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                                _ = stop_rx.changed() => break,
                            }
                        }
                    },
                    Ok(r) => {
                        let _ = ui_tx.send(UiEvent::PeerStatus(format!(
                            "Servidor respondió {} al registrar equipo",
                            r.status()
                        )));
                        ctx.request_repaint();
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                            _ = stop_rx.changed() => break,
                        }
                    }
                    Err(_) => {
                        let _ = ui_tx.send(UiEvent::PeerStatus(format!(
                            "Sin conexión con {server} (reintentando...)"
                        )));
                        ctx.request_repaint();
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                            _ = stop_rx.changed() => break,
                        }
                    }
                };

                let _ = ui_tx.send(UiEvent::PeerRegistered {
                    device_id: reg.device_id,
                    connection_id: reg.connection_id.clone(),
                    device_token: reg.device_token.clone(),
                    password: access_pw.clone(),
                });
                ctx.request_repaint();

                let ws_base = if let Some(rest) = server.strip_prefix("https://") {
                    format!("wss://{rest}")
                } else if let Some(rest) = server.strip_prefix("http://") {
                    format!("ws://{rest}")
                } else {
                    server.clone()
                };
                let ws_url = format!(
                    "{ws_base}/ws/agent?device_id={}&token={}",
                    reg.device_id, reg.device_token
                );

                match agentdesk_transport::connect_ws(&ws_url).await {
                    Ok((ws_stream, _)) => {
                        let (mut ws_tx, mut ws_rx) = ws_stream.split();
                        let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel::<AgentToServerMessage>();

                        let tx_task = tokio::spawn(async move {
                            while let Some(msg) = ctrl_rx.recv().await {
                                if let Ok(txt) = serde_json::to_string(&msg) {
                                    if ws_tx.send(Message::Text(txt.into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        });

                        let hb_tx = ctrl_tx.clone();
                        let hb_task = tokio::spawn(async move {
                            let mut interval = tokio::time::interval(Duration::from_secs(10));
                            loop {
                                interval.tick().await;
                                let (mac_addresses, lan_ips) = enumerate_local_mac_and_ips();
                                let _ = hb_tx.send(AgentToServerMessage::Heartbeat {
                                    screen_width: screen_w as i32,
                                    screen_height: screen_h as i32,
                                    private_mem_mb: current_process_private_mb(),
                                    mac_addresses,
                                    lan_ips,
                                });
                            }
                        });

                        let mut active_sess: Option<(Uuid, tokio::sync::watch::Sender<bool>)> = None;

                        loop {
                            tokio::select! {
                                _ = stop_rx.changed() => {
                                    if *stop_rx.borrow() {
                                        break;
                                    }
                                }
                                incoming = ws_rx.next() => {
                                    let Some(Ok(msg)) = incoming else { break; };
                                    if let Message::Text(txt) = msg {
                                        if let Ok(srv_msg) = serde_json::from_str::<ServerToAgentMessage>(&txt) {
                                            match srv_msg {
                                                ServerToAgentMessage::Welcome { .. } => {
                                                    let _ = ui_tx.send(UiEvent::PeerReady);
                                                    ctx.request_repaint();
                                                }
                                                ServerToAgentMessage::IncomingSession {
                                                    session_id,
                                                    technician_name,
                                                    allow_control,
                                                    relay_path,
                                                    host_relay_token,
                                                    ..
                                                } => {
                                                    let _ = ctrl_tx.send(AgentToServerMessage::SessionConsentResult {
                                                        session_id,
                                                        accepted: true,
                                                        reason: None,
                                                    });

                                                    if let Some((_, prev_stop)) = active_sess.take() {
                                                        let _ = prev_stop.send(true);
                                                    }
                                                    let (sess_stop_tx, sess_stop_rx) = tokio::sync::watch::channel(false);
                                                    active_sess = Some((session_id, sess_stop_tx.clone()));

                                                    let _ = ui_tx.send(UiEvent::IncomingControlStarted {
                                                        caller_name: technician_name,
                                                        allow_control,
                                                        stop_tx: sess_stop_tx,
                                                    });
                                                    ctx.request_repaint();

                                                    let relay_url = format!("{ws_base}{relay_path}?role=host&token={host_relay_token}");
                                                    let ui_tx_done = ui_tx.clone();
                                                    let ctx_done = ctx.clone();
                                                    tokio::spawn(async move {
                                                        let _ = run_incoming_host_session(relay_url, allow_control, sess_stop_rx).await;
                                                        let _ = ui_tx_done.send(UiEvent::IncomingControlEnded);
                                                        ctx_done.request_repaint();
                                                    });
                                                }
                                                ServerToAgentMessage::TerminateSession { session_id, .. } => {
                                                    if let Some((cur, st)) = active_sess.take() {
                                                        if cur == session_id {
                                                            let _ = st.send(true);
                                                        }
                                                    }
                                                    let _ = ui_tx.send(UiEvent::IncomingControlEnded);
                                                    ctx.request_repaint();
                                                }
                                                ServerToAgentMessage::RunDiagnostic { request_id, kind } => {
                                                    let tx_rep = ctrl_tx.clone();
                                                    tokio::task::spawn_blocking(move || {
                                                        let (ok, data, shot, err) = run_local_diagnostic(&kind);
                                                        let _ = tx_rep.send(AgentToServerMessage::DiagnosticResult {
                                                            request_id,
                                                            ok,
                                                            data,
                                                            screenshot_jpeg_b64: shot,
                                                            error: err,
                                                        });
                                                    });
                                                }
                                                ServerToAgentMessage::RunCommand {
                                                    request_id,
                                                    shell,
                                                    command,
                                                    timeout_secs,
                                                } => {
                                                    let tx_rep = ctrl_tx.clone();
                                                    tokio::task::spawn_blocking(move || {
                                                        let res = agentdesk_rmm::execute_shell_command(
                                                            &shell,
                                                            &command,
                                                            timeout_secs,
                                                        );
                                                        let _ = tx_rep.send(AgentToServerMessage::CommandResult {
                                                            request_id,
                                                            ok: res.ok,
                                                            exit_code: res.exit_code,
                                                            stdout: res.stdout,
                                                            stderr: res.stderr,
                                                            duration_ms: res.duration_ms,
                                                        });
                                                    });
                                                }
                                                ServerToAgentMessage::SendWakeOnLan {
                                                    request_id,
                                                    mac_addresses,
                                                    broadcast_ips,
                                                } => {
                                                    let tx_rep = ctrl_tx.clone();
                                                    tokio::task::spawn_blocking(move || {
                                                        let sent_count = send_wol_magic_packets(&mac_addresses, &broadcast_ips);
                                                        let _ = tx_rep.send(AgentToServerMessage::WolResult {
                                                            request_id,
                                                            ok: sent_count > 0,
                                                            sent_count,
                                                            detail: format!("Paquetes UDP enviados en LAN: {sent_count}"),
                                                        });
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some((_, st)) = active_sess.take() {
                            let _ = st.send(true);
                        }
                        hb_task.abort();
                        tx_task.abort();
                        if !*stop_rx.borrow() {
                            let _ = ui_tx.send(UiEvent::PeerStatus("Reconectando…".into()));
                            ctx.request_repaint();
                        }
                    }
                    Err(e) => {
                        let _ = ui_tx.send(UiEvent::PeerStatus(format!(
                            "Reconectando canal WS ({e})..."
                        )));
                        ctx.request_repaint();
                    }
                }

                if *stop_rx.borrow() {
                    break;
                }
                access_pw = generate_random_password();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = stop_rx.changed() => break,
                }
            }
        });
    }

    fn rotate_random_password(&mut self) {
        let new_pw = generate_random_password();
        self.local_password = new_pw.clone();
        let (Some(did), Some(dtok)) = (self.local_device_id, self.local_device_token.clone())
        else {
            return;
        };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/clients/password"))
                .header("x-device-token", dtok)
                .json(&RotatePasswordRequest {
                    device_id: did,
                    new_password: new_pw.clone(),
                })
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let _ = tx.send(UiEvent::PasswordRotated(new_pw));
                }
                Ok(r) => {
                    let _ = tx.send(UiEvent::Toast(format!(
                        "Error rotando contraseña: {}",
                        r.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Toast(format!("Error red contraseña: {e}")));
                }
            }
        });
    }

    fn connect_by_id(&mut self) {
        let cid = self.partner_id_input.trim().to_string();
        let pw = self.partner_password_input.trim().to_string();
        if cid.is_empty() || pw.is_empty() {
            self.connect_feedback =
                "Ingresa el ID de conexión y la contraseña del asociado.".into();
            return;
        }
        let allow = self.partner_allow_control;
        self.connect_direct(cid, pw, None, allow);
    }

    fn connect_direct(
        &mut self,
        cid: String,
        pw: String,
        reconnect_token: Option<String>,
        allow_control: bool,
    ) {
        if pw.is_empty() && reconnect_token.is_none() {
            self.connect_feedback = "Falta la contraseña del asociado.".into();
            return;
        }
        self.connect_feedback = format!("Conectando a {cid}...");
        self.last_connect_target = Some(ReconnectTarget {
            connection_id: cid.clone(),
            hostname: cid.clone(),
            reconnect_token: reconnect_token.clone(),
            allow_control,
        });
        let caller_name = self
            .auth
            .as_ref()
            .map(|a| a.user.display_name.clone())
            .or_else(|| {
                if self.local_connection_id != "..." {
                    Some(format!("Equipo {}", self.local_connection_id))
                } else {
                    None
                }
            });
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();

        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/sessions/connect-by-id"))
                .json(&DirectConnectRequest {
                    connection_id: cid,
                    password: pw,
                    reconnect_token,
                    caller_name,
                    allow_control,
                    allow_clipboard: true,
                })
                .send()
                .await;

            match res {
                Ok(r) if r.status().is_success() => match r.json::<DirectConnectResponse>().await {
                    Ok(dr) => {
                        let ws_base = if let Some(rest) = base.strip_prefix("https://") {
                            format!("wss://{rest}")
                        } else if let Some(rest) = base.strip_prefix("http://") {
                            format!("ws://{rest}")
                        } else {
                            base.clone()
                        };
                        let ws_url = format!(
                            "{ws_base}{}?role=viewer&token={}",
                            dr.relay_ws_path, dr.viewer_relay_token
                        );
                        let _ = tx.send(UiEvent::DirectConnected {
                            connection_id: dr.target_connection_id.clone(),
                            hostname: dr.target_hostname.clone(),
                            token: dr.reconnect_token.clone(),
                            expires_unix: dr.reconnect_expires_at.map(|t| t.timestamp()),
                        });
                        let _ = tx.send(UiEvent::SessionStarted {
                            session_id: dr.session_id,
                            ws_url,
                            allow_control,
                            label: format!("{} ({})", dr.target_hostname, dr.target_connection_id),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(UiEvent::Toast(format!("Respuesta inválida: {e}")));
                    }
                },
                Ok(r) => {
                    let body = r.text().await.unwrap_or_default();
                    let msg = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| {
                            v.get("error")
                                .and_then(|s| s.as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| format!("Error: {body}"));
                    let _ = tx.send(UiEvent::Toast(msg));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Toast(format!("Error de red: {e}")));
                }
            }
        });
    }

    fn test_connection(&mut self, raw_url: &str) {
        let base = normalize_server_url(raw_url);
        self.conn_test_status = Some((true, format!("Probando conexión con {base}...")));
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            let t0 = Instant::now();
            match client.get(format!("{base}/health/ready")).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let ms = t0.elapsed().as_millis();
                    let _ = tx.send(UiEvent::ConnectionTestResult {
                        ok: true,
                        detail: format!("OK — Servidor listo en {base} ({ms} ms)"),
                    });
                }
                Ok(resp) => {
                    let _ = tx.send(UiEvent::ConnectionTestResult {
                        ok: false,
                        detail: format!("Servidor respondió HTTP {} en {base}", resp.status()),
                    });
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::ConnectionTestResult {
                        ok: false,
                        detail: format!("No se pudo conectar a {base}: {e}"),
                    });
                }
            }
        });
    }

    fn logout(&mut self) {
        self.terminate_active_session();
        self.auth = None;
        self.devices.clear();
        self.agenda.clear();
        self.tickets.clear();
        self.password.clear();
        self.status_msg = "Sesión de cuenta cerrada".into();
    }

    fn do_login(&mut self) {
        self.server_url = normalize_server_url(&self.server_url);
        self.settings_server_url = self.server_url.clone();
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let email = self.email.clone();
        let pw = self.password.clone();
        let totp_code = if self.mfa_login_code.trim().is_empty() {
            None
        } else {
            Some(self.mfa_login_code.trim().to_string())
        };

        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/auth/login"))
                .json(&LoginRequest {
                    email,
                    password: pw,
                    totp_code,
                })
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => match r.json::<AuthResponse>().await {
                    Ok(auth) => {
                        let _ = tx.send(UiEvent::LoginOk(auth));
                    }
                    Err(e) => {
                        let _ = tx.send(UiEvent::LoginErr(e.to_string()));
                    }
                },
                Ok(r) => {
                    let status = r.status();
                    let error = r.json::<serde_json::Value>().await.ok().and_then(|body| {
                        body.get("error")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    });
                    let message = match error.as_deref() {
                        Some("mfa_code_required") => {
                            "Esta cuenta requiere el código MFA de 6 dígitos.".to_string()
                        }
                        Some("invalid_mfa_code") => "El código MFA no es válido.".to_string(),
                        _ => format!(
                            "No se pudo iniciar sesión (HTTP {status}). Verificá tus credenciales."
                        ),
                    };
                    let _ = tx.send(UiEvent::LoginErr(message));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::LoginErr(e.to_string()));
                }
            }
        });
    }

    fn refresh_data(&mut self) {
        let Some(auth) = &self.auth else { return };
        self.inventory_loading = true;
        self.inventory_error = None;
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();

        self.rt.spawn(async move {
            let load = async {
                let (devices, agenda, tickets) = tokio::try_join!(
                    async {
                        http.get(format!("{base}/api/v1/devices"))
                            .bearer_auth(&token)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<Vec<DeviceDto>>()
                            .await
                    },
                    async {
                        http.get(format!("{base}/api/v1/address-book"))
                            .bearer_auth(&token)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<Vec<AddressBookEntryDto>>()
                            .await
                    },
                    async {
                        http.get(format!("{base}/api/v1/tickets"))
                            .bearer_auth(&token)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<Vec<SupportTicketDto>>()
                            .await
                    }
                )?;
                Ok::<_, reqwest::Error>((devices, agenda, tickets))
            }
            .await;

            match load {
                Ok((devices, agenda, tickets)) => {
                    let _ = tx.send(UiEvent::DataLoaded {
                        devices,
                        agenda,
                        tickets,
                    });
                }
                Err(error) => {
                    let _ = tx.send(UiEvent::DataLoadFailed(error.to_string()));
                }
            }
        });
    }

    fn load_device_telemetry(&mut self, device_id: Uuid) {
        let Some(auth) = &self.auth else { return };
        self.device_telemetry_device = Some(device_id);
        self.device_telemetry.clear();
        self.device_telemetry_status = "Cargando historial…".into();

        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .get(format!("{base}/api/v1/devices/{device_id}/telemetry"))
                .bearer_auth(&token)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<Vec<serde_json::Value>>()
                    .await
                    .map_err(|error| error.to_string()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::DeviceTelemetryLoaded(device_id, result));
        });
    }

    fn load_rmm_alert_settings(&mut self) {
        if self.rmm_alert_settings_loading {
            return;
        }
        let Some(auth) = &self.auth else { return };
        if !matches!(auth.user.role.as_str(), "instance_admin" | "org_admin") {
            return;
        }
        self.rmm_alert_settings_loading = true;
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .get(format!("{base}/api/v1/rmm/alert-settings"))
                .bearer_auth(&token)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<serde_json::Value>()
                    .await
                    .map_err(|error| error.to_string()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::RmmAlertSettingsLoaded(result));
        });
    }

    fn save_rmm_alert_settings(&mut self) {
        let Some(auth) = &self.auth else { return };
        if !matches!(auth.user.role.as_str(), "instance_admin" | "org_admin")
            || self.rmm_alert_settings_saving
        {
            return;
        }
        let parsed: Option<Vec<f64>> = self
            .rmm_alert_threshold_inputs
            .iter()
            .map(|value| value.trim().replace(',', ".").parse::<f64>().ok())
            .collect();
        let Some(values) = parsed.filter(|values| values.len() == 6) else {
            self.rmm_alert_settings_status =
                "Ingresá porcentajes válidos en los seis campos.".into();
            return;
        };
        self.rmm_alert_settings_saving = true;
        self.rmm_alert_settings_status = "Guardando umbrales…".into();
        let body = serde_json::json!({
            "cpu_warning_pct": values[0],
            "cpu_critical_pct": values[1],
            "ram_warning_pct": values[2],
            "ram_critical_pct": values[3],
            "disk_free_warning_pct": values[4],
            "disk_free_critical_pct": values[5],
        });
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .put(format!("{base}/api/v1/rmm/alert-settings"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<serde_json::Value>()
                    .await
                    .map_err(|error| error.to_string()),
                Ok(response) => {
                    let status = response.status();
                    let message = response
                        .json::<serde_json::Value>()
                        .await
                        .ok()
                        .and_then(|body| {
                            body.get("error")
                                .and_then(|value| value.as_str())
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| format!("HTTP {status}"));
                    Err(message)
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::RmmAlertSettingsSaved(result));
        });
    }

    fn load_device_alerts(&mut self, device_id: Uuid) {
        let Some(auth) = &self.auth else { return };
        self.device_alerts_device = Some(device_id);
        self.device_alerts_status = "Cargando alertas…".into();

        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .get(format!("{base}/api/v1/devices/{device_id}/alerts"))
                .bearer_auth(&token)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<Vec<serde_json::Value>>()
                    .await
                    .map_err(|error| error.to_string()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::DeviceAlertsLoaded(device_id, result));
        });
    }

    fn acknowledge_device_alert(&mut self, device_id: Uuid, alert_id: Uuid) {
        if !self.device_alerts_pending.insert(alert_id) {
            return;
        }
        let Some(auth) = &self.auth else {
            self.device_alerts_pending.remove(&alert_id);
            return;
        };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .post(format!("{base}/api/v1/alerts/{alert_id}/acknowledge"))
                .bearer_auth(&token)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => Ok(()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::DeviceAlertAcknowledged(
                device_id, alert_id, result,
            ));
        });
    }

    fn update_device_group(
        &mut self,
        device_id: Uuid,
        previous_group_id: Option<Uuid>,
        group_id: Option<Uuid>,
    ) {
        let Some(auth) = &self.auth else { return };
        self.device_group_update_pending = Some(device_id);
        self.device_group_update_status = "Guardando grupo…".into();
        if let Some(device) = self
            .devices
            .iter_mut()
            .find(|device| device.id == device_id)
        {
            device.group_id = group_id;
        }

        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .patch(format!("{base}/api/v1/devices/{device_id}/group"))
                .bearer_auth(&token)
                .json(&serde_json::json!({ "group_id": group_id }))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => Ok(()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::DeviceGroupUpdated(
                device_id,
                previous_group_id,
                result,
            ));
        });
    }

    fn load_ticket_comments(&mut self, ticket_id: Uuid) {
        let Some(auth) = &self.auth else { return };
        self.ticket_comments_error.remove(&ticket_id);
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let result = match http
                .get(format!("{base}/api/v1/tickets/{ticket_id}/comments"))
                .bearer_auth(&token)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<Vec<TicketCommentDto>>()
                    .await
                    .map_err(|error| error.to_string()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::TicketCommentsLoaded(ticket_id, result));
        });
    }

    fn load_device_detail_diagnostic(&mut self, device_id: Uuid, kind: &str) {
        let key = (device_id, kind.to_string());
        if !self.device_detail_diagnostics_loading.insert(key.clone()) {
            return;
        }
        self.device_detail_diagnostics_error.remove(&key);
        let Some(auth) = &self.auth else {
            self.device_detail_diagnostics_loading.remove(&key);
            return;
        };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        let kind = kind.to_string();
        self.rt.spawn(async move {
            let result = match http
                .post(format!("{base}/api/v1/devices/{device_id}/diagnostics"))
                .bearer_auth(&token)
                .json(&RunDiagnosticRequest { kind: kind.clone() })
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<serde_json::Value>()
                    .await
                    .map(|value| value.get("data").cloned().unwrap_or(value))
                    .map_err(|error| error.to_string()),
                Ok(response) => Err(format!("HTTP {}", response.status())),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(UiEvent::DeviceDetailDiagnosticLoaded(
                device_id, kind, result,
            ));
        });
    }

    fn start_session(
        &self,
        device_id: Uuid,
        ticket_id: Option<Uuid>,
        allow_control: bool,
        label: String,
    ) {
        let Some(auth) = &self.auth else { return };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();

        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/sessions"))
                .bearer_auth(&token)
                .json(&CreateSessionRequest {
                    device_id,
                    ticket_id,
                    allow_control,
                    allow_clipboard: allow_control,
                    allow_files: false,
                })
                .send()
                .await;

            match res {
                Ok(r) if r.status().is_success() => {
                    if let Ok(sr) = r.json::<CreateSessionResponse>().await {
                        let ws_base = if let Some(rest) = base.strip_prefix("https://") {
                            format!("wss://{rest}")
                        } else if let Some(rest) = base.strip_prefix("http://") {
                            format!("ws://{rest}")
                        } else {
                            base.clone()
                        };
                        let ws_url = format!(
                            "{ws_base}{}?role=viewer&token={}",
                            sr.relay_ws_path, sr.viewer_relay_token
                        );
                        let _ = tx.send(UiEvent::SessionStarted {
                            session_id: sr.session.id,
                            ws_url,
                            allow_control,
                            label,
                        });
                    }
                }
                Ok(r) => {
                    let _ = tx.send(UiEvent::Toast(format!(
                        "No se pudo iniciar sesión: {}",
                        r.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Toast(format!("Error red sesión: {e}")));
                }
            }
        });
    }

    fn spawn_viewer_tunnel(
        &mut self,
        ctx: egui::Context,
        session_id: Uuid,
        ws_url: String,
        allow_control: bool,
        label: String,
    ) {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<DataPacket>();
        let ui_tx = self.ui_tx.clone();

        self.viewer = Some(ActiveViewer {
            session_id,
            label,
            allow_control,
            out_tx: out_tx.clone(),
            texture: None,
            frame_count: 0,
            fps_window_start: Instant::now(),
            fps: 0.0,
            kbps: 0.0,
            rtt_ms: 0,
            jpeg_quality: 70,
            max_fps: 30,
            chat_input: String::new(),
            chat_log: Vec::new(),
            last_mouse_norm: (0.0, 0.0),
            monitors: Vec::new(),
            active_monitor: 0,
            file_path_input: String::new(),
            file_status: String::new(),
            active_file_transfer: None,
            hw_resources: None,
            show_resources_panel: false,
            confirm_reboot_open: false,
            show_user_switch_popup: false,
            reconnect_target: self.last_connect_target.take(),
        });
        self.tab = NavTab::Viewer;
        self.connect_feedback.clear();

        self.rt.spawn(async move {
            let mut tunnel = match RelayTunnel::connect(&ws_url, true).await {
                Ok(t) => t,
                Err(e) => {
                    let _ = ui_tx.send(UiEvent::SessionEnded(format!("Error E2EE/Relay: {e}")));
                    return;
                }
            };

            let mut ping_interval = tokio::time::interval(Duration::from_secs(2));
            let mut bytes_in_window = 0usize;
            let mut tick_n = 0u32;
            let mut bw_start = Instant::now();
            let mut canvas = ViewerFrameCanvas::new();

            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let ts = now_ms();
                        let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Ping { ts_ms: ts })).await;
                        if tick_n % 5 == 0 {
                            let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::RequestHardwareResources)).await;
                        }
                        tick_n = tick_n.wrapping_add(1);
                    }
                    Some(pkt) = out_rx.recv() => {
                        let is_term = matches!(pkt, DataPacket::Control(ControlMessage::Terminate { .. }));
                        if tunnel.send_packet(&pkt).await.is_err() || is_term {
                            break;
                        }
                    }
                    incoming = tunnel.recv_packet() => {
                        match incoming {
                            Ok(DataPacket::VideoFrame(vf)) => {
                                bytes_in_window += vf.payload.len();
                                let secs = bw_start.elapsed().as_secs_f32().max(0.1);
                                let kbps = (bytes_in_window as f32 / 1024.0) / secs;
                                if secs >= 1.5 {
                                    bytes_in_window = 0;
                                    bw_start = Instant::now();
                                }
                                let t_dec = Instant::now();
                                if let Ok((w, h, rgba)) = canvas.apply_frame_rgba(vf.encoding, vf.width, vf.height, &vf.payload) {
                                    let decode_ms = t_dec.elapsed().as_millis().min(u16::MAX as u128) as u16;
                                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::FrameAck { seq: vf.seq, decode_ms })).await;
                                    let _ = ui_tx.send(UiEvent::RemoteFrame { width: w, height: h, rgba, kbps });
                                    ctx.request_repaint();
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::Pong { ts_ms })) => {
                                let _ = ui_tx.send(UiEvent::RemoteRtt(now_ms().saturating_sub(ts_ms)));
                            }
                            Ok(DataPacket::Control(ControlMessage::Chat { sender, text })) => {
                                let _ = ui_tx.send(UiEvent::RemoteChat(sender, text));
                            }
                            Ok(DataPacket::Control(ControlMessage::MonitorList { monitors, active_index })) => {
                                let _ = ui_tx.send(UiEvent::RemoteMonitors { monitors, active_index });
                            }
                            Ok(DataPacket::Control(ControlMessage::ClipboardText { text })) => {
                                write_clipboard_text(&text);
                                let _ = ui_tx.send(UiEvent::RemoteClipboard(text));
                            }
                            Ok(DataPacket::Control(ControlMessage::FileAck { transfer_id, ok, message })) => {
                                let prefix = if ok { "✓ " } else { "✗ " };
                                let _ = ui_tx.send(UiEvent::RemoteFileAck(session_id, transfer_id, format!("{prefix}{message}")));
                            }
                            Ok(DataPacket::Control(ControlMessage::HardwareResourcesSnapshot { resources_json })) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resources_json) {
                                    let _ = ui_tx.send(UiEvent::RemoteHardwareResources(v));
                                    ctx.request_repaint();
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::RebootPending { delay_secs, reason })) => {
                                let _ = ui_tx.send(UiEvent::RemoteRebootPending(delay_secs, reason));
                            }
                            Ok(DataPacket::Control(ControlMessage::Terminate { reason })) => {
                                let _ = ui_tx.send(UiEvent::SessionEnded(reason));
                                break;
                            }
                            Err(_) => {
                                let _ = ui_tx.send(UiEvent::SessionEnded("Conexión cerrada".into()));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            let _ = tunnel.close().await;
        });
    }

    fn terminate_active_session(&mut self) {
        if let Some(v) = self.viewer.take() {
            let _ = v
                .out_tx
                .send(DataPacket::Control(ControlMessage::Terminate {
                    reason: "closed by technician".into(),
                }));
            if let Some(auth) = &self.auth {
                let http = self.http.clone();
                let base = self.server_url.trim_end_matches('/').to_string();
                let token = auth.token.clone();
                let sid = v.session_id;
                self.rt.spawn(async move {
                    let _ = http
                        .post(format!("{base}/api/v1/sessions/{sid}/terminate"))
                        .bearer_auth(&token)
                        .send()
                        .await;
                });
            }
        }
        if self.tab == NavTab::Viewer {
            self.tab = NavTab::Connect;
        }
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.ui_rx.try_recv() {
            match ev {
                UiEvent::PeerStatus(st) => {
                    self.peer_ready = false;
                    self.peer_status = st;
                }
                UiEvent::TrayInitError(error) => {
                    self.status_msg = format!("Systray no disponible: {error}");
                }
                UiEvent::PeerReady => {
                    self.peer_ready = true;
                    self.peer_status = "Listo para recibir conexiones".into();
                }
                UiEvent::PeerRegistered {
                    device_id,
                    connection_id,
                    device_token,
                    password,
                } => {
                    self.local_device_id = Some(device_id);
                    self.local_connection_id = connection_id.clone();
                    self.local_device_token = Some(device_token);
                    self.local_password = password;
                    self.peer_ready = false;
                    self.peer_status = "Estableciendo conexión…".into();
                }
                UiEvent::PasswordRotated(pw) => {
                    self.local_password = pw;
                    self.status_msg = "Contraseña aleatoria renovada".into();
                }
                UiEvent::IncomingControlStarted {
                    caller_name,
                    allow_control,
                    stop_tx,
                } => {
                    self.incoming_control = Some(IncomingControl {
                        caller_name: caller_name.clone(),
                        allow_control,
                        stop_tx,
                    });
                    self.status_msg = format!("Sesión entrante activa con {caller_name}");
                }
                UiEvent::IncomingControlEnded => {
                    self.incoming_control = None;
                    self.status_msg = "Sesión entrante finalizada".into();
                }
                UiEvent::LoginOk(auth) => {
                    self.save_config(true);
                    self.mfa_login_code.clear();
                    self.status_msg = format!(
                        "Conectado como {} ({})",
                        auth.user.display_name, auth.user.organization_name
                    );
                    self.auth = Some(auth);
                    self.refresh_data();
                    self.load_extra_tabs_data();
                    self.load_rmm_alert_settings();
                    if let Some(did) = self.pending_deeplink_connect.take() {
                        self.start_session(did, None, true, format!("Equipo {did}"));
                    }
                }
                UiEvent::LoginErr(err) => {
                    self.status_msg = format!("Error de acceso: {err}");
                }
                UiEvent::ConnectionTestResult { ok, detail } => {
                    self.conn_test_status = Some((ok, detail));
                }
                UiEvent::DataLoaded {
                    devices,
                    agenda,
                    tickets,
                } => {
                    self.devices = devices;
                    self.agenda = agenda;
                    self.tickets = tickets;
                    self.inventory_loading = false;
                    self.inventory_error = None;
                }
                UiEvent::DataLoadFailed(error) => {
                    self.inventory_loading = false;
                    self.inventory_error = Some(error);
                }
                UiEvent::TicketCommentsLoaded(ticket_id, comments) => match comments {
                    Ok(comments) => {
                        self.ticket_comments.insert(ticket_id, comments);
                        self.ticket_comments_error.remove(&ticket_id);
                    }
                    Err(error) => {
                        self.ticket_comments_error.insert(ticket_id, error);
                    }
                },
                UiEvent::TicketResolved(ticket_id, result) => match result {
                    Ok(()) => {
                        self.ticket_close_confirm = None;
                        self.ticket_comments.remove(&ticket_id);
                        self.ticket_comments_error.remove(&ticket_id);
                        if self.ai_ticket_context == Some(ticket_id) {
                            self.ai_ticket_context = None;
                            self.ai_resolution_draft = None;
                        }
                        let _ = self.ui_tx.send(UiEvent::Toast("Solicitud cerrada".into()));
                        let _ = self.ui_tx.send(UiEvent::RefreshRequested);
                    }
                    Err(error) => {
                        self.ticket_resolution_error = error;
                    }
                },
                UiEvent::DeviceTelemetryLoaded(device_id, result) => {
                    if self.device_telemetry_device == Some(device_id) {
                        match result {
                            Ok(rows) => {
                                self.device_telemetry = rows;
                                self.device_telemetry_status = if self.device_telemetry.is_empty() {
                                    "Todavía no hay diagnósticos guardados para este equipo.".into()
                                } else {
                                    format!("{} registros recientes", self.device_telemetry.len())
                                };
                            }
                            Err(error) => {
                                self.device_telemetry_status =
                                    format!("No se pudo cargar el historial: {error}");
                            }
                        }
                    }
                }
                UiEvent::DeviceAlertsLoaded(device_id, result) => {
                    if self.device_alerts_device == Some(device_id) {
                        match result {
                            Ok(alerts) => {
                                self.device_alerts = alerts;
                                self.device_alerts_status =
                                    format!("{} alertas recientes", self.device_alerts.len());
                            }
                            Err(error) => {
                                self.device_alerts_status =
                                    format!("No se pudieron cargar las alertas: {error}");
                            }
                        }
                    }
                }
                UiEvent::DeviceAlertAcknowledged(device_id, alert_id, result) => {
                    self.device_alerts_pending.remove(&alert_id);
                    match result {
                        Ok(()) => {
                            self.device_alerts_status = "Alerta reconocida".into();
                            self.load_device_alerts(device_id);
                        }
                        Err(error) => {
                            self.device_alerts_status =
                                format!("No se pudo reconocer la alerta: {error}");
                        }
                    }
                }
                UiEvent::RmmAlertSettingsLoaded(result) => {
                    self.rmm_alert_settings_loading = false;
                    match result {
                        Ok(settings) => {
                            let keys = [
                                "cpu_warning_pct",
                                "cpu_critical_pct",
                                "ram_warning_pct",
                                "ram_critical_pct",
                                "disk_free_warning_pct",
                                "disk_free_critical_pct",
                            ];
                            for (index, key) in keys.iter().enumerate() {
                                if let Some(value) =
                                    settings.get(*key).and_then(|value| value.as_f64())
                                {
                                    self.rmm_alert_threshold_inputs[index] = format!("{value}");
                                }
                            }
                            self.rmm_alert_settings_status = "Umbrales cargados".into();
                        }
                        Err(error) => {
                            self.rmm_alert_settings_status =
                                format!("No se pudieron cargar los umbrales: {error}");
                        }
                    }
                }
                UiEvent::RmmAlertSettingsSaved(result) => {
                    self.rmm_alert_settings_saving = false;
                    match result {
                        Ok(settings) => {
                            let keys = [
                                "cpu_warning_pct",
                                "cpu_critical_pct",
                                "ram_warning_pct",
                                "ram_critical_pct",
                                "disk_free_warning_pct",
                                "disk_free_critical_pct",
                            ];
                            for (index, key) in keys.iter().enumerate() {
                                if let Some(value) =
                                    settings.get(*key).and_then(|value| value.as_f64())
                                {
                                    self.rmm_alert_threshold_inputs[index] = format!("{value}");
                                }
                            }
                            self.rmm_alert_settings_status = "Umbrales guardados".into();
                        }
                        Err(error) => {
                            self.rmm_alert_settings_status =
                                format!("No se pudieron guardar: {error}");
                        }
                    }
                }
                UiEvent::DeviceDetailDiagnosticLoaded(device_id, kind, result) => {
                    let key = (device_id, kind.clone());
                    self.device_detail_diagnostics_loading.remove(&key);
                    match result {
                        Ok(value) => {
                            self.device_detail_diagnostics
                                .entry(device_id)
                                .or_default()
                                .insert(kind, value);
                            self.device_detail_diagnostics_error.remove(&key);
                        }
                        Err(error) => {
                            self.device_detail_diagnostics_error.insert(key, error);
                        }
                    }
                }
                UiEvent::DeviceGroupUpdated(device_id, previous_group_id, result) => {
                    if self.device_group_update_pending == Some(device_id) {
                        self.device_group_update_pending = None;
                        match result {
                            Ok(()) => {
                                self.device_group_update_status = "Grupo actualizado".into();
                                self.refresh_data();
                                self.load_extra_tabs_data();
                            }
                            Err(error) => {
                                if let Some(device) = self
                                    .devices
                                    .iter_mut()
                                    .find(|device| device.id == device_id)
                                {
                                    device.group_id = previous_group_id;
                                }
                                self.device_group_update_status =
                                    format!("No se pudo actualizar el grupo: {error}");
                            }
                        }
                    }
                }
                UiEvent::EnrollmentCreated(cmd) => {
                    self.enroll_cmd_out = cmd;
                }
                UiEvent::Toast(msg) => {
                    self.last_connect_target = None;
                    self.status_msg = msg.clone();
                    self.connect_feedback = msg;
                }
                UiEvent::SessionStarted {
                    session_id,
                    ws_url,
                    allow_control,
                    label,
                } => {
                    self.spawn_viewer_tunnel(ctx.clone(), session_id, ws_url, allow_control, label);
                }
                UiEvent::RemoteFrame {
                    width,
                    height,
                    rgba,
                    kbps,
                } => {
                    if let Some(v) = &mut self.viewer {
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [width as usize, height as usize],
                            &rgba,
                        );
                        match &mut v.texture {
                            Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                            None => {
                                v.texture = Some(ctx.load_texture(
                                    "remote_screen",
                                    img,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                        }
                        v.frame_count += 1;
                        v.kbps = kbps;
                        let secs = v.fps_window_start.elapsed().as_secs_f32();
                        if secs >= 1.0 {
                            v.fps = v.frame_count as f32 / secs;
                            v.frame_count = 0;
                            v.fps_window_start = Instant::now();
                        }
                    }
                }
                UiEvent::RemoteRtt(ms) => {
                    if let Some(v) = &mut self.viewer {
                        v.rtt_ms = ms;
                    }
                }
                UiEvent::RemoteChat(sender, text) => {
                    if let Some(v) = &mut self.viewer {
                        v.chat_log.push(format!("{sender}: {text}"));
                    }
                }
                UiEvent::RemoteMonitors {
                    monitors,
                    active_index,
                } => {
                    if let Some(v) = &mut self.viewer {
                        v.monitors = monitors;
                        v.active_monitor = active_index;
                    }
                }
                UiEvent::RemoteClipboard(txt) => {
                    self.status_msg = format!("Portapapeles recibido ({} caracteres)", txt.len());
                }
                UiEvent::RemoteFileAck(session_id, transfer_id, msg) => {
                    if let Some(v) = &mut self.viewer {
                        if v.session_id == session_id && v.active_file_transfer == Some(transfer_id)
                        {
                            v.file_status = msg.clone();
                            v.active_file_transfer = None;
                            self.status_msg = msg;
                        }
                    }
                }
                UiEvent::FileTransferProgress(session_id, transfer_id, status) => {
                    if let Some(v) = &mut self.viewer {
                        if v.session_id == session_id && v.active_file_transfer == Some(transfer_id)
                        {
                            v.file_status = status;
                        }
                    }
                }
                UiEvent::FileTransferFailed(session_id, transfer_id, error) => {
                    if let Some(v) = &mut self.viewer {
                        if v.session_id == session_id && v.active_file_transfer == Some(transfer_id)
                        {
                            v.file_status = error.clone();
                            v.active_file_transfer = None;
                            self.status_msg = error;
                        }
                    }
                }
                UiEvent::GroupsLoaded(g) => {
                    self.groups = g;
                }
                UiEvent::RemoteHardwareResources(v) => {
                    if let Some(vw) = &mut self.viewer {
                        vw.hw_resources = Some(v);
                    }
                }
                UiEvent::RemoteRebootPending(d, r) => {
                    self.status_msg = format!("El equipo remoto se reiniciará en {d}s: {r}");
                }
                UiEvent::Tray(act) => match act {
                    TrayAction::ShowWindow => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::CopyId => {
                        ctx.copy_text(self.local_connection_id.clone());
                        self.status_msg = "ID copiado al portapapeles".into();
                    }
                    TrayAction::QuitApp => {
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                },
                UiEvent::PasswordChanged(res) => match res {
                    Ok(()) => {
                        self.change_pw_modal_open = false;
                        self.cur_pw_input.clear();
                        self.new_pw_input.clear();
                        self.new_pw_confirm.clear();
                        self.change_pw_msg.clear();
                        self.status_msg = "Contraseña de cuenta actualizada".into();
                    }
                    Err(e) => {
                        self.change_pw_msg = e;
                    }
                },
                UiEvent::PersonalPasswordUpdated(has, msg) => {
                    self.personal_pw_set = has;
                    self.status_msg = msg;
                    self.save_config(true);
                }
                UiEvent::DirectConnected {
                    connection_id,
                    hostname,
                    token,
                    expires_unix,
                } => {
                    let norm = normalize_conn_code(&connection_id);
                    self.connection_history
                        .retain(|h| normalize_conn_code(&h.connection_id) != norm);
                    self.connection_history.insert(
                        0,
                        ConnectionHistoryEntry {
                            connection_id,
                            hostname: hostname.clone(),
                            last_connected_unix: now_unix(),
                            reconnect_token: token.clone(),
                            reconnect_expires_unix: expires_unix,
                        },
                    );
                    self.connection_history.truncate(20);
                    if let Some(t) = &mut self.last_connect_target {
                        t.hostname = hostname;
                        t.reconnect_token = token;
                    }
                    self.save_config(true);
                }
                UiEvent::RefreshRequested => {
                    self.refresh_data();
                    self.load_extra_tabs_data();
                }
                UiEvent::SessionEnded(reason) => {
                    let target = self
                        .viewer
                        .as_ref()
                        .and_then(|v| v.reconnect_target.clone());
                    if self.auto_reconnect.is_none() {
                        if let Some(t) = target {
                            self.reconnect_prompt = Some((reason.clone(), t));
                        }
                    }
                    self.viewer = None;
                    self.status_msg = format!("Sesión finalizada: {reason}");
                    if self.tab == NavTab::Viewer {
                        self.tab = NavTab::Connect;
                    }
                }
                UiEvent::AiSettingsLoaded(s) => {
                    self.ai_provider = s.provider.clone();
                    self.ai_model = s.model.clone();
                    self.ai_base_url = s.base_url.clone();
                    self.ai_oauth_client_id = s.oauth_client_id.clone();
                    self.ai_autonomy = s.autonomy_level.clone();
                    self.ai_redact_pii = s.redact_pii;
                    self.ai_settings = Some(s);
                }
                UiEvent::AiOAuthUrl(url) => {
                    self.ai_oauth_url = url;
                    self.status_msg = "Navegador abierto para conectar con ChatGPT OAuth".into();
                }
                UiEvent::AiChatResult(res) => {
                    self.ai_busy = false;
                    if res.provider_used != "error" {
                        if let Some(ticket_id) = self.ai_ticket_context {
                            self.ai_resolution_draft = Some((ticket_id, res.reply.clone()));
                        }
                    }
                    self.ai_messages.push((
                        format!("IA ({}/{})", res.provider_used, res.model_used),
                        res.reply,
                    ));
                    self.ai_proposed_actions = res.proposed_actions;
                }
                UiEvent::AiActionExecuted(cmd) => {
                    let msg = format!(
                        "[Acción aprobada exit={}] {}\n{}",
                        cmd.exit_code, cmd.command, cmd.stdout
                    );
                    self.ai_messages.push(("Resultado".into(), msg));
                }
                UiEvent::RmmDiagnosticResult(v) => {
                    if let Some(hw) = extract_resources(&v) {
                        self.rmm_hw_snapshot = Some(hw);
                    }
                    self.rmm_diag_output = serde_json::to_string_pretty(&v).unwrap_or_default();
                }
                UiEvent::RmmCommandResult(cmd) => {
                    self.rmm_cmd_output = format!(
                        "$ {} ({} ms, exit={})\n{}{}",
                        cmd.command,
                        cmd.duration_ms,
                        cmd.exit_code,
                        cmd.stdout,
                        if cmd.stderr.is_empty() {
                            String::new()
                        } else {
                            format!("\n[stderr]\n{}", cmd.stderr)
                        }
                    );
                }
                UiEvent::CalendarLoaded(list) => {
                    self.calendar_events = list;
                }
                UiEvent::UsersLoaded(list) => {
                    self.users = list;
                }
                UiEvent::MfaSetupResult(ms) => {
                    self.mfa_setup = Some(ms);
                }
            }
        }
    }
}

fn run_local_diagnostic(kind: &str) -> (bool, serde_json::Value, Option<String>, Option<String>) {
    match kind {
        "inventory" => (true, agentdesk_rmm::collect_inventory(), None, None),
        "metrics" => (true, agentdesk_rmm::collect_metrics(), None, None),
        "resources" => (
            true,
            agentdesk_rmm::collect_hardware_resources(),
            None,
            None,
        ),
        "services" => (true, agentdesk_rmm::collect_services(), None, None),
        "events" => (true, agentdesk_rmm::collect_recent_events(), None, None),
        "screenshot" => {
            let mut cap = WindowsCapturer::new();
            match cap
                .capture_rgba()
                .and_then(|raw| encode_frame(&raw, FrameEncoding::Jpeg, 65))
            {
                Ok(jpg) => (
                    true,
                    serde_json::json!({ "captured": true }),
                    Some(B64.encode(jpg)),
                    None,
                ),
                Err(e) => (false, serde_json::json!({}), None, Some(e.to_string())),
            }
        }
        _ => (
            true,
            serde_json::json!({
                "inventory": agentdesk_rmm::collect_inventory(),
                "metrics": agentdesk_rmm::collect_metrics(),
                "resources": agentdesk_rmm::collect_hardware_resources(),
                "services": agentdesk_rmm::collect_services(),
                "events": agentdesk_rmm::collect_recent_events(),
            }),
            None,
            None,
        ),
    }
}

struct PendingHostUpload {
    filename: String,
    expected_sha256: String,
    target: FileDropTarget,
    data: Vec<u8>,
}

async fn run_incoming_host_session(
    relay_url: String,
    allow_control: bool,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut tunnel = RelayTunnel::connect(&relay_url, false).await?;
    let _wallpaper_guard = WallpaperBlackoutGuard::activate_in_background();
    let monitors = enumerate_monitors();
    let _ = tunnel
        .send_packet(&DataPacket::Control(ControlMessage::MonitorList {
            monitors,
            active_index: 0,
        }))
        .await;

    let (pkt_tx, mut pkt_rx) = tokio::sync::mpsc::channel::<DataPacket>(2);
    let (cursor_tx, mut cursor_rx) = tokio::sync::mpsc::channel::<DataPacket>(4);
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<DataPacket>(6);
    let abr = std::sync::Arc::new(std::sync::Mutex::new(AbrController::new()));
    let monitor_idx = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let keyframe_req = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let codec_pref = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let audio_enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    spawn_capture_worker(
        stop_rx.clone(),
        abr.clone(),
        monitor_idx.clone(),
        keyframe_req.clone(),
        codec_pref.clone(),
        pkt_tx,
        cursor_tx,
    );
    spawn_audio_capture_worker(stop_rx.clone(), audio_enabled.clone(), audio_tx);

    let whiteboard = WhiteboardOverlay::new();
    let mut pending_print_jobs: std::collections::VecDeque<(Uuid, Option<String>)> =
        std::collections::VecDeque::new();
    let mut uploads: HashMap<Uuid, PendingHostUpload> = HashMap::new();
    let mut udp_ch: Option<P2pUdpChannel> = None;
    let mut p2p_ep_rx: Option<tokio::sync::oneshot::Receiver<Vec<String>>> = None;
    let mut p2p_ch_rx: Option<tokio::sync::oneshot::Receiver<P2pUdpChannel>> = None;
    let mut p2p_meta: Option<([u8; 16], [u8; 32])> = None;
    let mut udp_buf = vec![0u8; 2048];
    let mut ka_tick = tokio::time::interval(Duration::from_secs(1));
    let mut clip_tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_clip_seq = clipboard_sequence_number();
    let mut last_clip_text = read_clipboard_text().unwrap_or_default();

    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Terminate {
                        reason: "closed locally".into(),
                    })).await;
                    break;
                }
            }
            _ = clip_tick.tick() => {
                let seq = clipboard_sequence_number();
                if seq != last_clip_seq {
                    last_clip_seq = seq;
                    let files = read_clipboard_files();
                    if !files.is_empty() {
                        let _ = tunnel.send_files_to_target(&files, FileDropTarget::Clipboard).await;
                    } else if let Some(txt) = read_clipboard_text() {
                        if !txt.is_empty() && txt != last_clip_text {
                            last_clip_text = txt.clone();
                            let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::ClipboardText { text: txt })).await;
                        }
                    }
                }
            }
            Some(my_eps) = async {
                match p2p_ep_rx.as_mut() {
                    Some(rx) => rx.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                p2p_ep_rx = None;
                if let Some((session_nonce, p2p_key)) = p2p_meta {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::P2pOffer {
                        session_nonce,
                        p2p_key,
                        endpoints: my_eps,
                    })).await;
                }
            }
            Some(ch) = async {
                match p2p_ch_rx.as_mut() {
                    Some(rx) => rx.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                p2p_ch_rx = None;
                let addr_str = ch.peer_addr().to_string();
                udp_ch = Some(ch);
                keyframe_req.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::P2pReady {
                    remote_addr: addr_str,
                })).await;
            }
            udp_recv = async {
                match udp_ch.as_ref() {
                    Some(ch) => ch.socket().recv_from(&mut udp_buf).await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                if let Some((n, _)) = udp_recv {
                    if let Some(pkt) = udp_ch.as_mut().and_then(|c| c.try_feed_datagram(&udp_buf[..n])) {
                        match pkt {
                            DataPacket::Input(ev) => {
                                if allow_control {
                                    inject_input_event(&ev);
                                }
                            }
                            DataPacket::Control(ControlMessage::Ping { ts_ms }) => {
                                let _ = send_adaptive_packet(&mut tunnel, &mut udp_ch, &DataPacket::Control(ControlMessage::Pong { ts_ms })).await;
                            }
                            DataPacket::Control(ControlMessage::FrameAck { seq, decode_ms }) => {
                                abr.lock().unwrap().on_ack_measured(seq, decode_ms);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ = ka_tick.tick() => {
                if let Some(ch) = udp_ch.as_mut() {
                    ch.maybe_send_keepalive().await;
                    if !ch.is_alive() {
                        udp_ch = None;
                    }
                }
            }
            Some(cur_pkt) = cursor_rx.recv() => {
                if send_adaptive_packet(&mut tunnel, &mut udp_ch, &cur_pkt).await.is_err() {
                    break;
                }
            }
            Some(audio_pkt) = audio_rx.recv() => {
                if send_adaptive_packet(&mut tunnel, &mut udp_ch, &audio_pkt).await.is_err() {
                    break;
                }
            }
            Some(pkt) = pkt_rx.recv() => {
                if send_adaptive_packet(&mut tunnel, &mut udp_ch, &pkt).await.is_err() {
                    break;
                }
            }
            incoming = tunnel.recv_packet() => {
                match incoming {
                    Ok(DataPacket::Input(ev)) => {
                        if allow_control {
                            inject_input_event(&ev);
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::Ping { ts_ms })) => {
                        let _ = send_adaptive_packet(&mut tunnel, &mut udp_ch, &DataPacket::Control(ControlMessage::Pong { ts_ms })).await;
                    }
                    Ok(DataPacket::Control(ControlMessage::FrameAck { seq, decode_ms })) => {
                        abr.lock().unwrap().on_ack_measured(seq, decode_ms);
                    }
                    Ok(DataPacket::Control(ControlMessage::RequestKeyframe)) => {
                        keyframe_req.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::SetQuality { jpeg_quality, max_fps })) => {
                        abr.lock().unwrap().set_legacy_quality(jpeg_quality, max_fps);
                        keyframe_req.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::StreamPolicy { max_width, jpeg_quality, max_fps, auto_adapt })) => {
                        abr.lock().unwrap().set_policy(max_width, jpeg_quality, max_fps, auto_adapt);
                        keyframe_req.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::P2pOffer { session_nonce, p2p_key, endpoints })) => {
                        p2p_meta = Some((session_nonce, p2p_key));
                        let (ep_rx, ch_rx) = start_host_p2p_handshake(relay_url.clone(), session_nonce, p2p_key, endpoints);
                        p2p_ep_rx = Some(ep_rx);
                        p2p_ch_rx = Some(ch_rx);
                    }
                    Ok(DataPacket::Control(ControlMessage::SelectMonitor { index })) => {
                        monitor_idx.store(index, std::sync::atomic::Ordering::Relaxed);
                        whiteboard.set_monitor(index);
                    }
                    Ok(DataPacket::Control(ControlMessage::AudioControl { enabled })) => {
                        audio_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::VideoCodecPolicy { preference })) => {
                        let v = match preference {
                            agentdesk_protocol::VideoCodecPreference::Auto => 0,
                            agentdesk_protocol::VideoCodecPreference::HardwareH264 => 1,
                            agentdesk_protocol::VideoCodecPreference::HybridTiles => 2,
                        };
                        codec_pref.store(v, std::sync::atomic::Ordering::Relaxed);
                        keyframe_req.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::WhiteboardToggle { active })) => {
                        whiteboard.set_visible(active);
                    }
                    Ok(DataPacket::Control(ControlMessage::WhiteboardStroke(stroke))) => {
                        whiteboard.add_stroke(stroke);
                    }
                    Ok(DataPacket::Control(ControlMessage::WhiteboardUndo)) => {
                        whiteboard.undo();
                    }
                    Ok(DataPacket::Control(ControlMessage::WhiteboardClear)) => {
                        whiteboard.clear();
                    }
                    Ok(DataPacket::Control(ControlMessage::RequestPrinters)) => {
                        let printers = tokio::task::spawn_blocking(enumerate_printers)
                            .await
                            .unwrap_or_default();
                        let _ = tunnel
                            .send_packet(&DataPacket::Control(ControlMessage::PrinterList { printers }))
                            .await;
                    }
                    Ok(DataPacket::Control(ControlMessage::RemotePrintJob {
                        job_id,
                        printer_name,
                        ..
                    })) => {
                        pending_print_jobs.push_back((job_id, printer_name));
                    }
                    Ok(DataPacket::Control(ControlMessage::ClipboardText { text })) => {
                        if allow_control {
                            write_clipboard_text(&text);
                            last_clip_text = text;
                            last_clip_seq = clipboard_sequence_number();
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::SpecialKey { combo })) => {
                        if allow_control {
                            inject_special_combo(&combo);
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::RequestHardwareResources)) => {
                        let hw = tokio::task::spawn_blocking(agentdesk_rmm::collect_hardware_resources)
                            .await
                            .unwrap_or_else(|_| serde_json::json!({}));
                        let _ = tunnel
                            .send_packet(&DataPacket::Control(ControlMessage::HardwareResourcesSnapshot {
                                resources_json: hw.to_string(),
                            }))
                            .await;
                    }
                    Ok(DataPacket::Control(ControlMessage::SessionAction { action, target_user, delay_secs })) => {
                        if allow_control {
                            match action.as_str() {
                                "reboot" => {
                                    let delay = delay_secs.unwrap_or(5).clamp(2, 120);
                                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::RebootPending {
                                        delay_secs: delay,
                                        reason: "Reinicio remoto solicitado".into(),
                                    })).await;
                                    let _ = tokio::task::spawn_blocking(move || agentdesk_rmm::trigger_reboot(delay)).await;
                                }
                                "lock" => {
                                    let _ = tokio::task::spawn_blocking(agentdesk_rmm::lock_workstation).await;
                                }
                                "signout" | "switch_user" => {
                                    let sid = target_user.clone();
                                    let _ = tokio::task::spawn_blocking(move || agentdesk_rmm::sign_out_windows_user(sid.as_deref())).await;
                                }
                                "list_users" => {
                                    let hw = tokio::task::spawn_blocking(agentdesk_rmm::collect_hardware_resources)
                                        .await
                                        .unwrap_or_else(|_| serde_json::json!({}));
                                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::HardwareResourcesSnapshot {
                                        resources_json: hw.to_string(),
                                    })).await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileOffer { transfer_id, filename, size_bytes, sha256 })) => {
                        if size_bytes <= 64 * 1024 * 1024 {
                            uploads.insert(transfer_id, PendingHostUpload {
                                filename,
                                expected_sha256: sha256,
                                target: FileDropTarget::Downloads,
                                data: Vec::with_capacity(size_bytes as usize),
                            });
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileOfferEx { transfer_id, filename, size_bytes, sha256, target })) => {
                        if size_bytes <= 64 * 1024 * 1024 {
                            uploads.insert(transfer_id, PendingHostUpload {
                                filename,
                                expected_sha256: sha256,
                                target,
                                data: Vec::with_capacity(size_bytes as usize),
                            });
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileChunk { transfer_id, data, eof, .. })) => {
                        if let Some(up) = uploads.get_mut(&transfer_id) {
                            up.data.extend_from_slice(&data);
                            if eof {
                                let finished = uploads.remove(&transfer_id).unwrap();
                                let (ok, msg) = if !verify_sha256_hex(&finished.expected_sha256, &finished.data) {
                                    (false, format!("SHA-256 inválido para {}", finished.filename))
                                } else {
                                    match store_received_file_for_target(finished.target, transfer_id, &finished.filename, &finished.data) {
                                        Ok(dest_path) => {
                                            if finished.target == FileDropTarget::Clipboard {
                                                let _ = write_clipboard_files(&[dest_path.clone()]);
                                                last_clip_seq = clipboard_sequence_number();
                                                (true, format!("Archivo copiado al portapapeles remoto: {}", finished.filename))
                                            } else if finished.target == FileDropTarget::RemotePrint {
                                                let (job_id, printer_hint) = pending_print_jobs
                                                    .pop_front()
                                                    .unwrap_or((transfer_id, None));
                                                let p_clone = dest_path.clone();
                                                let hint_clone = printer_hint.clone();
                                                let print_res = tokio::task::spawn_blocking(move || {
                                                    print_file_to_printer(&p_clone, hint_clone.as_deref())
                                                })
                                                .await
                                                .unwrap_or_else(|e| Err(e.to_string()));
                                                let (p_ok, p_msg) = match print_res {
                                                    Ok(m) => (true, m),
                                                    Err(e) => (false, e),
                                                };
                                                let _ = tunnel
                                                    .send_packet(&DataPacket::Control(ControlMessage::RemotePrintResult {
                                                        job_id,
                                                        ok: p_ok,
                                                        message: p_msg.clone(),
                                                    }))
                                                    .await;
                                                (p_ok, p_msg)
                                            } else {
                                                (true, format!("Recibido en {}", dest_path.display()))
                                            }
                                        }
                                        Err(e) => (false, format!("Error archivo: {e}")),
                                    }
                                };
                                let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::FileAck {
                                    transfer_id,
                                    ok,
                                    message: msg,
                                })).await;
                            }
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::Terminate { .. })) | Err(_) => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = tunnel.close().await;
    Ok(())
}

impl eframe::App for AgentDeskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events(ctx);
        self.frame_logic(ctx);

        if self.theme_mode == ThemeMode::System
            && self.palette.mode != self.theme_mode.resolved(ctx)
        {
            self.palette = apply_theme(ctx, self.theme_mode);
        }

        let p = self.palette;

        let mut submit_search = false;

        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::NONE
                    .fill(p.card_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new((
                        self.brand_icon.id(),
                        egui::vec2(30.0, 30.0),
                    )));
                    ui.label(
                        RichText::new("AgentDesk")
                            .strong()
                            .size(21.0)
                            .color(p.text_primary),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = match &self.auth {
                            Some(a) => ui.button(
                                RichText::new(&a.user.display_name).strong().color(p.accent),
                            ),
                            None => theme::session_action_button(ui, true),
                        };
                        if resp.clicked() {
                            self.user_menu_open = !self.user_menu_open;
                        }
                        self.user_menu_rect = Some(resp.rect);
                        if theme::theme_button(ui, &p, self.palette.mode).clicked() {
                            self.set_theme(ctx, self.palette.mode.toggle());
                        }
                        ui.add_space(4.0);
                        let search_id = egui::Id::new("global_search");
                        if ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::CTRL, egui::Key::K)
                        }) {
                            self.user_menu_open = false;
                            ui.memory_mut(|memory| memory.request_focus(search_id));
                        }
                        if ui.memory(|memory| memory.has_focus(search_id))
                            && ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                            })
                        {
                            self.global_search.clear();
                            ui.memory_mut(|memory| memory.surrender_focus(search_id));
                        }
                        submit_search = ui.memory(|memory| memory.has_focus(search_id))
                            && ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            });
                        let search_width = ui.available_width().clamp(0.0, 340.0);
                        let sresp = ui.add_sized(
                            [search_width, 32.0],
                            theme::centered_text_edit(&mut self.global_search)
                                .id(search_id)
                                .hint_text(theme::placeholder_text(
                                    &p,
                                    "Buscar equipo, grupo o ID  ·  Ctrl+K",
                                )),
                        );
                        self.search_rect = Some(sresp.rect);
                    });
                });
            });

        egui::TopBottomPanel::bottom("connection_status")
            .frame(
                egui::Frame::NONE
                    .fill(p.card_bg)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let color = if self.peer_ready { p.ok } else { p.warn };
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, color);
                    ui.label(RichText::new(&self.peer_status).small().color(p.text_muted))
                        .on_hover_text(&self.server_url);
                    if !self.status_msg.is_empty() {
                        ui.separator();
                        ui.add(
                            egui::Label::new(RichText::new(&self.status_msg).small()).truncate(),
                        )
                        .on_hover_text(&self.status_msg);
                    }
                });
            });

        self.ui_overlays(ctx);

        egui::SidePanel::left("nav_panel")
            .exact_width(204.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(p.sidebar_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (tab, label) in [
                        (NavTab::Connect, "Inicio"),
                        (NavTab::Devices, "Dispositivos"),
                        (NavTab::Tickets, "Centro de soporte"),
                        (NavTab::Agenda, "Agenda"),
                        (NavTab::Groups, "Grupos y clientes"),
                    ] {
                        self.nav_item(ui, tab, label);
                    }
                    if self.viewer.is_some() {
                        self.nav_item(ui, NavTab::Viewer, "Sesiones activas");
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    for (tab, label) in [
                        (NavTab::Rmm, "Monitoreo y diagnóstico"),
                        (NavTab::AiCopilot, "AgentDesk AI"),
                        (NavTab::Calendar, "Agenda e informes"),
                        (NavTab::UsersMfa, "Usuarios y MFA"),
                        (NavTab::Settings, "Configuración"),
                    ] {
                        self.nav_item(ui, tab, label);
                    }

                    if matches!(self.tab, NavTab::Agenda | NavTab::Devices) {
                        ui.separator();
                        ui.label(RichText::new("Filtro rápido").small().color(p.text_muted));
                        ui.add(theme::centered_text_edit(&mut self.search));
                        if self.tab == NavTab::Agenda {
                            ui.checkbox(&mut self.only_favorites, "Solo favoritos");
                        }
                        if self.auth.is_some() && ui.button("Actualizar lista").clicked() {
                            self.refresh_data();
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.canvas_bg)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| match self.tab {
                NavTab::Connect => {
                    egui::ScrollArea::vertical().show(ui, |ui| self.ui_connect(ui, ctx));
                }
                NavTab::Groups => self.ui_groups(ui),
                NavTab::Agenda => self.ui_agenda(ui),
                NavTab::Devices => self.ui_devices(ui),
                NavTab::Tickets => self.ui_tickets(ui),
                NavTab::AiCopilot => self.ui_ai_copilot(ui, ctx),
                NavTab::Rmm => self.ui_rmm_terminal(ui),
                NavTab::Calendar => self.ui_calendar_reports(ui),
                NavTab::UsersMfa => self.ui_users_mfa(ui),
                NavTab::Viewer => self.ui_viewer(ui),
                NavTab::Settings => self.ui_settings(ui, ctx),
            });
        self.ui_search_results(ctx, submit_search);
    }
}

fn paint_nav_icon(painter: &egui::Painter, rect: egui::Rect, tab: NavTab, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.5_f32, color);
    let center = rect.center();
    let line = |a: egui::Pos2, b: egui::Pos2| painter.line_segment([a, b], stroke);

    match tab {
        NavTab::Connect => {
            line(
                egui::pos2(rect.left() + 2.0, center.y - 3.0),
                egui::pos2(rect.right() - 2.0, center.y - 3.0),
            );
            line(
                egui::pos2(rect.right() - 5.0, center.y - 6.0),
                egui::pos2(rect.right() - 2.0, center.y - 3.0),
            );
            line(
                egui::pos2(rect.right() - 5.0, center.y),
                egui::pos2(rect.right() - 2.0, center.y - 3.0),
            );
            line(
                egui::pos2(rect.right() - 2.0, center.y + 3.0),
                egui::pos2(rect.left() + 2.0, center.y + 3.0),
            );
            line(
                egui::pos2(rect.left() + 5.0, center.y),
                egui::pos2(rect.left() + 2.0, center.y + 3.0),
            );
            line(
                egui::pos2(rect.left() + 5.0, center.y + 6.0),
                egui::pos2(rect.left() + 2.0, center.y + 3.0),
            );
        }
        NavTab::Devices | NavTab::Viewer => {
            let screen = rect.shrink2(egui::vec2(2.0, 3.0));
            painter.rect_stroke(screen, 2.0, stroke, egui::StrokeKind::Inside);
            line(
                egui::pos2(screen.left() + 3.0, screen.bottom() + 1.0),
                egui::pos2(screen.right() - 3.0, screen.bottom() + 1.0),
            );
            line(
                egui::pos2(center.x, screen.bottom() + 1.0),
                egui::pos2(center.x, rect.bottom() - 1.0),
            );
        }
        NavTab::Agenda => {
            let board = rect.shrink2(egui::vec2(3.0, 2.5));
            painter.rect_stroke(board, 2.5, stroke, egui::StrokeKind::Inside);
            for row in 0..3 {
                let y = board.top() + 4.0 + row as f32 * 4.5;
                painter.circle_filled(egui::pos2(board.left() + 4.0, y), 0.9, color);
                line(
                    egui::pos2(board.left() + 7.0, y),
                    egui::pos2(board.right() - 3.0, y),
                );
            }
        }
        NavTab::Calendar => {
            let calendar = rect.shrink2(egui::vec2(2.5, 2.0));
            painter.rect_stroke(calendar, 2.0, stroke, egui::StrokeKind::Inside);
            line(
                egui::pos2(calendar.left(), calendar.top() + 4.5),
                egui::pos2(calendar.right(), calendar.top() + 4.5),
            );
            for x in [calendar.left() + 4.0, calendar.right() - 4.0] {
                line(
                    egui::pos2(x, calendar.top() - 0.5),
                    egui::pos2(x, calendar.top() + 2.5),
                );
            }
            painter.circle_filled(
                egui::pos2(calendar.left() + 5.0, calendar.top() + 8.0),
                0.8,
                color,
            );
            painter.circle_filled(
                egui::pos2(calendar.right() - 5.0, calendar.top() + 8.0),
                0.8,
                color,
            );
        }
        NavTab::Groups => {
            painter.circle_stroke(egui::pos2(center.x, center.y - 3.0), 2.4, stroke);
            painter.circle_stroke(egui::pos2(center.x - 6.0, center.y - 1.5), 1.8, stroke);
            painter.circle_stroke(egui::pos2(center.x + 6.0, center.y - 1.5), 1.8, stroke);
            line(
                egui::pos2(center.x - 4.0, rect.bottom() - 3.0),
                egui::pos2(center.x - 3.0, center.y + 2.0),
            );
            line(
                egui::pos2(center.x - 3.0, center.y + 2.0),
                egui::pos2(center.x + 3.0, center.y + 2.0),
            );
            line(
                egui::pos2(center.x + 3.0, center.y + 2.0),
                egui::pos2(center.x + 4.0, rect.bottom() - 3.0),
            );
        }
        NavTab::UsersMfa => {
            painter.circle_stroke(egui::pos2(center.x - 2.0, center.y - 3.0), 2.8, stroke);
            line(
                egui::pos2(center.x - 7.0, rect.bottom() - 3.0),
                egui::pos2(center.x - 6.0, center.y + 2.0),
            );
            line(
                egui::pos2(center.x - 6.0, center.y + 2.0),
                egui::pos2(center.x + 2.0, center.y + 2.0),
            );
            let shield = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 8.0, center.y - 3.0),
                egui::vec2(5.0, 7.0),
            );
            painter.rect_stroke(shield, 1.5, stroke, egui::StrokeKind::Inside);
            line(
                egui::pos2(shield.left() + 1.5, shield.top() + 2.0),
                egui::pos2(shield.right() - 1.5, shield.top() + 2.0),
            );
        }
        NavTab::Tickets => {
            let bubble = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 2.0, rect.top() + 2.0),
                egui::pos2(rect.right() - 2.0, rect.bottom() - 5.0),
            );
            painter.rect_stroke(bubble, 4.0, stroke, egui::StrokeKind::Inside);
            line(
                egui::pos2(bubble.left() + 4.0, bubble.bottom() - 0.5),
                egui::pos2(bubble.left() + 2.0, rect.bottom() - 2.0),
            );
            for x in [center.x - 3.0, center.x, center.x + 3.0] {
                painter.circle_filled(egui::pos2(x, center.y - 1.5), 0.8, color);
            }
        }
        NavTab::AiCopilot => {
            painter.circle_stroke(center, 3.0, stroke);
            for (a, b) in [
                (
                    egui::pos2(center.x, rect.top()),
                    egui::pos2(center.x, center.y - 5.0),
                ),
                (
                    egui::pos2(center.x, center.y + 5.0),
                    egui::pos2(center.x, rect.bottom()),
                ),
                (
                    egui::pos2(rect.left(), center.y),
                    egui::pos2(center.x - 5.0, center.y),
                ),
                (
                    egui::pos2(center.x + 5.0, center.y),
                    egui::pos2(rect.right(), center.y),
                ),
            ] {
                line(a, b);
            }
        }
        NavTab::Rmm => {
            line(
                egui::pos2(rect.left() + 2.0, rect.top() + 2.0),
                egui::pos2(rect.left() + 2.0, rect.bottom() - 2.0),
            );
            line(
                egui::pos2(rect.left() + 2.0, rect.bottom() - 2.0),
                egui::pos2(rect.right() - 1.0, rect.bottom() - 2.0),
            );
            let points = [
                egui::pos2(rect.left() + 4.0, center.y + 2.0),
                egui::pos2(center.x - 2.0, center.y - 1.0),
                egui::pos2(center.x + 1.0, center.y + 1.0),
                egui::pos2(rect.right() - 2.0, rect.top() + 3.0),
            ];
            for pair in points.windows(2) {
                line(pair[0], pair[1]);
            }
        }
        NavTab::Settings => {
            painter.circle_stroke(center, 4.0, stroke);
            painter.circle_stroke(center, 1.4, stroke);
            for ray in 0..8 {
                let direction = egui::Vec2::angled(ray as f32 * std::f32::consts::FRAC_PI_4);
                line(center + direction * 5.0, center + direction * 7.5);
            }
        }
    }
}

impl AgentDeskApp {
    fn nav_item(&mut self, ui: &mut egui::Ui, tab: NavTab, label: &str) {
        let selected = self.tab == tab;
        let p = self.palette;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });

        let visuals = ui.style().interact_selectable(&response, selected);
        let painter = ui.painter();
        let fill = if selected {
            p.badge_bg
        } else if response.hovered() {
            visuals.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        if fill != egui::Color32::TRANSPARENT {
            painter.rect_filled(rect, 8.0, fill);
        }
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(1.0, 8.0),
                    egui::vec2(3.0, rect.height() - 16.0),
                ),
                2.0,
                p.accent,
            );
        }

        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 24.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        );
        paint_nav_icon(
            painter,
            icon_rect,
            tab,
            if selected { p.accent } else { p.text_muted },
        );
        painter.text(
            egui::pos2(rect.left() + 42.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.0),
            p.text_primary,
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                8.0,
                egui::Stroke::new(2.0_f32, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        if response.on_hover_text(label).clicked() {
            self.tab = tab;
        }
    }

    fn ui_connect(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Centro de soporte");
        ui.label(
            RichText::new("Conectá con un equipo o compartí el acceso seguro a este dispositivo.")
                .small()
                .color(self.palette.text_muted),
        );
        ui.add_space(14.0);

        if ui.available_width() >= 820.0 {
            ui.columns(2, |cols| {
                self.ui_share_connection(&mut cols[0], ctx);
                self.ui_remote_connection(&mut cols[1]);
            });
        } else {
            self.ui_remote_connection(ui);
            ui.add_space(10.0);
            self.ui_share_connection(ui, ctx);
        }

        if !self.connection_history.is_empty() {
            ui.add_space(14.0);
            self.ui_recent_connections(ui);
        }
    }

    fn ui_share_connection(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.palette;
        styled_card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(252.0);
            ui.label(
                RichText::new("Compartir mi equipo")
                    .size(16.0)
                    .strong()
                    .color(p.accent),
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Tu ID").small().color(p.text_muted));
            badge_frame(&p).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(&self.local_connection_id)
                            .font(FontId::monospace(27.0))
                            .strong()
                            .color(p.mono_highlight),
                    );
                    if theme::copy_button(ui, &p, "Copiar ID").clicked() {
                        ctx.copy_text(self.local_connection_id.clone());
                        self.status_msg = "ID copiado al portapapeles".into();
                    }
                });
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new("Contraseña temporal")
                    .small()
                    .color(p.text_muted),
            );
            badge_frame(&p).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let displayed = if self.show_local_password {
                        self.local_password.clone()
                    } else {
                        "••••••••".to_string()
                    };
                    ui.label(
                        RichText::new(displayed)
                            .font(FontId::monospace(19.0))
                            .strong()
                            .color(p.mono_highlight),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if theme::refresh_button(ui, &p, "Regenerar contraseña temporal").clicked()
                        {
                            self.rotate_random_password();
                        }
                        if theme::copy_button(ui, &p, "Copiar contraseña temporal").clicked() {
                            ctx.copy_text(self.local_password.clone());
                            self.status_msg = "Contraseña copiada".into();
                        }
                        if theme::visibility_button(ui, self.show_local_password).clicked() {
                            self.show_local_password = !self.show_local_password;
                        }
                    });
                });
            });

            ui.add_space(12.0);
            if ui.button("Copiar datos de acceso").clicked() {
                let clip = format!(
                    "AgentDesk ID: {}\nContraseña: {}\nServidor: {}",
                    self.local_connection_id, self.local_password, self.server_url
                );
                ctx.copy_text(clip);
                self.status_msg = "Credenciales de conexión copiadas".into();
            }

            ui.add_space(10.0);
            ui.separator();
            egui::CollapsingHeader::new("Acceso desatendido").show(ui, |ui| {
                ui.label(
                    RichText::new("Contraseña personal fija (acceso desatendido)")
                        .small()
                        .color(p.text_muted),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        theme::centered_text_edit(&mut self.personal_pw_input)
                            .password(true)
                            .hint_text(theme::placeholder_text(
                                &p,
                                if self.personal_pw_set {
                                    "Fijada ✔ (escribí para cambiarla)"
                                } else {
                                    "mínimo 4 caracteres"
                                },
                            ))
                            .desired_width(190.0),
                    );
                    if ui.button("Fijar").clicked() {
                        self.set_personal_password(true);
                    }
                    if self.personal_pw_set && ui.button("Quitar").clicked() {
                        self.set_personal_password(false);
                    }
                });
                ui.label(
                    RichText::new("Email de usuario propietario (opcional)")
                        .small()
                        .color(p.text_muted),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        theme::centered_text_edit(&mut self.owner_email_input)
                            .hint_text(theme::placeholder_text(&p, "usuario@empresa.com"))
                            .desired_width(220.0),
                    );
                    if ui.button("Asignar").clicked() {
                        self.save_config(true);
                        self.start_embedded_host(ctx.clone());
                        self.status_msg = "Email propietario enviado al registrar el equipo".into();
                    }
                });
                let mut sw = self.start_with_windows;
                if ui
                    .checkbox(&mut sw, "Iniciar AgentDesk al iniciar sesión en Windows")
                    .changed()
                {
                    match set_windows_autostart(sw) {
                        Ok(()) => {
                            self.start_with_windows = sw;
                            self.save_config(true);
                        }
                        Err(e) => {
                            self.status_msg =
                                format!("No se pudo cambiar el inicio con Windows: {e}")
                        }
                    }
                }
                egui::CollapsingHeader::new("Servicio Windows (disponible antes del login)")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Pegá un token de enrolamiento (Equipos → Generar token).",
                            )
                            .small()
                            .color(p.text_muted),
                        );
                        ui.add(
                            theme::centered_text_edit(&mut self.service_enroll_token)
                                .desired_width(260.0),
                        );
                        if ui.button("Instalar servicio AgentDeskHost (UAC)").clicked() {
                            match install_host_service_elevated(
                                &self.server_url,
                                &self.service_enroll_token,
                            ) {
                                Ok(m) => self.status_msg = m,
                                Err(e) => self.status_msg = format!("Error servicio: {e}"),
                            }
                        }
                    });
            });

            if let Some(inc) = &self.incoming_control {
                ui.add_space(12.0);
                egui::Frame::NONE
                    .fill(p.badge_bg)
                    .stroke(egui::Stroke::new(1.5_f32, p.danger))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "⚠ Control remoto activo: {} ({})",
                                inc.caller_name,
                                if inc.allow_control {
                                    "Control total"
                                } else {
                                    "Solo ver"
                                }
                            ))
                            .strong()
                            .color(p.danger),
                        );
                        if ui.add(danger_button(&p, "Cortar control remoto")).clicked() {
                            let _ = inc.stop_tx.send(true);
                        }
                    });
            }
        });
    }

    fn ui_remote_connection(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        styled_card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(248.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("↗").size(20.0).strong().color(p.accent));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Nueva conexión")
                            .size(17.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.label(
                        RichText::new("Conectate a un equipo remoto de forma segura")
                            .small()
                            .color(p.text_muted),
                    );
                });
            });
            ui.add_space(14.0);

            ui.label(
                RichText::new("ID DEL EQUIPO")
                    .size(11.0)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add(
                theme::centered_text_edit(&mut self.partner_id_input)
                    .font(FontId::monospace(18.0))
                    .hint_text(theme::placeholder_text(
                        &p,
                        "Ingresá el ID o nombre del equipo",
                    ))
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(10.0);
            ui.label(
                RichText::new("CONTRASEÑA")
                    .size(11.0)
                    .strong()
                    .color(p.text_muted),
            );
            ui.horizontal(|ui| {
                let pw_resp = ui.add(
                    theme::centered_text_edit(&mut self.partner_password_input)
                        .password(!self.show_partner_password)
                        .font(FontId::monospace(16.0))
                        .hint_text(theme::placeholder_text(&p, "Contraseña del equipo remoto"))
                        .desired_width((ui.available_width() - 46.0).max(40.0)),
                );
                if theme::visibility_button(ui, self.show_partner_password).clicked() {
                    self.show_partner_password = !self.show_partner_password;
                }
                if pw_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.connect_by_id();
                }
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Permisos").small().color(p.text_muted));
                ui.selectable_value(&mut self.partner_allow_control, true, "⌘  Control remoto");
                ui.selectable_value(&mut self.partner_allow_control, false, "◉  Solo ver");
            });

            ui.add_space(10.0);
            if ui
                .add_sized(
                    [ui.available_width(), 40.0],
                    primary_button(&p, "Conectar al equipo  →"),
                )
                .clicked()
            {
                self.connect_by_id();
            }

            if !self.connect_feedback.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(&self.connect_feedback).color(p.warn));
            }
        });
    }

    fn ui_recent_connections(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        styled_card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Conexiones recientes")
                    .size(16.0)
                    .color(p.accent),
            );
            ui.add_space(4.0);
            let now = now_unix();
            let mut pick: Option<usize> = None;
            for (i, h) in self.connection_history.iter().enumerate().take(6) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{} ({})", h.hostname, h.connection_id)).monospace(),
                    );
                    ui.label(
                        RichText::new(fmt_elapsed(now - h.last_connected_unix))
                            .small()
                            .color(p.text_muted),
                    );
                    if h.is_trust_valid(now) {
                        ui.colored_label(p.ok, "<1h · reconexión rápida");
                    } else {
                        ui.colored_label(p.warn, ">1h · pedirá contraseña");
                    }
                    if ui.button("Reconectar").clicked() {
                        pick = Some(i);
                    }
                });
            }
            if let Some(i) = pick {
                self.reconnect_from_history(i);
            }
        });
    }

    fn ui_account_login_card(&mut self, ui: &mut egui::Ui, section_title: &str) {
        let p = self.palette;
        styled_card(&p).show(ui, |ui| {
            ui.set_max_width(480.0);
            ui.label(
                RichText::new(format!("Acceso de Técnico — {section_title}"))
                    .size(16.0)
                    .strong()
                    .color(p.accent),
            );
            ui.label(
                RichText::new("Para usar ID+Contraseña no necesitas iniciar sesión. Inicia sesión solo para gestionar la Agenda, Equipos Desatendidos y Tickets.")
                    .small()
                    .color(p.text_muted),
            );
            ui.add_space(8.0);
            ui.label("Email (por defecto: juan@dolan.com.ar):");
            ui.add(theme::centered_text_edit(&mut self.email).desired_width(420.0));
            ui.label("Contraseña:");
            let resp = ui.add(
                theme::centered_text_edit(&mut self.password)
                    .password(true)
                    .desired_width(420.0),
            );
            ui.label("Código MFA (si tu cuenta lo tiene activado):");
            ui.add(
                theme::centered_text_edit(&mut self.mfa_login_code)
                    .hint_text(theme::placeholder_text(&p, "000000"))
                    .char_limit(6)
                    .desired_width(160.0),
            );
            ui.add_space(8.0);
            if theme::session_action_button(ui, true).clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                self.do_login();
            }
            if !self.status_msg.is_empty() {
                ui.label(RichText::new(&self.status_msg).color(p.warn));
            }
        });
    }

    fn ui_agenda(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Clientes Agendados");
            return;
        }
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.heading("Clientes Agendados");
            if ui.add(primary_button(&p, "+ Agendar cliente")).clicked() {
                self.show_add_agenda = !self.show_add_agenda;
            }
        });

        if self.show_add_agenda {
            styled_card(&p).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Alias:");
                    ui.add(theme::centered_text_edit(&mut self.new_alias));
                    ui.label("Empresa:");
                    ui.add(theme::centered_text_edit(&mut self.new_company));
                });
                ui.horizontal(|ui| {
                    ui.label("Contacto:");
                    ui.add(theme::centered_text_edit(&mut self.new_contact));
                    ui.label("Grupo:");
                    ui.add(theme::centered_text_edit(&mut self.new_group));
                });
                ui.horizontal(|ui| {
                    ui.label("Notas:");
                    ui.add(theme::centered_text_edit(&mut self.new_notes));
                    ui.checkbox(&mut self.new_fav, "Favorito");
                });
                let dev_names: Vec<String> = std::iter::once("Sin vincular".to_string())
                    .chain(
                        self.devices
                            .iter()
                            .map(|d| format!("{} ({})", d.hostname, d.public_code)),
                    )
                    .collect();
                egui::ComboBox::from_label("Equipo agente")
                    .selected_text(
                        dev_names
                            .get(self.new_device_idx)
                            .cloned()
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (idx, name) in dev_names.iter().enumerate() {
                            ui.selectable_value(&mut self.new_device_idx, idx, name);
                        }
                    });
                let group_names: Vec<String> = std::iter::once("General (sin grupo)".to_string())
                    .chain(self.groups.iter().map(|g| g.name.clone()))
                    .collect();
                egui::ComboBox::from_label("Grupo compartido")
                    .selected_text(
                        group_names
                            .get(self.new_agenda_group_idx)
                            .cloned()
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (idx, name) in group_names.iter().enumerate() {
                            ui.selectable_value(&mut self.new_agenda_group_idx, idx, name);
                        }
                    });
                if ui.button("Guardar cliente").clicked() && !self.new_alias.trim().is_empty() {
                    let device_id = if self.new_device_idx > 0 {
                        self.devices.get(self.new_device_idx - 1).map(|d| d.id)
                    } else {
                        None
                    };
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let payload = UpsertAddressBookRequest {
                        device_id,
                        group_id: if self.new_agenda_group_idx > 0 {
                            self.groups.get(self.new_agenda_group_idx - 1).map(|g| g.id)
                        } else {
                            None
                        },
                        alias: self.new_alias.clone(),
                        client_company: self.new_company.clone(),
                        contact_name: self.new_contact.clone(),
                        contact_email: String::new(),
                        group_name: self.new_group.clone(),
                        is_favorite: self.new_fav,
                        notes: self.new_notes.clone(),
                        scheduled_next_at: None,
                        icon_kind: "desktop".into(),
                        accent_color: String::new(),
                    };
                    self.rt.spawn(async move {
                        let _ = http
                            .post(format!("{base}/api/v1/address-book"))
                            .bearer_auth(&token)
                            .json(&payload)
                            .send()
                            .await;
                        let _ = tx.send(UiEvent::Toast("Cliente agendado guardado".into()));
                    });
                    self.new_alias.clear();
                    self.new_notes.clear();
                    self.show_add_agenda = false;
                    self.refresh_data();
                }
            });
        }

        ui.separator();
        let q = self.search.to_lowercase();
        let entries = self.agenda.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for a in entries {
                if self.only_favorites && !a.is_favorite {
                    continue;
                }
                if !q.is_empty()
                    && !a.alias.to_lowercase().contains(&q)
                    && !a.client_company.to_lowercase().contains(&q)
                    && !a.contact_name.to_lowercase().contains(&q)
                {
                    continue;
                }
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let star = if a.is_favorite { "★" } else { "☆" };
                        ui.strong(format!("{star} {} [{}]", a.alias, a.group_name));
                        if !a.client_company.is_empty() {
                            ui.label(format!("· {}", a.client_company));
                        }
                        if let Some(st) = &a.device_status {
                            let (color, label) = if st == "online" {
                                (p.ok, "En línea")
                            } else {
                                (p.text_muted, "Desconectado")
                            };
                            theme::status_indicator(ui, color, label);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(did) = a.device_id {
                                if ui.add(primary_button(&p, "Conectar (Control)")).clicked() {
                                    self.start_session(did, None, true, a.alias.clone());
                                }
                                if ui.button("Solo Ver").clicked() {
                                    self.start_session(did, None, false, a.alias.clone());
                                }
                            }
                        });
                    });
                    if !a.notes.is_empty() {
                        ui.label(RichText::new(&a.notes).small().color(p.text_muted));
                    }
                });
                ui.add_space(6.0);
            }
        });
    }

    fn ui_devices(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Dispositivos");
            return;
        }

        let p = self.palette;
        ui.horizontal(|ui| {
            ui.heading("Dispositivos");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::refresh_button(ui, &p, "Actualizar inventario").clicked()
                    && !self.inventory_loading
                {
                    self.refresh_data();
                }
                if ui.button("Generar token").clicked() {
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    self.rt.spawn(async move {
                        if let Ok(response) = http
                            .post(format!("{base}/api/v1/enrollments"))
                            .bearer_auth(&token)
                            .json(&serde_json::json!({
                                "label": "Desktop Enroll",
                                "max_uses": 20,
                                "expires_hours": 168
                            }))
                            .send()
                            .await
                        {
                            if let Ok(enrollment) =
                                response.json::<CreateEnrollmentResponse>().await
                            {
                                let _ = tx.send(UiEvent::EnrollmentCreated(format!(
                                    "agentdesk-host.exe --server {base} --enroll-token {}",
                                    enrollment.enrollment_token
                                )));
                            }
                        }
                    });
                }
            });
        });

        if self.inventory_loading {
            ui.label(
                RichText::new("Actualizando inventario…")
                    .small()
                    .color(p.text_muted),
            );
        }
        if let Some(error) = self.inventory_error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    p.danger,
                    format!("No se pudo actualizar el inventario: {error}"),
                );
                if ui.small_button("Reintentar").clicked() && !self.inventory_loading {
                    self.refresh_data();
                }
            });
        }

        if !self.enroll_cmd_out.is_empty() {
            ui.add(
                theme::centered_text_edit(&mut self.enroll_cmd_out).desired_width(f32::INFINITY),
            );
        }

        ui.horizontal_wrapped(|ui| {
            ui.add(
                theme::centered_text_edit(&mut self.search)
                    .hint_text(theme::placeholder_text(&p, "Nombre o ID"))
                    .desired_width(260.0),
            );
            egui::ComboBox::from_id_salt("devices_status_filter")
                .selected_text(&self.device_status_filter)
                .show_ui(ui, |ui| {
                    for value in ["Todos los estados", "En línea", "Desconectados"] {
                        ui.selectable_value(&mut self.device_status_filter, value.into(), value);
                    }
                });
            egui::ComboBox::from_id_salt("devices_group_filter")
                .selected_text(
                    self.device_group_filter
                        .and_then(|id| self.groups.iter().find(|group| group.id == id))
                        .map(|group| group.name.as_str())
                        .unwrap_or("Todos los grupos"),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.device_group_filter, None, "Todos los grupos");
                    for group in &self.groups {
                        ui.selectable_value(
                            &mut self.device_group_filter,
                            Some(group.id),
                            &group.name,
                        );
                    }
                });
            ui.checkbox(&mut self.devices_favorites_only, "Favoritos");
        });

        let query = self.search.trim().to_lowercase();
        let devices: Vec<DeviceDto> = self
            .devices
            .iter()
            .filter(|device| {
                let matches_query = query.is_empty()
                    || device.hostname.to_lowercase().contains(&query)
                    || device.public_code.to_lowercase().contains(&query)
                    || device
                        .owner_email
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query);
                let matches_status = match self.device_status_filter.as_str() {
                    "En línea" => device.status == "online",
                    "Desconectados" => device.status != "online",
                    _ => true,
                };
                let entries: Vec<&AddressBookEntryDto> = self
                    .agenda
                    .iter()
                    .filter(|entry| entry.device_id == Some(device.id))
                    .collect();
                let matches_favorite =
                    !self.devices_favorites_only || entries.iter().any(|entry| entry.is_favorite);
                let matches_group = self
                    .device_group_filter
                    .is_none_or(|group_id| device.group_id == Some(group_id));
                matches_query && matches_status && matches_favorite && matches_group
            })
            .cloned()
            .collect();

        ui.label(
            RichText::new(format!(
                "{} de {} dispositivos",
                devices.len(),
                self.devices.len()
            ))
            .small()
            .color(p.text_muted),
        );

        ui.add_space(8.0);
        ui.columns(2, |columns| {
            let list = &mut columns[0];
            egui::ScrollArea::vertical()
                .id_salt("device_inventory")
                .show(list, |ui| {
                    egui::Grid::new("device_table")
                        .num_columns(4)
                        .striped(true)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("DISPOSITIVO");
                            ui.strong("ESTADO");
                            ui.strong("GRUPO");
                            ui.strong("TIPO");
                            ui.end_row();

                            for device in &devices {
                                let selected = self.selected_device_id == Some(device.id);
                                if ui
                                    .selectable_label(
                                        selected,
                                        RichText::new(&device.hostname).strong(),
                                    )
                                    .clicked()
                                {
                                    self.selected_device_id = Some(device.id);
                                }
                                let (label, color) = if device.status == "online" {
                                    ("En línea", p.ok)
                                } else {
                                    ("Desconectado", p.text_muted)
                                };
                                theme::status_indicator(ui, color, label);
                                let group_name = device
                                    .group_id
                                    .and_then(|group_id| self.groups.iter().find(|group| group.id == group_id))
                                    .map(|group| group.name.as_str())
                                    .unwrap_or("Sin grupo");
                                ui.label(RichText::new(group_name).color(p.text_muted));
                                ui.label(&device.kind);
                                ui.end_row();
                            }
                        });

                    if devices.is_empty() {
                        ui.label(
                            RichText::new("No hay dispositivos para esta búsqueda.")
                                .color(p.text_muted),
                        );
                    }
                });

            let detail_panel = &mut columns[1];
            let detail_height = detail_panel.available_height();
            egui::ScrollArea::vertical()
                .id_salt("device_details_scroll")
                .max_height(detail_height)
                .auto_shrink([false, false])
                .show(detail_panel, |detail_panel| {
            let selected = self
                .selected_device_id
                .and_then(|id| devices.iter().find(|device| device.id == id))
                .or_else(|| devices.first());
            if let Some(device) = selected {
                self.selected_device_id = Some(device.id);
                if self.device_telemetry_device != Some(device.id) {
                    self.load_device_telemetry(device.id);
                }
                if self.device_alerts_device != Some(device.id) {
                    self.load_device_alerts(device.id);
                }
                styled_card(&p).show(detail_panel, |ui| {
                    ui.heading(&device.hostname);
                    ui.label(
                        RichText::new(format!("ID {}", device.public_code))
                            .monospace()
                            .color(p.text_muted),
                    );
                    ui.separator();

                    let current_group_name = device.group_id.and_then(|group_id| {
                        self.groups
                            .iter()
                            .find(|group| group.id == group_id)
                            .map(|group| group.name.clone())
                    });
                    let current_group_writable = match device.group_id {
                        Some(group_id) => self.groups.iter().any(|group| {
                            group.id == group_id
                                && matches!(group.my_permission.as_str(), "full_control" | "read_write")
                        }),
                        None => self.groups.iter().any(|group| {
                            matches!(group.my_permission.as_str(), "full_control" | "read_write")
                        }),
                    };
                    let role_can_control = self.auth.as_ref().is_some_and(|auth| {
                        !matches!(auth.user.role.as_str(), "client" | "auditor")
                    });
                    let can_control_device = role_can_control
                        && device.group_id.map_or(true, |group_id| {
                            self.groups.iter().any(|group| {
                                group.id == group_id
                                    && matches!(group.my_permission.as_str(), "full_control" | "read_write")
                            })
                        });
                    let writable_groups: Vec<_> = self
                        .groups
                        .iter()
                        .filter(|group| matches!(group.my_permission.as_str(), "full_control" | "read_write"))
                        .cloned()
                        .collect();
                    let mut chosen_group = device.group_id;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Grupo");
                        if current_group_writable {
                            ui.add_enabled_ui(self.device_group_update_pending != Some(device.id), |ui| {
                                egui::ComboBox::from_id_salt(("device_group", device.id))
                                    .selected_text(chosen_group.and_then(|id| writable_groups.iter().find(|group| group.id == id).map(|group| group.name.clone())).unwrap_or_else(|| "Sin grupo".into()))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut chosen_group, None, "Sin grupo");
                                        for group in &writable_groups {
                                            ui.selectable_value(&mut chosen_group, Some(group.id), &group.name);
                                        }
                                    });
                            });
                        } else {
                            ui.label(current_group_name.unwrap_or_else(|| "Grupo restringido".into()));
                        }
                        if chosen_group != device.group_id {
                            self.update_device_group(device.id, device.group_id, chosen_group);
                        }
                    });
                    if !self.device_group_update_status.is_empty() {
                        let color = if self.device_group_update_status.starts_with("No se pudo") { p.danger } else { p.text_muted };
                        ui.label(RichText::new(&self.device_group_update_status).small().color(color));
                    }
                    ui.separator();

                    let diagnostic_kind = match self.device_detail_tab.as_str() {
                        "Hardware" => Some("resources"),
                        "Rendimiento" => Some("metrics"),
                        "Servicios" => Some("services"),
                        "Sistema y red" => Some("inventory"),
                        _ => None,
                    };
                    if let Some(kind) = diagnostic_kind {
                        let key = (device.id, kind.to_string());
                        if device.status == "online"
                            && !self.device_detail_diagnostics_loading.contains(&key)
                            && !self.device_detail_diagnostics_error.contains_key(&key)
                            && !self.device_detail_diagnostics.get(&device.id).is_some_and(|items| items.contains_key(kind))
                        {
                            self.load_device_detail_diagnostic(device.id, kind);
                        }
                    }
                    let mut selected_diagnostic = None;
                    ui.horizontal_wrapped(|ui| {
                        for tab in ["General", "Hardware", "Rendimiento", "Servicios", "Sistema y red", "Historial"] {
                            if ui.selectable_value(&mut self.device_detail_tab, tab.into(), tab).changed() {
                                selected_diagnostic = match tab {
                                    "Hardware" => Some("resources"),
                                    "Rendimiento" => Some("metrics"),
                                    "Servicios" => Some("services"),
                                    "Sistema y red" => Some("inventory"),
                                    _ => None,
                                };
                            }
                        }
                        if theme::refresh_button(ui, &p, "Actualizar esta vista").clicked() {
                            let active_kind = match self.device_detail_tab.as_str() {
                                "Hardware" => Some("resources"),
                                "Rendimiento" => Some("metrics"),
                                "Servicios" => Some("services"),
                                "Sistema y red" => Some("inventory"),
                                _ => None,
                            };
                            if let Some(kind) = active_kind {
                                self.load_device_detail_diagnostic(device.id, kind);
                            } else if self.device_detail_tab == "Historial" {
                                self.load_device_telemetry(device.id);
                            }
                            self.load_device_alerts(device.id);
                        }
                    });
                    if let Some(kind) = selected_diagnostic.filter(|_| device.status == "online") {
                        self.load_device_detail_diagnostic(device.id, kind);
                    }
                    ui.separator();

                    if self.device_detail_tab == "Historial" {
                        ui.label(RichText::new(&self.device_telemetry_status).small().color(p.text_muted));
                        if self.device_telemetry_device != Some(device.id) {
                            ui.label(RichText::new("Cargando historial…").small().color(p.text_muted));
                        } else if self.device_telemetry.is_empty() {
                            ui.label("Todavía no hay diagnósticos guardados para este equipo.");
                        } else {
                            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                for record in &self.device_telemetry {
                                    let kind = record.get("kind").and_then(|v| v.as_str()).unwrap_or("diagnóstico");
                                    let collected_at = record.get("collected_at").and_then(|v| v.as_str()).unwrap_or("fecha desconocida");
                                    egui::CollapsingHeader::new(format!("{} · {}", kind, collected_at))
                                        .id_salt(record.get("id").cloned().unwrap_or(serde_json::Value::Null))
                                        .show(ui, |ui| {
                                            let payload = record.get("payload").cloned().unwrap_or(serde_json::Value::Null);
                                            ui.monospace(serde_json::to_string_pretty(&payload).unwrap_or_default());
                                        });
                                }
                            });
                        }
                    } else if self.device_detail_tab == "General" {
                        let (label, color) = if device.status == "online" {
                            ("En línea", p.ok)
                        } else {
                            ("Desconectado", p.text_muted)
                        };
                        theme::status_indicator(ui, color, label);
                        ui.label(format!("Tipo {}", device.kind));
                        ui.label(format!("Sistema {} · {}", device.os_name, device.os_arch));
                        ui.label(format!("Versión del agente {}", device.agent_version));
                        ui.label(format!("Pantalla {} × {}", device.screen_width, device.screen_height));
                        ui.label(format!("Memoria privada del agente {:.1} MB", device.private_mem_mb));
                        ui.label(format!("Último contacto {}", device.last_heartbeat_at));
                        if let Some(owner) = &device.owner_email {
                            ui.label(format!("Propietario {owner}"));
                        }
                        if device.has_personal_password {
                            ui.label("Acceso desatendido configurado");
                        }
                        ui.add_space(12.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(device.status == "online" && can_control_device, primary_button(&p, "Conectar")).clicked() {
                                self.start_session(device.id, None, true, device.hostname.clone());
                            }
                            if ui.add_enabled(device.status == "online", egui::Button::new("Solo ver")).clicked() {
                                self.start_session(device.id, None, false, device.hostname.clone());
                            }
                            if ui.button("Diagnóstico RMM").clicked() {
                                if let Some(index) = self.devices.iter().position(|item| item.id == device.id) {
                                    self.rmm_device_idx = index;
                                }
                                self.tab = NavTab::Rmm;
                            }
                            if ui.button("Copiloto IA").clicked() {
                                if let Some(index) = self.devices.iter().position(|item| item.id == device.id) {
                                    self.ai_device_idx = index + 1;
                                }
                                self.ai_ticket_context = None;
                                self.ai_resolution_draft = None;
                                self.tab = NavTab::AiCopilot;
                            }
                        });
                        if !can_control_device {
                            let message = if role_can_control && device.group_id.is_some() {
                                "Este grupo permite solo lectura; el control remoto está deshabilitado."
                            } else {
                                "Tu rol permite sesiones de solo vista."
                            };
                            ui.label(RichText::new(message).small().color(p.text_muted));
                        }
                    } else if device.status != "online" {
                        ui.colored_label(p.warn, "El equipo está desconectado; el diagnóstico en vivo requiere que vuelva a estar en línea.");
                    } else if let Some(kind) = diagnostic_kind {
                        let key = (device.id, kind.to_string());
                        if self.device_detail_diagnostics_loading.contains(&key) {
                            ui.label(RichText::new("Consultando al agente…").small().color(p.text_muted));
                        } else if let Some(error) = self.device_detail_diagnostics_error.get(&key) {
                            ui.colored_label(p.danger, format!("No se pudo consultar el diagnóstico: {error}"));
                            if ui.button("Reintentar").clicked() {
                                self.load_device_detail_diagnostic(device.id, kind);
                            }
                        } else if let Some(data) = self.device_detail_diagnostics.get(&device.id).and_then(|items| items.get(kind)).cloned() {
                            match kind {
                                "resources" => {
                                    if let Some(resources) = extract_resources(&data) {
                                        render_hardware_resources_ui(ui, &p, &resources);
                                    } else {
                                        ui.label(RichText::new("El agente no devolvió datos de hardware utilizables.").color(p.text_muted));
                                    }
                                }
                                "metrics" => {
                                    let number = |key: &str| data.get(key).and_then(|value| value.as_f64());
                                    let cpu_trend = telemetry_percent_series(&self.device_telemetry, "cpu_load_pct");
                                    let ram_trend = telemetry_percent_series(&self.device_telemetry, "ram_used_pct");
                                    let disk_trend = telemetry_percent_series(&self.device_telemetry, "disk_used_pct");
                                    ui.horizontal_wrapped(|ui| {
                                        if let Some(cpu) = number("cpu_load_pct") {
                                            ui.label(format!("CPU {:.0}%", cpu));
                                        }
                                        if let (Some(used), Some(total)) = (number("ram_used_mb"), number("ram_total_mb")) {
                                            ui.label(format!("RAM {:.0} / {:.0} MB", used, total));
                                        }
                                        if let Some(free) = number("c_drive_free_pct") {
                                            ui.label(format!("Disco C: {:.0}% libre", free));
                                        }
                                        if let Some(count) = number("process_count") {
                                            ui.label(format!("{} procesos", count));
                                        }
                                    });
                                    let cpu = number("cpu_load_pct");
                                    let ram_pct = number("ram_used_mb").zip(number("ram_total_mb"))
                                        .filter(|(_, total)| *total > 0.0)
                                        .map(|(used, total)| used * 100.0 / total);
                                    let disk_pct = number("c_drive_free_pct").map(|free| 100.0 - free);
                                    if cpu.is_some() || ram_pct.is_some() || disk_pct.is_some() {
                                        ui.add_space(6.0);
                                        ui.strong("Tendencia · muestras guardadas");
                                        ui.horizontal_wrapped(|ui| {
                                            if let Some(value) = cpu {
                                                render_percent_trend(ui, &p, "CPU", value, &cpu_trend);
                                            }
                                            if let Some(value) = ram_pct {
                                                render_percent_trend(ui, &p, "RAM usada", value, &ram_trend);
                                            }
                                            if let Some(value) = disk_pct {
                                                render_percent_trend(ui, &p, "Disco C usado", value, &disk_trend);
                                            }
                                        });
                                    }
                                    ui.separator();
                                    ui.horizontal(|ui| {
                                        ui.strong("Alertas del equipo");
                                        if theme::refresh_button(ui, &p, "Actualizar alertas").clicked() {
                                            self.load_device_alerts(device.id);
                                        }
                                    });
                                    ui.label(RichText::new(&self.device_alerts_status).small().color(p.text_muted));
                                    ui.label(RichText::new("Se actualizan periódicamente mientras el agente está conectado y al ejecutar el diagnóstico manual.").small().color(p.text_muted));
                                    if self.device_alerts_device != Some(device.id) {
                                        ui.label(RichText::new("Cargando alertas…").small().color(p.text_muted));
                                    } else {
                                        let alerts = self.device_alerts.clone();
                                        let active_alerts: Vec<_> = alerts.iter()
                                            .filter(|alert| alert.get("status").and_then(|value| value.as_str()) != Some("resolved"))
                                            .collect();
                                        if active_alerts.is_empty() {
                                            ui.colored_label(p.ok, "Sin alertas activas");
                                        }
                                        for alert in active_alerts {
                                            let status = alert.get("status").and_then(|value| value.as_str()).unwrap_or("open");
                                            let severity = alert.get("severity").and_then(|value| value.as_str()).unwrap_or("warning");
                                            let severity_color = if severity == "critical" { p.danger } else { p.warn };
                                            let alert_id = alert.get("id").and_then(|value| value.as_str()).and_then(|value| Uuid::parse_str(value).ok());
                                            styled_card(&p).show(ui, |ui| {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.colored_label(severity_color, severity.to_uppercase());
                                                    ui.label(alert.get("summary").and_then(|value| value.as_str()).unwrap_or("Alerta de equipo"));
                                                });
                                                let occurrences = alert.get("occurrence_count").and_then(|value| value.as_u64()).unwrap_or(1);
                                                let last_seen = alert.get("last_seen_at").and_then(|value| value.as_str()).unwrap_or("fecha desconocida");
                                                ui.label(RichText::new(format!("{occurrences} detecciones · última vez {last_seen}")).small().color(p.text_muted));
                                                if status == "acknowledged" {
                                                    let by = alert.get("acknowledged_by").and_then(|value| value.as_str()).unwrap_or("técnico");
                                                    ui.label(RichText::new(format!("Reconocida por {by}")).small().color(p.text_muted));
                                                } else if status == "open" && can_control_device {
                                                    if let Some(alert_id) = alert_id {
                                                        let pending = self.device_alerts_pending.contains(&alert_id);
                                                        if ui.add_enabled(!pending, egui::Button::new(if pending { "Reconociendo…" } else { "Reconocer" })).clicked() {
                                                            self.acknowledge_device_alert(device.id, alert_id);
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                        let resolved: Vec<_> = alerts.iter()
                                            .filter(|alert| alert.get("status").and_then(|value| value.as_str()) == Some("resolved"))
                                            .collect();
                                        if !resolved.is_empty() {
                                            egui::CollapsingHeader::new(format!("Historial resuelto · {}", resolved.len()))
                                                .id_salt(("resolved_device_alerts", device.id))
                                                .show(ui, |ui| {
                                                    for alert in resolved {
                                                        ui.label(format!("{} · {}", alert.get("summary").and_then(|value| value.as_str()).unwrap_or("Alerta"), alert.get("resolved_at").and_then(|value| value.as_str()).unwrap_or("fecha desconocida")));
                                                    }
                                                });
                                        }
                                    }
                                    if let Some(processes) = data.get("top_processes").and_then(|value| value.as_array()) {
                                        ui.separator();
                                        ui.strong("Procesos con mayor uso de memoria");
                                        for process in processes {
                                            ui.label(format!("{} · {} MB · PID {}",
                                                process.get("name").and_then(|value| value.as_str()).unwrap_or("Proceso"),
                                                process.get("ram_mb").and_then(|value| value.as_f64()).unwrap_or(0.0),
                                                process.get("pid").and_then(|value| value.as_u64()).unwrap_or(0)));
                                        }
                                    }
                                }
                                "services" => {
                                    if let Some(services) = data.get("key_services").and_then(|value| value.as_array()) {
                                        ui.strong("Servicios clave");
                                        for service in services {
                                            let status = service.get("status").and_then(|value| value.as_str()).unwrap_or("desconocido");
                                            ui.horizontal_wrapped(|ui| {
                                                ui.colored_label(if status.eq_ignore_ascii_case("running") { p.ok } else { p.warn }, status);
                                                ui.label(service.get("display").and_then(|value| value.as_str()).unwrap_or("Servicio"));
                                                ui.label(RichText::new(service.get("name").and_then(|value| value.as_str()).unwrap_or("")).small().monospace().color(p.text_muted));
                                            });
                                        }
                                    }
                                    if let Some(stopped) = data.get("stopped_auto_services").and_then(|value| value.as_array()) {
                                        if !stopped.is_empty() {
                                            ui.separator();
                                            ui.colored_label(p.warn, format!("{} servicios automáticos detenidos", stopped.len()));
                                        }
                                    }
                                }
                                "inventory" => {
                                    for (label, key) in [("Sistema operativo", "os_caption"), ("Versión", "os_version"), ("Arquitectura", "arch"), ("Procesador", "cpu")] {
                                        if let Some(value) = data.get(key).and_then(|value| value.as_str()) {
                                            ui.label(format!("{label}: {value}"));
                                        }
                                    }
                                    ui.label(format!("Núcleos: {} · Hilos: {}", data.get("cores").and_then(|value| value.as_u64()).unwrap_or(0), data.get("logical_processors").and_then(|value| value.as_u64()).unwrap_or(0)));
                                    if let Some(ips) = data.get("ipv4").and_then(|value| value.as_array()) {
                                        ui.label(format!("IPv4: {}", ips.iter().filter_map(|value| value.as_str()).collect::<Vec<_>>().join(" · ")));
                                    }
                                    if let Some(disks) = data.get("disks").and_then(|value| value.as_array()) {
                                        ui.separator();
                                        for disk in disks {
                                            ui.label(format!("{} · {:.1} GB total · {:.1} GB libres", disk.get("drive").and_then(|value| value.as_str()).unwrap_or("Disco"), disk.get("total_gb").and_then(|value| value.as_f64()).unwrap_or(0.0), disk.get("free_gb").and_then(|value| value.as_f64()).unwrap_or(0.0)));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                });
            } else {
                self.selected_device_id = None;
                styled_card(&p).show(detail_panel, |ui| {
                    ui.label(
                        RichText::new("Seleccioná un dispositivo para ver sus detalles.")
                            .color(p.text_muted),
                    );
                });
            }
                });
        });
    }

    fn ui_tickets(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Cola de Asistencia Web");
            return;
        }
        let p = self.palette;
        let queued = self
            .tickets
            .iter()
            .filter(|ticket| ticket.status == "queued")
            .count();
        let assigned = self
            .tickets
            .iter()
            .filter(|ticket| ticket.status == "assigned")
            .count();
        ui.horizontal(|ui| {
            ui.heading("Centro de soporte");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("ticket_status_filter")
                    .selected_text(&self.ticket_status_filter)
                    .show_ui(ui, |ui| {
                        for value in ["Abiertos", "En cola", "Asignados", "Cerrados", "Todos"] {
                            ui.selectable_value(
                                &mut self.ticket_status_filter,
                                value.into(),
                                value,
                            );
                        }
                    });
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{queued} en cola")).color(p.warn));
            ui.label(RichText::new(format!("{assigned} asignados")).color(p.accent));
            ui.label(
                RichText::new(format!("{} solicitudes", self.tickets.len()))
                    .small()
                    .color(p.text_muted),
            );
        });
        ui.separator();
        let tks: Vec<SupportTicketDto> = self
            .tickets
            .iter()
            .filter(|ticket| match self.ticket_status_filter.as_str() {
                "Abiertos" => ticket.status == "queued" || ticket.status == "assigned",
                "En cola" => ticket.status == "queued",
                "Asignados" => ticket.status == "assigned",
                "Cerrados" => ticket.status == "closed",
                _ => true,
            })
            .cloned()
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for t in tks {
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&t.requester_name);
                        ui.label(
                            RichText::new(&t.public_code)
                                .monospace()
                                .color(p.text_muted),
                        );
                        let (status_label, status_color) = match t.status.as_str() {
                            "queued" => ("En cola", p.warn),
                            "assigned" => ("Asignado", p.accent),
                            "closed" => ("Cerrado", p.text_muted),
                            _ => (t.status.as_str(), p.text_muted),
                        };
                        ui.colored_label(status_color, status_label);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if t.status == "queued" && ui.button("Reclamar").clicked() {
                                let auth = self.auth.as_ref().unwrap();
                                let http = self.http.clone();
                                let tx = self.ui_tx.clone();
                                let base = self.server_url.trim_end_matches('/').to_string();
                                let token = auth.token.clone();
                                let tid = t.id;
                                self.rt.spawn(async move {
                                    let result = http
                                        .post(format!("{base}/api/v1/tickets/{tid}/claim"))
                                        .bearer_auth(&token)
                                        .send()
                                        .await;
                                    match result {
                                        Ok(response) if response.status().is_success() => {
                                            let _ =
                                                tx.send(UiEvent::Toast("Ticket reclamado".into()));
                                            let _ = tx.send(UiEvent::RefreshRequested);
                                        }
                                        Ok(response) => {
                                            let _ = tx.send(UiEvent::Toast(format!(
                                                "No se pudo reclamar: HTTP {}",
                                                response.status()
                                            )));
                                        }
                                        Err(error) => {
                                            let _ = tx.send(UiEvent::Toast(format!(
                                                "No se pudo reclamar: {error}"
                                            )));
                                        }
                                    }
                                });
                            }
                            if t.status != "closed" && ui.button("Resolver y cerrar").clicked() {
                                self.ticket_close_confirm = Some((t.id, t.public_code.clone()));
                                self.ticket_resolution_outcome = "resolved".into();
                                self.ticket_resolution_summary.clear();
                                self.ticket_resolution_error.clear();
                                self.ticket_comments.remove(&t.id);
                                self.ticket_comments_error.remove(&t.id);
                            }
                            let history_open = self.ticket_comments_open == Some(t.id);
                            if ui
                                .button(if history_open {
                                    "Ocultar historial"
                                } else {
                                    "Ver historial"
                                })
                                .clicked()
                            {
                                if history_open {
                                    self.ticket_comments_open = None;
                                } else {
                                    self.ticket_comments_open = Some(t.id);
                                    if !self.ticket_comments.contains_key(&t.id) {
                                        self.load_ticket_comments(t.id);
                                    }
                                }
                            }
                            if ui.button("Copiloto IA").clicked() {
                                self.ai_ticket_context = Some(t.id);
                                self.ai_resolution_draft = None;
                                self.ai_device_idx = t
                                    .device_id
                                    .and_then(|did| {
                                        self.devices.iter().position(|device| device.id == did)
                                    })
                                    .map(|index| index + 1)
                                    .unwrap_or(0);
                                self.tab = NavTab::AiCopilot;
                            }
                            if let Some(did) = t.device_id {
                                let online = self
                                    .devices
                                    .iter()
                                    .any(|device| device.id == did && device.status == "online");
                                if ui
                                    .add_enabled(online, primary_button(&p, "Iniciar sesión"))
                                    .clicked()
                                {
                                    self.start_session(
                                        did,
                                        Some(t.id),
                                        true,
                                        t.public_code.clone(),
                                    );
                                }
                            }
                        });
                    });
                    ui.label(RichText::new(&t.description).color(p.text_primary));
                    ui.horizontal_wrapped(|ui| {
                        if !t.company_name.is_empty() {
                            ui.label(RichText::new(&t.company_name).color(p.text_muted));
                        }
                        if !t.category.is_empty() {
                            ui.label(RichText::new(&t.category).small().color(p.text_muted));
                        }
                        ui.label(
                            RichText::new(&t.requester_contact)
                                .small()
                                .color(p.text_muted),
                        );
                        ui.label(
                            RichText::new(t.created_at.format("%d/%m/%Y %H:%M").to_string())
                                .small()
                                .color(p.text_muted),
                        );
                    });
                    if self.ticket_comments_open == Some(t.id) {
                        ui.separator();
                        if let Some(comments) = self.ticket_comments.get(&t.id) {
                            if comments.is_empty() {
                                ui.label(
                                    RichText::new(
                                        "Todavía no hay comentarios o resolución registrada.",
                                    )
                                    .small()
                                    .color(p.text_muted),
                                );
                            }
                            for comment in comments {
                                styled_card(&p).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.strong(&comment.author_name);
                                        if comment.is_internal {
                                            ui.colored_label(p.warn, "Nota interna");
                                        }
                                        ui.label(
                                            RichText::new(
                                                comment
                                                    .created_at
                                                    .format("%d/%m/%Y %H:%M")
                                                    .to_string(),
                                            )
                                            .small()
                                            .color(p.text_muted),
                                        );
                                    });
                                    ui.label(&comment.body);
                                });
                            }
                        } else if let Some(error) = self.ticket_comments_error.get(&t.id).cloned() {
                            ui.colored_label(
                                p.danger,
                                format!("No se pudo cargar el historial: {error}"),
                            );
                            if ui.small_button("Reintentar").clicked() {
                                self.load_ticket_comments(t.id);
                            }
                        } else {
                            ui.label(
                                RichText::new("Cargando historial…")
                                    .small()
                                    .color(p.text_muted),
                            );
                        }
                    }
                });
                ui.add_space(6.0);
            }
            if self.tickets.is_empty() {
                styled_card(&p).show(ui, |ui| {
                    ui.label(
                        RichText::new("Todavía no hay solicitudes de soporte.").color(p.text_muted),
                    );
                });
            } else if self
                .tickets
                .iter()
                .all(|ticket| match self.ticket_status_filter.as_str() {
                    "Abiertos" => ticket.status != "queued" && ticket.status != "assigned",
                    "En cola" => ticket.status != "queued",
                    "Asignados" => ticket.status != "assigned",
                    "Cerrados" => ticket.status != "closed",
                    _ => false,
                })
            {
                ui.label(RichText::new("No hay solicitudes en este filtro.").color(p.text_muted));
            }
        });

        if let Some((ticket_id, public_code)) = self.ticket_close_confirm.clone() {
            let mut open = true;
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Cerrar solicitud")
                .id(egui::Id::new("ticket_close_confirm"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("Resolución del caso {public_code}"));
                    egui::ComboBox::from_id_salt("ticket_resolution_outcome")
                        .selected_text(match self.ticket_resolution_outcome.as_str() {
                            "partial" => "Resolución parcial",
                            "unresolved" => "Sin solución",
                            _ => "Resuelto",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.ticket_resolution_outcome,
                                "resolved".into(),
                                "Resuelto",
                            );
                            ui.selectable_value(
                                &mut self.ticket_resolution_outcome,
                                "partial".into(),
                                "Resolución parcial",
                            );
                            ui.selectable_value(
                                &mut self.ticket_resolution_outcome,
                                "unresolved".into(),
                                "Sin solución",
                            );
                        });
                    ui.add(
                        egui::TextEdit::multiline(&mut self.ticket_resolution_summary)
                            .hint_text(theme::placeholder_text(
                                &p,
                                "Qué se diagnosticó, qué se hizo y qué queda pendiente",
                            ))
                            .char_limit(4000)
                            .desired_rows(4)
                            .desired_width(360.0),
                    );
                    if !self.ticket_resolution_error.is_empty() {
                        ui.colored_label(p.danger, &self.ticket_resolution_error);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.ticket_resolution_summary.trim().is_empty(),
                                danger_button(&p, "Guardar resolución y cerrar"),
                            )
                            .clicked()
                        {
                            confirm = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                if self.ticket_resolution_summary.trim().is_empty() {
                    self.ticket_resolution_error =
                        "Escribe un resumen antes de cerrar el caso.".into();
                } else if let Some(auth) = &self.auth {
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let outcome = self.ticket_resolution_outcome.clone();
                    let resolution_summary = self.ticket_resolution_summary.trim().to_string();
                    self.rt.spawn(async move {
                        let result = http
                            .post(format!("{base}/api/v1/tickets/{ticket_id}/resolve"))
                            .bearer_auth(&token)
                            .json(&serde_json::json!({
                                "outcome": outcome,
                                "resolution_summary": resolution_summary,
                            }))
                            .send()
                            .await;
                        let result = match result {
                            Ok(response) if response.status().is_success() => Ok(()),
                            Ok(response) => {
                                Err(format!("No se pudo cerrar: HTTP {}", response.status()))
                            }
                            Err(error) => Err(format!("No se pudo cerrar: {error}")),
                        };
                        let _ = tx.send(UiEvent::TicketResolved(ticket_id, result));
                    });
                }
            } else if cancel || !open {
                self.ticket_close_confirm = None;
            }
        }
    }

    fn ui_viewer(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let mut should_terminate = false;
        let mut send_quality = false;
        let mut send_chat: Option<String> = None;
        let mut actions: Vec<ControlMessage> = Vec::new();
        let mut arm_reboot: Option<ReconnectTarget> = None;

        if let Some(v) = &mut self.viewer {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(p.ok, "🔒 E2EE Noise_XX");
                ui.strong(&v.label);
                ui.label(
                    RichText::new(format!(
                        "FPS: {:.1} · RTT: {} ms · {:.1} KB/s",
                        v.fps, v.rtt_ms, v.kbps
                    ))
                    .small()
                    .color(p.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(danger_button(&p, "Terminar sesión")).clicked() {
                        should_terminate = true;
                    }
                });
            });

            ui.horizontal_wrapped(|ui| {
                if !v.monitors.is_empty() {
                    let cur_label = v
                        .monitors
                        .get(v.active_monitor)
                        .map(|m| format!("Mon {} ({}x{})", m.index + 1, m.width, m.height))
                        .unwrap_or_else(|| "Monitor 1".into());
                    egui::ComboBox::from_id_salt("mon_pick")
                        .selected_text(cur_label)
                        .show_ui(ui, |ui| {
                            let mons = v.monitors.clone();
                            for m in mons {
                                let txt = format!("Mon {} ({}x{})", m.index + 1, m.width, m.height);
                                if ui
                                    .selectable_value(&mut v.active_monitor, m.index, txt)
                                    .clicked()
                                {
                                    let _ = v.out_tx.send(DataPacket::Control(
                                        ControlMessage::SelectMonitor { index: m.index },
                                    ));
                                }
                            }
                        });
                }
                ui.label(RichText::new("Calidad").small().color(p.text_muted));
                if ui
                    .add_sized(
                        [150.0, 22.0],
                        egui::Slider::new(&mut v.jpeg_quality, 30..=95).show_value(true),
                    )
                    .changed()
                {
                    send_quality = true;
                }
                ui.menu_button("Herramientas", |ui| {
                    if v.allow_control {
                        ui.label(RichText::new("Control remoto").small().color(p.text_muted));
                        for combo in ["Win", "Alt+Tab", "Ctrl+Shift+Esc"] {
                            if ui.button(combo).clicked() {
                                let _ = v.out_tx.send(DataPacket::Control(
                                    ControlMessage::SpecialKey {
                                        combo: combo.into(),
                                    },
                                ));
                                ui.close_menu();
                            }
                        }
                        if ui.button("Enviar portapapeles").clicked() {
                            if let Some(clip) = read_clipboard_text() {
                                let _ = v.out_tx.send(DataPacket::Control(
                                    ControlMessage::ClipboardText { text: clip },
                                ));
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Reiniciar equipo…").clicked() {
                            v.confirm_reboot_open = true;
                            ui.close_menu();
                        }
                        if ui.button("Bloquear equipo").clicked() {
                            actions.push(ControlMessage::SessionAction {
                                action: "lock".into(),
                                target_user: None,
                                delay_secs: None,
                            });
                            ui.close_menu();
                        }
                        if ui.button("Usuario de Windows").clicked() {
                            v.show_user_switch_popup = !v.show_user_switch_popup;
                            if v.show_user_switch_popup {
                                actions.push(ControlMessage::RequestHardwareResources);
                            }
                            ui.close_menu();
                        }
                    }
                    let res_label = if v.show_resources_panel {
                        "Ocultar recursos del equipo"
                    } else {
                        "Ver recursos del equipo"
                    };
                    if ui.button(res_label).clicked() {
                        v.show_resources_panel = !v.show_resources_panel;
                        if v.show_resources_panel {
                            actions.push(ControlMessage::RequestHardwareResources);
                        }
                        ui.close_menu();
                    }
                });
            });
            ui.separator();

            if v.confirm_reboot_open {
                let mut open = true;
                let mut go = false;
                let mut cancel = false;
                egui::Window::new("Reiniciar equipo remoto")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .open(&mut open)
                    .show(ui.ctx(), |ui| {
                        ui.label(format!(
                            "¿Reiniciar {} y reconectar automáticamente cuando vuelva a iniciar?",
                            v.label
                        ));
                        ui.horizontal(|ui| {
                            if ui
                                .add(danger_button(&p, "Reiniciar y auto-reconectar"))
                                .clicked()
                            {
                                go = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if go {
                    actions.push(ControlMessage::SessionAction {
                        action: "reboot".into(),
                        target_user: None,
                        delay_secs: Some(5),
                    });
                    arm_reboot = v.reconnect_target.clone();
                    v.confirm_reboot_open = false;
                }
                if cancel || !open {
                    v.confirm_reboot_open = false;
                }
            }

            if v.show_user_switch_popup {
                let mut open = true;
                let hw = v.hw_resources.clone();
                egui::Window::new("Usuarios de Windows del equipo remoto")
                    .collapsible(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .open(&mut open)
                    .show(ui.ctx(), |ui| {
                        match &hw {
                            Some(hw) => {
                                if let Some(users) =
                                    hw.get("windows_users").and_then(|u| u.as_array())
                                {
                                    ui.strong("Cuentas locales:");
                                    for u in users {
                                        ui.label(format!(
                                            "• {}",
                                            u.get("name").and_then(|x| x.as_str()).unwrap_or("?")
                                        ));
                                    }
                                }
                                if let Some(ss) =
                                    hw.get("windows_sessions").and_then(|u| u.as_array())
                                {
                                    ui.strong("Sesiones activas:");
                                    for s in ss {
                                        let id = s
                                            .get("id")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        ui.horizontal(|ui| {
                                            ui.label(format!(
                                                "#{} {} ({})",
                                                id,
                                                s.get("username")
                                                    .and_then(|x| x.as_str())
                                                    .unwrap_or("?"),
                                                s.get("state")
                                                    .and_then(|x| x.as_str())
                                                    .unwrap_or("?")
                                            ));
                                            if ui.small_button("Cerrar esa sesión").clicked() {
                                                actions.push(ControlMessage::SessionAction {
                                                    action: "signout".into(),
                                                    target_user: Some(id.clone()),
                                                    delay_secs: None,
                                                });
                                            }
                                        });
                                    }
                                }
                            }
                            None => {
                                ui.label("Consultando usuarios...");
                            }
                        }
                        ui.separator();
                        if ui.button("↻ Refrescar").clicked() {
                            actions.push(ControlMessage::RequestHardwareResources);
                        }
                        if ui
                            .add(danger_button(
                                &p,
                                "Cerrar sesión actual / cambiar de usuario",
                            ))
                            .clicked()
                        {
                            actions.push(ControlMessage::SessionAction {
                                action: "switch_user".into(),
                                target_user: None,
                                delay_secs: None,
                            });
                        }
                        ui.label(
                            RichText::new(
                                "Luego elegí el usuario en la pantalla de inicio de sesión remota.",
                            )
                            .small()
                            .color(p.text_muted),
                        );
                    });
                if !open {
                    v.show_user_switch_popup = false;
                }
            }

            if send_quality {
                let _ = v
                    .out_tx
                    .send(DataPacket::Control(ControlMessage::SetQuality {
                        jpeg_quality: v.jpeg_quality,
                        max_fps: v.max_fps,
                    }));
            }

            ui.columns(2, |cols| {
                let avail = cols[0].available_size();
                if let Some(tex) = &v.texture {
                    let tex_size = tex.size_vec2();
                    let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                    let draw_size = tex_size * scale;
                    let (rect, resp) =
                        cols[0].allocate_exact_size(draw_size, egui::Sense::click_and_drag());
                    cols[0].painter().image(
                        tex.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );

                    if v.allow_control && resp.hovered() {
                        if let Some(pos) = cols[0].input(|i| i.pointer.hover_pos()) {
                            let nx = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                            let ny = ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
                            if (nx - v.last_mouse_norm.0).abs() > 0.002
                                || (ny - v.last_mouse_norm.1).abs() > 0.002
                            {
                                v.last_mouse_norm = (nx, ny);
                                let _ =
                                    v.out_tx.send(DataPacket::Input(InputEvent::MouseMoveNorm {
                                        x: nx,
                                        y: ny,
                                    }));
                            }
                        }
                        cols[0].input(|i| {
                            if i.pointer.button_pressed(egui::PointerButton::Primary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton {
                                    button: MouseBtn::Left,
                                    pressed: true,
                                }));
                            }
                            if i.pointer.button_released(egui::PointerButton::Primary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton {
                                    button: MouseBtn::Left,
                                    pressed: false,
                                }));
                            }
                            if i.pointer.button_pressed(egui::PointerButton::Secondary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton {
                                    button: MouseBtn::Right,
                                    pressed: true,
                                }));
                            }
                            if i.pointer.button_released(egui::PointerButton::Secondary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton {
                                    button: MouseBtn::Right,
                                    pressed: false,
                                }));
                            }
                            let wheel_y = i.raw_scroll_delta.y;
                            if wheel_y.abs() > 0.1 {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseWheel {
                                    delta_y: (wheel_y as i32) * 3,
                                }));
                            }
                            for ev in &i.events {
                                match ev {
                                    egui::Event::Text(txt) => {
                                        for ch in txt.chars() {
                                            if !ch.is_control() {
                                                let _ = v.out_tx.send(DataPacket::Input(
                                                    InputEvent::UnicodeChar { ch },
                                                ));
                                            }
                                        }
                                    }
                                    egui::Event::Key {
                                        key,
                                        pressed,
                                        repeat,
                                        ..
                                    } if !repeat => {
                                        if let Some(vk) = egui_key_to_vk(*key) {
                                            // Printable alphanumeric/space already handled by Event::Text on press
                                            if vk < 0x30 || !*pressed {
                                                let _ = v.out_tx.send(DataPacket::Input(
                                                    InputEvent::Key {
                                                        vk,
                                                        pressed: *pressed,
                                                    },
                                                ));
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        });
                    }
                } else {
                    cols[0].centered_and_justified(|ui| {
                        ui.label(
                            "Esperando handshake Noise_XX y primer frame del equipo destino...",
                        );
                    });
                }

                cols[1].heading("Chat de sesión");
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(&mut cols[1], |ui| {
                        for line in &v.chat_log {
                            ui.label(line);
                        }
                    });
                cols[1].horizontal(|ui| {
                    ui.add(theme::centered_text_edit(&mut v.chat_input));
                    if ui.button("Enviar").clicked() && !v.chat_input.trim().is_empty() {
                        send_chat = Some(v.chat_input.clone());
                        v.chat_log.push(format!("Yo: {}", v.chat_input));
                        v.chat_input.clear();
                    }
                });

                cols[1].separator();
                cols[1].heading("Transferencia de archivos");
                cols[1].label(
                    RichText::new("El archivo se cifra durante la sesión y se valida con SHA-256.")
                        .small()
                        .color(p.text_muted),
                );
                cols[1].add(theme::centered_text_edit(&mut v.file_path_input));
                let send_enabled = v.allow_control
                    && v.active_file_transfer.is_none()
                    && !v.file_path_input.trim().is_empty();
                if cols[1]
                    .add_enabled(send_enabled, primary_button(&p, "Enviar archivo"))
                    .clicked()
                {
                    let path = PathBuf::from(v.file_path_input.trim().trim_matches('"'));
                    let transfer_id = Uuid::new_v4();
                    let session_id = v.session_id;
                    let out_tx = v.out_tx.clone();
                    let ui_tx = self.ui_tx.clone();
                    v.active_file_transfer = Some(transfer_id);
                    v.file_status = "Preparando archivo…".into();
                    std::thread::spawn(move || {
                        let abort_tx = out_tx.clone();
                        if let Err(error) = transfer_file_over_session(
                            path,
                            out_tx,
                            ui_tx.clone(),
                            session_id,
                            transfer_id,
                        ) {
                            let _ = abort_tx.send(DataPacket::Control(ControlMessage::FileChunk {
                                transfer_id,
                                offset: 0,
                                data: Vec::new(),
                                eof: true,
                            }));
                            let _ = ui_tx.send(UiEvent::FileTransferFailed(
                                session_id,
                                transfer_id,
                                error,
                            ));
                        }
                    });
                }
                if !v.file_status.is_empty() {
                    cols[1].label(&v.file_status);
                }

                if v.show_resources_panel {
                    cols[1].separator();
                    cols[1].horizontal(|ui| {
                        ui.heading("Recursos del PC remoto");
                        if theme::refresh_button(ui, &p, "Actualizar recursos").clicked() {
                            actions.push(ControlMessage::RequestHardwareResources);
                        }
                    });
                    match &v.hw_resources {
                        Some(hw) => {
                            egui::ScrollArea::vertical()
                                .id_salt("remote_hw_scroll")
                                .max_height(380.0)
                                .show(&mut cols[1], |ui| render_hardware_resources_ui(ui, &p, hw));
                        }
                        None => {
                            cols[1].label("Consultando CPU / RAM / Disco / GPU...");
                        }
                    }
                }
            });

            for a in actions.drain(..) {
                let _ = v.out_tx.send(DataPacket::Control(a));
            }
            if let Some(txt) = send_chat {
                let _ = v.out_tx.send(DataPacket::Control(ControlMessage::Chat {
                    sender: "Cliente AgentDesk".into(),
                    text: txt,
                }));
            }
        }

        if let Some(t) = arm_reboot {
            self.auto_reconnect = Some((t, Instant::now() + Duration::from_secs(25), 0));
            self.status_msg =
                "Reinicio solicitado: se reconectará automáticamente cuando el equipo vuelva"
                    .into();
        }
        if should_terminate {
            self.terminate_active_session();
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.palette;
        ui.heading("Configuración de AgentDesk Desktop");
        ui.separator();

        styled_card(&p).show(ui, |ui| {
            ui.strong("Conexión al servidor AgentDesk");
            ui.label(
                RichText::new(
                    "URL base del servidor (ej. http://10.1.1.156:8080 o http://127.0.0.1:8080):",
                )
                .small()
                .color(p.text_muted),
            );
            ui.add(
                theme::centered_text_edit(&mut self.settings_server_url)
                    .hint_text(theme::placeholder_text(&p, "http://10.1.1.156:8080"))
                    .desired_width(480.0),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Probar conexión").clicked() {
                    let url = self.settings_server_url.clone();
                    self.test_connection(&url);
                }
                if ui.add(primary_button(&p, "Guardar y reconectar")).clicked() {
                    let next_url = normalize_server_url(&self.settings_server_url);
                    let changed = next_url != self.server_url;
                    self.server_url = next_url.clone();
                    self.settings_server_url = next_url;
                    self.save_config(false);
                    if changed {
                        self.logout();
                        self.start_embedded_host(ctx.clone());
                        self.status_msg = format!("Servidor actualizado a {}", self.server_url);
                    }
                }
                if ui.button("Localhost").clicked() {
                    self.settings_server_url = "http://127.0.0.1:8080".into();
                }
                if let Some(lan) = detect_lan_ip() {
                    let lan_url = format!("http://{lan}:8080");
                    if ui.button(format!("Usar LAN ({lan})")).clicked() {
                        self.settings_server_url = lan_url;
                    }
                }
            });
            if let Some((ok, detail)) = &self.conn_test_status {
                ui.colored_label(if *ok { p.ok } else { p.danger }, detail);
            }
            ui.add_space(4.0);
            ui.small(format!("Archivo: {}", self.config_path.display()));
        });

        if self
            .auth
            .as_ref()
            .is_some_and(|auth| matches!(auth.user.role.as_str(), "instance_admin" | "org_admin"))
        {
            ui.add_space(10.0);
            styled_card(&p).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Alertas de salud RMM");
                    if theme::refresh_button(ui, &p, "Recargar umbrales").clicked() {
                        self.load_rmm_alert_settings();
                    }
                });
                ui.label(
                    RichText::new("Umbrales para esta organización. Los cambios aplican en la próxima muestra.")
                        .small()
                        .color(p.text_muted),
                );
                let fields = [
                    "CPU aviso %",
                    "CPU crítico %",
                    "RAM aviso %",
                    "RAM crítico %",
                    "Disco libre aviso %",
                    "Disco libre crítico %",
                ];
                ui.horizontal_wrapped(|ui| {
                    for (index, label) in fields.iter().enumerate() {
                        ui.label(*label);
                        ui.add(
                            theme::centered_text_edit(&mut self.rmm_alert_threshold_inputs[index])
                                .desired_width(58.0),
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.rmm_alert_settings_saving,
                            primary_button(
                                &p,
                                if self.rmm_alert_settings_saving { "Guardando…" } else { "Guardar umbrales" },
                            ),
                        )
                        .clicked()
                    {
                        self.save_rmm_alert_settings();
                    }
                    if !self.rmm_alert_settings_status.is_empty() {
                        let is_error = self.rmm_alert_settings_status.starts_with("No se")
                            || self.rmm_alert_settings_status.starts_with("Ingresá");
                        ui.label(RichText::new(&self.rmm_alert_settings_status).small().color(if is_error { p.danger } else { p.text_muted }));
                    }
                });
            });
        }

        ui.add_space(10.0);
        styled_card(&p).show(ui, |ui| {
            ui.strong("Apariencia");
            ui.horizontal(|ui| {
                ui.label(format!("Tema {}", self.theme_mode.label().to_lowercase()));
                let mut selected_theme = self.theme_mode;
                egui::ComboBox::from_id_salt("desktop_theme_mode")
                    .selected_text(selected_theme.label())
                    .show_ui(ui, |ui| {
                        for mode in [
                            ThemeMode::System,
                            ThemeMode::OpenVmsLight,
                            ThemeMode::DraculaDark,
                        ] {
                            ui.selectable_value(&mut selected_theme, mode, mode.label());
                        }
                    });
                if selected_theme != self.theme_mode {
                    self.set_theme(ctx, selected_theme);
                }
                if theme::theme_button(ui, &p, self.palette.mode).clicked() {
                    self.set_theme(ctx, self.palette.mode.toggle());
                }
            });
        });

        ui.add_space(10.0);
        if self.auth.is_some() {
            styled_card(&p).show(ui, |ui| {
                ui.strong("Sesión de cuenta técnica");
                if let Some(auth) = self.auth.clone() {
                    ui.label(format!(
                        "Usuario: {} ({})",
                        auth.user.display_name, auth.user.email
                    ));
                    ui.label(format!(
                        "Organización: {} · Rol: {}",
                        auth.user.organization_name, auth.user.role
                    ));
                    if theme::session_action_button(ui, false).clicked() {
                        self.logout();
                    }
                }
            });
        } else {
            self.ui_account_login_card(ui, "Cuenta Técnica");
        }
    }
}

#[cfg(windows)]
fn show_error_box(title: &str, body: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let b: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(b.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn desktop_log_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("desktop.log")
}

fn telemetry_percent_series(records: &[serde_json::Value], metric: &str) -> Vec<f64> {
    let mut recent: Vec<_> = records
        .iter()
        .filter(|record| record.get("kind").and_then(|value| value.as_str()) == Some("metrics"))
        .take(12)
        .collect();
    recent.reverse();

    recent
        .into_iter()
        .filter_map(|record| {
            let payload = record.get("payload")?;
            let data = payload.get("data").unwrap_or(payload);
            match metric {
                "ram_used_pct" => {
                    let used = data.get("ram_used_mb")?.as_f64()?;
                    let total = data.get("ram_total_mb")?.as_f64()?;
                    (total > 0.0).then_some((used * 100.0 / total).clamp(0.0, 100.0))
                }
                "disk_used_pct" => data
                    .get("c_drive_free_pct")
                    .and_then(|value| value.as_f64())
                    .map(|free| (100.0 - free).clamp(0.0, 100.0)),
                _ => data
                    .get(metric)
                    .and_then(|value| value.as_f64())
                    .map(|value| value.clamp(0.0, 100.0)),
            }
        })
        .collect()
}

fn init_desktop_logging() {
    let path = desktop_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("agentdesk_desktop=info,eframe=info")),
            )
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt().with_ansi(false).try_init();
    }
}

#[cfg(windows)]
fn focus_existing_desktop_window() {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let title: Vec<u16> = "AgentDesk — Cliente Remoto"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn acquire_desktop_instance_guard() -> Result<Option<windows::Win32::Foundation::HANDLE>, String> {
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
        focus_existing_desktop_window();
        Ok(None)
    } else {
        Ok(Some(handle))
    }
}

fn main() {
    init_desktop_logging();

    std::panic::set_hook(Box::new(|info| {
        let msg = format!("Error inesperado en AgentDesk Desktop: {info}");
        tracing::error!("{msg}");
        #[cfg(windows)]
        show_error_box("Error crítico en AgentDesk Desktop", &msg);
    }));

    #[cfg(windows)]
    let _instance_guard = match acquire_desktop_instance_guard() {
        Ok(Some(handle)) => handle,
        Ok(None) => return,
        Err(error) => {
            tracing::error!("{error}");
            show_error_box("AgentDesk ya está en ejecución", &error);
            return;
        }
    };

    let deeplink = std::env::args().nth(1);
    let icon = egui::IconData {
        rgba: build_desktop_icon_rgba(),
        width: 32,
        height: 32,
    };

    let make_opts = |renderer: eframe::Renderer| eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_title("AgentDesk — Cliente Remoto")
            .with_icon(icon.clone())
            .with_active(true),
        ..Default::default()
    };

    let dl_glow = deeplink.clone();
    match eframe::run_native(
        "AgentDesk",
        make_opts(eframe::Renderer::Glow),
        Box::new(move |cc| Ok(Box::new(AgentDeskApp::new(cc, dl_glow)))),
    ) {
        Ok(()) => {}
        Err(glow_err) => {
            tracing::warn!(error = %glow_err, "Glow renderer failed, falling back to Wgpu");
            let dl_wgpu = deeplink;
            if let Err(wgpu_err) = eframe::run_native(
                "AgentDesk",
                make_opts(eframe::Renderer::Wgpu),
                Box::new(move |cc| Ok(Box::new(AgentDeskApp::new(cc, dl_wgpu)))),
            ) {
                let msg = format!(
                    "No se pudo iniciar la interfaz gráfica de AgentDesk Desktop.\n\nGlow: {glow_err}\nWgpu: {wgpu_err}\n\nLog: {}",
                    desktop_log_path().display()
                );
                tracing::error!("{msg}");
                #[cfg(windows)]
                show_error_box("Error iniciando AgentDesk Desktop", &msg);
            }
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fmt_elapsed(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m < 1 {
        "hace instantes".to_string()
    } else if m < 60 {
        format!("hace {m} min")
    } else {
        format!("hace {} h {} min", m / 60, m % 60)
    }
}

fn load_brand_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let decoded = image::load_from_memory(include_bytes!("../../../logos/agentdesk-icono.png"))
        .expect("logo AgentDesk válido")
        .to_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
    ctx.load_texture("agentdesk-brand-icon", pixels, egui::TextureOptions::LINEAR)
}

fn build_desktop_icon_rgba() -> Vec<u8> {
    image::load_from_memory(include_bytes!("../../../logos/agentdesk-icono.png"))
        .expect("logo AgentDesk válido")
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8()
        .into_raw()
}

#[cfg(windows)]
fn hide_native_window(window_handle: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    unsafe {
        let _ = ShowWindow(HWND(window_handle as *mut _), SW_HIDE);
    }
}

#[cfg(windows)]
fn spawn_desktop_tray(
    tx: Sender<UiEvent>,
    ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ctx: egui::Context,
    window_handle: isize,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, MSG,
        SW_RESTORE,
    };

    std::thread::spawn(move || {
        let menu = Menu::new();
        let item_open = MenuItem::new("Abrir AgentDesk", true, None);
        let item_copy = MenuItem::new("Copiar mi ID de conexión", true, None);
        let item_quit = MenuItem::new("Salir completamente", true, None);
        let _ = menu.append(&item_open);
        let _ = menu.append(&item_copy);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_quit);

        let id_open = item_open.id().clone();
        let id_copy = item_copy.id().clone();
        let id_quit = item_quit.id().clone();

        let icon = match tray_icon::Icon::from_rgba(build_desktop_icon_rgba(), 32, 32) {
            Ok(i) => i,
            Err(error) => {
                let _ = tx.send(UiEvent::TrayInitError(format!(
                    "No se pudo crear el icono: {error}"
                )));
                ctx.request_repaint();
                return;
            }
        };
        let _tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("AgentDesk — Cliente remoto")
            .with_icon(icon)
            .build()
        {
            Ok(t) => t,
            Err(error) => {
                let _ = tx.send(UiEvent::TrayInitError(format!(
                    "No se pudo registrar el icono en Windows: {error}"
                )));
                ctx.request_repaint();
                return;
            }
        };
        ok.store(true, std::sync::atomic::Ordering::Relaxed);

        let menu_tx = tx.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == id_open {
                Some(TrayAction::ShowWindow)
            } else if event.id == id_copy {
                Some(TrayAction::CopyId)
            } else if event.id == id_quit {
                Some(TrayAction::QuitApp)
            } else {
                None
            };
            if let Some(action) = action {
                unsafe {
                    let hwnd = HWND(window_handle as *mut _);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                if menu_tx.send(UiEvent::Tray(action)).is_ok() {
                    menu_ctx.request_repaint();
                }
            }
        }));

        let tray_ctx = ctx;
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
                unsafe {
                    let hwnd = HWND(window_handle as *mut _);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                if tx.send(UiEvent::Tray(TrayAction::ShowWindow)).is_ok() {
                    tray_ctx.request_repaint();
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
        ok.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

impl AgentDeskApp {
    fn reconnect_from_history(&mut self, idx: usize) {
        let Some(h) = self.connection_history.get(idx).cloned() else {
            return;
        };
        if h.is_trust_valid(now_unix()) {
            self.connect_direct(h.connection_id, String::new(), h.reconnect_token, true);
        } else {
            self.partner_id_input = h.connection_id;
            self.partner_password_input.clear();
            self.tab = NavTab::Connect;
            self.connect_feedback =
                "Pasó más de 1 hora desde la última conexión. Ingresá la contraseña para reconectar."
                    .into();
        }
    }

    fn set_personal_password(&mut self, set: bool) {
        let (Some(did), Some(tok)) = (self.local_device_id, self.local_device_token.clone()) else {
            self.status_msg = "El equipo todavía no está registrado en el servidor".into();
            return;
        };
        let pw = self.personal_pw_input.trim().to_string();
        if set && pw.len() < 4 {
            self.status_msg = "La contraseña fija debe tener al menos 4 caracteres".into();
            return;
        }
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/clients/personal-password"))
                .header("x-device-token", tok)
                .json(&SetPersonalPasswordRequest {
                    device_id: did,
                    personal_password: if set { Some(pw) } else { None },
                })
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let msg = if set {
                        "Contraseña personal fija establecida"
                    } else {
                        "Contraseña personal fija eliminada"
                    };
                    let _ = tx.send(UiEvent::PersonalPasswordUpdated(set, msg.into()));
                }
                Ok(r) => {
                    let _ = tx.send(UiEvent::Toast(format!(
                        "Error al fijar contraseña: {}",
                        r.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Toast(format!("Error de red: {e}")));
                }
            }
        });
        self.personal_pw_input.clear();
    }

    fn submit_change_password(&mut self) {
        let Some(auth) = &self.auth else {
            self.change_pw_msg = "Iniciá sesión primero".into();
            return;
        };
        if self.new_pw_input.trim().len() < 6 {
            self.change_pw_msg = "La nueva contraseña debe tener al menos 6 caracteres".into();
            return;
        }
        if self.new_pw_input != self.new_pw_confirm {
            self.change_pw_msg = "Las contraseñas nuevas no coinciden".into();
            return;
        }
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        let cur = self.cur_pw_input.clone();
        let new = self.new_pw_input.clone();
        self.change_pw_msg = "Guardando...".into();
        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/auth/change-password"))
                .bearer_auth(&token)
                .json(&serde_json::json!({ "current_password": cur, "new_password": new }))
                .send()
                .await;
            let out = match res {
                Ok(r) if r.status().is_success() => Ok(()),
                Ok(r) => {
                    let txt = r.text().await.unwrap_or_default();
                    Err(serde_json::from_str::<serde_json::Value>(&txt)
                        .ok()
                        .and_then(|v| {
                            v.get("error")
                                .and_then(|e| e.as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or(txt))
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(UiEvent::PasswordChanged(out));
        });
    }

    fn frame_logic(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && self.tray_ok.load(std::sync::atomic::Ordering::Relaxed)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            #[cfg(windows)]
            if let Some(window_handle) = self.tray_window_handle {
                hide_native_window(window_handle);
            }
            #[cfg(not(windows))]
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if self.auth.is_some() {
            self.login_modal_open = false;
        }
        if let Some((target, last, n)) = self.auto_reconnect.clone() {
            if self.viewer.is_some() {
                self.auto_reconnect = None;
                self.status_msg = "Auto-reconexión completada".into();
            } else {
                if last.elapsed() >= Duration::from_secs(4) {
                    if n >= 150 {
                        self.auto_reconnect = None;
                        self.status_msg =
                            "Auto-reconexión cancelada: el equipo no volvió en 10 minutos".into();
                    } else {
                        self.auto_reconnect = Some((target.clone(), Instant::now(), n + 1));
                        self.connect_direct(
                            target.connection_id.clone(),
                            String::new(),
                            target.reconnect_token.clone(),
                            target.allow_control,
                        );
                    }
                }
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
    }

    fn ui_search_results(&mut self, ctx: &egui::Context, submit: bool) {
        let search_id = egui::Id::new("global_search");
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.global_search.clear();
        }

        let query = self.global_search.trim().to_lowercase();
        let Some(rect) = self.search_rect.filter(|_| !query.is_empty()) else {
            return;
        };
        let compact = query.replace(' ', "");
        let mut results = Vec::new();
        if compact.len() >= 6 && compact.bytes().all(|byte| byte.is_ascii_digit()) {
            results.push((
                format!("Usar ID {compact}"),
                SearchAct::FillId(compact.clone()),
                None,
            ));
        }
        results.extend(
            self.connection_history
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    (entry.hostname.to_lowercase().contains(&query)
                        || entry.connection_id.replace(' ', "").contains(&compact))
                    .then(|| {
                        (
                            format!("{} ({}) · reciente", entry.hostname, entry.connection_id),
                            SearchAct::History(index),
                            None,
                        )
                    })
                })
                .take(10 - results.len()),
        );
        results.extend(
            self.devices
                .iter()
                .filter_map(|device| {
                    (device.hostname.to_lowercase().contains(&query)
                        || device.public_code.replace(' ', "").contains(&compact))
                    .then(|| {
                        (
                            format!("{} ({})", device.hostname, device.public_code),
                            SearchAct::Device(device.id, device.hostname.clone()),
                            Some(if device.status == "online" {
                                self.palette.ok
                            } else {
                                self.palette.text_muted
                            }),
                        )
                    })
                })
                .take(10 - results.len()),
        );
        results.extend(
            self.agenda
                .iter()
                .filter_map(|entry| {
                    let id = entry.device_id?;
                    (entry.alias.to_lowercase().contains(&query)
                        || entry.client_company.to_lowercase().contains(&query)
                        || entry.contact_name.to_lowercase().contains(&query))
                    .then(|| {
                        (
                            format!("{} · {}", entry.alias, entry.group_name),
                            SearchAct::Device(id, entry.alias.clone()),
                            None,
                        )
                    })
                })
                .take(10 - results.len()),
        );
        results.extend(
            self.groups
                .iter()
                .filter_map(|group| {
                    group
                        .name
                        .to_lowercase()
                        .contains(&query)
                        .then(|| (format!("Grupo · {}", group.name), SearchAct::Group, None))
                })
                .take(10 - results.len()),
        );

        let p = self.palette;
        let mut action = None;
        egui::Area::new(egui::Id::new("search_results"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(p.card_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                    .show(ui, |ui| {
                        ui.set_width(rect.width());
                        if results.is_empty() {
                            ui.label(RichText::new("Sin resultados").small().color(p.text_muted));
                        }
                        for (index, (label, result, status_color)) in
                            results.into_iter().enumerate()
                        {
                            ui.horizontal(|ui| {
                                if let Some(color) = status_color {
                                    let (dot, _) = ui.allocate_exact_size(
                                        egui::vec2(12.0, 30.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dot.center(), 3.5, color);
                                }
                                let button = egui::Button::new(label)
                                    .min_size(egui::vec2(ui.available_width(), 30.0))
                                    .truncate();
                                if ui.add(button).clicked() || (submit && index == 0) {
                                    action = Some(result);
                                }
                            });
                        }
                    });
            });

        if let Some(action) = action {
            self.global_search.clear();
            ctx.memory_mut(|memory| memory.surrender_focus(search_id));
            match action {
                SearchAct::History(index) => self.reconnect_from_history(index),
                SearchAct::Device(id, label) => {
                    if self.auth.is_some() {
                        self.start_session(id, None, true, label);
                    } else {
                        self.status_msg =
                            "Iniciá sesión para conectarte a equipos de tu cuenta".into();
                        self.login_modal_open = true;
                    }
                }
                SearchAct::Group => self.tab = NavTab::Groups,
                SearchAct::FillId(id) => {
                    self.partner_id_input = id;
                    self.partner_password_input.clear();
                    self.tab = NavTab::Connect;
                }
            }
            ctx.request_repaint();
        }
    }

    fn ui_overlays(&mut self, ctx: &egui::Context) {
        let p = self.palette;

        // Banner de auto-reconexión activa
        let banner = self
            .auto_reconnect
            .as_ref()
            .map(|(t, _, n)| (t.connection_id.clone(), *n));
        if let Some((id, att)) = banner {
            let mut cancel = false;
            egui::TopBottomPanel::top("reconnect_banner")
                .frame(
                    egui::Frame::NONE
                        .fill(p.badge_bg)
                        .inner_margin(egui::Margin::symmetric(14, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "🔄 Auto-reconexión activa a {id} (intento {att}) — reintenta cada 4 s"
                            ))
                            .color(p.warn),
                        );
                        if ui.button("Cancelar auto-reconexión").clicked() {
                            cancel = true;
                        }
                    });
                });
            if cancel {
                self.auto_reconnect = None;
            }
        }

        // Pregunta tras desconexión / reinicio remoto
        if let Some((reason, target)) = self.reconnect_prompt.clone() {
            let mut yes = false;
            let mut no = false;
            egui::Window::new("Conexión interrumpida")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Se detectó una desconexión o reinicio de {} ({reason}).",
                        target.hostname
                    ));
                    ui.label("¿Querés que AgentDesk se reconecte automáticamente cuando el equipo vuelva a iniciar?");
                    ui.horizontal(|ui| {
                        if ui.add(primary_button(&p, "Sí, auto-reconectar")).clicked() {
                            yes = true;
                        }
                        if ui.button("No").clicked() {
                            no = true;
                        }
                    });
                });
            if yes {
                self.auto_reconnect = Some((target, Instant::now() + Duration::from_secs(10), 0));
                self.reconnect_prompt = None;
                self.tab = NavTab::Connect;
            }
            if no {
                self.reconnect_prompt = None;
            }
        }

        // Menú desplegable de usuario (estilo OpenVMS)
        if self.user_menu_open {
            if let Some(rect) = self.user_menu_rect {
                let mut do_login = false;
                let mut do_pw = false;
                let mut do_settings = false;
                let mut do_logout = false;
                let auth = self.auth.clone();
                let area = egui::Area::new(egui::Id::new("user_menu"))
                    .order(egui::Order::Foreground)
                    .pivot(egui::Align2::RIGHT_TOP)
                    .fixed_pos(rect.right_bottom() + egui::vec2(0.0, 4.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .fill(p.card_bg)
                            .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                            .show(ui, |ui| {
                                ui.set_min_width(250.0);
                                match &auth {
                                    Some(a) => {
                                        ui.label(
                                            RichText::new(&a.user.display_name)
                                                .strong()
                                                .color(p.text_primary),
                                        );
                                        ui.label(
                                            RichText::new(&a.user.email)
                                                .small()
                                                .color(p.text_muted),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}",
                                                a.user.organization_name, a.user.role
                                            ))
                                            .small()
                                            .color(p.text_muted),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("Sin sesión de cuenta")
                                                .color(p.text_muted),
                                        );
                                        if theme::session_action_button(ui, true).clicked() {
                                            do_login = true;
                                        }
                                    }
                                }
                                ui.separator();
                                if auth.is_some() && ui.button("🔑 Cambiar contraseña").clicked()
                                {
                                    do_pw = true;
                                }
                                if ui.button("⚙ Configuración").clicked() {
                                    do_settings = true;
                                }
                                if auth.is_some() {
                                    ui.separator();
                                    if theme::session_action_button(ui, false).clicked() {
                                        do_logout = true;
                                    }
                                }
                            });
                    });
                let clicked_elsewhere = area.response.clicked_elsewhere()
                    && !ctx
                        .input(|i| i.pointer.interact_pos())
                        .map(|pos| rect.contains(pos))
                        .unwrap_or(false);
                if do_login {
                    self.login_modal_open = true;
                }
                if do_pw {
                    self.change_pw_modal_open = true;
                    self.change_pw_msg.clear();
                }
                if do_settings {
                    self.tab = NavTab::Settings;
                }
                if do_logout {
                    self.logout();
                }
                if do_login || do_pw || do_settings || do_logout || clicked_elsewhere {
                    self.user_menu_open = false;
                }
            }
        }

        // Modal de login
        if self.login_modal_open {
            let mut open = true;
            egui::Window::new("Iniciar sesión")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| self.ui_account_login_card(ui, "Cuenta técnica"));
            if !open {
                self.login_modal_open = false;
            }
        }

        // Modal de cambio de contraseña
        if self.change_pw_modal_open {
            let mut open = true;
            let mut submit = false;
            egui::Window::new("Cambiar contraseña")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Contraseña actual:");
                    ui.add(
                        theme::centered_text_edit(&mut self.cur_pw_input)
                            .password(true)
                            .desired_width(300.0),
                    );
                    ui.label("Nueva contraseña (mínimo 6):");
                    ui.add(
                        theme::centered_text_edit(&mut self.new_pw_input)
                            .password(true)
                            .desired_width(300.0),
                    );
                    ui.label("Repetir nueva contraseña:");
                    ui.add(
                        theme::centered_text_edit(&mut self.new_pw_confirm)
                            .password(true)
                            .desired_width(300.0),
                    );
                    if !self.change_pw_msg.is_empty() {
                        ui.colored_label(p.warn, &self.change_pw_msg);
                    }
                    if ui.add(primary_button(&p, "Guardar")).clicked() {
                        submit = true;
                    }
                });
            if submit {
                self.submit_change_password();
            }
            if !open {
                self.change_pw_modal_open = false;
            }
        }
    }
}
