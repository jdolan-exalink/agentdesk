use agentdesk_protocol::{ControlMessage, DataPacket, InputEvent, ProtocolError};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UdpSocket;

const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_RESPONSE: u16 = 0x0101;
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

const MAGIC_PUNCH: &[u8; 4] = b"ADPN";
const MAGIC_KEEPALIVE: &[u8; 4] = b"ADKA";
const MAGIC_DATA: &[u8; 4] = b"ADU1";
const PUNCH_REQ: u8 = 0x01;
const PUNCH_ACK: u8 = 0x02;

const HEADER_LEN: usize = 24;
const MAX_FRAG_PLAIN: usize = 1160;
const DATA_FLAG_DROPPABLE_VIDEO: u8 = 0x01;

pub fn build_stun_binding_request(tx_id: [u8; 12]) -> [u8; 20] {
    let mut pkt = [0u8; 20];
    pkt[0..2].copy_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    pkt[2..4].copy_from_slice(&0u16.to_be_bytes());
    pkt[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    pkt[8..20].copy_from_slice(&tx_id);
    pkt
}

pub fn build_stun_binding_response(req: &[u8], peer: SocketAddr) -> Option<Vec<u8>> {
    if req.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([req[0], req[1]]);
    let cookie = u32::from_be_bytes([req[4], req[5], req[6], req[7]]);
    if msg_type != STUN_BINDING_REQUEST || cookie != STUN_MAGIC_COOKIE {
        return None;
    }

    let mut attr = Vec::with_capacity(24);
    match peer {
        SocketAddr::V4(v4) => {
            let xport = v4.port() ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
            let xip = u32::from_be_bytes(v4.ip().octets()) ^ STUN_MAGIC_COOKIE;
            attr.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
            attr.extend_from_slice(&8u16.to_be_bytes());
            attr.push(0);
            attr.push(0x01); // IPv4
            attr.extend_from_slice(&xport.to_be_bytes());
            attr.extend_from_slice(&xip.to_be_bytes());
        }
        SocketAddr::V6(v6) => {
            let xport = v6.port() ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
            let mut xip = v6.ip().octets();
            let magic = STUN_MAGIC_COOKIE.to_be_bytes();
            for i in 0..4 {
                xip[i] ^= magic[i];
            }
            for i in 0..12 {
                xip[4 + i] ^= req[8 + i];
            }
            attr.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
            attr.extend_from_slice(&20u16.to_be_bytes());
            attr.push(0);
            attr.push(0x02); // IPv6
            attr.extend_from_slice(&xport.to_be_bytes());
            attr.extend_from_slice(&xip);
        }
    }

    let mut out = Vec::with_capacity(20 + attr.len());
    out.extend_from_slice(&STUN_BINDING_RESPONSE.to_be_bytes());
    out.extend_from_slice(&(attr.len() as u16).to_be_bytes());
    out.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(&req[8..20]);
    out.extend_from_slice(&attr);
    Some(out)
}

pub fn parse_stun_binding_response(packet: &[u8], expected_tx_id: &[u8; 12]) -> Option<SocketAddr> {
    if packet.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([packet[0], packet[1]]);
    let msg_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    let cookie = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    if msg_type != STUN_BINDING_RESPONSE
        || cookie != STUN_MAGIC_COOKIE
        || &packet[8..20] != expected_tx_id
    {
        return None;
    }

    let end = (20 + msg_len).min(packet.len());
    let mut pos = 20usize;
    let mut mapped_fallback: Option<SocketAddr> = None;

    while pos + 4 <= end {
        let attr_type = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let attr_len = u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]) as usize;
        pos += 4;
        if pos + attr_len > end {
            break;
        }
        let val = &packet[pos..pos + attr_len];
        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS if val.len() >= 8 => {
                let family = val[1];
                let xport = u16::from_be_bytes([val[2], val[3]]) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                if family == 0x01 {
                    let ip_u32 = u32::from_be_bytes([val[4], val[5], val[6], val[7]]) ^ STUN_MAGIC_COOKIE;
                    return Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip_u32), xport)));
                } else if family == 0x02 && val.len() >= 20 {
                    let mut ip = [0u8; 16];
                    ip.copy_from_slice(&val[4..20]);
                    let magic = STUN_MAGIC_COOKIE.to_be_bytes();
                    for i in 0..4 {
                        ip[i] ^= magic[i];
                    }
                    for i in 0..12 {
                        ip[4 + i] ^= expected_tx_id[i];
                    }
                    return Some(SocketAddr::V6(SocketAddrV6::new(ip.into(), xport, 0, 0)));
                }
            }
            ATTR_MAPPED_ADDRESS if val.len() >= 8 => {
                let family = val[1];
                let port = u16::from_be_bytes([val[2], val[3]]);
                if family == 0x01 {
                    let ip = Ipv4Addr::new(val[4], val[5], val[6], val[7]);
                    mapped_fallback = Some(SocketAddr::V4(SocketAddrV4::new(ip, port)));
                }
            }
            _ => {}
        }
        pos += (attr_len + 3) & !3;
    }

    mapped_fallback
}

