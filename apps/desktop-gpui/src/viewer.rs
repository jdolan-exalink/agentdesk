use crate::host::{now_ms, runtime};
use crate::theme::{render_hardware_resources, Palette, DARK, LIGHT};
use agentdesk_capture::win::{
    clipboard_sequence_number, read_clipboard_files, read_clipboard_text, write_clipboard_files,
    write_clipboard_text,
};
use agentdesk_capture::{
    enumerate_printers, print_file_to_printer, AudioPlayer, ViewerFrameCanvas,
};
use agentdesk_protocol::{
    ControlMessage, CursorShape, DataPacket, FileDropTarget, InputEvent, MonitorInfo, MouseBtn,
    PrinterInfo, VideoCodecPreference, VideoFrame, WhiteboardStroke, WhiteboardTool,
};
use agentdesk_transport::{
    bind_p2p_udp_socket, discover_udp_candidates, extract_stun_addr_from_ws_url,
    connect_ws, send_adaptive_packet, spawn_p2p_punch, store_received_file_for_target,
    verify_sha256_hex, P2pUdpChannel, RelayTunnel,
};
use gpui_kit::base::StyledExt;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ReconnectTarget {
    Direct {
        connection_id: String,
        hostname: String,
        reconnect_token: Option<String>,
        allow_control: bool,
    },
    Managed {
        device_id: Uuid,
        label: String,
        allow_control: bool,
    },
}

