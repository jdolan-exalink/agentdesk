use agentdesk_capture::win::{
    clipboard_sequence_number, current_process_private_mb, enumerate_monitors, inject_input_event,
    inject_special_combo, read_clipboard_files, read_clipboard_text, write_clipboard_files,
    write_clipboard_text, WallpaperBlackoutGuard, WindowsCapturer,
};
use agentdesk_capture::{
    encode_frame, enumerate_printers, print_file_to_printer, spawn_audio_capture_worker,
    spawn_capture_worker, AbrController, ScreenCapturer, WhiteboardOverlay,
};
use agentdesk_identity::DeviceIdentity;
use agentdesk_protocol::{
    AgentToServerMessage, ControlMessage, DataPacket, FileDropTarget, FrameEncoding,
    QuickRegisterRequest, QuickRegisterResponse, RotatePasswordRequest, ServerToAgentMessage,
};
use agentdesk_transport::{
    connect_ws, enumerate_local_mac_and_ips, send_adaptive_packet, send_wol_magic_packets,
    start_host_p2p_handshake, store_received_file_for_target, verify_sha256_hex, P2pUdpChannel,
    RelayTunnel,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc::Sender, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn desktop_identity_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("quicksupport_identity.dpapi")
}

pub enum HostEvent {
    Status(String),
    Registered {
        device_id: Uuid,
        connection_id: String,
        device_token: String,
        password: String,
    },
    PasswordRotated(String),
    IncomingStarted {
        caller_name: String,
        allow_control: bool,
        stop_tx: watch::Sender<bool>,
        chat_tx: mpsc::UnboundedSender<String>,
    },
    IncomingChat {
        sender: String,
        text: String,
    },
    IncomingToast(String),
    IncomingEnded,
}

