mod config;

use agentdesk_capture::win::{
    clipboard_sequence_number, current_process_private_mb, enumerate_monitors,
    inject_special_combo, read_clipboard_files, read_clipboard_text, write_clipboard_files,
    write_clipboard_text, WallpaperBlackoutGuard, WindowsCapturer,
};
use agentdesk_capture::{
    encode_frame, enumerate_printers, print_file_to_printer, spawn_audio_capture_worker,
    AbrController, ScreenCapturer, WhiteboardOverlay,
};
use agentdesk_identity::DeviceIdentity;
use agentdesk_protocol::{
    AgentToServerMessage, ControlMessage, DataPacket, EnrollDeviceRequest, EnrollDeviceResponse,
    FileDropTarget, FrameEncoding, ServerToAgentMessage,
};
use agentdesk_transport::{
    connect_ws, enumerate_local_mac_and_ips, send_adaptive_packet, send_wol_magic_packets,
    start_host_p2p_handshake, store_received_file_for_target, verify_sha256_hex, P2pUdpChannel,
    RelayTunnel,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use clap::{Parser, ValueEnum};
use config::{normalize_server_url, HostConfig};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum AgentMode {
    Host,
    Quicksupport,
}

fn default_agent_mode() -> AgentMode {
    #[cfg(feature = "quicksupport")]
    {
        AgentMode::Quicksupport
    }
    #[cfg(not(feature = "quicksupport"))]
    {
        AgentMode::Host
    }
}

#[derive(Parser, Debug, Clone)]
#[command(name = "agentdesk-host", about = "AgentDesk Host & QuickSupport Agent")]
struct Cli {
    /// Run as a Windows Service (AgentDeskHost via Windows SCM)
    #[arg(long, default_value_t = false)]
    service: bool,

    /// Server base URL (overrides saved host_config.json / AGENTDESK_SERVER)
    #[arg(long)]
    server: Option<String>,

    /// Save server URL into host configuration and exit
    #[arg(long, value_name = "URL")]
    configure_server: Option<String>,

    /// Print resolved host configuration path and sanitized settings, then exit
    #[arg(long, default_value_t = false)]
    show_config: bool,

    #[arg(long, value_enum, default_value_t = default_agent_mode())]
    mode: AgentMode,

    #[arg(long)]
    enroll_token: Option<String>,

    #[arg(long)]
    support_code: Option<String>,

    #[arg(long, default_value_t = false)]
    auto_accept: bool,

    #[arg(long, value_name = "FRAMES")]
    benchmark: Option<usize>,

    #[arg(long)]
    fps: Option<u8>,

    #[arg(long)]
    quality: Option<u8>,
}

#[allow(dead_code)]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn enroll_token_file() -> PathBuf {
    std::env::var("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("AgentDesk")
        .join("host-enroll.token")
}

fn read_enroll_token_file() -> Option<String> {
    let raw = std::fs::read_to_string(enroll_token_file()).ok()?;
    let tok = raw.trim_start_matches('\u{feff}').trim().to_string();
    (!tok.is_empty()).then_some(tok)
}

fn remove_enroll_token_file() {
    let _ = std::fs::remove_file(enroll_token_file());
}

/// Rewrites this service's command line without `--enroll-token <secret>` (services
/// installed by AgentDesk <= 0.1.6 had it there, readable by any local user).
#[cfg(windows)]
fn scrub_service_enroll_token() {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let mut kept: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--enroll-token" {
            let _ = args.next();
            continue;
        }
        if a.starts_with("--enroll-token=") {
            continue;
        }
        kept.push(a);
    }
    if !kept.iter().any(|a| a == "--service") {
        kept.insert(0, "--service".into());
    }
    // sc.exe reads \" as a literal quote inside binPath=.
    let quote = |s: &str| {
        if s.contains(' ') {
            format!(r#"\"{s}\""#)
        } else {
            s.to_string()
        }
    };
    let mut bin = format!(r#"\"{}\""#, exe.display());
    for a in &kept {
        bin.push(' ');
        bin.push_str(&quote(a));
    }
    let status = std::process::Command::new("sc.exe")
        .args(["config", win_service::SERVICE_NAME, "binPath="])
        .raw_arg(format!("\"{bin}\""))
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
    match status {
        Ok(st) if st.success() => {
            tracing::info!("Enrollment token removed from the service command line")
        }
        other => tracing::warn!(?other, "Could not remove the enrollment token from the service command line"),
    }
}

fn identity_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("host_identity.dpapi")
}

