use crate::state::{record_audit, ApiError, AppState};
use agentdesk_identity::sha256_hex;
use agentdesk_protocol::{AgentToServerMessage, ServerToAgentMessage};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct AgentWsQuery {
    pub device_id: Uuid,
    pub token: String,
}

pub async fn ws_agent_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<AgentWsQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let token_hash = sha256_hex(q.token.as_bytes());
    let row = sqlx::query(
        "SELECT id, organization_id, public_code FROM devices WHERE id = $1 AND device_token_hash = $2",
    )
    .bind(q.device_id)
    .bind(&token_hash)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("invalid device credentials"))?;

    let org_id: Uuid = row.get("organization_id");
    let public_code: String = row.get("public_code");

    Ok(ws.on_upgrade(move |socket| {
        handle_agent_socket(socket, state, q.device_id, org_id, public_code)
    }))
}

async fn handle_agent_socket(
    socket: WebSocket,
    state: AppState,
    device_id: Uuid,
    org_id: Uuid,
    public_code: String,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToAgentMessage>();

    {
        let mut agents = state.active_agents.write().await;
        agents.insert(device_id, tx.clone());
    }

    let _ = sqlx::query(
        "UPDATE devices SET status = 'online', last_heartbeat_at = now() WHERE id = $1",
    )
    .bind(device_id)
    .execute(&state.pool)
    .await;

    state.emit_event(
        "device.online",
        org_id,
        device_id.to_string(),
        format!("Equipo {public_code} en línea"),
    );

    let _ = tx.send(ServerToAgentMessage::Welcome {
        device_id,
        public_code: public_code.clone(),
    });

    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(25));
        ping_interval.tick().await;
        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                maybe_msg = rx.recv() => {
                    let Some(msg) = maybe_msg else {
                        break;
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        if ws_tx.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut send_task => break,
            incoming = ws_rx.next() => {
                let Some(Ok(msg)) = incoming else { break; };
                match msg {
                    Message::Text(txt) => {
                        if let Ok(parsed) = serde_json::from_str::<AgentToServerMessage>(&txt) {
                            match parsed {
                                AgentToServerMessage::Heartbeat {
                                    screen_width,
                                    screen_height,
                                    private_mem_mb,
                                    mac_addresses,
                                    lan_ips,
                                } => {
                                    let _ = sqlx::query(
                                        "UPDATE devices
                                         SET status = 'online',
                                             screen_width = $2,
                                             screen_height = $3,
                                             private_mem_mb = $4,
                                             mac_addresses = CASE WHEN cardinality($5::text[]) > 0 THEN $5 ELSE mac_addresses END,
                                             lan_ips = CASE WHEN cardinality($6::text[]) > 0 THEN $6 ELSE lan_ips END,
                                             last_heartbeat_at = now()
                                         WHERE id = $1",
                                    )
                                    .bind(device_id)
                                    .bind(screen_width)
                                    .bind(screen_height)
                                    .bind(private_mem_mb)
                                    .bind(&mac_addresses)
                                    .bind(&lan_ips)
                                    .execute(&state.pool)
                                    .await;
                                }
                                AgentToServerMessage::SessionConsentResult {
                                    session_id,
                                    accepted,
                                    reason,
                                } => {
                                    let status = if accepted { "active" } else { "rejected" };
                                    let _ = sqlx::query(
                                        "UPDATE remote_sessions SET status = $2, started_at = CASE WHEN $3 THEN now() ELSE started_at END WHERE id = $1",
                                    )
                                    .bind(session_id)
                                    .bind(status)
                                    .bind(accepted)
                                    .execute(&state.pool)
                                    .await;
                                    record_audit(
                                        &state.pool,
                                        Some(org_id),
                                        Some(device_id),
                                        "device",
                                        "session.consent",
                                        "remote_session",
                                        &session_id.to_string(),
                                        status,
                                        json!({ "reason": reason }),
                                    )
                                    .await;
                                }
                                AgentToServerMessage::DiagnosticResult {
                                    request_id,
                                    ok,
                                    data,
                                    screenshot_jpeg_b64,
                                    error,
                                } => {
                                    if let Some(tx_wait) = state.pending_agent_requests.write().await.remove(&request_id) {
                                        let _ = tx_wait.send(json!({
                                            "ok": ok,
                                            "data": data,
                                            "screenshot_jpeg_b64": screenshot_jpeg_b64,
                                            "error": error,
                                        }));
                                    }
                                }
                                AgentToServerMessage::CommandResult {
                                    request_id,
                                    ok,
                                    exit_code,
                                    stdout,
                                    stderr,
                                    duration_ms,
                                } => {
                                    if let Some(tx_wait) = state.pending_agent_requests.write().await.remove(&request_id) {
                                        let _ = tx_wait.send(json!({
                                            "ok": ok,
                                            "exit_code": exit_code,
                                            "stdout": stdout,
                                            "stderr": stderr,
                                            "duration_ms": duration_ms,
                                        }));
                                    }
                                }
                                AgentToServerMessage::WolResult {
                                    request_id,
                                    ok,
                                    sent_count,
                                    detail,
                                } => {
                                    tracing::info!(
                                        %device_id,
                                        %request_id,
                                        ok,
                                        sent_count,
                                        %detail,
                                        "peer Wake-on-LAN relay reported"
                                    );
                                }
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    send_task.abort();
    let removed_current = {
        let mut agents = state.active_agents.write().await;
        if agents
            .get(&device_id)
            .is_some_and(|cur| cur.same_channel(&tx))
        {
            agents.remove(&device_id);
            true
        } else {
            false
        }
    };

    if removed_current {
        let _ = sqlx::query("UPDATE devices SET status = 'offline' WHERE id = $1")
            .bind(device_id)
            .execute(&state.pool)
            .await;

        state.emit_event(
            "device.offline",
            org_id,
            device_id.to_string(),
            format!("Equipo {public_code} desconectado"),
        );
    }
}

// ---------------------------------------------------------------------------
// Blind E2EE Relay (/ws/relay/{session_id})
// Never inspects or decrypts binary payloads (SEC-01).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RelayWsQuery {
    pub role: String,
    pub token: String,
}

pub async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    Query(q): Query<RelayWsQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let row = sqlx::query(
        "SELECT host_relay_token, viewer_relay_token, status FROM remote_sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("session not found"))?;

    let status: String = row.get("status");
    if status == "ended" || status == "rejected" {
        return Err(ApiError::forbidden("session already closed"));
    }

    let is_host = match q.role.as_str() {
        "host" => {
            let expected: String = row.get("host_relay_token");
            expected == q.token
        }
        "viewer" => {
            let expected: String = row.get("viewer_relay_token");
            expected == q.token
        }
        _ => return Err(ApiError::bad_request("invalid relay role")),
    };

    if !is_host && q.role != "viewer" {
        return Err(ApiError::unauthorized("invalid relay token"));
    }
    if (q.role == "host" && row.get::<String, _>("host_relay_token") != q.token)
        || (q.role == "viewer" && row.get::<String, _>("viewer_relay_token") != q.token)
    {
        return Err(ApiError::unauthorized("invalid relay token"));
    }

    let role_is_host = q.role == "host";
    Ok(ws.on_upgrade(move |socket| {
        handle_relay_socket(socket, state, session_id, role_is_host)
    }))
}

const MAX_PENDING_RELAY_FRAMES: usize = 64;

fn register_relay_peer(
    room: &mut crate::state::RelayRoom,
    role_is_host: bool,
    tx: mpsc::Sender<Vec<u8>>,
) -> bool {
    if role_is_host {
        room.host_tx = Some(tx.clone());
        for frame in std::mem::take(&mut room.pending_for_host) {
            let _ = tx.try_send(frame);
        }
    } else {
        room.viewer_tx = Some(tx.clone());
        for frame in std::mem::take(&mut room.pending_for_viewer) {
            let _ = tx.try_send(frame);
        }
    }
    room.host_tx.is_some() && room.viewer_tx.is_some()
}

fn cleanup_relay_peer(
    room: &mut crate::state::RelayRoom,
    role_is_host: bool,
    tx: &mpsc::Sender<Vec<u8>>,
) -> bool {
    if role_is_host {
        if room.host_tx.as_ref().is_some_and(|cur| cur.same_channel(tx)) {
            room.host_tx = None;
        }
    } else if room
        .viewer_tx
        .as_ref()
        .is_some_and(|cur| cur.same_channel(tx))
    {
        room.viewer_tx = None;
    }
    room.host_tx.is_none() && room.viewer_tx.is_none()
}

async fn handle_relay_socket(
    socket: WebSocket,
    state: AppState,
    session_id: Uuid,
    role_is_host: bool,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(24);

    let both_connected = {
        let mut rooms = state.relay_rooms.write().await;
        let room = rooms.entry(session_id).or_default();
        register_relay_peer(room, role_is_host, tx.clone())
    };
    if both_connected {
        let _ = sqlx::query(
            "UPDATE remote_sessions SET status = 'active', started_at = COALESCE(started_at, now()) WHERE id = $1 AND status = 'pending'",
        )
        .bind(session_id)
        .execute(&state.pool)
        .await;
    }

    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(25));
        ping_interval.tick().await;
        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                maybe_bytes = rx.recv() => {
                    let Some(bytes) = maybe_bytes else {
                        break;
                    };
                    if ws_tx.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut send_task => break,
            incoming = ws_rx.next() => {
                let Some(Ok(msg)) = incoming else { break; };
                match msg {
                    Message::Binary(data) => {
                        let peer_tx = {
                            let rooms = state.relay_rooms.read().await;
                            rooms.get(&session_id).and_then(|r| {
                                if role_is_host {
                                    r.viewer_tx.clone()
                                } else {
                                    r.host_tx.clone()
                                }
                            })
                        };
                        if let Some(peer) = peer_tx {
                            if peer.send(data.to_vec()).await.is_err() {
                                break;
                            }
                        } else {
                            // Peer hasn't joined yet (common across WAN/NAT latency); queue early handshake frames.
                            let peer_after_lock = {
                                let mut rooms = state.relay_rooms.write().await;
                                if let Some(room) = rooms.get_mut(&session_id) {
                                    if role_is_host {
                                        if let Some(viewer) = room.viewer_tx.clone() {
                                            Some(viewer)
                                        } else {
                                            if room.pending_for_viewer.len() < MAX_PENDING_RELAY_FRAMES {
                                                room.pending_for_viewer.push(data.to_vec());
                                            }
                                            None
                                        }
                                    } else if let Some(host) = room.host_tx.clone() {
                                        Some(host)
                                    } else {
                                        if room.pending_for_host.len() < MAX_PENDING_RELAY_FRAMES {
                                            room.pending_for_host.push(data.to_vec());
                                        }
                                        None
                                    }
                                } else {
                                    None
                                }
                            };
                            if let Some(peer) = peer_after_lock {
                                if peer.send(data.to_vec()).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    send_task.abort();
    {
        let mut rooms = state.relay_rooms.write().await;
        if let Some(room) = rooms.get_mut(&session_id) {
            if cleanup_relay_peer(room, role_is_host, &tx) {
                rooms.remove(&session_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RelayRoom;

    #[test]
    fn relay_room_flushes_pending_frames_and_guards_stale_disconnect() {
        let mut room = RelayRoom::default();
        room.pending_for_host.push(b"noise_m1".to_vec());

        let (host_tx, mut host_rx) = mpsc::channel(16);
        let (viewer_tx, _viewer_rx) = mpsc::channel(16);

        assert!(!register_relay_peer(&mut room, false, viewer_tx.clone()));
        assert!(register_relay_peer(&mut room, true, host_tx.clone()));
        assert!(room.pending_for_host.is_empty());
        assert_eq!(host_rx.try_recv().unwrap(), b"noise_m1".to_vec());

        // Stale host channel must not clear the current active host channel.
        let (stale_tx, _stale_rx) = mpsc::channel(16);
        assert!(!cleanup_relay_peer(&mut room, true, &stale_tx));
        assert!(room.host_tx.is_some());

        // Current host + viewer cleanup removes both.
        assert!(!cleanup_relay_peer(&mut room, true, &host_tx));
        assert!(cleanup_relay_peer(&mut room, false, &viewer_tx));
    }
}