pub fn start_embedded_host(
    server_url: String,
    initial_pw: String,
    owner_email: Option<String>,
    tx: Sender<HostEvent>,
) -> watch::Sender<bool> {
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let server = server_url.trim_end_matches('/').to_string();
    let _ = tx.send(HostEvent::Status(format!("Conectando a {server}…")));

    runtime().spawn(async move {
        let identity = match DeviceIdentity::load_or_create(&desktop_identity_path()) {
            Ok(id) => id,
            Err(e) => {
                let _ = tx.send(HostEvent::Status(format!("Error identidad DPAPI: {e}")));
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
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap_or_default();

        let access_pw = initial_pw;

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
                        let _ = tx.send(HostEvent::Status(format!("Error registro: {e}")));
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                            _ = stop_rx.changed() => break,
                        }
                    }
                },
                Ok(r) => {
                    let _ = tx.send(HostEvent::Status(format!("Registro HTTP {}", r.status())));
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                        _ = stop_rx.changed() => break,
                    }
                }
                Err(_) => {
                    let _ = tx.send(HostEvent::Status(format!(
                        "Sin conexión con {server} (reintentando…)"
                    )));
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                        _ = stop_rx.changed() => break,
                    }
                }
            };

            let _ = tx.send(HostEvent::Registered {
                device_id: reg.device_id,
                connection_id: reg.connection_id.clone(),
                device_token: reg.device_token.clone(),
                password: access_pw.clone(),
            });

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

            let ws_stream = match connect_ws(&ws_url).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    let _ = tx.send(HostEvent::Status(format!(
                        "Reconectando canal de control ({e})…"
                    )));
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(4)) => continue,
                        _ = stop_rx.changed() => break,
                    }
                }
            };

            let (mut ws_tx, mut ws_rx) = ws_stream.split();
            let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<AgentToServerMessage>();

            let writer = tokio::spawn(async move {
                while let Some(msg) = ctrl_rx.recv().await {
                    if let Ok(txt) = serde_json::to_string(&msg) {
                        if ws_tx.send(Message::Text(txt.into())).await.is_err() {
                            break;
                        }
                    }
                }
            });

            let hb_tx = ctrl_tx.clone();
            let hb_stop = stop_rx.clone();
            let hb_task = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(15));
                loop {
                    if *hb_stop.borrow() {
                        break;
                    }
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

            let mut active_incoming: Option<(Uuid, watch::Sender<bool>)> = None;

            loop {
                tokio::select! {
                    _ = stop_rx.changed() => {
                        if *stop_rx.borrow() {
                            break;
                        }
                    }
                    incoming = ws_rx.next() => {
                        let Some(Ok(msg)) = incoming else { break; };
                        let Message::Text(txt) = msg else { continue; };
                        let Ok(srv_msg) = serde_json::from_str::<ServerToAgentMessage>(&txt) else { continue; };
                        match srv_msg {
                            ServerToAgentMessage::Welcome { public_code, .. } => {
                                let _ = tx.send(HostEvent::Status(format!(
                                    "En línea · Listo para recibir (ID: {public_code})"
                                )));
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
                                if let Some((_, prev_stop)) = active_incoming.take() {
                                    let _ = prev_stop.send(true);
                                }
                                let (sess_stop_tx, sess_stop_rx) = watch::channel(false);
                                let (chat_tx, chat_rx) = mpsc::unbounded_channel::<String>();
                                active_incoming = Some((session_id, sess_stop_tx.clone()));
                                let _ = tx.send(HostEvent::IncomingStarted {
                                    caller_name: technician_name.clone(),
                                    allow_control,
                                    stop_tx: sess_stop_tx,
                                    chat_tx,
                                });
                                let relay_url = format!(
                                    "{ws_base}{relay_path}?role=host&token={host_relay_token}"
                                );
                                let tx_sess = tx.clone();
                                let local_label = host_name.clone();
                                tokio::spawn(async move {
                                    let _ = run_incoming_host_session(
                                        relay_url,
                                        allow_control,
                                        local_label,
                                        tx_sess.clone(),
                                        sess_stop_rx,
                                        chat_rx,
                                    )
                                    .await;
                                    let _ = tx_sess.send(HostEvent::IncomingEnded);
                                });
                            }
                            ServerToAgentMessage::TerminateSession { session_id, .. } => {
                                if let Some((cur, stop)) = active_incoming.take() {
                                    if cur == session_id {
                                        let _ = stop.send(true);
                                    }
                                }
                                let _ = tx.send(HostEvent::IncomingEnded);
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
                            ServerToAgentMessage::RunCommand { request_id, shell, command, timeout_secs } => {
                                let tx_rep = ctrl_tx.clone();
                                tokio::task::spawn_blocking(move || {
                                    let res = agentdesk_rmm::execute_shell_command(&shell, &command, timeout_secs);
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
                            ServerToAgentMessage::SendWakeOnLan { request_id, mac_addresses, broadcast_ips } => {
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

            if let Some((_, stop)) = active_incoming.take() {
                let _ = stop.send(true);
            }
            hb_task.abort();
            writer.abort();
            if *stop_rx.borrow() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = stop_rx.changed() => break,
            }
        }
    });

    stop_tx
}

pub fn rotate_password_async(
    server_url: String,
    device_id: Uuid,
    device_token: String,
    new_password: String,
    tx: Sender<HostEvent>,
) {
    let base = server_url.trim_end_matches('/').to_string();
    runtime().spawn(async move {
        let http = reqwest::Client::new();
        let res = http
            .post(format!("{base}/api/v1/clients/password"))
            .header("x-device-token", device_token)
            .json(&RotatePasswordRequest {
                device_id,
                new_password: new_password.clone(),
            })
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                let _ = tx.send(HostEvent::PasswordRotated(new_password));
            }
            Ok(r) => {
                let _ = tx.send(HostEvent::Status(format!(
                    "No se pudo rotar la contraseña (HTTP {})",
                    r.status()
                )));
            }
            Err(e) => {
                let _ = tx.send(HostEvent::Status(format!("Error rotando contraseña: {e}")));
            }
        }
    });
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
    local_label: String,
    host_ev_tx: Sender<HostEvent>,
    mut stop_rx: watch::Receiver<bool>,
    mut chat_rx: mpsc::UnboundedReceiver<String>,
) -> anyhow::Result<()> {
    let mut tunnel = RelayTunnel::connect(&relay_url, false).await?;
    let _wallpaper_guard = WallpaperBlackoutGuard::activate();

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
            Some(chat_msg) = chat_rx.recv() => {
                let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::Chat {
                    sender: local_label.clone(),
                    text: chat_msg,
                })).await;
            }
            _ = clip_tick.tick() => {
                let seq = clipboard_sequence_number();
                if seq != last_clip_seq {
                    last_clip_seq = seq;
                    let files = read_clipboard_files();
                    if !files.is_empty() {
                        if let Ok(n) = tunnel.send_files_to_target(&files, FileDropTarget::Clipboard).await {
                            if n > 0 {
                                let _ = host_ev_tx.send(HostEvent::IncomingToast(format!("Archivo(s) copiados al portapapeles remoto ({n})")));
                            }
                        }
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
                    Ok(DataPacket::Control(ControlMessage::Chat { sender, text })) => {
                        let _ = host_ev_tx.send(HostEvent::IncomingChat { sender, text });
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
                        let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::HardwareResourcesSnapshot {
                            resources_json: hw.to_string(),
                        })).await;
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
                                                    Ok(m) => {
                                                        let _ = host_ev_tx.send(HostEvent::IncomingToast(m.clone()));
                                                        (true, m)
                                                    }
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
                                                let note = format!("Recibido en {}", dest_path.display());
                                                let _ = host_ev_tx.send(HostEvent::IncomingToast(note.clone()));
                                                (true, note)
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
                    Ok(DataPacket::Control(ControlMessage::Terminate { .. })) | Err(_) => break,
                    _ => {}
                }
            }
            Some(pkt) = pkt_rx.recv() => {
                if send_adaptive_packet(&mut tunnel, &mut udp_ch, &pkt).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = tunnel.close().await;
    Ok(())
}