fn run_benchmark(frames: usize, quality: u8) -> anyhow::Result<()> {
    let mut cap = WindowsCapturer::new();
    let (w, h) = cap.dimensions();
    let backend = cap.backend().as_str();

    let mut frame_times_ms = Vec::with_capacity(frames);
    let mut encoded_sizes = Vec::with_capacity(frames);
    let t_total = Instant::now();

    for _ in 0..frames {
        let t0 = Instant::now();
        let raw = cap.capture_rgba()?;
        let enc = encode_frame(&raw, FrameEncoding::Jpeg, quality)?;
        frame_times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        encoded_sizes.push(enc.len());
    }

    let elapsed_s = t_total.elapsed().as_secs_f64().max(0.0001);
    frame_times_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg_ms = frame_times_ms.iter().sum::<f64>() / frame_times_ms.len() as f64;
    let p95_idx = ((frame_times_ms.len() as f64) * 0.95).floor() as usize;
    let p95_ms = frame_times_ms[p95_idx.min(frame_times_ms.len() - 1)];
    let avg_kb = (encoded_sizes.iter().sum::<usize>() as f64 / encoded_sizes.len() as f64) / 1024.0;
    let priv_mb = current_process_private_mb();

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "backend": backend,
            "resolution": format!("{w}x{h}"),
            "frames": frames,
            "jpeg_quality": quality,
            "total_elapsed_s": (elapsed_s * 100.0).round() / 100.0,
            "effective_fps": ((frames as f64 / elapsed_s) * 10.0).round() / 10.0,
            "avg_frame_ms": (avg_ms * 100.0).round() / 100.0,
            "p95_frame_ms": (p95_ms * 100.0).round() / 100.0,
            "avg_encoded_kb": (avg_kb * 10.0).round() / 10.0,
            "private_mem_mb": (priv_mb * 100.0).round() / 100.0,
        }))?
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.service {
        #[cfg(windows)]
        {
            return win_service::dispatch(cli);
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("--service is only supported on Windows");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("agentdesk_host=info,agentdesk_capture=info")),
        )
        .init();

    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_agent(cli, shutdown_rx))
}

#[cfg(windows)]
mod win_service {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    pub const SERVICE_NAME: &str = "AgentDeskHost";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    static SERVICE_CLI: OnceLock<Cli> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn dispatch(cli: Cli) -> anyhow::Result<()> {
        let _ = SERVICE_CLI.set(cli);
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    fn init_service_logging() {
        let log_path = std::env::var("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("AgentDesk")
            .join("logs")
            .join("host.log");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("agentdesk_host=info,agentdesk_capture=info"));
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init();
        }
    }

    fn set_status(
        handle: &service_control_handler::ServiceStatusHandle,
        state: ServiceState,
        controls_accepted: ServiceControlAccept,
        exit_code: ServiceExitCode,
        wait_hint_secs: u64,
    ) {
        let _ = handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted,
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::from_secs(wait_hint_secs),
            process_id: None,
        });
    }

    fn service_main(_arguments: Vec<OsString>) {
        init_service_logging();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let status_handle = match service_control_handler::register(SERVICE_NAME, move |control| {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = shutdown_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        }) {
            Ok(h) => h,
            Err(_) => return,
        };

        set_status(
            &status_handle,
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            10,
        );

        let cli = SERVICE_CLI
            .get()
            .cloned()
            .unwrap_or_else(|| Cli::parse_from(["agentdesk-host"]));

        let Ok(rt) = tokio::runtime::Builder::new_multi_thread().enable_all().build() else {
            set_status(
                &status_handle,
                ServiceState::Stopped,
                ServiceControlAccept::empty(),
                ServiceExitCode::ServiceSpecific(1),
                0,
            );
            return;
        };

        set_status(
            &status_handle,
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ServiceExitCode::Win32(0),
            0,
        );

        let res = rt.block_on(run_agent(cli, shutdown_rx));

        set_status(
            &status_handle,
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            5,
        );
        set_status(
            &status_handle,
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            if res.is_ok() {
                ServiceExitCode::Win32(0)
            } else {
                ServiceExitCode::ServiceSpecific(2)
            },
            0,
        );
    }
}

