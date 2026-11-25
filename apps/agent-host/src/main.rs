mod config;

use agentdesk_capture::win::{current_process_private_mb, inject_input_event, WindowsCapturer};
use agentdesk_capture::{encode_frame, ScreenCapturer};
use agentdesk_identity::DeviceIdentity;
use agentdesk_protocol::{
    AgentToServerMessage, ControlMessage, DataPacket, EnrollDeviceRequest, EnrollDeviceResponse,
    FrameEncoding, ServerToAgentMessage, VideoFrame,
};
use agentdesk_transport::RelayTunnel;
use clap::{Parser, ValueEnum};
use config::{normalize_server_url, HostConfig};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum AgentMode {
    Host,
    Quicksupport,
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

    #[arg(long, value_enum, default_value_t = AgentMode::Host)]
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

    let (device_id, public_code, device_token) = if let Some((ref mut cfg, ref cfg_path)) =
        host_cfg_opt
    {
        if cli.enroll_token.is_none() {
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
            let http = reqwest::Client::new();
            let resp: EnrollDeviceResponse = http
                .post(format!("{server}/api/v1/devices/enroll"))
                .json(&EnrollDeviceRequest {
                    enrollment_token: cli.enroll_token.clone(),
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
            (resp.device_id, resp.public_code, resp.device_token)
        }
    } else {
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
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        (resp.device_id, resp.public_code, resp.device_token)
    };

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
        match connect_async(&agent_ws_url).await {
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

async fn run_host_session(
    relay_url: String,
    allow_control: bool,
    initial_fps: u8,
    initial_quality: u8,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut tunnel = RelayTunnel::connect(&relay_url, false).await?;
    tracing::info!("Noise_XX E2EE session established with viewer");

    let (pkt_tx, mut pkt_rx) = mpsc::channel::<DataPacket>(4);
    let quality = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(initial_quality));
    let fps = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(initial_fps));

    let q_cap = quality.clone();
    let f_cap = fps.clone();
    let cap_stop = stop_rx.clone();

    std::thread::spawn(move || {
        let mut cap = WindowsCapturer::new();
        let mut seq = 0u64;
        while !*cap_stop.borrow() {
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

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Terminate {
                        reason: "host stopped".into(),
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
