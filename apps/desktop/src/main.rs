#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod panels;
mod theme;

use agentdesk_capture::win::{
    current_process_private_mb, enumerate_monitors, inject_input_event, inject_special_combo,
    read_clipboard_text, write_clipboard_text, WindowsCapturer,
};
use agentdesk_capture::{decode_frame_to_rgba, encode_frame, ScreenCapturer};
use agentdesk_identity::DeviceIdentity;
use agentdesk_protocol::{
    AddressBookEntryDto, AgentToServerMessage, AiChatResponse, AiProposedActionDto,
    DeviceGroupDto, SetPersonalPasswordRequest,
    AiSettingsDto, AuthResponse, CalendarEventDto, ControlMessage, CreateEnrollmentResponse,
    CreateSessionRequest, CreateSessionResponse, DataPacket, DeviceDto, DirectConnectRequest,
    DirectConnectResponse, FrameEncoding, InputEvent, LoginRequest, MfaSetupResponse, MonitorInfo,
    MouseBtn, QuickRegisterRequest, QuickRegisterResponse, RmmCommandDto, RotatePasswordRequest,
    ServerToAgentMessage, SupportTicketDto, UpsertAddressBookRequest, UserDto, VideoFrame,
};
use agentdesk_transport::RelayTunnel;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use config::{
    detect_lan_ip, install_host_service_elevated, normalize_conn_code, normalize_server_url,
    set_windows_autostart, ConnectionHistoryEntry, DesktopConfig,
};
use eframe::egui::{self, FontId, RichText};
use futures_util::{SinkExt, StreamExt};
use panels::{egui_key_to_vk, extract_resources, render_hardware_resources_ui};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use theme::{apply_theme, badge_frame, danger_button, primary_button, styled_card, Palette, ThemeMode};
use tokio_tungstenite::connect_async;
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
    RemoteFileAck(String),
    SessionEnded(String),
    AiSettingsLoaded(AiSettingsDto),
    AiOAuthUrl(String),
    AiChatResult(AiChatResponse),
    AiActionExecuted(RmmCommandDto),
    RmmDiagnosticResult(serde_json::Value),
    RmmCommandResult(RmmCommandDto),
    CalendarLoaded(Vec<CalendarEventDto>),
    UsersLoaded(Vec<UserDto>),
    MfaSetupResult(MfaSetupResponse),
    GroupsLoaded(Vec<DeviceGroupDto>),
    RemoteHardwareResources(serde_json::Value),
    LocalHardwareResources(serde_json::Value),
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
    auth: Option<AuthResponse>,
    status_msg: String,

    tab: NavTab,
    search: String,
    only_favorites: bool,

    devices: Vec<DeviceDto>,
    agenda: Vec<AddressBookEntryDto>,
    tickets: Vec<SupportTicketDto>,

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
    local_hw: Option<serde_json::Value>,
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

impl AgentDeskApp {
    fn new(cc: &eframe::CreationContext<'_>, deeplink: Option<String>) -> Self {
        let (cfg, config_path) = DesktopConfig::load();
        let theme_mode = cfg.effective_theme();
        let palette = apply_theme(&cc.egui_ctx, theme_mode);

        let (ui_tx, ui_rx) = mpsc::channel();
        let pending_deeplink_connect = deeplink.and_then(|uri| {
            uri.strip_prefix("agentdesk://connect/")
                .and_then(|s| Uuid::parse_str(s.trim_end_matches('/')).ok())
        });

        let mut app = Self {
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
            http: reqwest::Client::new(),
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
            incoming_control: None,
            partner_id_input: String::new(),
            partner_password_input: String::new(),
            show_partner_password: false,
            partner_allow_control: true,
            connect_feedback: String::new(),
            email: cfg.last_email,
            password: String::new(),
            auth: None,
            status_msg: String::new(),
            tab: NavTab::Connect,
            search: String::new(),
            only_favorites: false,
            devices: Vec::new(),
            agenda: Vec::new(),
            tickets: Vec::new(),
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
            local_hw: None,
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
        };

        #[cfg(windows)]
        spawn_desktop_tray(cc.egui_ctx.clone(), app.ui_tx.clone(), app.tray_ok.clone());
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
                    self.status_msg = format!(
                        "Configuración guardada en {}",
                        self.config_path.display()
                    );
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
            if e.is_empty() { None } else { Some(e) }
        };
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

