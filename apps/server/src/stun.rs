use std::net::SocketAddr;
use tokio::net::UdpSocket;

const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_RESPONSE: u16 = 0x0101;
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

fn build_stun_binding_response(req: &[u8], peer: SocketAddr) -> Option<Vec<u8>> {
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
            attr.push(0x01);
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
            attr.push(0x02);
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

/// Spawns a lightweight RFC 5389 STUN Binding responder on `0.0.0.0:3478` so peers
/// behind NAT can discover reflexive UDP endpoints against our own server too.
pub fn spawn_stun_responder() {
    tokio::spawn(async move {
        let sock = match UdpSocket::bind("0.0.0.0:3478").await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "STUN responder could not bind UDP 0.0.0.0:3478");
                return;
            }
        };
        tracing::info!("Built-in RFC 5389 STUN responder listening on 0.0.0.0:3478/udp");
        let mut buf = [0u8; 512];
        loop {
            match sock.recv_from(&mut buf).await {
                Ok((n, src)) => {
                    if let Some(resp) = build_stun_binding_response(&buf[..n], src) {
                        let _ = sock.send_to(&resp, src).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "STUN responder recv error");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });
}
