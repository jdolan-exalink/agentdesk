use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_DATA_PACKET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("wire packet exceeds maximum allowed size ({0} bytes)")]
    PacketTooLarge(usize),
    #[error("serialization error: {0}")]
    Codec(#[from] bincode::Error),
}

// ---------------------------------------------------------------------------
// E2EE Binary Data-Plane Protocol (travels encrypted inside Noise_XX)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameEncoding {
    Jpeg,
    Lz4Rgba,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub encoding: FrameEncoding,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMoveNorm { x: f32, y: f32 },
    MouseButton { button: MouseBtn, pressed: bool },
    MouseWheel { delta_y: i32 },
    Key { vk: u16, pressed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    Ping { ts_ms: u64 },
    Pong { ts_ms: u64 },
    Chat { sender: String, text: String },
    SetQuality { jpeg_quality: u8, max_fps: u8 },
    Terminate { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataPacket {
    VideoFrame(VideoFrame),
    Input(InputEvent),
    Control(ControlMessage),
}

impl DataPacket {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let bytes = bincode::serialize(self)?;
        if bytes.len() > MAX_DATA_PACKET_BYTES {
            return Err(ProtocolError::PacketTooLarge(bytes.len()));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_DATA_PACKET_BYTES {
            return Err(ProtocolError::PacketTooLarge(bytes.len()));
        }
        Ok(bincode::deserialize(bytes)?)
    }
}

// ---------------------------------------------------------------------------
// Agent Control WebSocket Messages (/ws/agent)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToAgentMessage {
    Welcome {
        device_id: Uuid,
        public_code: String,
    },
    IncomingSession {
        session_id: Uuid,
        technician_name: String,
        allow_control: bool,
        allow_clipboard: bool,
        allow_files: bool,
        relay_path: String,
        host_relay_token: String,
    },
    TerminateSession {
        session_id: Uuid,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentToServerMessage {
    Heartbeat {
        screen_width: i32,
        screen_height: i32,
        private_mem_mb: f64,
    },
    SessionConsentResult {
        session_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// HTTP REST API DTOs (/api/v1/*)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub organization_name: String,
    pub organization_slug: String,
    pub admin_email: String,
    pub admin_name: String,
    pub admin_password: String,
    pub bootstrap_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUserInfo {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub organization_name: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub user: AuthUserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnrollmentRequest {
    pub label: String,
    pub max_uses: Option<i32>,
    pub expires_hours: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub label: String,
    pub max_uses: i32,
    pub used_count: i32,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnrollmentResponse {
    pub enrollment: EnrollmentDto,
    pub enrollment_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollDeviceRequest {
    pub enrollment_token: Option<String>,
    pub support_code: Option<String>,
    pub ed25519_pubkey: String,
    pub hostname: String,
    pub os_name: String,
    pub os_arch: String,
    pub agent_version: String,
    pub kind: String,
    pub screen_width: i32,
    pub screen_height: i32,
    pub private_mem_mb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollDeviceResponse {
    pub device_id: Uuid,
    pub organization_id: Uuid,
    pub public_code: String,
    pub device_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickRegisterRequest {
    pub ed25519_pubkey: String,
    pub hostname: String,
    pub os_name: String,
    pub os_arch: String,
    pub agent_version: String,
    pub screen_width: i32,
    pub screen_height: i32,
    pub private_mem_mb: f64,
    pub access_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickRegisterResponse {
    pub device_id: Uuid,
    pub organization_id: Uuid,
    pub connection_id: String,
    pub device_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotatePasswordRequest {
    pub device_id: Uuid,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConnectRequest {
    pub connection_id: String,
    pub password: String,
    pub caller_name: Option<String>,
    pub allow_control: bool,
    pub allow_clipboard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConnectResponse {
    pub session_id: Uuid,
    pub viewer_relay_token: String,
    pub relay_ws_path: String,
    pub target_hostname: String,
    pub target_connection_id: String,
    pub device_pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub device_id: Uuid,
    pub screen_width: i32,
    pub screen_height: i32,
    pub private_mem_mb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub public_code: String,
    pub ed25519_pubkey: String,
    pub hostname: String,
    pub os_name: String,
    pub os_arch: String,
    pub agent_version: String,
    pub kind: String,
    pub screen_width: i32,
    pub screen_height: i32,
    pub private_mem_mb: f64,
    pub status: String,
    pub last_heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBookEntryDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub device_id: Option<Uuid>,
    pub alias: String,
    pub client_company: String,
    pub contact_name: String,
    pub contact_email: String,
    pub group_name: String,
    pub is_favorite: bool,
    pub notes: String,
    pub scheduled_next_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub device_public_code: Option<String>,
    pub device_hostname: Option<String>,
    pub device_status: Option<String>,
    pub device_os: Option<String>,
    pub device_last_heartbeat: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertAddressBookRequest {
    pub device_id: Option<Uuid>,
    pub alias: String,
    pub client_company: String,
    pub contact_name: String,
    pub contact_email: String,
    pub group_name: String,
    pub is_favorite: bool,
    pub notes: String,
    pub scheduled_next_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupportRequest {
    pub requester_name: String,
    pub requester_contact: String,
    pub company_name: Option<String>,
    pub category: Option<String>,
    pub description: String,
    pub consent_accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupportResponse {
    pub ticket_id: Uuid,
    pub public_code: String,
    pub tracking_token: String,
    pub deep_link: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportTicketDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub public_code: String,
    pub requester_name: String,
    pub requester_contact: String,
    pub company_name: String,
    pub category: String,
    pub description: String,
    pub status: String,
    pub device_id: Option<Uuid>,
    pub assigned_to: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub device_id: Uuid,
    pub ticket_id: Option<Uuid>,
    pub allow_control: bool,
    pub allow_clipboard: bool,
    pub allow_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub device_id: Uuid,
    pub ticket_id: Option<Uuid>,
    pub technician_id: Option<Uuid>,
    pub allow_control: bool,
    pub allow_clipboard: bool,
    pub allow_files: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session: SessionDto,
    pub viewer_relay_token: String,
    pub relay_ws_path: String,
    pub device_pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEventDto {
    pub id: i64,
    pub organization_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub actor_type: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub outcome: String,
    pub metadata: serde_json::Value,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadArtifactDto {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub platform: String,
    pub architecture: String,
    pub version: String,
    pub available: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub download_url: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEvent {
    pub kind: String,
    pub organization_id: Uuid,
    pub resource_id: String,
    pub summary: String,
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_packet_roundtrip() {
        let pkt = DataPacket::VideoFrame(VideoFrame {
            seq: 42,
            timestamp_ms: 123456,
            width: 1920,
            height: 1080,
            encoding: FrameEncoding::Jpeg,
            payload: vec![1, 2, 3, 4, 5],
        });
        let raw = pkt.encode().unwrap();
        let decoded = DataPacket::decode(&raw).unwrap();
        match decoded {
            DataPacket::VideoFrame(f) => {
                assert_eq!(f.seq, 42);
                assert_eq!(f.width, 1920);
                assert_eq!(f.payload, vec![1, 2, 3, 4, 5]);
            }
            _ => panic!("unexpected variant"),
        }
    }
}
