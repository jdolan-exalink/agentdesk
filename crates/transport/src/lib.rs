use agentdesk_identity::{IdentityError, NoiseChannel, NoiseHandshake};
use agentdesk_protocol::{DataPacket, ProtocolError};
use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, WebSocketStream};

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

impl RelayTunnel<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    pub async fn connect(url: &str, is_initiator: bool) -> Result<Self, TransportError> {
        let (ws, _) = connect_async(url).await?;
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
