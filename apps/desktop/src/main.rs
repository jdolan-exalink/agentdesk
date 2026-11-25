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
    AiSettingsDto, AuthResponse, CalendarEventDto, ControlMessage, CreateEnrollmentResponse,
    CreateSessionRequest, CreateSessionResponse, DataPacket, DeviceDto, DirectConnectRequest,
    DirectConnectResponse, FrameEncoding, InputEvent, LoginRequest, MfaSetupResponse, MonitorInfo,
    MouseBtn, QuickRegisterRequest, QuickRegisterResponse, RmmCommandDto, RotatePasswordRequest,
    ServerToAgentMessage, SupportTicketDto, UpsertAddressBookRequest, UserDto, VideoFrame,
};
use agentdesk_transport::RelayTunnel;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use config::{detect_lan_ip, normalize_server_url, DesktopConfig};
use eframe::egui::{self, FontId, RichText};
use futures_util::{SinkExt, StreamExt};
use panels::egui_key_to_vk;
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
        };

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
        self.connect_feedback = format!("Conectando a {cid}...");
        let allow_control = self.partner_allow_control;
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
            let mut bw_start = Instant::now();

            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let ts = now_ms();
                        let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Ping { ts_ms: ts })).await;
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
                UiEvent::SessionEnded(reason) => {
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

                    let auth_name = self.auth.as_ref().map(|a| a.user.display_name.clone());
                    let mut do_logout = false;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let next_theme = self.theme_mode.toggle();
                        let toggle_txt = match self.theme_mode {
                            ThemeMode::OpenVmsLight => "Tema: OpenVMS Claro → Dracula",
                            ThemeMode::DraculaDark => "Tema: Dracula → OpenVMS Claro",
                        };
                        if ui.button(toggle_txt).clicked() {
                            self.set_theme(ctx, next_theme);
                        }
                        if let Some(name) = auth_name {
                            if ui.button("Cerrar cuenta").clicked() {
                                do_logout = true;
                            }
                            ui.label(RichText::new(format!("👤 {name}")).color(p.ok));
                        } else if ui.button("Cuenta / Agenda").clicked() {
                            self.tab = NavTab::Agenda;
                        }
                    });
                    if do_logout {
                        self.logout();
                    }
                });
            });

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
                NavTab::Connect => self.ui_connect(ui, ctx),
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
            });
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
                                "{} · {}x{} · {:.1} MB",
                                d.kind, d.screen_width, d.screen_height, d.private_mem_mb
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
                if ui.add(danger_button(&p, "Terminar sesión")).clicked() {
                    should_terminate = true;
                }
            });
            ui.separator();

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
            });

            if let Some(txt) = send_chat {
                let _ = v.out_tx.send(DataPacket::Control(ControlMessage::Chat {
                    sender: "Cliente AgentDesk".into(),
                    text: txt,
                }));
            }
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

fn build_desktop_icon_rgba() -> Vec<u8> {
    let w = 32u32;
    let h = 32u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            let dx = (x as f32) - 15.5;
            let dy = (y as f32) - 15.5;
            if (dx * dx + dy * dy).sqrt() <= 14.5 {
                let mut r = 29u8;
                let mut g = 78u8;
                let mut b = 216u8;
                let in_screen = (7..=24).contains(&x) && (8..=19).contains(&y);
                let in_inner = (9..=22).contains(&x) && (10..=17).contains(&y);
                let in_stand = (13..=18).contains(&x) && (20..=23).contains(&y);
                if (in_screen && !in_inner) || in_stand {
                    r = 255;
                    g = 255;
                    b = 255;
                } else if in_inner && (11..=14).contains(&y) && (11..=15).contains(&x) {
                    r = 56;
                    g = 189;
                    b = 248;
                }
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }
    rgba
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