pub fn discover_local_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    let mut seen = HashSet::new();
    for probe in ["192.168.99.8:80", "10.0.0.1:80", "192.168.1.1:80", "1.1.1.1:80"] {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(probe).is_ok() {
                if let Ok(local) = sock.local_addr() {
                    let ip = local.ip();
                    if !ip.is_unspecified() && !ip.is_loopback() && seen.insert(ip) {
                        ips.push(ip);
                    }
                }
            }
        }
    }
    ips
}

/// Discovers LAN + public reflexive UDP candidates on an already-bound socket.
pub async fn discover_udp_candidates(socket: &UdpSocket, extra_stun: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let local_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
    if local_port > 0 {
        for ip in discover_local_ips() {
            let s = SocketAddr::new(ip, local_port).to_string();
            if seen.insert(s.clone()) {
                out.push(s);
            }
        }
    }

    let mut stun_servers: Vec<String> = Vec::new();
    if let Some(extra) = extra_stun {
        if !extra.trim().is_empty() {
            stun_servers.push(extra.trim().to_string());
        }
    }
    stun_servers.push("stun.cloudflare.com:3478".to_string());
    stun_servers.push("stun.l.google.com:19302".to_string());

    let mut tx_ids: Vec<[u8; 12]> = Vec::new();
    for host in &stun_servers {
        if let Ok(addrs) = tokio::net::lookup_host(host.as_str()).await {
            if let Some(addr) = addrs.filter(|a| a.is_ipv4()).next() {
                let mut tx = [0u8; 12];
                rand::thread_rng().fill_bytes(&mut tx);
                let req = build_stun_binding_request(tx);
                let _ = socket.send_to(&req, addr).await;
                tx_ids.push(tx);
            }
        }
    }

    if !tx_ids.is_empty() {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(650);
        let mut buf = [0u8; 512];
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, socket.recv_from(&mut buf)).await {
                Ok(Ok((n, _src))) => {
                    for tx in &tx_ids {
                        if let Some(mapped) = parse_stun_binding_response(&buf[..n], tx) {
                            let s = mapped.to_string();
                            if seen.insert(s.clone()) {
                                out.push(s);
                            }
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    out
}

fn is_private_addr(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    }
}

pub fn bind_p2p_udp_socket() -> std::io::Result<Arc<UdpSocket>> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = sock.set_recv_buffer_size(1024 * 1024);
    let _ = sock.set_send_buffer_size(1024 * 1024);
    sock.set_nonblocking(true)?;
    let addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
    sock.bind(&addr.into())?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(Arc::new(UdpSocket::from_std(std_sock)?))
}

pub fn extract_stun_addr_from_ws_url(ws_url: &str) -> Option<String> {
    let after_scheme = ws_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(ws_url);
    let authority = after_scheme.split('/').next()?.trim();
    if authority.is_empty() {
        return None;
    }
    let host = if authority.starts_with('[') {
        let end = authority.find(']')?;
        &authority[..=end]
    } else {
        authority.split(':').next()?
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("{host}:3478"))
}

pub fn spawn_p2p_punch(
    socket: Arc<UdpSocket>,
    nonce: [u8; 16],
    key: [u8; 32],
    endpoints: Vec<String>,
    is_host: bool,
) -> tokio::sync::oneshot::Receiver<P2pUdpChannel> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Some(peer) = punch_udp_peer(socket.clone(), nonce, endpoints, is_host).await {
            let _ = tx.send(P2pUdpChannel::new(socket, key, nonce, peer, is_host));
        }
    });
    rx
}