                match connect_async(&ws_url).await {
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
                                let _ = hb_tx.send(AgentToServerMessage::Heartbeat {
                                    screen_width: screen_w as i32,
                                    screen_height: screen_h as i32,
                                    private_mem_mb: current_process_private_mb(),
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
                                                ServerToAgentMessage::Welcome { public_code, .. } => {
                                                    let _ = ui_tx.send(UiEvent::PeerStatus(format!(
                                                        "Listo para recibir conexiones (ID: {public_code})"
                                                    )));
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
        let (Some(did), Some(dtok)) = (self.local_device_id, self.local_device_token.clone()) else {
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
                    let _ = tx.send(UiEvent::Toast(format!("Error rotando contraseña: {}", r.status())));
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
            self.connect_feedback = "Ingresa el ID de conexión y la contraseña del asociado.".into();
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
                        .and_then(|v| v.get("error").and_then(|s| s.as_str()).map(|s| s.to_string()))
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

        self.rt.spawn(async move {
            let res = http
                .post(format!("{base}/api/v1/auth/login"))
                .json(&LoginRequest {
                    email,
                    password: pw,
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
                    let _ = tx.send(UiEvent::LoginErr(format!("Credenciales inválidas (HTTP {})", r.status())));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::LoginErr(e.to_string()));
                }
            }
        });
    }

    fn refresh_data(&self) {
        let Some(auth) = &self.auth else { return };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();

        self.rt.spawn(async move {
            let d = http
                .get(format!("{base}/api/v1/devices"))
                .bearer_auth(&token)
                .send()
                .await;
            let a = http
                .get(format!("{base}/api/v1/address-book"))
                .bearer_auth(&token)
                .send()
                .await;
            let t = http
                .get(format!("{base}/api/v1/tickets"))
                .bearer_auth(&token)
                .send()
                .await;

            if let (Ok(dr), Ok(ar), Ok(tr)) = (d, a, t) {
                let devices = dr.json::<Vec<DeviceDto>>().await.unwrap_or_default();
                let agenda = ar.json::<Vec<AddressBookEntryDto>>().await.unwrap_or_default();
                let tickets = tr.json::<Vec<SupportTicketDto>>().await.unwrap_or_default();
                let _ = tx.send(UiEvent::DataLoaded {
                    devices,
                    agenda,
                    tickets,
                });
            }
        });
    }

    fn start_session(&self, device_id: Uuid, ticket_id: Option<Uuid>, allow_control: bool, label: String) {
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
                    allow_clipboard: true,
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
                    let _ = tx.send(UiEvent::Toast(format!("No se pudo iniciar sesión: {}", r.status())));
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
                                if let Ok((dw, dh, rgba)) = decode_frame_to_rgba(vf.encoding, &vf.payload) {
                                    let w = if dw > 0 { dw } else { vf.width };
                                    let h = if dh > 0 { dh } else { vf.height };
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
                            Ok(DataPacket::Control(ControlMessage::FileAck { ok, message, .. })) => {
                                let prefix = if ok { "✓ " } else { "✗ " };
                                let _ = ui_tx.send(UiEvent::RemoteFileAck(format!("{prefix}{message}")));
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
            let _ = v.out_tx.send(DataPacket::Control(ControlMessage::Terminate {
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
                    self.peer_status = st;
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
                    self.peer_status = format!("En línea (ID: {connection_id})");
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
                    self.status_msg = format!(
                        "Conectado como {} ({})",
                        auth.user.display_name, auth.user.organization_name
                    );
                    self.auth = Some(auth);
                    self.refresh_data();
                    self.load_extra_tabs_data();
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
                UiEvent::RemoteMonitors { monitors, active_index } => {
                    if let Some(v) = &mut self.viewer {
                        v.monitors = monitors;
                        v.active_monitor = active_index;
                    }
                }
                UiEvent::RemoteClipboard(txt) => {
                    self.status_msg = format!("Portapapeles recibido ({} caracteres)", txt.len());
                }
                UiEvent::RemoteFileAck(msg) => {
                    if let Some(v) = &mut self.viewer {
                        v.file_status = msg.clone();
                    }
                    self.status_msg = msg;
                }
                UiEvent::GroupsLoaded(g) => {
                    self.groups = g;
                }
                UiEvent::RemoteHardwareResources(v) => {
                    if let Some(vw) = &mut self.viewer {
                        vw.hw_resources = Some(v);
                    }
                }
                UiEvent::LocalHardwareResources(v) => {
                    self.local_hw = Some(v);
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
                    let target = self.viewer.as_ref().and_then(|v| v.reconnect_target.clone());
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

fn run_local_diagnostic(
    kind: &str,
) -> (bool, serde_json::Value, Option<String>, Option<String>) {
    match kind {
        "inventory" => (true, agentdesk_rmm::collect_inventory(), None, None),
        "metrics" => (true, agentdesk_rmm::collect_metrics(), None, None),
        "resources" => (true, agentdesk_rmm::collect_hardware_resources(), None, None),
        "services" => (true, agentdesk_rmm::collect_services(), None, None),
        "events" => (true, agentdesk_rmm::collect_recent_events(), None, None),
        "screenshot" => {
            let mut cap = WindowsCapturer::new();
            match cap.capture_rgba().and_then(|raw| encode_frame(&raw, FrameEncoding::Jpeg, 65)) {
                Ok(jpg) => (true, serde_json::json!({ "captured": true }), Some(B64.encode(jpg)), None),
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
    data: Vec<u8>,
}

async fn run_incoming_host_session(
    relay_url: String,
    allow_control: bool,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut tunnel = RelayTunnel::connect(&relay_url, false).await?;
    let monitors = enumerate_monitors();
    let _ = tunnel
        .send_packet(&DataPacket::Control(ControlMessage::MonitorList {
            monitors,
            active_index: 0,
        }))
        .await;

    let (pkt_tx, mut pkt_rx) = tokio::sync::mpsc::channel::<DataPacket>(4);
    let quality = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(72));
    let fps = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(24));
    let monitor_idx = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let q_cap = quality.clone();
    let f_cap = fps.clone();
    let m_cap = monitor_idx.clone();
    let cap_stop = stop_rx.clone();

    std::thread::spawn(move || {
        let mut active_mon = m_cap.load(std::sync::atomic::Ordering::Relaxed);
        let mut cap = WindowsCapturer::new_for_monitor(active_mon);
        let mut seq = 0u64;
        while !*cap_stop.borrow() {
            let desired_mon = m_cap.load(std::sync::atomic::Ordering::Relaxed);
            if desired_mon != active_mon {
                if cap.switch_monitor(desired_mon) {
                    active_mon = desired_mon;
                } else {
                    m_cap.store(active_mon, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let target_fps = f_cap.load(std::sync::atomic::Ordering::Relaxed).max(1) as u64;
            let frame_budget = Duration::from_millis(1000 / target_fps);
            let t0 = Instant::now();

            if let Ok(raw) = cap.capture_rgba() {
                let q = q_cap.load(std::sync::atomic::Ordering::Relaxed);
                if let Ok(payload) = encode_frame(&raw, FrameEncoding::Jpeg, q) {
                    seq += 1;
                    let pkt = DataPacket::VideoFrame(VideoFrame {
                        seq,
                        timestamp_ms: now_ms(),
                        width: raw.width,
                        height: raw.height,
                        encoding: FrameEncoding::Jpeg,
                        payload,
                    });
                    let _ = pkt_tx.try_send(pkt);
                }
            }

            let elapsed = t0.elapsed();
            if elapsed < frame_budget {
                std::thread::sleep(frame_budget - elapsed);
            }
        }
    });

    let mut uploads: HashMap<Uuid, PendingHostUpload> = HashMap::new();

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Terminate {
                        reason: "closed locally".into(),
                    })).await;
                    break;
                }
            }
            Some(pkt) = pkt_rx.recv() => {
                if tunnel.send_packet(&pkt).await.is_err() {
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
                        let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Pong { ts_ms })).await;
                    }
                    Ok(DataPacket::Control(ControlMessage::SetQuality { jpeg_quality, max_fps })) => {
                        quality.store(jpeg_quality.clamp(25, 95), std::sync::atomic::Ordering::Relaxed);
                        fps.store(max_fps.clamp(1, 60), std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::SelectMonitor { index })) => {
                        monitor_idx.store(index, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(DataPacket::Control(ControlMessage::ClipboardText { text })) => {
                        if allow_control {
                            write_clipboard_text(&text);
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
                            let safe_name = PathBuf::from(&filename)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| format!("archivo_{transfer_id}.bin"));
                            uploads.insert(transfer_id, PendingHostUpload {
                                filename: safe_name,
                                expected_sha256: sha256,
                                data: Vec::with_capacity(size_bytes as usize),
                            });
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileChunk { transfer_id, data, eof, .. })) => {
                        if let Some(up) = uploads.get_mut(&transfer_id) {
                            up.data.extend_from_slice(&data);
                            if eof {
                                let finished = uploads.remove(&transfer_id).unwrap();
                                let actual_sha = hex::encode(Sha256::digest(&finished.data));
                                let (ok, msg) = if !finished.expected_sha256.is_empty() && actual_sha != finished.expected_sha256 {
                                    (false, format!("SHA-256 inválido para {}", finished.filename))
                                } else {
                                    let base = std::env::var("USERPROFILE")
                                        .map(PathBuf::from)
                                        .unwrap_or_else(|_| std::env::temp_dir());
                                    let dest_dir = base.join("Downloads").join("AgentDesk");
                                    let _ = std::fs::create_dir_all(&dest_dir);
                                    let dest_path = dest_dir.join(&finished.filename);
                                    match std::fs::write(&dest_path, &finished.data) {
                                        Ok(()) => (true, format!("Recibido en {}", dest_path.display())),
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

        let p = self.palette;

        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::NONE
                    .fill(p.card_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("AgentDesk")
                            .strong()
                            .size(18.0)
                            .color(p.accent),
                    );
                    badge_frame(&p).show(ui, |ui| {
                        let color = if self.local_device_id.is_some() {
                            p.ok
                        } else {
                            p.warn
                        };
                        ui.label(
                            RichText::new(format!("● ID: {}", self.local_connection_id))
                                .monospace()
                                .strong()
                                .color(color),
                        );
                    });
                    ui.label(
                        RichText::new(format!("Servidor: {}", self.server_url))
                            .small()
                            .color(p.text_muted),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_label = match &self.auth {
                            Some(a) => format!("👤 {} ▾", a.user.display_name),
                            None => "👤 Iniciar sesión ▾".to_string(),
                        };
                        let resp = ui.button(RichText::new(btn_label).strong().color(p.accent));
                        if resp.clicked() {
                            self.user_menu_open = !self.user_menu_open;
                        }
                        self.user_menu_rect = Some(resp.rect);
                        ui.add_space(12.0);
                        let sresp = ui.add(
                            egui::TextEdit::singleline(&mut self.global_search)
                                .hint_text("🔍 Buscar ID, equipo, grupo o historial...")
                                .desired_width(300.0),
                        );
                        self.search_rect = Some(sresp.rect);
                    });
                });
            });

        self.ui_overlays(ctx);

        egui::SidePanel::left("nav_panel")
            .default_width(225.0)
            .frame(
                egui::Frame::NONE
                    .fill(p.sidebar_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.selectable_value(&mut self.tab, NavTab::Connect, "🖥 Conexión Rápida");
                ui.selectable_value(
                    &mut self.tab,
                    NavTab::Agenda,
                    format!("📒 Clientes Agendados ({})", self.agenda.len()),
                );
                ui.selectable_value(
                    &mut self.tab,
                    NavTab::Devices,
                    format!("💻 Equipos con Agente ({})", self.devices.len()),
                );
                ui.selectable_value(
                    &mut self.tab,
                    NavTab::Groups,
                    format!("👥 Grupos y Compartir ({})", self.groups.len()),
                );
                ui.selectable_value(
                    &mut self.tab,
                    NavTab::Tickets,
                    format!("🎫 Cola Soporte ({})", self.tickets.len()),
                );
                if self.viewer.is_some() {
                    ui.selectable_value(&mut self.tab, NavTab::Viewer, "📺 Visor Remoto (Activo)");
                }
                ui.separator();
                ui.selectable_value(&mut self.tab, NavTab::AiCopilot, "🤖 Copiloto IA");
                ui.selectable_value(&mut self.tab, NavTab::Rmm, "🛠 RMM y Terminal");
                ui.selectable_value(&mut self.tab, NavTab::Calendar, "📅 Calendario y CSV");
                ui.selectable_value(&mut self.tab, NavTab::UsersMfa, "👥 Usuarios y MFA");
                ui.separator();
                ui.selectable_value(&mut self.tab, NavTab::Settings, "⚙ Configuración");

                if matches!(self.tab, NavTab::Agenda | NavTab::Devices) {
                    ui.separator();
                    ui.label(RichText::new("Filtro rápido:").small().color(p.text_muted));
                    ui.text_edit_singleline(&mut self.search);
                    ui.checkbox(&mut self.only_favorites, "Solo favoritos");
                    if self.auth.is_some() && ui.button("↻ Actualizar lista").clicked() {
                        self.refresh_data();
                    }
                }
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
    }
}

impl AgentDeskApp {
    fn ui_connect(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.palette;

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Control Remoto Directo")
                    .size(22.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("Comparte tu ID y contraseña aleatoria o conéctate a otro equipo.")
                    .color(p.text_muted),
            );
        });
        ui.add_space(10.0);

        ui.columns(2, |cols| {
            // Left Column: Allow Remote Control (My ID + Random Password)
            styled_card(&p).show(&mut cols[0], |ui| {
                ui.set_min_height(320.0);
                ui.label(
                    RichText::new("Permitir control remoto (Este equipo)")
                        .size(16.0)
                        .strong()
                        .color(p.accent),
                );
                ui.label(
                    RichText::new(&self.peer_status)
                        .small()
                        .color(if self.local_device_id.is_some() { p.ok } else { p.warn }),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("Su ID de conexión").small().color(p.text_muted));
                badge_frame(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&self.local_connection_id)
                                .font(FontId::monospace(26.0))
                                .strong()
                                .color(p.mono_highlight),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Copiar ID").clicked() {
                                ctx.copy_text(self.local_connection_id.clone());
                                self.status_msg = "ID copiado al portapapeles".into();
                            }
                        });
                    });
                });

                ui.add_space(10.0);
                ui.label(
                    RichText::new("Contraseña aleatoria de sesión")
                        .small()
                        .color(p.text_muted),
                );
                badge_frame(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let displayed = if self.show_local_password {
                            self.local_password.clone()
                        } else {
                            "••••••••".to_string()
                        };
                        ui.label(
                            RichText::new(displayed)
                                .font(FontId::monospace(22.0))
                                .strong()
                                .color(p.mono_highlight),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("↻ Nueva").clicked() {
                                self.rotate_random_password();
                            }
                            if ui.button("Copiar").clicked() {
                                ctx.copy_text(self.local_password.clone());
                                self.status_msg = "Contraseña copiada".into();
                            }
                            let eye = if self.show_local_password { "Ocultar" } else { "Ver" };
                            if ui.button(eye).clicked() {
                                self.show_local_password = !self.show_local_password;
                            }
                        });
                    });
                });

                ui.add_space(12.0);
                if ui.button("📋 Copiar ID + Contraseña para compartir").clicked() {
                    let clip = format!(
                        "AgentDesk ID: {}\nContraseña: {}\nServidor: {}",
                        self.local_connection_id, self.local_password, self.server_url
                    );
                    ctx.copy_text(clip);
                    self.status_msg = "Credenciales de conexión copiadas".into();
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(
                    RichText::new("Contraseña personal fija (acceso desatendido)")
                        .small()
                        .color(p.text_muted),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.personal_pw_input)
                            .password(true)
                            .hint_text(if self.personal_pw_set {
                                "Fijada ✔ (escribí para cambiarla)"
                            } else {
                                "mínimo 4 caracteres"
                            })
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
                        egui::TextEdit::singleline(&mut self.owner_email_input)
                            .hint_text("usuario@empresa.com")
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
                        Err(e) => self.status_msg = format!("No se pudo cambiar el inicio con Windows: {e}"),
                    }
                }
                egui::CollapsingHeader::new("Servicio Windows (disponible antes del login)")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Pegá un token de enrolamiento (Equipos → Generar token).")
                                .small()
                                .color(p.text_muted),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.service_enroll_token)
                                .desired_width(260.0),
                        );
                        if ui.button("Instalar servicio AgentDeskHost (UAC)").clicked() {
                            match install_host_service_elevated(&self.server_url, &self.service_enroll_token) {
                                Ok(m) => self.status_msg = m,
                                Err(e) => self.status_msg = format!("Error servicio: {e}"),
                            }
                        }
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
                                    if inc.allow_control { "Control total" } else { "Solo ver" }
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

            // Right Column: Control Remote Partner
            styled_card(&p).show(&mut cols[1], |ui| {
                ui.set_min_height(320.0);
                ui.label(
                    RichText::new("Controlar equipo remoto")
                        .size(16.0)
                        .strong()
                        .color(p.accent),
                );
                ui.label(
                    RichText::new("Ingresa el ID y la contraseña generados en el otro cliente.")
                        .small()
                        .color(p.text_muted),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("ID de asociado").small().color(p.text_muted));
                ui.add(
                    egui::TextEdit::singleline(&mut self.partner_id_input)
                        .font(FontId::monospace(18.0))
                        .hint_text("Ej: 482 910 374")
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(8.0);
                ui.label(RichText::new("Contraseña").small().color(p.text_muted));
                ui.horizontal(|ui| {
                    let pw_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.partner_password_input)
                            .password(!self.show_partner_password)
                            .font(FontId::monospace(16.0))
                            .hint_text("Contraseña aleatoria")
                            .desired_width(260.0),
                    );
                    let eye = if self.show_partner_password { "Ocultar" } else { "Ver" };
                    if ui.button(eye).clicked() {
                        self.show_partner_password = !self.show_partner_password;
                    }
                    if pw_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.connect_by_id();
                    }
                });

                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.partner_allow_control,
                    "Permitir control de mouse y teclado (desmarcar = Solo ver)",
                );