impl ReconnectTarget {
    pub fn display_name(&self) -> &str {
        match self {
            Self::Direct { hostname, .. } => hostname,
            Self::Managed { label, .. } => label,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ViewerEndedNotice {
    pub reason: String,
    pub reboot_pending: bool,
    pub target: Option<ReconnectTarget>,
}

pub struct ViewerLaunch {
    pub session_id: Uuid,
    pub ws_url: String,
    pub allow_control: bool,
    pub label: String,
    pub target: Option<ReconnectTarget>,
    pub server_url: String,
    pub auth_token: Option<String>,
    pub dark: bool,
    pub initial_panel: Option<String>,
    pub on_ended: Sender<ViewerEndedNotice>,
}

enum TunnelEvent {
    Frame {
        width: u32,
        height: u32,
        bgra: Vec<u8>,
        kbps: f32,
    },
    Rtt(u64),
    Chat(String, String),
    Monitors {
        monitors: Vec<MonitorInfo>,
        active_index: usize,
    },
    Clipboard(String),
    FileAck(String),
    Resources(serde_json::Value),
    RebootPending(u64, String),
    TransportMode(bool, String),
    Cursor(f32, f32, bool, CursorShape),
    VideoCodecActive(String, bool),
    Printers(Vec<PrinterInfo>),
    PrintResult(bool, String),
    /// Connection progress while the status line still says "Negociando…".
    Stage(String),
    Ended(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    None,
    Resources,
    Chat,
    Files,
    Print,
}

pub struct RemoteSession {
    opened_at: Instant,
    session_id: Uuid,
    label: SharedString,
    allow_control: bool,
    server_url: String,
    auth_token: Option<String>,
    target: Option<ReconnectTarget>,
    dark: bool,

    out_tx: tokio_mpsc::UnboundedSender<DataPacket>,
    file_tx: tokio_mpsc::UnboundedSender<(Vec<PathBuf>, FileDropTarget)>,
    rx: Receiver<TunnelEvent>,
    on_ended: Sender<ViewerEndedNotice>,

    current_frame: Option<Arc<RenderImage>>,
    frame_dims: (u32, u32),
    frame_count: u64,
    fps_window_start: Instant,
    fps: f32,
    kbps: f32,
    rtt_ms: u64,
    jpeg_quality: u8,
    max_fps: u8,
    quality_preset: u8,
    transport_p2p: bool,
    transport_label: String,
    remote_cursor: Option<(f32, f32, bool, CursorShape)>,
    last_mouse_sent: Instant,
    monitors: Vec<MonitorInfo>,
    active_monitor: usize,

    audio_enabled: bool,
    audio_mute_flag: Arc<std::sync::atomic::AtomicBool>,
    codec_pref: VideoCodecPreference,
    active_codec_label: String,
    active_codec_hw: bool,
    last_bgra_frame: Option<(u32, u32, Vec<u8>)>,

    whiteboard_active: bool,
    wb_tool: WhiteboardTool,
    wb_color: u32,
    wb_thickness: f32,
    wb_strokes: Vec<WhiteboardStroke>,
    wb_drawing: Option<WhiteboardStroke>,
    wb_next_id: u64,

    local_printers: Vec<PrinterInfo>,
    local_printer_idx: usize,
    remote_printers: Vec<PrinterInfo>,
    remote_printer_idx: usize,
    print_file_input: Entity<gpui_kit::component::input::InputState>,
    print_status: String,

    hw_resources: Option<serde_json::Value>,
    side_panel: SidePanel,
    confirm_reboot: bool,
    users_popup: bool,
    ai_summary_open: bool,
    ai_summary_busy: bool,
    ai_summary_text: String,
    chat_log: Vec<String>,
    chat_input: Entity<gpui_kit::component::input::InputState>,
    file_path_input: Entity<gpui_kit::component::input::InputState>,
    file_status: String,
    status_line: String,

    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    focus_handle: FocusHandle,
    last_mouse_norm: (f32, f32),
    manual_disconnect: bool,
    /// Terminate already sent to host + server (idempotent session teardown).
    terminate_sent: bool,
    notified_end: bool,
    /// Estado de la barra flotante (solo presentación).
    ui: ToolbarUi,
}

/// Estado de presentación de la barra flotante del visor: popover abierto,
/// última actividad del mouse cerca del borde superior (auto-ocultado) y si la
/// sesión se concedió con control (para el selector control / solo ver).
struct ToolbarUi {
    pop: Option<&'static str>,
    last_activity: Instant,
    granted_control: bool,
}

impl ToolbarUi {
    fn new(granted_control: bool) -> Self {
        Self { pop: None, last_activity: Instant::now(), granted_control }
    }
}

impl RemoteSession {
    pub fn new(launch: ViewerLaunch, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel::<DataPacket>();
        let (file_tx, mut file_rx) = tokio_mpsc::unbounded_channel::<(Vec<PathBuf>, FileDropTarget)>();
        let (ev_tx, ev_rx) = mpsc::channel::<TunnelEvent>();
        let ws_url = launch.ws_url.clone();
        let audio_mute_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let audio_mute_bg = audio_mute_flag.clone();

        let (dec_tx, dec_rx) = std::sync::mpsc::sync_channel::<VideoFrame>(4);
        // Telemetry shared between the network task and the decoder thread.
        let tel_queue_drops = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let tel_rtt_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tel_transport = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tel_queue_drops_dec, tel_rtt_dec, tel_transport_dec) =
            (tel_queue_drops.clone(), tel_rtt_ms.clone(), tel_transport.clone());
        let ev_tx_dec = ev_tx.clone();
        let ack_tx_dec = out_tx.clone();
        std::thread::spawn(move || {
            let mut canvas = ViewerFrameCanvas::new();
            use agentdesk_protocol::telemetry::{tlog, Stat};
            let mut st_report = Instant::now();
            let (mut st_rx, mut st_dec_n, mut st_skip, mut st_kf, mut st_err) = (0u32, 0u32, 0u32, 0u32, 0u32);
            let mut st_bytes = 0u64;
            let mut st_dec = Stat::default();
            let mut st_age = Stat::default();
            let mut st_codec = String::new();
            let mut st_first = true;
            let opened = Instant::now();
            // Create the H.264 decoder now, while the tunnel is still being set up, instead of
            // on the first frame.
            canvas.prewarm_h264();
            let mut bytes_in_window = 0usize;
            let mut bw_start = Instant::now();
            while let Ok(vf) = dec_rx.recv() {
                bytes_in_window += vf.payload.len();
                let secs = bw_start.elapsed().as_secs_f32().max(0.1);
                let kbps = (bytes_in_window as f32 / 1024.0) / secs;
                if secs >= 1.5 {
                    bytes_in_window = 0;
                    bw_start = Instant::now();
                }
                st_rx += 1;
                st_bytes += vf.payload.len() as u64;
                st_codec = format!("{:?} {}x{}", vf.encoding, vf.width, vf.height);
                // Host wall clock vs ours: only meaningful if both clocks are NTP-synced.
                let now_wall = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                st_age.add((now_wall - vf.timestamp_ms as i64) as f64);
                if st_first {
                    st_first = false;
                    tlog("viewer", format!("first video frame received {} ms after viewer start: {st_codec}, {} kB", opened.elapsed().as_millis(), vf.payload.len() / 1024));
                }
                if st_report.elapsed() >= Duration::from_secs(2) {
                    let secs = st_report.elapsed().as_secs_f32();
                    let dec = st_dec.take();
                    let age = st_age.take();
                    tlog("viewer", format!(
                        "video: recv {:.1} fps {:.0} kB/s decoded {:.1} fps (avg {:.1} ms max {:.1} ms) | skipped after loss {} kf_req {} decode_err {} dropped_in_queue {} | rtt {} ms via {} | host->viewer age avg {:.0} ms max {:.0} ms (clock-skew sensitive) | {}",
                        st_rx as f32 / secs,
                        st_bytes as f32 / secs / 1024.0,
                        st_dec_n as f32 / secs,
                        dec.avg(), dec.max,
                        st_skip, st_kf, st_err,
                        tel_queue_drops_dec.swap(0, std::sync::atomic::Ordering::Relaxed),
                        tel_rtt_dec.load(std::sync::atomic::Ordering::Relaxed),
                        if tel_transport_dec.load(std::sync::atomic::Ordering::Relaxed) { "P2P UDP" } else { "relay" },
                        age.avg(), age.max,
                        st_codec,
                    ));
                    st_report = Instant::now();
                    st_rx = 0; st_dec_n = 0; st_skip = 0; st_kf = 0; st_err = 0; st_bytes = 0;
                }
                let admission = canvas.admit(vf.seq, vf.encoding, &vf.payload);
                if admission.request_keyframe {
                    st_kf += 1;
                    let _ = ack_tx_dec.send(DataPacket::Control(ControlMessage::RequestKeyframe));
                }
                if !admission.decode {
                    st_skip += 1;
                    // Lost frame: keep the last good image instead of decoding grey smears.
                    // Still ACK so the host's send window keeps moving.
                    let _ = ack_tx_dec.send(DataPacket::Control(ControlMessage::FrameAck {
                        seq: vf.seq,
                        decode_ms: 0,
                    }));
                    continue;
                }
                let t_dec = Instant::now();
                match canvas.apply_frame(vf.encoding, vf.width, vf.height, &vf.payload) {
                    Ok((w, h, bgra)) => {
                        let decode_ms = t_dec.elapsed().as_millis().min(u16::MAX as u128) as u16;
                        st_dec_n += 1;
                        st_dec.add(t_dec.elapsed().as_secs_f64() * 1000.0);
                        let _ = ack_tx_dec.send(DataPacket::Control(ControlMessage::FrameAck {
                            seq: vf.seq,
                            decode_ms,
                        }));
                        let _ = ev_tx_dec.send(TunnelEvent::Frame {
                            width: w,
                            height: h,
                            bgra,
                            kbps,
                        });
                    }
                    Err(e) => {
                        st_err += 1;
                        tlog("viewer", format!("decode error ({:?}): {e}", vf.encoding));
                        let _ = ack_tx_dec.send(DataPacket::Control(ControlMessage::RequestKeyframe));
                    }
                }
            }
        });

        let connect_started = Instant::now();
        runtime().spawn(async move {
            let stage = |txt: &str| {
                let secs = connect_started.elapsed().as_secs_f32();
                agentdesk_protocol::telemetry::tlog("viewer", format!("stage: {txt} at {secs:.2} s"));
                let _ = ev_tx.send(TunnelEvent::Stage(format!("Negociando · {txt} ({secs:.1} s)")));
            };
            stage("conectando al relay");
            let ws = match connect_ws(&ws_url).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    agentdesk_protocol::telemetry::tlog("viewer", format!("relay connect FAILED: {e}"));
                    let _ = ev_tx.send(TunnelEvent::Ended(format!("Error Relay: {e}")));
                    return;
                }
            };
            stage("esperando al equipo remoto");
            let mut tunnel = match RelayTunnel::handshake_over_ws(ws, true).await {
                Ok(t) => t,
                Err(e) => {
                    agentdesk_protocol::telemetry::tlog("viewer", format!("Noise handshake FAILED: {e}"));
                    let _ = ev_tx.send(TunnelEvent::Ended(format!("Error E2EE: {e}")));
                    return;
                }
            };
            stage("túnel cifrado listo, esperando la primera imagen");
            let audio_player = AudioPlayer::new();

            tel_transport.store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = ev_tx.send(TunnelEvent::TransportMode(false, "☁ Relay WSS".into()));
            let _ = tunnel
                .send_packet(&DataPacket::Control(ControlMessage::RequestHardwareResources))
                .await;

            // Level 3: Bind UDP socket and start parallel LAN + STUN discovery for Direct P2P upgrade.
            let mut session_nonce = [0u8; 16];
            let mut p2p_key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut session_nonce);
            rand::thread_rng().fill_bytes(&mut p2p_key);

            let (ep_tx, ep_rx) = tokio::sync::oneshot::channel();
            let ws_for_stun = ws_url.clone();
            tokio::spawn(async move {
                if let Ok(sock) = bind_p2p_udp_socket() {
                    let stun = extract_stun_addr_from_ws_url(&ws_for_stun);
                    let eps = discover_udp_candidates(&sock, stun.as_deref()).await;
                    let _ = ep_tx.send((sock, eps));
                }
            });

            let mut p2p_sock: Option<std::sync::Arc<tokio::net::UdpSocket>> = None;
            let mut p2p_ep_rx: Option<tokio::sync::oneshot::Receiver<(std::sync::Arc<tokio::net::UdpSocket>, Vec<String>)>> = Some(ep_rx);
            let mut p2p_ch_rx: Option<tokio::sync::oneshot::Receiver<P2pUdpChannel>> = None;
            let mut udp_ch: Option<P2pUdpChannel> = None;
            let mut udp_buf = vec![0u8; 2048];
            let mut ka_interval = tokio::time::interval(Duration::from_secs(1));

            let mut ping_interval = tokio::time::interval(Duration::from_secs(2));
            let mut clip_tick = tokio::time::interval(Duration::from_millis(250));
            let mut last_clip_seq = clipboard_sequence_number();
            let mut last_clip_text = read_clipboard_text().unwrap_or_default();
            let mut incoming_uploads: std::collections::HashMap<Uuid, (String, String, FileDropTarget, Vec<u8>)> =
                std::collections::HashMap::new();
            let mut tick_n = 0u32;
            let mut last_vf_seq = 0u64;
            let mut last_kf_req = Instant::now() - Duration::from_secs(2);

            loop {
                tokio::select! {
                    biased;
                    Some(pkt) = out_rx.recv() => {
                        let is_term = matches!(pkt, DataPacket::Control(ControlMessage::Terminate { .. }));
                        let was_udp = udp_ch.is_some();
                        if send_adaptive_packet(&mut tunnel, &mut udp_ch, &pkt).await.is_err() || is_term {
                            break;
                        }
                        if was_udp && udp_ch.is_none() {
                            tel_transport.store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = ev_tx.send(TunnelEvent::TransportMode(false, "☁ Relay WSS".into()));
                        }
                    }
                    Some((paths, target)) = file_rx.recv() => {
                        match tunnel.send_files_to_target(&paths, target).await {
                            Ok(n) if n > 0 => {
                                let dest_txt = match target {
                                    FileDropTarget::Desktop => "al Escritorio remoto",
                                    FileDropTarget::Clipboard => "al portapapeles remoto",
                                    FileDropTarget::RemotePrint => "a la cola de impresión remota",
                                    FileDropTarget::Downloads => "a Descargas remotas",
                                };
                                let _ = ev_tx.send(TunnelEvent::FileAck(format!("Enviados {n} archivo(s) {dest_txt}")));
                            }
                            Err(e) => {
                                let _ = ev_tx.send(TunnelEvent::FileAck(format!("Error enviando archivo: {e}")));
                            }
                            _ => {}
                        }
                    }
                    _ = clip_tick.tick() => {
                        let seq = clipboard_sequence_number();
                        if seq != last_clip_seq {
                            last_clip_seq = seq;
                            let files = read_clipboard_files();
                            if !files.is_empty() {
                                if let Ok(n) = tunnel.send_files_to_target(&files, FileDropTarget::Clipboard).await {
                                    if n > 0 {
                                        let _ = ev_tx.send(TunnelEvent::Clipboard(format!("{n} archivo(s) listos para pegar")));
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
                        let Ok((sock, eps)) = p2p_res else { continue };
                        p2p_sock = Some(sock);
                        let _ = tunnel
                            .send_packet(&DataPacket::Control(ControlMessage::P2pOffer {
                                session_nonce,
                                p2p_key,
                                endpoints: eps,
                            }))
                            .await;
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
                        let peer_label = format!("⚡ P2P Directo ({})", ch.peer_addr().ip());
                        udp_ch = Some(ch);
                        tel_transport.store(true, std::sync::atomic::Ordering::Relaxed);
                        agentdesk_protocol::telemetry::tlog("viewer", format!("transport: P2P UDP {peer_label}"));
                        let _ = ev_tx.send(TunnelEvent::TransportMode(true, peer_label));
                        let _ = tunnel
                            .send_packet(&DataPacket::Control(ControlMessage::RequestKeyframe))
                            .await;
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
                                    DataPacket::VideoFrame(vf) => {
                                        if vf.seq > last_vf_seq + 1
                                            && last_vf_seq > 0
                                            && last_kf_req.elapsed() > Duration::from_millis(400)
                                        {
                                            last_kf_req = Instant::now();
                                            let _ = tunnel
                                                .send_packet(&DataPacket::Control(ControlMessage::RequestKeyframe))
                                                .await;
                                        }
                                        last_vf_seq = last_vf_seq.max(vf.seq);
                                        if dec_tx.try_send(vf).is_err() {
                                    tel_queue_drops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                                    }
                                    DataPacket::Audio(ap) => {
                                        audio_player
                                            .set_muted(audio_mute_bg.load(std::sync::atomic::Ordering::Relaxed));
                                        audio_player.push(ap);
                                    }
                                    DataPacket::Control(ControlMessage::CursorUpdate { x_norm, y_norm, visible, shape }) => {
                                        let _ = ev_tx.send(TunnelEvent::Cursor(x_norm, y_norm, visible, shape));
                                    }
                                    DataPacket::Control(ControlMessage::VideoCodecActive { codec_name, hardware }) => {
                                        let _ = ev_tx.send(TunnelEvent::VideoCodecActive(codec_name, hardware));
                                    }
                                    DataPacket::Control(ControlMessage::Pong { ts_ms }) => {
                                        let rtt_v = now_ms().saturating_sub(ts_ms);
                                tel_rtt_ms.store(rtt_v, std::sync::atomic::Ordering::Relaxed);
                                let _ = ev_tx.send(TunnelEvent::Rtt(rtt_v));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ = ka_interval.tick() => {
                        if let Some(ch) = udp_ch.as_mut() {
                            ch.maybe_send_keepalive().await;
                            if !ch.is_alive() {
                                udp_ch = None;
                                tel_transport.store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = ev_tx.send(TunnelEvent::TransportMode(false, "☁ Relay WSS".into()));
                                let _ = tunnel
                                    .send_packet(&DataPacket::Control(ControlMessage::RequestKeyframe))
                                    .await;
                            }
                        }
                    }
                    _ = ping_interval.tick() => {
                        let ts = now_ms();
                        let _ = send_adaptive_packet(&mut tunnel, &mut udp_ch, &DataPacket::Control(ControlMessage::Ping { ts_ms: ts })).await;
                        if tick_n % 5 == 0 {
                            let _ = tunnel.send_packet(&DataPacket::Control(ControlMessage::RequestHardwareResources)).await;
                        }
                        tick_n = tick_n.wrapping_add(1);
                    }
                    incoming = tunnel.recv_packet() => {
                        match incoming {
                            Ok(DataPacket::VideoFrame(vf)) => {
                                last_vf_seq = last_vf_seq.max(vf.seq);
                                if dec_tx.try_send(vf).is_err() {
                                    tel_queue_drops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            Ok(DataPacket::Audio(ap)) => {
                                audio_player
                                    .set_muted(audio_mute_bg.load(std::sync::atomic::Ordering::Relaxed));
                                audio_player.push(ap);
                            }
                            Ok(DataPacket::Control(ControlMessage::CursorUpdate { x_norm, y_norm, visible, shape })) => {
                                let _ = ev_tx.send(TunnelEvent::Cursor(x_norm, y_norm, visible, shape));
                            }
                            Ok(DataPacket::Control(ControlMessage::VideoCodecActive { codec_name, hardware })) => {
                                let _ = ev_tx.send(TunnelEvent::VideoCodecActive(codec_name, hardware));
                            }
                            Ok(DataPacket::Control(ControlMessage::PrinterList { printers })) => {
                                let _ = ev_tx.send(TunnelEvent::Printers(printers));
                            }
                            Ok(DataPacket::Control(ControlMessage::RemotePrintResult { ok, message, .. })) => {
                                let _ = ev_tx.send(TunnelEvent::PrintResult(ok, message));
                            }
                            Ok(DataPacket::Control(ControlMessage::P2pOffer { session_nonce: h_nonce, endpoints, .. })) => {
                                if h_nonce == session_nonce {
                                    if let Some(sock) = p2p_sock.take() {
                                        p2p_ch_rx = Some(spawn_p2p_punch(sock, session_nonce, p2p_key, endpoints, false));
                                    }
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::Pong { ts_ms })) => {
                                let rtt_v = now_ms().saturating_sub(ts_ms);
                                tel_rtt_ms.store(rtt_v, std::sync::atomic::Ordering::Relaxed);
                                let _ = ev_tx.send(TunnelEvent::Rtt(rtt_v));
                            }
                            Ok(DataPacket::Control(ControlMessage::Chat { sender, text })) => {
                                let _ = ev_tx.send(TunnelEvent::Chat(sender, text));
                            }
                            Ok(DataPacket::Control(ControlMessage::MonitorList { monitors, active_index })) => {
                                let _ = ev_tx.send(TunnelEvent::Monitors { monitors, active_index });
                            }
                            Ok(DataPacket::Control(ControlMessage::ClipboardText { text })) => {
                                write_clipboard_text(&text);
                                last_clip_text = text.clone();
                                last_clip_seq = clipboard_sequence_number();
                                let _ = ev_tx.send(TunnelEvent::Clipboard(text));
                            }
                            Ok(DataPacket::Control(ControlMessage::FileOfferEx { transfer_id, filename, size_bytes, sha256, target })) => {
                                if size_bytes <= 64 * 1024 * 1024 {
                                    incoming_uploads.insert(transfer_id, (filename, sha256, target, Vec::with_capacity(size_bytes as usize)));
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::FileChunk { transfer_id, data, eof, .. })) => {
                                if let Some(up) = incoming_uploads.get_mut(&transfer_id) {
                                    up.3.extend_from_slice(&data);
                                    if eof {
                                        let (fname, exp_sha, target, bytes) = incoming_uploads.remove(&transfer_id).unwrap();
                                        if verify_sha256_hex(&exp_sha, &bytes) {
                                            if let Ok(dest) = store_received_file_for_target(target, transfer_id, &fname, &bytes) {
                                                if target == FileDropTarget::Clipboard {
                                                    let _ = write_clipboard_files(&[dest]);
                                                    last_clip_seq = clipboard_sequence_number();
                                                    let _ = ev_tx.send(TunnelEvent::Clipboard(format!("Archivo remoto copiado: {fname}")));
                                                } else if target == FileDropTarget::RemotePrint {
                                                    let res = print_file_to_printer(&dest, None);
                                                    match res {
                                                        Ok(m) => {
                                                            let _ = ev_tx.send(TunnelEvent::PrintResult(true, m));
                                                        }
                                                        Err(e) => {
                                                            let _ = ev_tx.send(TunnelEvent::PrintResult(false, e));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::FileAck { ok, message, .. })) => {
                                let prefix = if ok { "✓ " } else { "✗ " };
                                let _ = ev_tx.send(TunnelEvent::FileAck(format!("{prefix}{message}")));
                            }
                            Ok(DataPacket::Control(ControlMessage::HardwareResourcesSnapshot { resources_json })) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resources_json) {
                                    let _ = ev_tx.send(TunnelEvent::Resources(v));
                                }
                            }
                            Ok(DataPacket::Control(ControlMessage::RebootPending { delay_secs, reason })) => {
                                let _ = ev_tx.send(TunnelEvent::RebootPending(delay_secs, reason));
                            }
                            Ok(DataPacket::Control(ControlMessage::Terminate { reason })) => {
                                let _ = ev_tx.send(TunnelEvent::Ended(reason));
                                break;
                            }
                            Err(_) => {
                                let _ = ev_tx.send(TunnelEvent::Ended("Conexión cerrada".into()));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            let _ = tunnel.close().await;
        });

        let chat_input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder("Escribí un mensaje al equipo remoto…")
        });
        let file_path_input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(r"C:\ruta\archivo.zip")
        });
        let print_file_input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(r"C:\ruta\documento.pdf")
        });
        let focus_handle = cx.focus_handle();

        let entity_weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
                let keep_going = cx
                    .update(|window, cx| {
                        let Some(entity) = entity_weak.upgrade() else {
                            return false;
                        };
                        entity.update(cx, |this, cx| this.drain_events(window, cx));
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            })
            .detach();

        let initial_side = match launch.initial_panel.as_deref() {
            Some("resources") => SidePanel::Resources,
            Some("chat") => SidePanel::Chat,
            Some("files") => SidePanel::Files,
            Some("print") => SidePanel::Print,
            _ => SidePanel::None,
        };
        let initial_users = launch.initial_panel.as_deref() == Some("users");
        let local_printers = enumerate_printers();

        Self {
            session_id: launch.session_id,
            label: launch.label.into(),
            allow_control: launch.allow_control,
            server_url: launch.server_url,
            auth_token: launch.auth_token,
            target: launch.target,
            dark: launch.dark,
            out_tx,
            file_tx,
            rx: ev_rx,
            on_ended: launch.on_ended,
            current_frame: None,
            frame_dims: (1280, 720),
            frame_count: 0,
            fps_window_start: Instant::now(),
            fps: 0.0,
            kbps: 0.0,
            rtt_ms: 0,
            jpeg_quality: 66,
            max_fps: 25,
            quality_preset: 0,
            transport_p2p: false,
            transport_label: "☁ Relay WSS".into(),
            remote_cursor: None,
            last_mouse_sent: Instant::now() - Duration::from_secs(1),
            monitors: Vec::new(),
            active_monitor: 0,
            audio_enabled: true,
            audio_mute_flag,
            codec_pref: VideoCodecPreference::Auto,
            active_codec_label: "Auto".into(),
            active_codec_hw: false,
            last_bgra_frame: None,
            whiteboard_active: false,
            wb_tool: WhiteboardTool::Pen,
            wb_color: 0xEF44_44FF,
            wb_thickness: 3.5,
            wb_strokes: Vec::new(),
            wb_drawing: None,
            wb_next_id: 1,
            local_printers,
            local_printer_idx: 0,
            remote_printers: Vec::new(),
            remote_printer_idx: 0,
            print_file_input,
            print_status: String::new(),
            hw_resources: None,
            side_panel: initial_side,
            confirm_reboot: false,
            users_popup: initial_users,
            ai_summary_open: false,
            ai_summary_busy: false,
            ai_summary_text: String::new(),
            chat_log: Vec::new(),
            chat_input,
            file_path_input,
            file_status: String::new(),
            status_line: "Negociando túnel E2EE Noise_XX…".into(),
            opened_at: Instant::now(),
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            focus_handle,
            last_mouse_norm: (0.0, 0.0),
            manual_disconnect: false,
            terminate_sent: false,
            notified_end: false,
            ui: ToolbarUi::new(launch.allow_control),
        }
    }

    fn palette(&self) -> Palette {
        if self.dark { DARK } else { LIGHT }
    }

    fn drain_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut latest_frame: Option<(u32, u32, Vec<u8>, f32)> = None;
        let mut changed = false;

        while let Ok(ev) = self.rx.try_recv() {
            changed = true;
            match ev {
                TunnelEvent::Frame { width, height, bgra, kbps } => {
                    latest_frame = Some((width, height, bgra, kbps));
                    self.frame_count += 1;
                    let secs = self.fps_window_start.elapsed().as_secs_f32();
                    if secs >= 1.0 {
                        self.fps = self.frame_count as f32 / secs;
                        self.frame_count = 0;
                        self.fps_window_start = Instant::now();
                    }
                }
                TunnelEvent::Rtt(ms) => {
                    self.rtt_ms = ms;
                }
                TunnelEvent::Chat(sender, text) => {
                    // Host-side notices (e.g. Windows blocking injected input) come as chat from
                    // "AgentDesk": surface them as a toast too, the chat panel is usually closed.
                    if sender == "AgentDesk" {
                        self.status_line = text.clone();
                        self.ui.last_activity = Instant::now();
                    }
                    self.chat_log.push(format!("{sender}: {text}"));
                }
                TunnelEvent::Monitors { monitors, active_index } => {
                    self.monitors = monitors;
                    self.active_monitor = active_index;
                }
                TunnelEvent::Clipboard(txt) => {
                    self.status_line = format!("Portapapeles recibido ({} chars)", txt.len());
                }
                TunnelEvent::FileAck(msg) => {
                    self.file_status = msg.clone();
                    self.status_line = msg;
                }
                TunnelEvent::Resources(hw) => {
                    self.hw_resources = Some(hw);
                }
                TunnelEvent::RebootPending(delay, reason) => {
                    self.status_line = format!("Reinicio remoto en {delay}s: {reason}");
                    if !self.notified_end {
                        self.notified_end = true;
                        let _ = self.on_ended.send(ViewerEndedNotice {
                            reason: format!("Reinicio remoto ({delay}s)"),
                            reboot_pending: true,
                            target: self.target.clone(),
                        });
                    }
                }
                TunnelEvent::TransportMode(is_p2p, label) => {
                    self.transport_p2p = is_p2p;
                    self.transport_label = label;
                }
                TunnelEvent::Cursor(x, y, visible, shape) => {
                    self.remote_cursor = Some((x, y, visible, shape));
                }
                TunnelEvent::VideoCodecActive(codec_name, hardware) => {
                    self.active_codec_label = codec_name;
                    self.active_codec_hw = hardware;
                }
                TunnelEvent::Printers(printers) => {
                    self.remote_printers = printers;
                    if self.remote_printer_idx >= self.remote_printers.len() {
                        self.remote_printer_idx = 0;
                    }
                }
                TunnelEvent::PrintResult(ok, message) => {
                    let prefix = if ok { "✓ " } else { "✗ " };
                    self.print_status = format!("{prefix}{message}");
                    self.status_line = self.print_status.clone();
                }
                TunnelEvent::Stage(text) => {
                    if self.status_line.starts_with("Negociando") {
                        self.status_line = text;
                        changed = true;
                    }
                }
                TunnelEvent::Ended(reason) => {
                    self.status_line = format!("Sesión finalizada: {reason}");
                    if !self.manual_disconnect && !self.notified_end {
                        self.notified_end = true;
                        let _ = self.on_ended.send(ViewerEndedNotice {
                            reason,
                            reboot_pending: false,
                            target: self.target.clone(),
                        });
                    }
                }
            }
        }

        if let Some((w, h, bgra, kbps)) = latest_frame {
            self.kbps = kbps;
            self.frame_dims = (w, h);
            self.last_bgra_frame = Some((w, h, bgra.clone()));
            if self.status_line.starts_with("Negociando") {
                self.status_line = format!(
                    "Sesión E2EE activa · primera imagen en {:.1} s",
                    self.opened_at.elapsed().as_secs_f32()
                );
            }
            if let Some(buf) = image::RgbaImage::from_raw(w, h, bgra) {
                let next_img = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
                if let Some(old) = self.current_frame.replace(next_img) {
                    let _ = window.drop_image(old);
                }
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn send_control(&self, msg: ControlMessage) {
        let _ = self.out_tx.send(DataPacket::Control(msg));
    }

    /// Ends the session exactly once, whatever closed it (hang-up button, the window's X, or
    /// the view being dropped): tells the host and the server, and tells the main window so its
    /// "sesiones abiertas" count goes down. Before this, closing the window left the tunnel
    /// streaming and the session listed as connected.
    pub fn finish_session(&mut self, reboot_arm: bool) {
        self.manual_disconnect = true;
        if !self.notified_end {
            self.notified_end = true;
            let _ = self.on_ended.send(ViewerEndedNotice {
                reason: if reboot_arm { "Reinicio solicitado".into() } else { "Cerrada por el usuario".into() },
                reboot_pending: reboot_arm,
                // No reconnect prompt for a session the user closed on purpose.
                target: if reboot_arm { self.target.clone() } else { None },
            });
        }
        if self.terminate_sent {
            return;
        }
        self.terminate_sent = true;
        self.send_control(ControlMessage::Terminate {
            reason: if reboot_arm {
                "reboot requested".into()
            } else {
                "closed by user".into()
            },
        });
        if let Some(tok) = self.auth_token.clone() {
            let url = format!(
                "{}/api/v1/sessions/{}/terminate",
                self.server_url.trim_end_matches('/'),
                self.session_id
            );
            runtime().spawn(async move {
                let _ = reqwest::Client::new()
                    .post(url)
                    .bearer_auth(tok)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
            });
        }
    }

    fn terminate_and_close(&mut self, reboot_arm: bool, window: &mut Window) {
        self.finish_session(reboot_arm);
        window.remove_window();
    }

    fn send_file(&mut self, cx: &mut Context<Self>) {
        let raw = self.file_path_input.read(cx).value().to_string();
        let path = PathBuf::from(raw.trim().trim_matches('"'));
        match std::fs::read(&path) {
            Ok(bytes) => {
                let transfer_id = Uuid::new_v4();
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "archivo.bin".into());
                let sha256 = hex::encode(Sha256::digest(&bytes));
                self.send_control(ControlMessage::FileOffer {
                    transfer_id,
                    filename: filename.clone(),
                    size_bytes: bytes.len() as u64,
                    sha256,
                });
                let chunk_size = 64 * 1024;
                let total = bytes.len();
                if total == 0 {
                    self.send_control(ControlMessage::FileChunk {
                        transfer_id,
                        offset: 0,
                        data: Vec::new(),
                        eof: true,
                    });
                } else {
                    for (i, chunk) in bytes.chunks(chunk_size).enumerate() {
                        let offset = (i * chunk_size) as u64;
                        let eof = (i + 1) * chunk_size >= total;
                        self.send_control(ControlMessage::FileChunk {
                            transfer_id,
                            offset,
                            data: chunk.to_vec(),
                            eof,
                        });
                    }
                }
                self.file_status = format!("Enviando {filename} ({total} bytes)…");
            }
            Err(e) => {
                self.file_status = format!("Error leyendo archivo: {e}");
            }
        }
        cx.notify();
    }

    fn norm_coords(&self, pos: Point<Pixels>) -> (f32, f32) {
        let bounds = self.viewport_bounds.get();
        let bx: f32 = bounds.origin.x.into();
        let by: f32 = bounds.origin.y.into();
        let bw: f32 = bounds.size.width.into();
        let bh: f32 = bounds.size.height.into();
        if bw <= 1.0 || bh <= 1.0 {
            return (0.0, 0.0);
        }
        let fw = self.frame_dims.0.max(1) as f32;
        let fh = self.frame_dims.1.max(1) as f32;
        let scale = (bw / fw).min(bh / fh);
        let dw = fw * scale;
        let dh = fh * scale;
        let ox = bx + (bw - dw) * 0.5;
        let oy = by + (bh - dh) * 0.5;
        let px: f32 = pos.x.into();
        let py: f32 = pos.y.into();
        (
            ((px - ox) / dw).clamp(0.0, 1.0),
            ((py - oy) / dh).clamp(0.0, 1.0),
        )
    }

    fn norm_to_local_viewport(&self, x_norm: f32, y_norm: f32) -> Option<(f32, f32)> {
        let bounds = self.viewport_bounds.get();
        let bw: f32 = bounds.size.width.into();
        let bh: f32 = bounds.size.height.into();
        if bw <= 1.0 || bh <= 1.0 {
            return None;
        }
        let fw = self.frame_dims.0.max(1) as f32;
        let fh = self.frame_dims.1.max(1) as f32;
        let scale = (bw / fw).min(bh / fh);
        let dw = fw * scale;
        let dh = fh * scale;
        let ox = (bw - dw) * 0.5;
        let oy = (bh - dh) * 0.5;
        Some((
            ox + x_norm.clamp(0.0, 1.0) * dw,
            oy + y_norm.clamp(0.0, 1.0) * dh,
        ))
    }

    fn send_dropped_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let valid: Vec<PathBuf> = paths
            .iter()
            .filter(|p| p.is_file())
            .cloned()
            .collect();
        if valid.is_empty() {
            self.status_line = "Soltá archivos válidos para enviar al Escritorio remoto".into();
            cx.notify();
            return;
        }
        let count = valid.len();
        let _ = self.file_tx.send((valid, FileDropTarget::Desktop));
        self.status_line = format!("Subiendo {count} archivo(s) al Escritorio remoto…");
        cx.notify();
    }

    fn finish_whiteboard_stroke(&mut self) {
        if let Some(st) = self.wb_drawing.take() {
            if !st.points.is_empty() {
                self.wb_strokes.push(st.clone());
                self.send_control(ControlMessage::WhiteboardStroke(st));
            }
        }
    }

    fn print_current_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some((w, h, bgra)) = self.last_bgra_frame.as_ref() else {
            self.print_status = "Esperando primer frame de pantalla…".into();
            cx.notify();
            return;
        };
        let mut rgba = Vec::with_capacity(bgra.len());
        for px in bgra.chunks_exact(4) {
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(255);
        }
        let Some(img) = image::RgbaImage::from_raw(*w, *h, rgba) else {
            self.print_status = "No se pudo armar la captura para imprimir".into();
            cx.notify();
            return;
        };
        let dir = std::env::temp_dir().join("AgentDeskPrint");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("captura_{}.png", now_ms()));
        if let Err(e) = img.save(&path) {
            self.print_status = format!("Error guardando captura: {e}");
            cx.notify();
            return;
        }
        let target_printer = self
            .local_printers
            .get(self.local_printer_idx)
            .map(|p| p.name.clone());
        self.print_status = match print_file_to_printer(&path, target_printer.as_deref()) {
            Ok(msg) => format!("✓ {msg}"),
            Err(e) => format!("✗ {e}"),
        };
        self.status_line = self.print_status.clone();
        cx.notify();
    }

    fn send_remote_print_file(&mut self, cx: &mut Context<Self>) {
        let raw = self.print_file_input.read(cx).value().to_string();
        let path = PathBuf::from(raw.trim().trim_matches('"'));
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 64 * 1024 * 1024 {
                    self.print_status = "✗ El documento supera el límite de 64 MB".into();
                    cx.notify();
                    return;
                }
                let job_id = Uuid::new_v4();
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "documento.pdf".into());
                let sha256 = hex::encode(Sha256::digest(&bytes));
                let printer_name = self
                    .remote_printers
                    .get(self.remote_printer_idx)
                    .map(|p| p.name.clone());
                self.send_control(ControlMessage::RemotePrintJob {
                    job_id,
                    filename: filename.clone(),
                    printer_name: printer_name.clone(),
                    sha256,
                    size_bytes: bytes.len() as u64,
                });
                let _ = self.file_tx.send((vec![path], FileDropTarget::RemotePrint));
                let dst_label = printer_name.unwrap_or_else(|| "impresora predeterminada".into());
                self.print_status = format!("Enviando {filename} para imprimir en {dst_label}…");
                self.status_line = self.print_status.clone();
            }
            Err(e) => {
                self.print_status = format!("✗ No se pudo leer el archivo: {e}");
            }
        }
        cx.notify();
    }

    fn request_ai_session_summary(&mut self, cx: &mut Context<Self>) {
        let local_notes = format!(
            "Sesión con {} · Control: {} · FPS {:.1} · RTT {} ms · Mensajes chat: {}",
            self.label,
            if self.allow_control { "completo" } else { "solo ver" },
            self.fps,
            self.rtt_ms,
            self.chat_log.len()
        );
        let Some(token) = self.auth_token.clone() else {
            self.ai_summary_text = format!(
                "{local_notes}\n\n(Iniciá sesión en el cliente para resumir con AgentDesk AI / ChatGPT OAuth; las contraseñas se redactan automáticamente.)"
            );
            cx.notify();
            return;
        };
        let base = self.server_url.trim_end_matches('/').to_string();
        let device_id = match &self.target {
            Some(ReconnectTarget::Managed { device_id, .. }) => Some(*device_id),
            _ => None,
        };
        let prompt = format!(
            "Generá un resumen breve de auditoría y soporte para la sesión remota actual (anonimizando datos sensibles).\nContexto: {local_notes}"
        );
        self.ai_summary_busy = true;
        self.ai_summary_text.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let reply = cx
                .background_executor()
                .spawn(async move {
                    let r = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(30))
                        .build()
                        .ok()?
                        .post(format!("{base}/api/v1/ai/chat"))
                        .bearer_auth(token)
                        .json(&agentdesk_protocol::AiChatRequest {
                            device_id,
                            ticket_id: None,
                            prompt,
                            include_diagnostics: device_id.is_some(),
                            include_screenshot: false,
                            chat_session_id: None,
                            model: None,
                            reasoning_effort: None,
                        })
                        .send()
                        .ok()?;
                    let body: agentdesk_protocol::AiChatResponse = r.json().ok()?;
                    Some(body.reply)
                })
                .await;
            this.update(cx, |this, cx| {
                this.ai_summary_busy = false;
                this.ai_summary_text = reply.unwrap_or_else(|| format!("{local_notes}\n\nNo se pudo obtener respuesta de IA desde el servidor."));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn tool_btn(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        active: bool,
        p: Palette,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(if active { p.accent } else { p.border }))
            .bg(rgb(if active { p.active } else { p.raised }))
            .text_color(rgb(if active { p.accent } else { p.text }))
            .text_xs()
            .child(label.into())
    }
}

fn map_gpui_key_to_vk(key: &str) -> Option<u16> {
    match key.to_ascii_lowercase().as_str() {
        "enter" => Some(0x0D),
        "tab" => Some(0x09),
        "backspace" => Some(0x08),
        "escape" => Some(0x1B),
        "space" => Some(0x20),
        "delete" => Some(0x2E),
        "insert" => Some(0x2D),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" => Some(0x21),
        "pagedown" => Some(0x22),
        "left" => Some(0x25),
        "up" => Some(0x26),
        "right" => Some(0x27),
        "down" => Some(0x28),
        "shift" => Some(0x10),
        "control" => Some(0x11),
        "alt" => Some(0x12),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        s if s.len() == 1 => {
            let b = s.as_bytes()[0];
            if b.is_ascii_lowercase() {
                Some((b - b'a' + 0x41) as u16)
            } else if b.is_ascii_digit() {
                Some(b as u16)
            } else {
                None
            }
        }
        _ => None,
    }
}

impl Render for RemoteSession {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let toolbar = self.render_floating_toolbar(p, cx);
        let popover = self.render_toolbar_popover(p, cx);
        let toast = self.render_status_toast(p);

        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(rgb(0x090B0E))
            .text_color(rgb(p.text))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let y: f32 = ev.position.y.into();
                if y < 72.0 {
                    let was_hidden = !this.toolbar_visible();
                    this.ui.last_activity = Instant::now();
                    if was_hidden {
                        cx.notify();
                    }
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("remote-viewport")
                            .track_focus(&self.focus_handle)
                            .relative()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .items_center()
                            .justify_center()
                            .bg(rgb(0x090B0E))
                            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                if this.whiteboard_active {
                                    let (nx, ny) = this.norm_coords(ev.position);
                                    if let Some(st) = this.wb_drawing.as_mut() {
                                        match st.tool {
                                            WhiteboardTool::Pen | WhiteboardTool::Highlighter => {
                                                let push = st.points.last().is_none_or(|p| {
                                                    (p.0 - nx).abs() > 0.003 || (p.1 - ny).abs() > 0.003
                                                });
                                                if push {
                                                    st.points.push((nx, ny));
                                                    cx.notify();
                                                }
                                            }
                                            WhiteboardTool::Arrow | WhiteboardTool::Rect => {
                                                if st.points.len() >= 2 {
                                                    st.points[1] = (nx, ny);
                                                } else {
                                                    st.points.push((nx, ny));
                                                }
                                                cx.notify();
                                            }
                                        }
                                    }
                                    return;
                                }
                                if !this.allow_control {
                                    return;
                                }
                                if this.last_mouse_sent.elapsed() < Duration::from_millis(16) {
                                    return;
                                }
                                let (nx, ny) = this.norm_coords(ev.position);
                                if (nx - this.last_mouse_norm.0).abs() > 0.002
                                    || (ny - this.last_mouse_norm.1).abs() > 0.002
                                {
                                    this.last_mouse_norm = (nx, ny);
                                    this.last_mouse_sent = Instant::now();
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseMoveNorm { x: nx, y: ny },
                                    ));
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    if this.whiteboard_active {
                                        let (nx, ny) = this.norm_coords(ev.position);
                                        let id = this.wb_next_id;
                                        this.wb_next_id = this.wb_next_id.wrapping_add(1);
                                        this.wb_drawing = Some(WhiteboardStroke {
                                            id,
                                            tool: this.wb_tool,
                                            color_rgba: this.wb_color,
                                            thickness: this.wb_thickness,
                                            points: vec![(nx, ny)],
                                        });
                                        cx.notify();
                                        return;
                                    }
                                    if !this.allow_control {
                                        return;
                                    }
                                    let (nx, ny) = this.norm_coords(ev.position);
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseMoveNorm { x: nx, y: ny },
                                    ));
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseButton {
                                            button: MouseBtn::Left,
                                            pressed: true,
                                        },
                                    ));
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.whiteboard_active {
                                        this.finish_whiteboard_stroke();
                                        cx.notify();
                                        return;
                                    }
                                    if this.allow_control {
                                        let _ = this.out_tx.send(DataPacket::Input(
                                            InputEvent::MouseButton {
                                                button: MouseBtn::Left,
                                                pressed: false,
                                            },
                                        ));
                                    }
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    if !this.allow_control {
                                        return;
                                    }
                                    let (nx, ny) = this.norm_coords(ev.position);
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseMoveNorm { x: nx, y: ny },
                                    ));
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseButton {
                                            button: MouseBtn::Right,
                                            pressed: true,
                                        },
                                    ));
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, _, _, _| {
                                    if this.allow_control {
                                        let _ = this.out_tx.send(DataPacket::Input(
                                            InputEvent::MouseButton {
                                                button: MouseBtn::Right,
                                                pressed: false,
                                            },
                                        ));
                                    }
                                }),
                            )
                            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, _| {
                                if !this.allow_control {
                                    return;
                                }
                                let dy = match ev.delta {
                                    ScrollDelta::Lines(pt) => (pt.y * 40.0) as i32,
                                    ScrollDelta::Pixels(pt) => {
                                        let y: f32 = pt.y.into();
                                        y as i32
                                    }
                                };
                                if dy != 0 {
                                    let _ = this.out_tx.send(DataPacket::Input(
                                        InputEvent::MouseWheel { delta_y: dy },
                                    ));
                                }
                            }))
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                                if !this.allow_control {
                                    return;
                                }
                                if !ev.is_held {
                                    if let Some(kc) = &ev.keystroke.key_char {
                                        for ch in kc.chars() {
                                            if !ch.is_control() {
                                                let _ = this.out_tx.send(DataPacket::Input(
                                                    InputEvent::UnicodeChar { ch },
                                                ));
                                            }
                                        }
                                    }
                                }
                                if let Some(vk) = map_gpui_key_to_vk(&ev.keystroke.key) {
                                    if vk < 0x30 {
                                        let _ = this.out_tx.send(DataPacket::Input(InputEvent::Key {
                                            vk,
                                            pressed: true,
                                        }));
                                    }
                                }
                            }))
                            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, _| {
                                if !this.allow_control {
                                    return;
                                }
                                if let Some(vk) = map_gpui_key_to_vk(&ev.keystroke.key) {
                                    let _ = this.out_tx.send(DataPacket::Input(InputEvent::Key {
                                        vk,
                                        pressed: false,
                                    }));
                                }
                            }))
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.send_dropped_files(paths.paths(), cx);
                            }))
                            .when_some(self.current_frame.clone(), |this, frame| {
                                this.child(
                                    img(ImageSource::Render(frame))
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .child({
                                let mut all_strokes = self.wb_strokes.clone();
                                if let Some(cur) = self.wb_drawing.clone() {
                                    all_strokes.push(cur);
                                }
                                let fdims = self.frame_dims;
                                let bounds_paint = self.viewport_bounds.clone();
                                canvas(
                                    move |bounds, _, _| {
                                        bounds_paint.set(bounds);
                                    },
                                    move |bounds, (), window, _| {
                                        if all_strokes.is_empty() {
                                            return;
                                        }
                                        let bx: f32 = bounds.origin.x.into();
                                        let by: f32 = bounds.origin.y.into();
                                        let bw: f32 = bounds.size.width.into();
                                        let bh: f32 = bounds.size.height.into();
                                        if bw <= 1.0 || bh <= 1.0 {
                                            return;
                                        }
                                        let fw = fdims.0.max(1) as f32;
                                        let fh = fdims.1.max(1) as f32;
                                        let scale = (bw / fw).min(bh / fh);
                                        let dw = fw * scale;
                                        let dh = fh * scale;
                                        let ox = bx + (bw - dw) * 0.5;
                                        let oy = by + (bh - dh) * 0.5;
                                        let to_pt = |nx: f32, ny: f32| {
                                            point(
                                                px(ox + nx.clamp(0.0, 1.0) * dw),
                                                px(oy + ny.clamp(0.0, 1.0) * dh),
                                            )
                                        };
                                        for st in &all_strokes {
                                            let width_px = match st.tool {
                                                WhiteboardTool::Highlighter => (st.thickness * 2.8).max(12.0),
                                                _ => st.thickness.max(2.5),
                                            };
                                            let color = match st.tool {
                                                WhiteboardTool::Highlighter => (st.color_rgba & 0xFFFF_FF00) | 0x6E,
                                                _ => st.color_rgba,
                                            };
                                            let mut pb = PathBuilder::stroke(px(width_px));
                                            match st.tool {
                                                WhiteboardTool::Pen | WhiteboardTool::Highlighter => {
                                                    if let Some(&(x0, y0)) = st.points.first() {
                                                        pb.move_to(to_pt(x0, y0));
                                                        if st.points.len() == 1 {
                                                            pb.line_to(to_pt(x0 + 0.001, y0 + 0.001));
                                                        } else {
                                                            for &(nx, ny) in &st.points[1..] {
                                                                pb.line_to(to_pt(nx, ny));
                                                            }
                                                        }
                                                    }
                                                }
                                                WhiteboardTool::Arrow => {
                                                    if let (Some(&a), Some(&b)) = (st.points.first(), st.points.last()) {
                                                        let p0 = to_pt(a.0, a.1);
                                                        let p1 = to_pt(b.0, b.1);
                                                        pb.move_to(p0);
                                                        pb.line_to(p1);
                                                        let x0: f32 = p0.x.into();
                                                        let y0: f32 = p0.y.into();
                                                        let x1: f32 = p1.x.into();
                                                        let y1: f32 = p1.y.into();
                                                        let ang = (y1 - y0).atan2(x1 - x0);
                                                        let head = (width_px * 4.5).max(16.0);
                                                        for sgn in [-1.0f32, 1.0f32] {
                                                            let ha = ang + std::f32::consts::PI - sgn * 0.52;
                                                            pb.move_to(p1);
                                                            pb.line_to(point(
                                                                px(x1 + ha.cos() * head),
                                                                px(y1 + ha.sin() * head),
                                                            ));
                                                        }
                                                    }
                                                }
                                                WhiteboardTool::Rect => {
                                                    if let (Some(&a), Some(&b)) = (st.points.first(), st.points.last()) {
                                                        let min_x = a.0.min(b.0);
                                                        let max_x = a.0.max(b.0);
                                                        let min_y = a.1.min(b.1);
                                                        let max_y = a.1.max(b.1);
                                                        pb.move_to(to_pt(min_x, min_y));
                                                        pb.line_to(to_pt(max_x, min_y));
                                                        pb.line_to(to_pt(max_x, max_y));
                                                        pb.line_to(to_pt(min_x, max_y));
                                                        pb.close();
                                                    }
                                                }
                                            }
                                            if let Ok(path) = pb.build() {
                                                window.paint_path(path, rgba(color));
                                            }
                                        }
                                    },
                                )
                                .absolute()
                                .inset_0()
                            })
                            .when(self.whiteboard_active, |this| {
                                this.child(self.render_whiteboard_palette(p, cx))
                            })
                            .when(self.current_frame.is_none(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(p.muted))
                                        .child(self.status_line.clone()),
                                )
                            })
                            .when_some(self.remote_cursor, |this, (cx_n, cy_n, visible, _shape)| {
                                if !visible {
                                    return this;
                                }
                                if let Some((lx, ly)) = self.norm_to_local_viewport(cx_n, cy_n) {
                                    this.child(
                                        div()
                                            .absolute()
                                            .left(px((lx - 5.0).max(0.0)))
                                            .top(px((ly - 5.0).max(0.0)))
                                            .size(px(10.0))
                                            .rounded_full()
                                            .border_2()
                                            .border_color(rgb(0x38BDF8))
                                            .bg(rgb(0x0F172A)),
                                    )
                                } else {
                                    this
                                }
                            })
                    )
            )
            .when(self.side_panel != SidePanel::None, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(64.))
                        .right(px(12.))
                        .bottom(px(16.))
                        .flex()
                        .rounded(px(14.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(p.border))
                        .shadow_lg()
                        .child(self.render_side_panel(p, cx)),
                )
            })
            .when_some(toast, |this, t| this.child(t))
            .when(popover.is_some(), |this| {
                this.child(
                    div()
                        .id("tb-pop-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.ui.pop = None;
                                cx.notify();
                            }),
                        ),
                )
            })
            .child(toolbar)
            .when_some(popover, |this, pop| this.child(pop))
            .when(self.confirm_reboot, |this| {
                this.child(self.render_reboot_modal(p, cx))
            })
            .when(self.users_popup, |this| {
                this.child(self.render_users_modal(p, cx))
            })
            .when(self.ai_summary_open, |this| {
                this.child(self.render_ai_summary_popover(p, cx))
            })
    }
}