/// Discovers host UDP candidates against STUN/LAN and spawns hole-punching toward viewer endpoints.
pub fn start_host_p2p_handshake(
    relay_url: String,
    session_nonce: [u8; 16],
    p2p_key: [u8; 32],
    viewer_endpoints: Vec<String>,
) -> (
    tokio::sync::oneshot::Receiver<Vec<String>>,
    tokio::sync::oneshot::Receiver<P2pUdpChannel>,
) {
    let (ep_tx, ep_rx) = tokio::sync::oneshot::channel();
    let (ch_tx, ch_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let Ok(sock) = bind_p2p_udp_socket() else {
            return;
        };
        let stun = extract_stun_addr_from_ws_url(&relay_url);
        let my_eps = discover_udp_candidates(&sock, stun.as_deref()).await;
        let _ = ep_tx.send(my_eps);
        if let Some(peer) =
            punch_udp_peer(sock.clone(), session_nonce, viewer_endpoints, true).await
        {
            let _ = ch_tx.send(P2pUdpChannel::new(
                sock,
                p2p_key,
                session_nonce,
                peer,
                true,
            ));
        }
    });
    (ep_rx, ch_rx)
}

pub fn should_route_over_udp(pkt: &DataPacket) -> bool {
    matches!(
        pkt,
        DataPacket::VideoFrame(_)
            | DataPacket::Input(InputEvent::MouseMoveNorm { .. })
            | DataPacket::Control(
                ControlMessage::CursorUpdate { .. }
                    | ControlMessage::FrameAck { .. }
                    | ControlMessage::Ping { .. }
                    | ControlMessage::Pong { .. }
            )
    )
}

pub async fn send_adaptive_packet<S>(
    tunnel: &mut crate::RelayTunnel<S>,
    udp: &mut Option<P2pUdpChannel>,
    pkt: &DataPacket,
) -> Result<(), crate::TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if should_route_over_udp(pkt) {
        if let Some(ch) = udp.as_mut() {
            if ch.is_alive() {
                let droppable = matches!(pkt, DataPacket::VideoFrame(_));
                if ch.send_packet(pkt, droppable).await.is_ok() {
                    return Ok(());
                }
            }
            *udp = None;
        }
    }
    tunnel.send_packet(pkt).await
}

/// Attempts direct UDP hole-punching across both LAN and STUN-discovered candidates.
pub async fn punch_udp_peer(
    socket: Arc<UdpSocket>,
    nonce: [u8; 16],
    endpoints: Vec<String>,
    is_host: bool,
) -> Option<SocketAddr> {
    let mut targets: Vec<SocketAddr> = endpoints
        .iter()
        .filter_map(|e| e.parse::<SocketAddr>().ok())
        .collect();
    if targets.is_empty() {
        return None;
    }
    // Try LAN candidates before WAN reflexive candidates.
    targets.sort_by_key(|a| if is_private_addr(a) { 0u8 } else { 1u8 });

    let mut req_pkt = [0u8; 22];
    req_pkt[0..4].copy_from_slice(MAGIC_PUNCH);
    req_pkt[4..20].copy_from_slice(&nonce);
    req_pkt[20] = PUNCH_REQ;
    req_pkt[21] = if is_host { 1 } else { 0 };

    let mut ack_pkt = req_pkt;
    ack_pkt[20] = PUNCH_ACK;

    let deadline = tokio::time::sleep(Duration::from_millis(2500));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let mut buf = [0u8; 256];
    let mut confirmed_peer: Option<SocketAddr> = None;
    let mut sent_ack = false;
    let mut got_ack = false;

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = tick.tick() => {
                for addr in &targets {
                    let _ = socket.send_to(&req_pkt, *addr).await;
                }
                if let Some(peer) = confirmed_peer {
                    let _ = socket.send_to(&ack_pkt, peer).await;
                    sent_ack = true;
                }
            }
            recv = socket.recv_from(&mut buf) => {
                let Ok((n, src)) = recv else { break; };
                if n >= 22 && &buf[0..4] == MAGIC_PUNCH && buf[4..20] == nonce {
                    let kind = buf[20];
                    let peer_role = buf[21] != 0;
                    if peer_role == is_host {
                        continue;
                    }
                    confirmed_peer = Some(src);
                    if kind == PUNCH_REQ {
                        let _ = socket.send_to(&ack_pkt, src).await;
                        let _ = socket.send_to(&ack_pkt, src).await;
                        sent_ack = true;
                    } else if kind == PUNCH_ACK {
                        if !sent_ack {
                            let _ = socket.send_to(&ack_pkt, src).await;
                            let _ = socket.send_to(&ack_pkt, src).await;
                            sent_ack = true;
                        }
                        got_ack = true;
                    }
                    if got_ack && sent_ack {
                        return Some(src);
                    }
                }
            }
        }
    }

    if got_ack || sent_ack {
        confirmed_peer
    } else {
        None
    }
}

