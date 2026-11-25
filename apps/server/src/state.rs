use agentdesk_protocol::{ServerEvent, ServerToAgentMessage};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use uuid::Uuid;

#[derive(Default)]
pub struct RelayRoom {
    pub host_tx: Option<mpsc::Sender<Vec<u8>>>,
    pub viewer_tx: Option<mpsc::Sender<Vec<u8>>>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub bootstrap_token: String,
    pub active_agents: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<ServerToAgentMessage>>>>,
    pub relay_rooms: Arc<RwLock<HashMap<Uuid, RelayRoom>>>,
    pub pending_agent_requests: Arc<RwLock<HashMap<Uuid, oneshot::Sender<serde_json::Value>>>>,
    pub pending_oauth_states: Arc<RwLock<HashMap<String, (Uuid, String, String)>>>,
    pub events_tx: broadcast::Sender<ServerEvent>,
    pub artifacts_dir: PathBuf,
    pub bind_addr: String,
    pub lan_ip: Option<String>,
    pub public_url: String,
}

impl AppState {
    pub fn new(pool: PgPool, bootstrap_token: String, artifacts_dir: PathBuf) -> Self {
        let (events_tx, _) = broadcast::channel(512);
        Self {
            pool,
            bootstrap_token,
            active_agents: Arc::new(RwLock::new(HashMap::new())),
            relay_rooms: Arc::new(RwLock::new(HashMap::new())),
            pending_agent_requests: Arc::new(RwLock::new(HashMap::new())),
            pending_oauth_states: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
            artifacts_dir,
            bind_addr: "0.0.0.0:8080".to_string(),
            lan_ip: None,
            public_url: "http://127.0.0.1:8080".to_string(),
        }
    }

    pub fn with_network_info(
        mut self,
        bind_addr: String,
        lan_ip: Option<String>,
        public_url: String,
    ) -> Self {
        self.bind_addr = bind_addr;
        self.lan_ip = lan_ip;
        self.public_url = public_url;
        self
    }

    pub fn emit_event(
        &self,
        kind: impl Into<String>,
        organization_id: Uuid,
        resource_id: impl Into<String>,
        summary: impl Into<String>,
    ) {
        let _ = self.events_tx.send(ServerEvent {
            kind: kind.into(),
            organization_id,
            resource_id: resource_id.into(),
            summary: summary.into(),
            at: chrono::Utc::now(),
        });
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: msg.into(),
        }
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        Self::internal("database error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

pub async fn record_audit(
    pool: &PgPool,
    organization_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    actor_type: &str,
    action: &str,
    resource_type: &str,
    resource_id: &str,
    outcome: &str,
    metadata: serde_json::Value,
) {
    let res = sqlx::query(
        "INSERT INTO audit_events (organization_id, actor_id, actor_type, action, resource_type, resource_id, outcome, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(actor_type)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(outcome)
    .bind(metadata)
    .execute(pool)
    .await;

    if let Err(e) = res {
        tracing::warn!(error = %e, "failed to record audit event");
    }
}