                ui.add_space(12.0);
                if ui
                    .add_sized(
                        [260.0, 38.0],
                        primary_button(&p, "Conectar ahora (E2EE Noise_XX)"),
                    )
                    .clicked()
                {
                    self.connect_by_id();
                }

                if !self.connect_feedback.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(&self.connect_feedback).color(p.warn));
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(
                    RichText::new("Histórico de conexiones")
                        .strong()
                        .color(p.accent),
                );
                if self.connection_history.is_empty() {
                    ui.label(
                        RichText::new("Todavía no te conectaste a ningún equipo.")
                            .small()
                            .color(p.text_muted),
                    );
                }
                let now = now_unix();
                let mut pick: Option<usize> = None;
                for (i, h) in self.connection_history.iter().enumerate().take(6) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{} ({})", h.hostname, h.connection_id))
                                .monospace(),
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
        });

        ui.add_space(12.0);
        egui::CollapsingHeader::new("📊 Recursos de este PC (CPU / RAM / Disco / GPU)")
            .default_open(false)
            .show(ui, |ui| {
                if ui.button("↻ Actualizar").clicked() {
                    self.refresh_local_hw(ctx);
                }
                if let Some(hw) = self.local_hw.clone() {
                    render_hardware_resources_ui(ui, &p, &hw);
                } else {
                    ui.label(
                        RichText::new("Presioná Actualizar para leer procesador, memoria, discos y GPU.")
                            .small()
                            .color(p.text_muted),
                    );
                }
            });

        ui.add_space(12.0);
        badge_frame(&p).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("🔒 Cifrado de extremo a extremo Noise_XX (25519 + ChaChaPoly)")
                        .small()
                        .strong()
                        .color(p.ok),
                );
                if !self.status_msg.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&self.status_msg).small().color(p.text_muted));
                }
            });
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
            ui.add(egui::TextEdit::singleline(&mut self.email).desired_width(420.0));
            ui.label("Contraseña:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.password)
                    .password(true)
                    .desired_width(420.0),
            );
            ui.add_space(8.0);
            if ui.add(primary_button(&p, "Iniciar sesión")).clicked()
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
                    ui.text_edit_singleline(&mut self.new_alias);
                    ui.label("Empresa:");
                    ui.text_edit_singleline(&mut self.new_company);
                });
                ui.horizontal(|ui| {
                    ui.label("Contacto:");
                    ui.text_edit_singleline(&mut self.new_contact);
                    ui.label("Grupo:");
                    ui.text_edit_singleline(&mut self.new_group);
                });
                ui.horizontal(|ui| {
                    ui.label("Notas:");
                    ui.text_edit_singleline(&mut self.new_notes);
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
                    .selected_text(dev_names.get(self.new_device_idx).cloned().unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (idx, name) in dev_names.iter().enumerate() {
                            ui.selectable_value(&mut self.new_device_idx, idx, name);
                        }
                    });
                let group_names: Vec<String> = std::iter::once("General (sin grupo)".to_string())
                    .chain(self.groups.iter().map(|g| g.name.clone()))
                    .collect();
                egui::ComboBox::from_label("Grupo compartido")
                    .selected_text(group_names.get(self.new_agenda_group_idx).cloned().unwrap_or_default())
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
                            let color = if st == "online" { p.ok } else { p.text_muted };
                            ui.colored_label(color, format!("● {st}"));
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
            self.ui_account_login_card(ui, "Equipos con Agente");
            return;
        }
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.heading("Equipos con Agente / Clientes Conectados");
            if ui.add(primary_button(&p, "+ Generar token enrolamiento")).clicked() {
                let auth = self.auth.as_ref().unwrap();
                let http = self.http.clone();
                let tx = self.ui_tx.clone();
                let base = self.server_url.trim_end_matches('/').to_string();
                let token = auth.token.clone();
                self.rt.spawn(async move {
                    if let Ok(r) = http
                        .post(format!("{base}/api/v1/enrollments"))
                        .bearer_auth(&token)
                        .json(&serde_json::json!({ "label": "Desktop Enroll", "max_uses": 20, "expires_hours": 168 }))
                        .send()
                        .await
                    {
                        if let Ok(er) = r.json::<CreateEnrollmentResponse>().await {
                            let _ = tx.send(UiEvent::EnrollmentCreated(format!(
                                "agentdesk-host.exe --server {base} --enroll-token {}",
                                er.enrollment_token
                            )));
                        }
                    }
                });
            }
        });
        if !self.enroll_cmd_out.is_empty() {
            ui.add(egui::TextEdit::singleline(&mut self.enroll_cmd_out).desired_width(f32::INFINITY));
        }
        ui.separator();
        let q = self.search.to_lowercase();
        let devs = self.devices.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for d in devs {
                if !q.is_empty()
                    && !d.hostname.to_lowercase().contains(&q)
                    && !d.public_code.to_lowercase().contains(&q)
                {
                    continue;
                }
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let color = if d.status == "online" { p.ok } else { p.text_muted };
                        ui.colored_label(color, format!("● {}", d.status));
                        ui.strong(format!("{} (ID: {})", d.hostname, d.public_code));
                        ui.label(
                            RichText::new(format!(
                                "{} · {}x{} · {:.1} MB{}{}",
                                d.kind,
                                d.screen_width,
                                d.screen_height,
                                d.private_mem_mb,
                                d.owner_email
                                    .as_ref()
                                    .map(|e| format!(" · 👤 {e}"))
                                    .unwrap_or_default(),
                                if d.has_personal_password { " · 🔑 pass fija" } else { "" }
                            ))
                            .small()
                            .color(p.text_muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(primary_button(&p, "Conectar (Control)")).clicked() {
                                self.start_session(d.id, None, true, d.hostname.clone());
                            }
                            if ui.button("Solo Ver").clicked() {
                                self.start_session(d.id, None, false, d.hostname.clone());
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });
    }

    fn ui_tickets(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Cola de Asistencia Web");
            return;
        }
        let p = self.palette;
        ui.heading("Cola de Asistencia Web");
        ui.separator();
        let tks = self.tickets.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for t in tks {
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(format!("{} — {} ({})", t.public_code, t.requester_name, t.status));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(did) = t.device_id {
                                if ui.add(primary_button(&p, "Conectar")).clicked() {
                                    self.start_session(did, Some(t.id), true, t.public_code.clone());
                                }
                            }
                            if t.status == "queued" && ui.button("Reclamar").clicked() {
                                let auth = self.auth.as_ref().unwrap();
                                let http = self.http.clone();
                                let tx = self.ui_tx.clone();
                                let base = self.server_url.trim_end_matches('/').to_string();
                                let token = auth.token.clone();
                                let tid = t.id;
                                self.rt.spawn(async move {
                                    let _ = http
                                        .post(format!("{base}/api/v1/tickets/{tid}/claim"))
                                        .bearer_auth(&token)
                                        .send()
                                        .await;
                                    let _ = tx.send(UiEvent::Toast("Ticket reclamado".into()));
                                });
                                self.refresh_data();
                            }
                        });
                    });
                    ui.label(&t.description);
                });
                ui.add_space(6.0);
            }
        });
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
                ui.label(format!("FPS: {:.1} · RTT: {} ms · {:.1} KB/s", v.fps, v.rtt_ms, v.kbps));
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
                                if ui.selectable_value(&mut v.active_monitor, m.index, txt).clicked() {
                                    let _ = v.out_tx.send(DataPacket::Control(ControlMessage::SelectMonitor {
                                        index: m.index,
                                    }));
                                }
                            }
                        });
                }
                if v.allow_control {
                    for combo in ["Win", "Alt+Tab", "Ctrl+Shift+Esc"] {
                        if ui.button(combo).clicked() {
                            let _ = v.out_tx.send(DataPacket::Control(ControlMessage::SpecialKey {
                                combo: combo.into(),
                            }));
                        }
                    }
                    if ui.button("📋 Enviar Portapapeles").clicked() {
                        if let Some(clip) = read_clipboard_text() {
                            let _ = v.out_tx.send(DataPacket::Control(ControlMessage::ClipboardText { text: clip }));
                        }
                    }
                }
                if ui.add(egui::Slider::new(&mut v.jpeg_quality, 30..=95).text("Calidad")).changed() {
                    send_quality = true;
                }
                if v.allow_control {
                    if ui.button("🔄 Reiniciar PC").clicked() {
                        v.confirm_reboot_open = true;
                    }
                    if ui.button("🔒 Bloquear").clicked() {
                        actions.push(ControlMessage::SessionAction {
                            action: "lock".into(),
                            target_user: None,
                            delay_secs: None,
                        });
                    }
                    if ui.button("👤 Usuario Windows").clicked() {
                        v.show_user_switch_popup = !v.show_user_switch_popup;
                        if v.show_user_switch_popup {
                            actions.push(ControlMessage::RequestHardwareResources);
                        }
                    }
                }
                let res_label = if v.show_resources_panel { "📊 Ocultar recursos" } else { "📊 Recursos PC" };
                if ui.button(res_label).clicked() {
                    v.show_resources_panel = !v.show_resources_panel;
                    if v.show_resources_panel {
                        actions.push(ControlMessage::RequestHardwareResources);
                    }
                }
                if ui.add(danger_button(&p, "Terminar sesión")).clicked() {
                    should_terminate = true;
                }
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
                            if ui.add(danger_button(&p, "Reiniciar y auto-reconectar")).clicked() {
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
                                if let Some(users) = hw.get("windows_users").and_then(|u| u.as_array()) {
                                    ui.strong("Cuentas locales:");
                                    for u in users {
                                        ui.label(format!(
                                            "• {}",
                                            u.get("name").and_then(|x| x.as_str()).unwrap_or("?")
                                        ));
                                    }
                                }
                                if let Some(ss) = hw.get("windows_sessions").and_then(|u| u.as_array()) {
                                    ui.strong("Sesiones activas:");
                                    for s in ss {
                                        let id = s.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        ui.horizontal(|ui| {
                                            ui.label(format!(
                                                "#{} {} ({})",
                                                id,
                                                s.get("username").and_then(|x| x.as_str()).unwrap_or("?"),
                                                s.get("state").and_then(|x| x.as_str()).unwrap_or("?")
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
                            .add(danger_button(&p, "Cerrar sesión actual / cambiar de usuario"))
                            .clicked()
                        {
                            actions.push(ControlMessage::SessionAction {
                                action: "switch_user".into(),
                                target_user: None,
                                delay_secs: None,
                            });
                        }
                        ui.label(
                            RichText::new("Luego elegí el usuario en la pantalla de inicio de sesión remota.")
                                .small()
                                .color(p.text_muted),
                        );
                    });
                if !open {
                    v.show_user_switch_popup = false;
                }
            }

            if send_quality {
                let _ = v.out_tx.send(DataPacket::Control(ControlMessage::SetQuality {
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
                    let (rect, resp) = cols[0].allocate_exact_size(draw_size, egui::Sense::click_and_drag());
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
                            if (nx - v.last_mouse_norm.0).abs() > 0.002 || (ny - v.last_mouse_norm.1).abs() > 0.002 {
                                v.last_mouse_norm = (nx, ny);
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseMoveNorm { x: nx, y: ny }));
                            }
                        }
                        cols[0].input(|i| {
                            if i.pointer.button_pressed(egui::PointerButton::Primary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton { button: MouseBtn::Left, pressed: true }));
                            }
                            if i.pointer.button_released(egui::PointerButton::Primary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton { button: MouseBtn::Left, pressed: false }));
                            }
                            if i.pointer.button_pressed(egui::PointerButton::Secondary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton { button: MouseBtn::Right, pressed: true }));
                            }
                            if i.pointer.button_released(egui::PointerButton::Secondary) {
                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::MouseButton { button: MouseBtn::Right, pressed: false }));
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
                                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::UnicodeChar { ch }));
                                            }
                                        }
                                    }
                                    egui::Event::Key { key, pressed, repeat, .. } if !repeat => {
                                        if let Some(vk) = egui_key_to_vk(*key) {
                                            // Printable alphanumeric/space already handled by Event::Text on press
                                            if vk < 0x30 || !*pressed {
                                                let _ = v.out_tx.send(DataPacket::Input(InputEvent::Key { vk, pressed: *pressed }));
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
                        ui.label("Esperando handshake Noise_XX y primer frame del equipo destino...");
                    });
                }

                cols[1].heading("Chat de sesión");
                egui::ScrollArea::vertical().max_height(200.0).show(&mut cols[1], |ui| {
                    for line in &v.chat_log {
                        ui.label(line);
                    }
                });
                cols[1].horizontal(|ui| {
                    ui.text_edit_singleline(&mut v.chat_input);
                    if ui.button("Enviar").clicked() && !v.chat_input.trim().is_empty() {
                        send_chat = Some(v.chat_input.clone());
                        v.chat_log.push(format!("Yo: {}", v.chat_input));
                        v.chat_input.clear();
                    }
                });

                cols[1].separator();
                cols[1].heading("Transferencia de archivos E2EE (SHA-256)");
                cols[1].label(RichText::new("Ruta local del archivo a enviar al destino:").small().color(p.text_muted));
                cols[1].text_edit_singleline(&mut v.file_path_input);
                if cols[1].button("📤 Enviar archivo").clicked() && !v.file_path_input.trim().is_empty() {
                    let path = PathBuf::from(v.file_path_input.trim().trim_matches('"'));
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            let transfer_id = Uuid::new_v4();
                            let filename = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "archivo.bin".into());
                            let sha256 = hex::encode(Sha256::digest(&bytes));
                            let _ = v.out_tx.send(DataPacket::Control(ControlMessage::FileOffer {
                                transfer_id,
                                filename: filename.clone(),
                                size_bytes: bytes.len() as u64,
                                sha256,
                            }));
                            let chunk_size = 64 * 1024;
                            let total = bytes.len();
                            for (i, chunk) in bytes.chunks(chunk_size).enumerate() {
                                let offset = (i * chunk_size) as u64;
                                let eof = (i + 1) * chunk_size >= total;
                                let _ = v.out_tx.send(DataPacket::Control(ControlMessage::FileChunk {
                                    transfer_id,
                                    offset,
                                    data: chunk.to_vec(),
                                    eof,
                                }));
                            }
                            if bytes.is_empty() {
                                let _ = v.out_tx.send(DataPacket::Control(ControlMessage::FileChunk {
                                    transfer_id,
                                    offset: 0,
                                    data: Vec::new(),
                                    eof: true,
                                }));
                            }
                            v.file_status = format!("Enviando {filename} ({total} bytes)...");
                        }
                        Err(e) => {
                            v.file_status = format!("Error leyendo archivo: {e}");
                        }
                    }
                }
                if !v.file_status.is_empty() {
                    cols[1].label(&v.file_status);
                }

                if v.show_resources_panel {
                    cols[1].separator();
                    cols[1].horizontal(|ui| {
                        ui.heading("Recursos del PC remoto");
                        if ui.button("↻").clicked() {
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
            self.status_msg = "Reinicio solicitado: se reconectará automáticamente cuando el equipo vuelva".into();
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
                RichText::new("URL base del servidor (ej. http://10.1.1.156:8080 o http://127.0.0.1:8080):")
                    .small()
                    .color(p.text_muted),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.settings_server_url)
                    .hint_text("http://10.1.1.156:8080")
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

        ui.add_space(10.0);
        styled_card(&p).show(ui, |ui| {
            ui.strong("Apariencia / Tema visual");
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.theme_mode == ThemeMode::OpenVmsLight, "☀ OpenVMS Claro")
                    .clicked()
                {
                    self.set_theme(ctx, ThemeMode::OpenVmsLight);
                }
                if ui
                    .selectable_label(self.theme_mode == ThemeMode::DraculaDark, "🧛 Dracula Oscuro")
                    .clicked()
                {
                    self.set_theme(ctx, ThemeMode::DraculaDark);
                }
            });
            ui.label(
                RichText::new(format!("Tema activo: {}", self.theme_mode.label()))
                    .small()
                    .color(p.text_muted),
            );
        });

        ui.add_space(10.0);
        if self.auth.is_some() {
            styled_card(&p).show(ui, |ui| {
                ui.strong("Sesión de cuenta técnica");
                if let Some(auth) = self.auth.clone() {
                    ui.label(format!("Usuario: {} ({})", auth.user.display_name, auth.user.email));
                    ui.label(format!("Organización: {} · Rol: {}", auth.user.organization_name, auth.user.role));
                    if ui.button("Cerrar sesión de cuenta").clicked() {
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
        MessageBoxW(None, PCWSTR(b.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

fn desktop_log_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("desktop.log")
}

fn init_desktop_logging() {
    let path = desktop_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
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

fn main() {
    init_desktop_logging();

    std::panic::set_hook(Box::new(|info| {
        let msg = format!("Error inesperado en AgentDesk Desktop: {info}");
        tracing::error!("{msg}");
        #[cfg(windows)]
        show_error_box("Error crítico en AgentDesk Desktop", &msg);
    }));

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

/// Icono de bandeja del cliente: cuadrado redondeado verde-teal con flechas ⇄
/// (distinto al círculo azul de la consola del servidor).
fn build_desktop_icon_rgba() -> Vec<u8> {
    let w = 32u32;
    let h = 32u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let cx = fx.clamp(7.0, 25.0);
            let cy = fy.clamp(7.0, 25.0);
            let inside = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() <= 7.0;
            if !inside {
                continue;
            }
            let (mut r, mut g, mut b) = (13u8, 148u8, 136u8);

            let top_shaft = (7..=21).contains(&x) && (10..=11).contains(&y);
            let top_head = (21..=26).contains(&x)
                && ((y as f32) - 10.5).abs() <= (26 - x) as f32 * 0.6 + 0.5;
            let bot_shaft = (10..=24).contains(&x) && (20..=21).contains(&y);
            let bot_head = (5..=10).contains(&x)
                && ((y as f32) - 20.5).abs() <= (x - 5) as f32 * 0.6 + 0.5;
            if top_shaft || top_head || bot_shaft || bot_head {
                r = 255;
                g = 255;
                b = 255;
            }
            rgba[idx] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = 255;
        }
    }
    rgba
}

#[cfg(windows)]
fn spawn_desktop_tray(
    ctx: egui::Context,
    tx: Sender<UiEvent>,
    ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
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
            Err(_) => return,
        };
        let _tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("AgentDesk — Cliente remoto")
            .with_icon(icon)
            .build()
        {
            Ok(t) => t,
            Err(_) => return,
        };
        ok.store(true, std::sync::atomic::Ordering::Relaxed);

        std::thread::spawn(move || loop {
            let mut woke = false;
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                woke = true;
                let act = if ev.id == id_open {
                    Some(TrayAction::ShowWindow)
                } else if ev.id == id_copy {
                    Some(TrayAction::CopyId)
                } else if ev.id == id_quit {
                    Some(TrayAction::QuitApp)
                } else {
                    None
                };
                if let Some(a) = act {
                    let _ = tx.send(UiEvent::Tray(a));
                }
            }
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } = ev
                {
                    woke = true;
                    let _ = tx.send(UiEvent::Tray(TrayAction::ShowWindow));
                }
                let _ = MouseButtonState::Up;
            }
            if woke {
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_millis(80));
        });

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
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

    fn refresh_local_hw(&self, ctx: &egui::Context) {
        let tx = self.ui_tx.clone();
        let c = ctx.clone();
        self.rt.spawn_blocking(move || {
            let hw = agentdesk_rmm::collect_hardware_resources();
            let _ = tx.send(UiEvent::LocalHardwareResources(hw));
            c.request_repaint();
        });
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
                    let _ = tx.send(UiEvent::Toast(format!("Error al fijar contraseña: {}", r.status())));
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
                        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
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
                let mut do_theme = false;
                let mut do_pw = false;
                let mut do_settings = false;
                let mut do_logout = false;
                let auth = self.auth.clone();
                let theme_mode = self.theme_mode;
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
                                        ui.label(RichText::new(&a.user.display_name).strong().color(p.text_primary));
                                        ui.label(RichText::new(&a.user.email).small().color(p.text_muted));
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
                                        ui.label(RichText::new("Sin sesión de cuenta").color(p.text_muted));
                                        if ui.add(primary_button(&p, "Iniciar sesión")).clicked() {
                                            do_login = true;
                                        }
                                    }
                                }
                                ui.separator();
                                let theme_txt = match theme_mode {
                                    ThemeMode::OpenVmsLight => "🧛 Cambiar a Dracula Oscuro",
                                    ThemeMode::DraculaDark => "☀ Cambiar a OpenVMS Claro",
                                };
                                if ui.button(theme_txt).clicked() {
                                    do_theme = true;
                                }
                                if auth.is_some() && ui.button("🔑 Cambiar contraseña").clicked() {
                                    do_pw = true;
                                }
                                if ui.button("⚙ Configuración").clicked() {
                                    do_settings = true;
                                }
                                if auth.is_some() {
                                    ui.separator();
                                    if ui.add(danger_button(&p, "⎋ Cerrar sesión")).clicked() {
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
                if do_theme {
                    let nt = self.theme_mode.toggle();
                    self.set_theme(ctx, nt);
                }
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
                if do_login || do_theme || do_pw || do_settings || do_logout || clicked_elsewhere {
                    self.user_menu_open = false;
                }
            }
        }

        // Resultados de la barra de búsqueda global
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.global_search.clear();
        }
        let q = self.global_search.trim().to_lowercase();
        if !q.is_empty() {
            if let Some(rect) = self.search_rect {
                let qn = q.replace(' ', "");
                let digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
                let mut act: Option<SearchAct> = None;
                egui::Area::new(egui::Id::new("search_results"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 4.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .fill(p.card_bg)
                            .stroke(egui::Stroke::new(1.0_f32, p.card_border))
                            .show(ui, |ui| {
                                ui.set_min_width(rect.width().max(380.0));
                                let mut shown = 0usize;
                                if digits.len() >= 6 {
                                    if ui.button(format!("🔌 Conectar al ID {digits}")).clicked() {
                                        act = Some(SearchAct::FillId(digits.clone()));
                                    }
                                    shown += 1;
                                }
                                for (i, h) in self.connection_history.iter().enumerate() {
                                    if shown >= 10 {
                                        break;
                                    }
                                    if h.hostname.to_lowercase().contains(&q)
                                        || h.connection_id.replace(' ', "").contains(&qn)
                                    {
                                        if ui
                                            .button(format!("🕘 {} ({}) — historial", h.hostname, h.connection_id))
                                            .clicked()
                                        {
                                            act = Some(SearchAct::History(i));
                                        }
                                        shown += 1;
                                    }
                                }
                                for d in &self.devices {
                                    if shown >= 10 {
                                        break;
                                    }
                                    if d.hostname.to_lowercase().contains(&q)
                                        || d.public_code.replace(' ', "").contains(&qn)
                                    {
                                        if ui
                                            .button(format!("💻 {} ({}) [{}]", d.hostname, d.public_code, d.status))
                                            .clicked()
                                        {
                                            act = Some(SearchAct::Device(d.id, d.hostname.clone()));
                                        }
                                        shown += 1;
                                    }
                                }
                                for a in &self.agenda {
                                    if shown >= 10 {
                                        break;
                                    }
                                    if let Some(did) = a.device_id {
                                        if a.alias.to_lowercase().contains(&q)
                                            || a.client_company.to_lowercase().contains(&q)
                                            || a.contact_name.to_lowercase().contains(&q)
                                        {
                                            if ui
                                                .button(format!("📒 {} [{}]", a.alias, a.group_name))
                                                .clicked()
                                            {
                                                act = Some(SearchAct::Device(did, a.alias.clone()));
                                            }
                                            shown += 1;
                                        }
                                    }
                                }
                                for g in &self.groups {
                                    if shown >= 10 {
                                        break;
                                    }
                                    if g.name.to_lowercase().contains(&q) {
                                        if ui.button(format!("📁 Grupo {}", g.name)).clicked() {
                                            act = Some(SearchAct::Group);
                                        }
                                        shown += 1;
                                    }
                                }
                                if shown == 0 {
                                    ui.label(RichText::new("Sin resultados").small().color(p.text_muted));
                                }
                            });
                    });
                if let Some(a) = act {
                    self.global_search.clear();
                    match a {
                        SearchAct::History(i) => self.reconnect_from_history(i),
                        SearchAct::Device(id, label) => {
                            if self.auth.is_some() {
                                self.start_session(id, None, true, label);
                            } else {
                                self.status_msg = "Iniciá sesión para conectarte a equipos de tu cuenta".into();
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
                    ui.add(egui::TextEdit::singleline(&mut self.cur_pw_input).password(true).desired_width(300.0));
                    ui.label("Nueva contraseña (mínimo 6):");
                    ui.add(egui::TextEdit::singleline(&mut self.new_pw_input).password(true).desired_width(300.0));
                    ui.label("Repetir nueva contraseña:");
                    ui.add(egui::TextEdit::singleline(&mut self.new_pw_confirm).password(true).desired_width(300.0));
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