struct PartialMsg {
    frag_total: u16,
    droppable: bool,
    created_at: Instant,
    frags: BTreeMap<u16, Vec<u8>>,
}

struct UdpReassembler {
    slots: HashMap<u32, PartialMsg>,
    highest_completed_video: u32,
}

impl UdpReassembler {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
            highest_completed_video: 0,
        }
    }

    fn insert(
        &mut self,
        msg_id: u32,
        frag_idx: u16,
        frag_total: u16,
        droppable: bool,
        plain_chunk: Vec<u8>,
    ) -> Option<Vec<u8>> {
        if frag_total == 0 || frag_idx >= frag_total {
            return None;
        }
        if droppable && msg_id <= self.highest_completed_video && self.highest_completed_video > 0 {
            return None;
        }
        if frag_total == 1 {
            if droppable && msg_id > self.highest_completed_video {
                self.highest_completed_video = msg_id;
            }
            return Some(plain_chunk);
        }

        let now = Instant::now();
        self.slots.retain(|&id, slot| {
            if slot.droppable && now.duration_since(slot.created_at) > Duration::from_millis(250) {
                return false;
            }
            if !slot.droppable && now.duration_since(slot.created_at) > Duration::from_secs(4) {
                return false;
            }
            !(slot.droppable && msg_id > id.saturating_add(2) && droppable)
        });

        if !self.slots.contains_key(&msg_id) && self.slots.len() >= 12 {
            if let Some(&drop_id) = self
                .slots
                .iter()
                .min_by_key(|(_, s)| (!s.droppable as u8, s.created_at))
                .map(|(id, _)| id)
            {
                self.slots.remove(&drop_id);
            }
        }

        let entry = self.slots.entry(msg_id).or_insert_with(|| PartialMsg {
            frag_total,
            droppable,
            created_at: now,
            frags: BTreeMap::new(),
        });
        if entry.frag_total != frag_total {
            return None;
        }
        entry.frags.entry(frag_idx).or_insert(plain_chunk);
        if entry.frags.len() == frag_total as usize {
            let done = self.slots.remove(&msg_id)?;
            if droppable && msg_id > self.highest_completed_video {
                self.highest_completed_video = msg_id;
            }
            let total_len: usize = done.frags.values().map(|v| v.len()).sum();
            let mut out = Vec::with_capacity(total_len);
            for (_, chunk) in done.frags {
                out.extend_from_slice(&chunk);
            }
            return Some(out);
        }
        None
    }
}

pub struct P2pUdpChannel {
    socket: Arc<UdpSocket>,
    cipher: ChaCha20Poly1305,
    nonce: [u8; 16],
    peer: SocketAddr,
    is_host: bool,
    next_msg_id: u32,
    next_nonce_seq: u64,
    reassembler: UdpReassembler,
    last_rx: Instant,
    last_tx: Instant,
}

