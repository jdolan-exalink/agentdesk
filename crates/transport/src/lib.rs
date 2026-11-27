pub mod p2p;
pub mod wol;

pub use p2p::{
    bind_p2p_udp_socket, discover_udp_candidates, extract_stun_addr_from_ws_url, punch_udp_peer,
    send_adaptive_packet, spawn_p2p_punch, start_host_p2p_handshake, P2pUdpChannel,
};
pub use wol::{
    build_wol_magic_packet, enumerate_local_mac_and_ips, format_mac, parse_mac,
    send_wol_magic_packets,
};

use agentdesk_identity::{IdentityError, NoiseChannel, NoiseHandshake};
use agentdesk_protocol::{ControlMessage, DataPacket, FileDropTarget, ProtocolError};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::sync::Once;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

static TLS_PROVIDER_ONCE: Once = Once::new();

/// Ensures a process-wide rustls CryptoProvider (`ring`) is installed before
/// `tokio-tungstenite` attempts any `wss://` handshake.
pub fn ensure_ws_tls_provider() {
    TLS_PROVIDER_ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Connects a WebSocket (supporting both `ws://` and `wss://`) with `TCP_NODELAY`
/// enabled and the rustls crypto provider initialized.
pub async fn connect_ws(
    url: &str,
) -> Result<
    (WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Response),
    tokio_tungstenite::tungstenite::Error,
> {
    ensure_ws_tls_provider();
    connect_async_with_config(url, None, true).await
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("websocket error: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("noise e2ee error: {0}")]
    Identity(#[from] IdentityError),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("connection closed")]
    Closed,
}

pub struct RelayTunnel<S> {
    ws: WebSocketStream<S>,
    noise: NoiseChannel,
}

impl RelayTunnel<MaybeTlsStream<tokio::net::TcpStream>> {
    pub async fn connect(url: &str, is_initiator: bool) -> Result<Self, TransportError> {
        let (ws, _) = connect_ws(url).await?;
        Self::handshake_over_ws(ws, is_initiator).await
    }
}

impl<S> RelayTunnel<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn handshake_over_ws(
        mut ws: WebSocketStream<S>,
        is_initiator: bool,
    ) -> Result<Self, TransportError> {
        let noise = if is_initiator {
            let mut hs = NoiseHandshake::new_initiator()?;
            let m1 = hs.write_message(&[])?;
            ws.send(Message::Binary(m1.into())).await?;

            let m2 = recv_ws_binary(&mut ws).await?;
            hs.read_message(&m2)?;

            let m3 = hs.write_message(&[])?;
            ws.send(Message::Binary(m3.into())).await?;
            hs.into_transport()?
        } else {
            let mut hs = NoiseHandshake::new_responder()?;
            let m1 = recv_ws_binary(&mut ws).await?;
            hs.read_message(&m1)?;

            let m2 = hs.write_message(&[])?;
            ws.send(Message::Binary(m2.into())).await?;

            let m3 = recv_ws_binary(&mut ws).await?;
            hs.read_message(&m3)?;
            hs.into_transport()?
        };

        Ok(Self { ws, noise })
    }

    pub async fn send_packet(&mut self, pkt: &DataPacket) -> Result<(), TransportError> {
        let raw = pkt.encode()?;
        let cipher = self.noise.encrypt(&raw)?;
        self.ws.send(Message::Binary(cipher.into())).await?;
        Ok(())
    }

    pub async fn recv_packet(&mut self) -> Result<DataPacket, TransportError> {
        let cipher = recv_ws_binary(&mut self.ws).await?;
        let plain = self.noise.decrypt(&cipher)?;
        Ok(DataPacket::decode(&plain)?)
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        self.ws.close(None).await?;
        Ok(())
    }

    pub async fn send_files_to_target(
        &mut self,
        paths: &[std::path::PathBuf],
        target: FileDropTarget,
    ) -> Result<usize, TransportError> {
        let mut sent = 0usize;
        for path in paths {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if bytes.len() > 64 * 1024 * 1024 {
                continue;
            }
            let transfer_id = uuid::Uuid::new_v4();
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archivo.bin".into());
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let sha256 = format!("{:x}", hasher.finalize());

            self.send_packet(&DataPacket::Control(ControlMessage::FileOfferEx {
                transfer_id,
                filename,
                size_bytes: bytes.len() as u64,
                sha256,
                target,
            }))
            .await?;

            const CHUNK: usize = 64 * 1024;
            let total = bytes.len();
            if total == 0 {
                self.send_packet(&DataPacket::Control(ControlMessage::FileChunk {
                    transfer_id,
                    offset: 0,
                    data: Vec::new(),
                    eof: true,
                }))
                .await?;
            } else {
                for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
                    let offset = (i * CHUNK) as u64;
                    let eof = (i + 1) * CHUNK >= total;
                    self.send_packet(&DataPacket::Control(ControlMessage::FileChunk {
                        transfer_id,
                        offset,
                        data: chunk.to_vec(),
                        eof,
                    }))
                    .await?;
                }
            }
            sent += 1;
        }
        Ok(sent)
    }
}

pub fn verify_sha256_hex(expected: &str, data: &[u8]) -> bool {
    if expected.trim().is_empty() {
        return true;
    }
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    actual.eq_ignore_ascii_case(expected.trim())
}

pub fn store_received_file_for_target(
    target: FileDropTarget,
    transfer_id: uuid::Uuid,
    filename: &str,
    data: &[u8],
) -> Result<std::path::PathBuf, std::io::Error> {
    let safe_name = std::path::Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("archivo_{transfer_id}.bin"));

    let base_home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());

    let dest_dir = match target {
        FileDropTarget::Desktop => base_home.join("Desktop"),
        FileDropTarget::Clipboard => std::env::temp_dir()
            .join("AgentDesk")
            .join("Clipboard")
            .join(transfer_id.to_string()),
        FileDropTarget::RemotePrint => std::env::temp_dir()
            .join("AgentDeskPrint")
            .join(transfer_id.to_string()),
        FileDropTarget::Downloads => base_home.join("Downloads").join("AgentDesk"),
    };

    std::fs::create_dir_all(&dest_dir)?;
    let mut dest_path = dest_dir.join(&safe_name);
    if target == FileDropTarget::Desktop && dest_path.exists() {
        let stem = std::path::Path::new(&safe_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("archivo");
        let ext = std::path::Path::new(&safe_name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        for idx in 1..=99u32 {
            let candidate_name = if ext.is_empty() {
                format!("{stem}_{idx}")
            } else {
                format!("{stem}_{idx}.{ext}")
            };
            let cand = dest_dir.join(candidate_name);
            if !cand.exists() {
                dest_path = cand;
                break;
            }
        }
    }
    std::fs::write(&dest_path, data)?;
    Ok(dest_path)
}

async fn recv_ws_binary<S>(ws: &mut WebSocketStream<S>) -> Result<Vec<u8>, TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Binary(b) => return Ok(b.to_vec()),
            Message::Close(_) => return Err(TransportError::Closed),
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => continue,
        }
    }
    Err(TransportError::Closed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wss_connect_uses_tls_instead_of_tls_disabled_error() {
        ensure_ws_tls_provider();
        let err = connect_ws("wss://127.0.0.1:1")
            .await
            .expect_err("port 1 should fail at TCP connect");
        let dbg = format!("{err:?}");
        assert!(
            !dbg.contains("TlsFeatureNotEnabled"),
            "TLS feature must be enabled in tokio-tungstenite: {dbg}"
        );
        assert!(
            matches!(err, tokio_tungstenite::tungstenite::Error::Io(_)),
            "expected TCP Io connection refusal, got: {dbg}"
        );
    }
}