impl RemoteSession {
    fn render_side_panel(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut box_div = div()
            .flex()
            .flex_col()
            .w(px(330.))
            .h_full()
            .p_3()
            .gap_3()
            .border_l_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel));

        match self.side_panel {
            SidePanel::Resources => {
                box_div = box_div
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_semibold().text_sm().child("Recursos del PC remoto"))
                            .child(Self::tool_btn("refresh-rem-hw", "↻", false, p).on_click(
                                cx.listener(|this, _, _, _| {
                                    this.send_control(ControlMessage::RequestHardwareResources);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .when_some(self.hw_resources.clone(), |this, hw| {
                                this.child(render_hardware_resources(&hw, p))
                            })
                            .when(self.hw_resources.is_none(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.muted))
                                        .child("Consultando CPU, RAM, discos y GPU…"),
                                )
                            }),
                    );
            }
            SidePanel::Chat => {
                let messages = self.chat_log.clone();
                box_div = box_div
                    .child(div().font_semibold().text_sm().child("Chat de sesión"))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(messages.into_iter().map(|m| {
                                div()
                                    .p_2()
                                    .rounded_md()
                                    .bg(rgb(p.raised))
                                    .text_xs()
                                    .child(m)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                gpui_kit::component::input::Input::new(&self.chat_input)
                                    .flex_1(),
                            )
                            .child(
                                div()
                                    .id("send-rem-chat")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(p.accent))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_xs()
                                    .child("Enviar")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let msg = this.chat_input.read(cx).value().to_string();
                                        if !msg.trim().is_empty() {
                                            this.chat_log.push(format!("Yo: {}", msg.trim()));
                                            this.send_control(ControlMessage::Chat {
                                                sender: "Cliente AgentDesk".into(),
                                                text: msg.trim().to_string(),
                                            });
                                            cx.notify();
                                        }
                                    })),
                            ),
                    );
            }
            SidePanel::Files => {
                box_div = box_div
                    .child(div().font_semibold().text_sm().child("Transferencia E2EE (SHA-256)"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child("Ruta local del archivo a enviar al destino:"),
                    )
                    .child(gpui_kit::component::input::Input::new(&self.file_path_input))
                    .child(
                        div()
                            .id("send-rem-file")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(p.accent))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .child("Enviar archivo")
                            .on_click(cx.listener(|this, _, _, cx| this.send_file(cx))),
                    )
                    .when(!self.file_status.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(self.file_status.clone()),
                        )
                    });
            }
            SidePanel::Print => {
                let locals = self.local_printers.clone();
                let l_idx = self.local_printer_idx;
                let remotes = self.remote_printers.clone();
                let r_idx = self.remote_printer_idx;
                box_div = box_div
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_semibold().text_sm().child("Impresión remota"))
                            .child(Self::tool_btn("ref-printers", "↻", false, p).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.local_printers = enumerate_printers();
                                    this.send_control(ControlMessage::RequestPrinters);
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(div().text_xs().font_semibold().child("1. Imprimir pantalla remota en MI impresora"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(locals.is_empty(), |this| {
                                this.child(div().text_xs().text_color(rgb(p.muted)).child("Sin impresoras locales detectadas"))
                            })
                            .children(locals.into_iter().enumerate().map(|(i, pr)| {
                                Self::tool_btn(
                                    SharedString::from(format!("loc-pr-{i}")),
                                    format!("{}{}", if pr.is_default { "★ " } else { "" }, pr.name),
                                    i == l_idx,
                                    p,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.local_printer_idx = i;
                                    cx.notify();
                                }))
                            })),
                    )
                    .child(
                        div()
                            .id("print-shot-local")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(p.accent))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .font_semibold()
                            .child("🖨 Imprimir captura actual")
                            .on_click(cx.listener(|this, _, _, cx| this.print_current_screenshot(cx))),
                    )
                    .child(div().h(px(1.)).bg(rgb(p.border)))
                    .child(div().text_xs().font_semibold().child("2. Imprimir documento en el PC remoto"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(remotes.is_empty(), |this| {
                                this.child(div().text_xs().text_color(rgb(p.muted)).child("Consultando impresoras del equipo remoto…"))
                            })
                            .children(remotes.into_iter().enumerate().map(|(i, pr)| {
                                Self::tool_btn(
                                    SharedString::from(format!("rem-pr-{i}")),
                                    format!("{}{} ({})", if pr.is_default { "★ " } else { "" }, pr.name, pr.status),
                                    i == r_idx,
                                    p,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remote_printer_idx = i;
                                    cx.notify();
                                }))
                            })),
                    )
                    .child(gpui_kit::component::input::Input::new(&self.print_file_input))
                    .child(
                        div()
                            .id("send-remote-print")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(p.accent))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .font_semibold()
                            .child("🖨 Imprimir en PC remoto")
                            .on_click(cx.listener(|this, _, _, cx| this.send_remote_print_file(cx))),
                    )
                    .when(!self.print_status.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(self.print_status.clone()),
                        )
                    });
            }
            SidePanel::None => {}
        }
        box_div
    }

    fn render_whiteboard_palette(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        let swatch = |col: u32, active: bool, id: &'static str| {
            div()
                .id(id)
                .size(px(18.))
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(rgb(if active { 0xFFFFFF } else { p.border }))
                .bg(rgba(col))
        };
        // Bottom-center: the session toolbar owns the top edge and was painting over this.
        div()
            .absolute()
            .bottom(px(18.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("wb-toolbar")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(p.sidebar_active))
                    .bg(rgb(p.sidebar_bg))
                    .text_color(rgb(p.sidebar_text))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        Self::tool_btn("wb-pen", "Lápiz", self.wb_tool == WhiteboardTool::Pen, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wb_tool = WhiteboardTool::Pen;
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool_btn("wb-hl", "Resaltador", self.wb_tool == WhiteboardTool::Highlighter, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wb_tool = WhiteboardTool::Highlighter;
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool_btn("wb-arr", "Flecha", self.wb_tool == WhiteboardTool::Arrow, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wb_tool = WhiteboardTool::Arrow;
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool_btn("wb-rect", "Rect", self.wb_tool == WhiteboardTool::Rect, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wb_tool = WhiteboardTool::Rect;
                                cx.notify();
                            })),
                    )
                    .child(swatch(0xEF44_44FF, self.wb_color == 0xEF44_44FF, "wb-c-red").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_color = 0xEF44_44FF;
                            cx.notify();
                        }),
                    ))
                    .child(swatch(0xFACC_15FF, self.wb_color == 0xFACC_15FF, "wb-c-yel").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_color = 0xFACC_15FF;
                            cx.notify();
                        }),
                    ))
                    .child(swatch(0x22C5_5EFF, self.wb_color == 0x22C5_5EFF, "wb-c-grn").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_color = 0x22C5_5EFF;
                            cx.notify();
                        }),
                    ))
                    .child(swatch(0x38BD_F8FF, self.wb_color == 0x38BD_F8FF, "wb-c-blu").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_color = 0x38BD_F8FF;
                            cx.notify();
                        }),
                    ))
                    .child(swatch(0xFFFF_FFFF, self.wb_color == 0xFFFF_FFFF, "wb-c-wht").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_color = 0xFFFF_FFFF;
                            cx.notify();
                        }),
                    ))
                    .child(Self::tool_btn("wb-undo", "Deshacer", false, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_strokes.pop();
                            this.send_control(ControlMessage::WhiteboardUndo);
                            cx.notify();
                        }),
                    ))
                    .child(Self::tool_btn("wb-clear", "Limpiar", false, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.wb_strokes.clear();
                            this.wb_drawing = None;
                            this.send_control(ControlMessage::WhiteboardClear);
                            cx.notify();
                        }),
                    ))
                    .child(Self::tool_btn("wb-done", "Cerrar", false, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.whiteboard_active = false;
                            this.send_control(ControlMessage::WhiteboardToggle {
                                active: !this.wb_strokes.is_empty(),
                            });
                            cx.notify();
                        }),
                    )),
            )
    }

    fn render_reboot_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
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
                    .child(div().font_semibold().child("Reiniciar equipo remoto"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "¿Reiniciar {} y activar la auto-reconexión cuando vuelva a iniciar?",
                                self.label
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Self::tool_btn("cancel-reboot", "Cancelar", false, p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_reboot = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("confirm-reboot-go")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(p.danger))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_xs()
                                    .font_semibold()
                                    .child("Reiniciar y auto-reconectar")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_reboot = false;
                                        this.send_control(ControlMessage::SessionAction {
                                            action: "reboot".into(),
                                            target_user: None,
                                            delay_secs: Some(5),
                                        });
                                        this.terminate_and_close(true, window);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }

    fn render_ai_summary_popover(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .top_12()
            .right_4()
            .w(px(360.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .shadow_lg()
            .text_color(rgb(p.text))
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
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded_full()
                                    .bg(rgb(p.badge_ok_bg))
                                    .text_color(rgb(p.badge_ok_fg))
                                    .text_xs()
                                    .font_semibold()
                                    .child("Nuevo"),
                            )
                            .child(div().font_semibold().text_sm().child("Iniciar resumen de sesión")),
                    )
                    .child(
                        Self::tool_btn("close-ai-sum", "×", false, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.ai_summary_open = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child("Resumir esta sesión con IA. Las contraseñas y los datos confidenciales están anonimizados."),
            )
            .when(!self.ai_summary_text.is_empty(), |this| {
                this.child(
                    div()
                        .id("ai-sum-txt")
                        .max_h(px(160.))
                        .overflow_y_scrollbar()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(p.raised))
                        .text_xs()
                        .child(self.ai_summary_text.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        Self::tool_btn("dismiss-ai-sum", "De acuerdo", false, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.ai_summary_open = false;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .id("run-ai-sum")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(p.accent))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .font_semibold()
                            .child(if self.ai_summary_busy {
                                "Generando…"
                            } else {
                                "Iniciar resumen"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.ai_summary_busy {
                                    this.request_ai_session_summary(cx);
                                }
                            })),
                    ),
            )
    }

    fn render_users_modal(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        let users: Vec<String> = self
            .hw_resources
            .as_ref()
            .and_then(|hw| hw.get("windows_users")?.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|u| {
                        u.get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("?")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();

        let sessions: Vec<(String, String, String)> = self
            .hw_resources
            .as_ref()
            .and_then(|hw| hw.get("windows_sessions")?.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|s| {
                        (
                            s.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            s.get("username")
                                .and_then(|x| x.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            s.get("state")
                                .and_then(|x| x.as_str())
                                .unwrap_or("?")
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_semibold().child("Usuarios de Windows"))
                            .child(
                                Self::tool_btn("close-users-pop", "Cerrar", false, p).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.users_popup = false;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .child(div().text_xs().font_semibold().child("Cuentas locales"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child(if users.is_empty() {
                                "Consultando…".into()
                            } else {
                                users.join(", ")
                            }),
                    )
                    .when(!sessions.is_empty(), |this| {
                        this.child(div().text_xs().font_semibold().child("Sesiones activas"))
                            .children(sessions.into_iter().map(|(id, uname, st)| {
                                let id_send = id.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_xs()
                                    .child(format!("#{id} {uname} ({st})"))
                                    .child(
                                        Self::tool_btn(
                                            SharedString::from(format!("logoff-{id}")),
                                            "Cerrar sesión",
                                            false,
                                            p,
                                        )
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            this.send_control(ControlMessage::SessionAction {
                                                action: "signout".into(),
                                                target_user: Some(id_send.clone()),
                                                delay_secs: None,
                                            });
                                        })),
                                    )
                            }))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .pt_2()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .child(
                                Self::tool_btn("ref-users", "↻ Refrescar", false, p).on_click(
                                    cx.listener(|this, _, _, _| {
                                        this.send_control(ControlMessage::RequestHardwareResources);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .id("switch-win-user")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(p.danger))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_xs()
                                    .child("Cerrar sesión actual / cambiar usuario")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send_control(ControlMessage::SessionAction {
                                            action: "switch_user".into(),
                                            target_user: None,
                                            delay_secs: None,
                                        });
                                        this.users_popup = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------
// Barra flotante del visor (rediseño): solo presentación. Reutiliza los mismos
// mensajes de control que la barra anterior; no toca captura/decodificación.
// ---------------------------------------------------------------------------
impl RemoteSession {
    fn toolbar_visible(&self) -> bool {
        self.ui.pop.is_some()
            || self.current_frame.is_none()
            || self.ui.last_activity.elapsed() < Duration::from_millis(2500)
    }

    fn apply_quality_preset(&mut self, preset: u8) {
        self.quality_preset = preset;
        let (mw, jq, fps, auto) = match preset {
            0 => (0u16, 72u8, 60u8, true),
            1 => (1280u16, 55u8, 30u8, false),
            2 => (1600u16, 70u8, 45u8, false),
            _ => (0u16, 90u8, 60u8, false),
        };
        self.jpeg_quality = jq;
        self.max_fps = fps;
        self.send_control(ControlMessage::StreamPolicy {
            max_width: mw,
            jpeg_quality: jq,
            max_fps: fps,
            auto_adapt: auto,
        });
        self.send_control(ControlMessage::RequestKeyframe);
    }

    fn apply_codec_pref(&mut self, pref: VideoCodecPreference) {
        self.codec_pref = pref;
        self.send_control(ControlMessage::VideoCodecPolicy { preference: pref });
        self.send_control(ControlMessage::RequestKeyframe);
    }

    fn toggle_side(&mut self, panel: SidePanel) {
        self.ui.pop = None;
        self.side_panel = if self.side_panel == panel {
            SidePanel::None
        } else {
            match panel {
                SidePanel::Resources => self.send_control(ControlMessage::RequestHardwareResources),
                SidePanel::Print => {
                    self.local_printers = enumerate_printers();
                    self.send_control(ControlMessage::RequestPrinters);
                }
                _ => {}
            }
            panel
        };
    }

    fn render_toolbar_popover(&mut self, p: Palette, cx: &mut Context<Self>) -> Option<Div> {
        use crate::ui::{floating, menu_head, menu_item};
        use gpui_kit::assets::IconName;
        let pop = self.ui.pop?;
        let mut card = floating(p).id("tb-pop").w(px(260.)).p(px(6.)).flex().flex_col();
        match pop {
            "mon" => {
                card = card.child(menu_head("Monitor", p));
                if self.monitors.is_empty() {
                    card = card.child(menu_item("tb-mon-none", IconName::Monitor, "Monitor principal", None, true, p));
                }
                for m in self.monitors.clone() {
                    let idx = m.index;
                    card = card.child(
                        menu_item(
                            SharedString::from(format!("tb-mon-{idx}")),
                            IconName::Monitor,
                            format!("Monitor {} · {}×{}", idx + 1, m.width, m.height),
                            Some(SharedString::from(format!("{}", idx + 1))),
                            idx == self.active_monitor,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.active_monitor = idx;
                            this.send_control(ControlMessage::SelectMonitor { index: idx });
                            this.ui.pop = None;
                            cx.notify();
                        })),
                    );
                }
                card = card.child(menu_head("Calidad", p));
                for (preset, icon, text) in [
                    (0u8, IconName::WandSparkles, "Auto"),
                    (3u8, IconName::Gem, "Alta (nativa)"),
                    (2u8, IconName::Gauge, "Media (900p)"),
                    (1u8, IconName::Rabbit, "Rápida (720p)"),
                ] {
                    card = card.child(
                        menu_item(
                            SharedString::from(format!("tb-q-{preset}")),
                            icon,
                            text,
                            None,
                            self.quality_preset == preset,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_quality_preset(preset);
                            this.ui.pop = None;
                            cx.notify();
                        })),
                    );
                }
                card = card.child(menu_head("Códec", p));
                for (pref, icon, text, key) in [
                    (VideoCodecPreference::Auto, IconName::WandSparkles, "Automático", "auto"),
                    (VideoCodecPreference::HardwareH264, IconName::Cpu, "H.264 por hardware", "hw"),
                    (VideoCodecPreference::HybridTiles, IconName::Grid2x2, "Tiles", "tiles"),
                ] {
                    card = card.child(
                        menu_item(
                            SharedString::from(format!("tb-codec-{key}")),
                            icon,
                            text,
                            None,
                            self.codec_pref == pref,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_codec_pref(pref);
                            this.ui.pop = None;
                            cx.notify();
                        })),
                    );
                }
            }
            "kbd" => {
                card = card.child(menu_head("Enviar atajo", p));
                for (combo, icon, text, hint) in [
                    ("Alt+Tab", IconName::Layers, "Alt+Tab", "Alt Tab"),
                    ("Ctrl+Shift+Esc", IconName::Activity, "Administrador de tareas", "Ctrl Shift Esc"),
                    ("Win", IconName::AppWindow, "Tecla Windows", "Win"),
                ] {
                    card = card.child(
                        menu_item(
                            SharedString::from(format!("tb-key-{combo}")),
                            icon,
                            text,
                            Some(hint.into()),
                            false,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.allow_control {
                                this.send_control(ControlMessage::SpecialKey { combo: combo.into() });
                            } else {
                                this.status_line = "Activá el modo control para enviar atajos".into();
                            }
                            this.ui.pop = None;
                            cx.notify();
                        })),
                    );
                }
                card = card.child(menu_head("Modo", p));
                let granted = self.ui.granted_control;
                card = card
                    .child(
                        menu_item("tb-mode-control", IconName::MousePointer2, "Control completo", None, self.allow_control, p)
                            .when(!granted, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if granted {
                                    this.allow_control = true;
                                } else {
                                    this.status_line = "La sesión se abrió en modo solo ver".into();
                                }
                                this.ui.pop = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        menu_item("tb-mode-view", IconName::Eye, "Solo ver", None, !self.allow_control, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.allow_control = false;
                                this.ui.pop = None;
                                cx.notify();
                            }),
                        ),
                    );
            }
            _ => {
                let ctl = self.allow_control;
                card = card
                    .child(
                        menu_item("tb-lock", IconName::Lock, "Bloquear equipo remoto", None, false, p)
                            .when(!ctl, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ctl {
                                    this.send_control(ControlMessage::SessionAction {
                                        action: "lock".into(),
                                        target_user: None,
                                        delay_secs: None,
                                    });
                                }
                                this.ui.pop = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        menu_item("tb-signout", IconName::LogOut, "Cerrar sesión de Windows", None, false, p)
                            .when(!ctl, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ctl {
                                    this.send_control(ControlMessage::SessionAction {
                                        action: "signout".into(),
                                        target_user: None,
                                        delay_secs: None,
                                    });
                                }
                                this.ui.pop = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        menu_item("tb-users", IconName::Users, "Usuarios de Windows", None, self.users_popup, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.users_popup = !this.users_popup;
                                if this.users_popup {
                                    this.send_control(ControlMessage::RequestHardwareResources);
                                }
                                this.ui.pop = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        menu_item("tb-reboot", IconName::RotateCcw, "Reiniciar y reconectar", None, false, p)
                            .when(!ctl, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ctl {
                                    this.confirm_reboot = true;
                                }
                                this.ui.pop = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        menu_item(
                            "tb-print",
                            IconName::Printer,
                            "Impresión remota",
                            None,
                            self.side_panel == SidePanel::Print,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_side(SidePanel::Print);
                            cx.notify();
                        })),
                    )
                    .child(
                        menu_item("tb-shot", IconName::Camera, "Imprimir captura de pantalla", None, false, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.ui.pop = None;
                                this.print_current_screenshot(cx);
                                cx.notify();
                            }),
                        ),
                    );
            }
        }
        Some(
            div()
                .absolute()
                .top(px(60.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(card.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())),
        )
    }

    fn render_floating_toolbar(&mut self, p: Palette, cx: &mut Context<Self>) -> Div {
        use crate::ui::{icon_btn, tip, vsep, MONO};
        use gpui_kit::assets::IconName;
        use gpui_kit::component::Icon;

        if !self.toolbar_visible() {
            // Asa mínima: acercar el mouse al borde superior vuelve a mostrar la barra.
            return div().absolute().top(px(6.)).left_0().right_0().flex().justify_center().child(
                div()
                    .w(px(56.))
                    .h(px(5.))
                    .rounded_full()
                    .bg(rgba(0xFFFFFF55)),
            );
        }

        let pop = self.ui.pop;
        let side = self.side_panel;
        let (q_col, q_name) = if self.rtt_ms == 0 {
            (p.accent, "Midiendo latencia")
        } else if self.rtt_ms < 45 {
            (p.success, "Excelente")
        } else if self.rtt_ms < 110 {
            (p.success, "Buena")
        } else if self.rtt_ms < 200 {
            (p.warning, "Regular")
        } else {
            (p.danger, "Lenta")
        };
        let route = if self.transport_p2p { "P2P" } else { "Relay" };
        let stat_tip = format!(
            "{q_name} · túnel E2EE · {} · {} · {:.0} KB/s",
            self.transport_label.trim_start_matches(|c: char| !c.is_alphanumeric()),
            self.active_codec_label,
            self.kbps
        );
        let pop_btn = |id: &'static str, icon: IconName, label: &'static str, key: &'static str| {
            icon_btn(id, icon, label, pop == Some(key), p)
        };
        let side_btn = |id: &'static str, icon: IconName, label: &'static str, panel: SidePanel| {
            icon_btn(id, icon, label, side == panel, p)
        };

        let bar = div()
            .id("session-toolbar")
            .flex()
            .items_center()
            .gap_1()
            .p(px(5.))
            .rounded(px(14.))
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgba((p.panel << 8) | 0xEB))
            .shadow_lg()
            .text_color(rgb(p.text))
            .aria_label("Herramientas de sesión")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .text_sm()
                    .font_semibold()
                    .child(Icon::new(IconName::Monitor).size(px(17.)).text_color(rgb(p.muted)))
                    .child(div().max_w(px(180.)).truncate().child(self.label.clone())),
            )
            .child(
                div()
                    .id("tb-status")
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .h(px(34.))
                    .pl_2()
                    .pr(px(10.))
                    .rounded(px(9.))
                    .text_xs()
                    .font_medium()
                    .child(div().size(px(7.)).rounded_full().bg(rgb(q_col)))
                    .child(Icon::new(IconName::Lock).size(px(14.)).text_color(rgb(p.success)))
                    .child(div().font_family(MONO).text_color(rgb(p.muted)).child(format!(
                        "{route} · {} ms · {:.0} fps",
                        self.rtt_ms, self.fps
                    )))
                    .tooltip(tip(stat_tip)),
            )
            .child(vsep(p))
            .child(
                pop_btn("tb-mon", IconName::Monitor, "Monitor, calidad y códec", "mon").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ui.pop = if this.ui.pop == Some("mon") { None } else { Some("mon") };
                        cx.notify();
                    },
                )),
            )
            .child(
                icon_btn("tb-full", IconName::Maximize, "Pantalla completa", false, p).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.ui.pop = None;
                        window.toggle_fullscreen();
                        cx.notify();
                    },
                )),
            )
            .child(vsep(p))
            .child(
                pop_btn("tb-kbd", IconName::Keyboard, "Atajos y modo de control", "kbd").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ui.pop = if this.ui.pop == Some("kbd") { None } else { Some("kbd") };
                        cx.notify();
                    },
                )),
            )
            .child(
                icon_btn("tb-clip", IconName::Clipboard, "Enviar portapapeles", false, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ui.pop = None;
                        if !this.allow_control {
                            this.status_line = "Modo solo ver: el portapapeles está deshabilitado".into();
                        } else if let Some(txt) = read_clipboard_text() {
                            this.send_control(ControlMessage::ClipboardText { text: txt });
                            this.status_line = "Portapapeles enviado".into();
                        }
                        cx.notify();
                    },
                )),
            )
            .child(vsep(p))
            .child(
                side_btn("tb-files", IconName::FolderUp, "Archivos", SidePanel::Files).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.toggle_side(SidePanel::Files);
                        cx.notify();
                    },
                )),
            )
            .child(
                side_btn("tb-chat", IconName::MessageSquare, "Chat", SidePanel::Chat).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.toggle_side(SidePanel::Chat);
                        cx.notify();
                    },
                )),
            )
            .child(
                icon_btn("tb-wb", IconName::PenTool, "Pizarra", self.whiteboard_active, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ui.pop = None;
                        this.whiteboard_active = !this.whiteboard_active;
                        this.send_control(ControlMessage::WhiteboardToggle {
                            active: this.whiteboard_active || !this.wb_strokes.is_empty(),
                        });
                        cx.notify();
                    },
                )),
            )
            .child(
                icon_btn(
                    "tb-audio",
                    if self.audio_enabled { IconName::Volume2 } else { IconName::VolumeX },
                    if self.audio_enabled { "Silenciar audio remoto" } else { "Activar audio remoto" },
                    false,
                    p,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ui.pop = None;
                    this.audio_enabled = !this.audio_enabled;
                    this.audio_mute_flag
                        .store(!this.audio_enabled, std::sync::atomic::Ordering::Relaxed);
                    this.send_control(ControlMessage::AudioControl { enabled: this.audio_enabled });
                    cx.notify();
                })),
            )
            .child(
                side_btn("tb-res", IconName::Cpu, "Recursos del equipo", SidePanel::Resources).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.toggle_side(SidePanel::Resources);
                        cx.notify();
                    },
                )),
            )
            .child(vsep(p))
            .child(
                icon_btn("tb-ai", IconName::Sparkles, "Resumen de sesión con IA", self.ai_summary_open, p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.ui.pop = None;
                        this.ai_summary_open = !this.ai_summary_open;
                        cx.notify();
                    }),
                ),
            )
            .child(
                pop_btn("tb-more", IconName::Ellipsis, "Más: bloquear, reiniciar, imprimir, captura", "more").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.ui.pop = if this.ui.pop == Some("more") { None } else { Some("more") };
                        cx.notify();
                    }),
                ),
            )
            .child(vsep(p))
            .child(
                div()
                    .id("end-session")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(40.))
                    .h(px(34.))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .bg(rgb(p.danger))
                    .text_color(rgb(0xFFFFFF))
                    .hover(|s| s.opacity(0.9))
                    .child(Icon::new(IconName::PhoneOff).size(px(18.)))
                    .aria_label("Terminar sesión")
                    .tooltip(tip("Terminar sesión"))
                    .on_click(cx.listener(|this, _, window, _| {
                        this.terminate_and_close(false, window);
                    })),
            );

        div()
            .absolute()
            .top(px(12.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(bar)
    }

    /// Aviso pequeño abajo a la izquierda (transferencias y estado).
    fn render_status_toast(&self, p: Palette) -> Option<Div> {
        use gpui_kit::assets::IconName;
        use gpui_kit::component::Icon;
        let (icon, text) = if !self.file_status.is_empty() && self.side_panel != SidePanel::Files {
            (IconName::FileArchive, self.file_status.clone())
        } else if self.current_frame.is_some() && !self.status_line.is_empty() && self.ui.last_activity.elapsed() < Duration::from_secs(6) {
            (IconName::Info, self.status_line.clone())
        } else {
            return None;
        };
        Some(
            div()
                .absolute()
                .left(px(12.))
                .bottom(px(16.))
                .max_w(px(360.))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgba((p.panel << 8) | 0xEB))
                .shadow_lg()
                .text_xs()
                .text_color(rgb(p.text))
                .child(Icon::new(icon).size(px(16.)).text_color(rgb(p.accent)))
                .child(div().flex_1().min_w_0().child(text)),
        )
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        self.finish_session(false);
    }
}