impl P2pUdpChannel {
    pub fn new(
        socket: Arc<UdpSocket>,
        key: [u8; 32],
        nonce: [u8; 16],
        peer: SocketAddr,
        is_host: bool,
    ) -> Self {
        let cipher = ChaCha20Poly1305::new((&key).into());
        let now = Instant::now();
        Self {
            socket,
            cipher,
            nonce,
            peer,
            is_host,
            next_msg_id: 1,
            next_nonce_seq: 1,
            reassembler: UdpReassembler::new(),
            last_rx: now,
            last_tx: now,
        }
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    pub fn socket(&self) -> &Arc<UdpSocket> {
        &self.socket
    }

    pub fn is_alive(&self) -> bool {
        self.last_rx.elapsed() < Duration::from_millis(3500)
    }

    pub async fn maybe_send_keepalive(&mut self) {
        if self.last_tx.elapsed() >= Duration::from_millis(900) {
            let mut ka = [0u8; 20];
            ka[0..4].copy_from_slice(MAGIC_KEEPALIVE);
            ka[4..20].copy_from_slice(&self.nonce);
            if self.socket.send_to(&ka, self.peer).await.is_ok() {
                self.last_tx = Instant::now();
            }
        }
    }

    pub async fn send_packet(
        &mut self,
        pkt: &DataPacket,
        droppable_video: bool,
    ) -> Result<(), ProtocolError> {
        let raw = pkt.encode()?;
        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1).max(1);

        let chunks: Vec<&[u8]> = if raw.is_empty() {
            vec![&[]]
        } else {
            raw.chunks(MAX_FRAG_PLAIN).collect()
        };
        let frag_total = chunks.len().min(u16::MAX as usize) as u16;
        let dir_byte = if self.is_host { 1u8 } else { 0u8 };
        let flags = if droppable_video { DATA_FLAG_DROPPABLE_VIDEO } else { 0 };

        for (idx, chunk) in chunks.into_iter().take(frag_total as usize).enumerate() {
            let nseq = self.next_nonce_seq;
            self.next_nonce_seq = self.next_nonce_seq.wrapping_add(1);
            let mut aead_nonce = [0u8; 12];
            aead_nonce[0] = dir_byte;
            aead_nonce[1..9].copy_from_slice(&nseq.to_le_bytes());

            let Ok(cipher_chunk) = self.cipher.encrypt(Nonce::from_slice(&aead_nonce), chunk) else {
                continue;
            };

            let mut datagram = Vec::with_capacity(HEADER_LEN + cipher_chunk.len());
            datagram.extend_from_slice(MAGIC_DATA);
            datagram.extend_from_slice(&msg_id.to_le_bytes());
            datagram.extend_from_slice(&(idx as u16).to_le_bytes());
            datagram.extend_from_slice(&frag_total.to_le_bytes());
            datagram.push(flags);
            datagram.push(dir_byte);
            datagram.extend_from_slice(&[0u8; 2]);
            datagram.extend_from_slice(&nseq.to_le_bytes());
            datagram.extend_from_slice(&cipher_chunk);

            if self.socket.send_to(&datagram, self.peer).await.is_ok() {
                self.last_tx = Instant::now();
            }
        }
        Ok(())
    }