async fn run_agent(
    cli: Cli,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {

    if let Some(url) = cli.configure_server {
        let (mut cfg, cfg_path) = HostConfig::load();
        cfg.server_url = normalize_server_url(&url);
        cfg.save(&cfg_path)?;
        println!(
            "Servidor configurado: {}\nArchivo: {}",
            cfg.server_url,
            cfg_path.display()
        );
        return Ok(());
    }

    if cli.show_config {
        let (cfg, cfg_path) = HostConfig::load();
        println!("Config path: {}", cfg_path.display());
        println!("{}", cfg.sanitized_json());
        return Ok(());
    }

    if let Some(frames) = cli.benchmark {
        return run_benchmark(frames.max(1), cli.quality.unwrap_or(70));
    }

    let (server, identity, mut host_cfg_opt) = match cli.mode {
        AgentMode::Host => {
            let (mut cfg, cfg_path) = HostConfig::load();
            if let Some(explicit) = &cli.server {
                cfg.server_url = normalize_server_url(explicit);
                cfg.save(&cfg_path)?;
            } else if let Ok(env_srv) = std::env::var("AGENTDESK_SERVER") {
                if !env_srv.trim().is_empty() {
                    cfg.server_url = normalize_server_url(&env_srv);
                }
            }
            let srv = cfg.server_url.clone();
            let id = DeviceIdentity::load_or_create(&identity_path())?;
            (srv, id, Some((cfg, cfg_path)))
        }
        AgentMode::Quicksupport => {
            // SUP-06: Strict zero persistence on disk in QuickSupport mode
            let srv = cli
                .server
                .clone()
                .or_else(|| std::env::var("AGENTDESK_SERVER").ok())
                .map(|s| normalize_server_url(&s))
                .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
            (srv, DeviceIdentity::generate(), None)
        }
    };

    let effective_fps = cli
        .fps
        .or_else(|| host_cfg_opt.as_ref().map(|(c, _)| c.fps))
        .unwrap_or(30);
    let effective_quality = cli
        .quality
        .or_else(|| host_cfg_opt.as_ref().map(|(c, _)| c.quality))
        .unwrap_or(70);
    let effective_auto_accept = match cli.mode {
        AgentMode::Host => {
            cli.auto_accept
                || host_cfg_opt
                    .as_ref()
                    .map(|(c, _)| c.auto_accept)
                    .unwrap_or(true)
        }
        AgentMode::Quicksupport => cli.auto_accept,
    };

    let capturer = WindowsCapturer::new();
    let (screen_w, screen_h) = capturer.dimensions();
    drop(capturer);

    let host_name = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "Windows-Endpoint".into());

    let kind_str = if cli.mode == AgentMode::Host {
        "host"
    } else {
        "quicksupport"
    };

    // The enrollment token is a secret: it must never live in the service command line
    // (the service's ImagePath in the registry is readable by every local user). Installers
    // drop it in an admin-only one-shot file instead; env var and CLI still work for manual runs.
    let file_token = if cli.mode == AgentMode::Host { read_enroll_token_file() } else { None };
    let has_saved_creds = host_cfg_opt
        .as_ref()
        .is_some_and(|(c, _)| c.device_id.is_some() && c.device_token.is_some());
    let enroll_token: Option<String> = file_token
        .clone()
        .or_else(|| {
            std::env::var("AGENTDESK_ENROLL_TOKEN")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        // A service installed by an older version still carries the token: once enrolled,
        // ignore it instead of re-enrolling on every start.
        .or_else(|| if cli.service && has_saved_creds { None } else { cli.enroll_token.clone() });

    let (device_id, public_code, device_token) = if let Some((ref mut cfg, ref cfg_path)) =
        host_cfg_opt
    {
        if enroll_token.is_none() {
            if let (Some(did), Some(dtok)) = (cfg.device_id, cfg.device_token.clone()) {
                let pcode = cfg
                    .public_code
                    .clone()
                    .unwrap_or_else(|| "AD-HOST".to_string());
                tracing::info!(
                    device_id = %did,
                    public_code = %pcode,
                    server = %server,
                    "Reusing saved Host enrollment configuration"
                );
                (did, pcode, dtok)
            } else {
                let (mac_addresses, lan_ips) = enumerate_local_mac_and_ips();
                let http = reqwest::Client::new();
                let resp: EnrollDeviceResponse = http
                    .post(format!("{server}/api/v1/devices/enroll"))
                    .json(&EnrollDeviceRequest {
                        enrollment_token: None,
                        support_code: cli.support_code.clone(),
                        ed25519_pubkey: identity.public_key_hex(),
                        hostname: host_name.clone(),
                        os_name: std::env::consts::OS.to_string(),
                        os_arch: std::env::consts::ARCH.to_string(),
                        agent_version: env!("CARGO_PKG_VERSION").to_string(),
                        kind: kind_str.to_string(),
                        screen_width: screen_w as i32,
                        screen_height: screen_h as i32,
                        private_mem_mb: current_process_private_mb(),
                        mac_addresses,
                        lan_ips,
                    })
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                cfg.device_id = Some(resp.device_id);
                cfg.public_code = Some(resp.public_code.clone());
                cfg.device_token = Some(resp.device_token.clone());
                cfg.fps = effective_fps;
                cfg.quality = effective_quality;
                cfg.save(cfg_path)?;
                (resp.device_id, resp.public_code, resp.device_token)
            }
        } else {
            let (mac_addresses, lan_ips) = enumerate_local_mac_and_ips();
            let http = reqwest::Client::new();
            let resp: EnrollDeviceResponse = http
                .post(format!("{server}/api/v1/devices/enroll"))
                .json(&EnrollDeviceRequest {
                    enrollment_token: enroll_token.clone(),
                    support_code: cli.support_code.clone(),
                    ed25519_pubkey: identity.public_key_hex(),
                    hostname: host_name.clone(),
                    os_name: std::env::consts::OS.to_string(),
                    os_arch: std::env::consts::ARCH.to_string(),
                    agent_version: env!("CARGO_PKG_VERSION").to_string(),
                    kind: kind_str.to_string(),
                    screen_width: screen_w as i32,
                    screen_height: screen_h as i32,
                    private_mem_mb: current_process_private_mb(),
                    mac_addresses,
                    lan_ips,
                })
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            cfg.server_url = server.clone();
            cfg.device_id = Some(resp.device_id);
            cfg.public_code = Some(resp.public_code.clone());
            cfg.device_token = Some(resp.device_token.clone());
            cfg.fps = effective_fps;
            cfg.quality = effective_quality;
            cfg.save(cfg_path)?;
            if file_token.is_some() {
                remove_enroll_token_file();
            }
            (resp.device_id, resp.public_code, resp.device_token)
        }
    } else {
        let (mac_addresses, lan_ips) = enumerate_local_mac_and_ips();
        let http = reqwest::Client::new();
        let resp: EnrollDeviceResponse = http
            .post(format!("{server}/api/v1/devices/enroll"))
            .json(&EnrollDeviceRequest {
                enrollment_token: cli.enroll_token.clone(),
                support_code: cli.support_code.clone(),
                ed25519_pubkey: identity.public_key_hex(),
                hostname: host_name,
                os_name: std::env::consts::OS.to_string(),
                os_arch: std::env::consts::ARCH.to_string(),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                kind: kind_str.to_string(),
                screen_width: screen_w as i32,
                screen_height: screen_h as i32,
                private_mem_mb: current_process_private_mb(),
                mac_addresses,
                lan_ips,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        (resp.device_id, resp.public_code, resp.device_token)
    };

    #[cfg(windows)]
    if cli.service && cli.enroll_token.is_some() {
        scrub_service_enroll_token();
    }

    tracing::info!(
        %device_id,
        %public_code,
        mode = kind_str,
        %server,
        "Agent ready to connect"
    );

    let ws_base = if let Some(rest) = server.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = server.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        server.clone()
    };

    let agent_ws_url = format!("{ws_base}/ws/agent?device_id={device_id}&token={device_token}");

    loop {
        match connect_ws(&agent_ws_url).await {
            Ok((ws_stream, _)) => {
                let (mut ws_tx, mut ws_rx) = ws_stream.split();
                let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<AgentToServerMessage>();

                let tx_task = tokio::spawn(async move {
                    while let Some(msg) = ctrl_rx.recv().await {
                        if let Ok(txt) = serde_json::to_string(&msg) {
                            if ws_tx.send(Message::Text(txt.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                });

                agentdesk_protocol::telemetry::enable_forwarding();
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
                        for (role, lines) in agentdesk_protocol::telemetry::drain_for_server() {
                            let _ = hb_tx.send(AgentToServerMessage::Telemetry { role, lines });
                        }
                    }
                });

                let mut active_session: Option<(Uuid, tokio::sync::watch::Sender<bool>)> = None;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                        incoming = ws_rx.next() => {
                            let Some(Ok(msg)) = incoming else { break; };
                            if let Message::Text(txt) = msg {
                                if let Ok(srv_msg) = serde_json::from_str::<ServerToAgentMessage>(&txt) {
                                    match srv_msg {
                                        ServerToAgentMessage::Welcome { public_code, .. } => {
                                            tracing::info!(%public_code, "Connected to AgentDesk control channel");
                                        }
                                        ServerToAgentMessage::IncomingSession {
                                            session_id,
                                            technician_name,
                                            allow_control,
                                            relay_path,
                                            host_relay_token,
                                            ..
                                        } => {
                                            let accepted = if effective_auto_accept {
                                                true
                                            } else {
                                                tracing::warn!(
                                                    %technician_name,
                                                    allow_control,
                                                    "Solicitud de conexión entrante. Acepta en consola [s/N]:"
                                                );
                                                let mut line = String::new();
                                                let _ = std::io::stdin().read_line(&mut line);
                                                matches!(
                                                    line.trim().to_lowercase().as_str(),
                                                    "s" | "y" | "si" | "yes"
                                                )
                                            };

                                            let _ = ctrl_tx.send(AgentToServerMessage::SessionConsentResult {
                                                session_id,
                                                accepted,
                                                reason: if accepted {
                                                    None
                                                } else {
                                                    Some("rejected by local user".into())
                                                },
                                            });

                                            if accepted {
                                                if let Some((_, prev_stop)) = active_session.take() {
                                                    let _ = prev_stop.send(true);
                                                }
                                                let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
                                                active_session = Some((session_id, stop_tx));
                                                let relay_url = format!(
                                                    "{ws_base}{relay_path}?role=host&token={host_relay_token}"
                                                );
                                                tokio::spawn(async move {
                                                    if let Err(e) = run_host_session(
                                                        relay_url,
                                                        allow_control,
                                                        effective_fps,
                                                        effective_quality,
                                                        stop_rx,
                                                    )
                                                    .await
                                                    {
                                                        tracing::warn!(error = %e, "Host E2EE session ended");
                                                    }
                                                });
                                            }
                                        }
                                        ServerToAgentMessage::TerminateSession { session_id, reason } => {
                                            tracing::info!(%session_id, %reason, "Session terminated by server");
                                            if let Some((cur, stop)) = active_session.take() {
                                                if cur == session_id {
                                                    let _ = stop.send(true);
                                                }
                                            }
                                        }
                                        ServerToAgentMessage::RunDiagnostic { request_id, kind } => {
                                            let tx_rep = ctrl_tx.clone();
                                            tokio::task::spawn_blocking(move || {
                                                let (ok, data, shot, err) = run_diagnostic_sync(&kind);
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

                if let Some((_, stop)) = active_session.take() {
                    let _ = stop.send(true);
                }
                hb_task.abort();
                tx_task.abort();

                if cli.mode == AgentMode::Quicksupport || *shutdown_rx.borrow() {
                    break;
                }
                tracing::warn!("Disconnected from AgentDesk server, reconnecting in 5s...");
            }
            Err(e) => {
                if cli.mode == AgentMode::Quicksupport {
                    return Err(e.into());
                }
                tracing::warn!(error = %e, "Unable to connect to AgentDesk server, retrying in 5s...");
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn run_diagnostic_sync(
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
                Ok(jpg) => {
                    let b64 = B64.encode(jpg);
                    (true, serde_json::json!({ "captured": true }), Some(b64), None)
                }
                Err(e) => (false, serde_json::json!({}), None, Some(e.to_string())),
            }
        }
        _ => {
            let data = serde_json::json!({
                "inventory": agentdesk_rmm::collect_inventory(),
                "metrics": agentdesk_rmm::collect_metrics(),
                "resources": agentdesk_rmm::collect_hardware_resources(),
                "services": agentdesk_rmm::collect_services(),
                "events": agentdesk_rmm::collect_recent_events(),
            });
            (true, data, None, None)
        }
    }
}

struct PendingUpload {
    filename: String,
    expected_sha256: String,
    target: FileDropTarget,
    data: Vec<u8>,
}

async fn run_host_session(
    relay_url: String,
    allow_control: bool,
    initial_fps: u8,
    initial_quality: u8,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let t_sess = std::time::Instant::now();
    agentdesk_protocol::telemetry::tlog("host", "incoming session: connecting to relay");
    let mut tunnel = match RelayTunnel::connect(&relay_url, false).await {
        Ok(t) => t,
        Err(e) => {
            agentdesk_protocol::telemetry::tlog("host", format!("relay/E2EE connect FAILED after {} ms: {e}", t_sess.elapsed().as_millis()));
            return Err(e.into());
        }
    };
    agentdesk_protocol::telemetry::tlog("host", format!("relay + Noise E2EE ready in {} ms", t_sess.elapsed().as_millis()));
    let mut net_send = agentdesk_protocol::telemetry::Stat::default();
    let mut net_report = std::time::Instant::now();
    let mut net_udp_frames = 0u32;
    let _wallpaper_guard = WallpaperBlackoutGuard::activate_in_background();
    tracing::info!("Noise_XX E2EE session established with viewer");

    let monitors = enumerate_monitors();
    let _ = tunnel
        .send_packet(&DataPacket::Control(ControlMessage::MonitorList {
            monitors,
            active_index: 0,
        }))
        .await;

    let (pkt_tx, mut pkt_rx) = mpsc::channel::<DataPacket>(2);
    let (cursor_tx, mut cursor_rx) = mpsc::channel::<DataPacket>(4);
    let (audio_tx, mut audio_rx) = mpsc::channel::<DataPacket>(6);
    let mut abr_init = AbrController::new();
    // Native resolution, adaptive; the configured quality/fps are the starting point and fps cap.
    abr_init.set_policy(0, initial_quality.clamp(25, 95), initial_fps.clamp(1, 60), true);
    let abr = std::sync::Arc::new(std::sync::Mutex::new(abr_init));
    let monitor_idx = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let keyframe_req = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let codec_pref = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let audio_enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    agentdesk_capture::spawn_capture_worker(
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
    let mut injector = agentdesk_capture::RemoteInputInjector::new();
    if let Some(text) = injector.startup_notice() {
        let _ = tunnel
            .send_packet(&DataPacket::Control(ControlMessage::Chat { sender: "AgentDesk".into(), text }))
            .await;
    }
    // The hardware snapshot shells out to PowerShell/WMI (seconds, far longer on domain-joined
    // servers). It must never run inside the session loop: while it ran, video, input and the
    // whiteboard all stopped, and the viewer polls it every 10 s.
    let (hw_tx, mut hw_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let hw_busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut pending_print_jobs: std::collections::VecDeque<(Uuid, Option<String>)> =
        std::collections::VecDeque::new();
    let mut uploads: HashMap<Uuid, PendingUpload> = HashMap::new();
    let mut udp_ch: Option<P2pUdpChannel> = None;
    let mut p2p_ep_rx: Option<tokio::sync::oneshot::Receiver<Vec<String>>> = None;
    let mut p2p_ch_rx: Option<tokio::sync::oneshot::Receiver<P2pUdpChannel>> = None;
    let mut p2p_meta: Option<([u8; 16], [u8; 32])> = None;
    let mut udp_buf = vec![0u8; 2048];
    let mut ka_tick = tokio::time::interval(Duration::from_secs(1));
    // The viewer pings every 2 s. If nothing arrives for 30 s it is gone (window killed,
    // network dropped) and the relay keeps our socket open, so end the session here instead of
    // showing "sesión activa" forever.
    let mut last_peer_rx = std::time::Instant::now();
    let mut clip_tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_clip_seq = clipboard_sequence_number();
    let mut last_clip_text = read_clipboard_text().unwrap_or_default();

    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Terminate {
                        reason: "host stopped".into(),
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
            p2p_res = async {
                match p2p_ep_rx.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                p2p_ep_rx = None;
                // A failed P2P attempt drops the sender. Matching `Some(..)` used to leave the
                // receiver in place, the next loop polled it again and tokio panicked ("called
                // after complete"), killing the whole session task: frozen image, no input.
                let Ok(my_eps) = p2p_res else { continue };
                if let Some((session_nonce, p2p_key)) = p2p_meta {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::P2pOffer {
                        session_nonce,
                        p2p_key,
                        endpoints: my_eps,
                    })).await;
                }
            }
            p2p_res = async {
                match p2p_ch_rx.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                p2p_ch_rx = None;
                // A failed P2P attempt drops the sender. Matching `Some(..)` used to leave the
                // receiver in place, the next loop polled it again and tokio panicked ("called
                // after complete"), killing the whole session task: frozen image, no input.
                let Ok(ch) = p2p_res else { continue };
                let addr_str = ch.peer_addr().to_string();
                tracing::info!(peer = %addr_str, "Direct P2P UDP channel active");
                agentdesk_protocol::telemetry::tlog("host", format!("P2P UDP ready with {addr_str} after {} ms", t_sess.elapsed().as_millis()));
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
                    last_peer_rx = std::time::Instant::now();
                    if let Some(pkt) = udp_ch.as_mut().and_then(|c| c.try_feed_datagram(&udp_buf[..n])) {
                        match pkt {
                            DataPacket::Input(ev) => {
                                if allow_control {
                                    if let Some(text) = injector.inject(&ev) {
                                        let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Chat { sender: "AgentDesk".into(), text })).await;
                                    }
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
                if last_peer_rx.elapsed() > std::time::Duration::from_secs(30) {
                    agentdesk_capture::hostdiag::log("session ended: no packets from the viewer for 30 s");
                    break;
                }
                if let Some(ch) = udp_ch.as_mut() {
                    ch.maybe_send_keepalive().await;
                    if !ch.is_alive() {
                        agentdesk_protocol::telemetry::tlog("host", "P2P UDP went silent; back to relay");
                        udp_ch = None;
                    }
                }
            }
            Some(cur_pkt) = cursor_rx.recv() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(15), send_adaptive_packet(&mut tunnel, &mut udp_ch, &cur_pkt)).await, Ok(Ok(()))) { // viewer not reading for 15 s = gone
                    break;
                }
            }
            Some(audio_pkt) = audio_rx.recv() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(15), send_adaptive_packet(&mut tunnel, &mut udp_ch, &audio_pkt)).await, Ok(Ok(()))) { // viewer not reading for 15 s = gone
                    break;
                }
            }
            Some(pkt) = pkt_rx.recv() => {
                let t_send = std::time::Instant::now();
                let via_udp = udp_ch.as_ref().map(|c| c.is_alive()).unwrap_or(false);
                if !matches!(tokio::time::timeout(Duration::from_secs(15), send_adaptive_packet(&mut tunnel, &mut udp_ch, &pkt)).await, Ok(Ok(()))) { // viewer not reading for 15 s = gone
                    agentdesk_protocol::telemetry::tlog("host", "video send failed; ending session");
                    break;
                }
                net_send.add(t_send.elapsed().as_secs_f64() * 1000.0);
                if via_udp {
                    net_udp_frames += 1;
                }
                if net_report.elapsed() >= Duration::from_secs(2) {
                    let st = net_send.take();
                    agentdesk_protocol::telemetry::tlog("host", format!(
                        "net: {} video frames, {} via P2P UDP, socket send avg {:.1} ms max {:.1} ms (slow sends here also delay mouse/keyboard)",
                        st.n, net_udp_frames, st.avg(), st.max
                    ));
                    net_udp_frames = 0;
                    net_report = std::time::Instant::now();
                }
            }
            Some(resources_json) = hw_rx.recv() => {
                let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::HardwareResourcesSnapshot { resources_json })).await;
            }
            incoming = tunnel.recv_packet() => {
                last_peer_rx = std::time::Instant::now();
                match incoming {
                    Ok(DataPacket::Input(ev)) => {
                        if allow_control {
                            if let Some(text) = injector.inject(&ev) {
                                let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Chat { sender: "AgentDesk".into(), text })).await;
                            }
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
                        injector.set_monitor(index);
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
                        if !hw_busy.swap(true, std::sync::atomic::Ordering::AcqRel) {
                            let (hw_done_tx, busy) = (hw_tx.clone(), hw_busy.clone());
                            tokio::task::spawn_blocking(move || {
                                let hw = agentdesk_rmm::collect_hardware_resources();
                                busy.store(false, std::sync::atomic::Ordering::Release);
                                let _ = hw_done_tx.send(hw.to_string());
                            });
                        }
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
                                    if !hw_busy.swap(true, std::sync::atomic::Ordering::AcqRel) {
                                        let (hw_done_tx, busy) = (hw_tx.clone(), hw_busy.clone());
                                        tokio::task::spawn_blocking(move || {
                                            let hw = agentdesk_rmm::collect_hardware_resources();
                                            busy.store(false, std::sync::atomic::Ordering::Release);
                                            let _ = hw_done_tx.send(hw.to_string());
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileOffer { transfer_id, filename, size_bytes, sha256 })) => {
                        if size_bytes <= 64 * 1024 * 1024 {
                            uploads.insert(transfer_id, PendingUpload {
                                filename,
                                expected_sha256: sha256,
                                target: FileDropTarget::Downloads,
                                data: Vec::with_capacity(size_bytes as usize),
                            });
                        }
                    }
                    Ok(DataPacket::Control(ControlMessage::FileOfferEx { transfer_id, filename, size_bytes, sha256, target })) => {
                        if size_bytes <= 64 * 1024 * 1024 {
                            uploads.insert(transfer_id, PendingUpload {
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
                                    (false, format!("SHA-256 no coincide para {}", finished.filename))
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
                                        Err(e) => (false, format!("Error escribiendo archivo: {e}")),
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

    // A peer that stopped reading would block the close handshake forever.
    let _ = tokio::time::timeout(Duration::from_secs(3), tunnel.close()).await;
    Ok(())
}