    pub fn try_feed_datagram(&mut self, datagram: &[u8]) -> Option<DataPacket> {
        if datagram.len() >= 20 && &datagram[0..4] == MAGIC_KEEPALIVE {
            if datagram[4..20] == self.nonce {
                self.last_rx = Instant::now();
            }
            return None;
        }
        if datagram.len() >= 22 && &datagram[0..4] == MAGIC_PUNCH {
            if datagram[4..20] == self.nonce {
                self.last_rx = Instant::now();
            }
            return None;
        }
        if datagram.len() < HEADER_LEN || &datagram[0..4] != MAGIC_DATA {
            return None;
        }

        let expected_remote_dir = if self.is_host { 0u8 } else { 1u8 };
        let dir = datagram[13];
        if dir != expected_remote_dir {
            return None;
        }

        let msg_id = u32::from_le_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]);
        let frag_idx = u16::from_le_bytes([datagram[8], datagram[9]]);
        let frag_total = u16::from_le_bytes([datagram[10], datagram[11]]);
        let flags = datagram[12];
        let droppable = (flags & DATA_FLAG_DROPPABLE_VIDEO) != 0;
        let mut nseq_bytes = [0u8; 8];
        nseq_bytes.copy_from_slice(&datagram[16..24]);

        let mut aead_nonce = [0u8; 12];
        aead_nonce[0] = dir;
        aead_nonce[1..9].copy_from_slice(&nseq_bytes);

        let plain = self
            .cipher
            .decrypt(Nonce::from_slice(&aead_nonce), &datagram[HEADER_LEN..])
            .ok()?;
        self.last_rx = Instant::now();

        let assembled = self
            .reassembler
            .insert(msg_id, frag_idx, frag_total, droppable, plain)?;
        DataPacket::decode(&assembled).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdesk_protocol::{FrameEncoding, InputEvent, VideoFrame};

    #[test]
    fn stun_request_response_and_parse_roundtrip() {
        let tx_id = [0x42u8; 12];
        let req = build_stun_binding_request(tx_id);
        let peer: SocketAddr = "203.0.113.55:51423".parse().unwrap();
        let resp = build_stun_binding_response(&req, peer).expect("valid STUN response");
        let parsed = parse_stun_binding_response(&resp, &tx_id).expect("parsed mapped addr");
        assert_eq!(parsed, peer);
    }

    #[tokio::test]
    async fn udp_punch_and_encrypted_fragmented_video_delivery() {
        let host_sock = bind_p2p_udp_socket().unwrap();
        let viewer_sock = bind_p2p_udp_socket().unwrap();
        let host_addr = format!("127.0.0.1:{}", host_sock.local_addr().unwrap().port());
        let viewer_addr = format!("127.0.0.1:{}", viewer_sock.local_addr().unwrap().port());

        let nonce = [7u8; 16];
        let key = [9u8; 32];

        let hs = host_sock.clone();
        let vs = viewer_sock.clone();
        let (h_res, v_res) = tokio::join!(
            punch_udp_peer(hs, nonce, vec![viewer_addr], true),
            punch_udp_peer(vs, nonce, vec![host_addr], false),
        );
        let h_peer = h_res.expect("host punch should succeed");
        let v_peer = v_res.expect("viewer punch should succeed");

        let mut host_ch = P2pUdpChannel::new(host_sock.clone(), key, nonce, h_peer, true);
        let mut viewer_ch = P2pUdpChannel::new(viewer_sock.clone(), key, nonce, v_peer, false);

        // 48 KB payload forces >40 MTU-safe encrypted UDP fragments
        let big_payload: Vec<u8> = (0..(48 * 1024)).map(|i| (i % 251) as u8).collect();
        let pkt = DataPacket::VideoFrame(VideoFrame {
            seq: 99,
            timestamp_ms: 12345,
            width: 1600,
            height: 900,
            encoding: FrameEncoding::HybridTiles,
            payload: big_payload.clone(),
        });
        host_ch.send_packet(&pkt, true).await.unwrap();

        let mut buf = vec![0u8; 2048];
        let mut decoded_video = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok((n, _))) =
                tokio::time::timeout(Duration::from_millis(200), viewer_sock.recv_from(&mut buf))
                    .await
            {
                if let Some(p) = viewer_ch.try_feed_datagram(&buf[..n]) {
                    decoded_video = Some(p);
                    break;
                }
            }
        }
        match decoded_video.expect("reassembled video frame") {
            DataPacket::VideoFrame(vf) => {
                assert_eq!(vf.seq, 99);
                assert_eq!(vf.payload, big_payload);
            }
            _ => panic!("expected VideoFrame"),
        }

        // Verify reverse direction (viewer -> host InputEvent)
        viewer_ch
            .send_packet(&DataPacket::Input(InputEvent::MouseMoveNorm { x: 0.4, y: 0.6 }), false)
            .await
            .unwrap();
        let input_pkt = {
            let mut got = None;
            let dl = tokio::time::Instant::now() + Duration::from_secs(1);
            while tokio::time::Instant::now() < dl {
                if let Ok(Ok((n, _))) =
                    tokio::time::timeout(Duration::from_millis(200), host_sock.recv_from(&mut buf))
                        .await
                {
                    if let Some(p) = host_ch.try_feed_datagram(&buf[..n]) {
                        got = Some(p);
                        break;
                    }
                }
            }
            got.expect("decoded InputEvent from viewer")
        };
        assert!(matches!(input_pkt, DataPacket::Input(InputEvent::MouseMoveNorm { .. })));
    }
}
