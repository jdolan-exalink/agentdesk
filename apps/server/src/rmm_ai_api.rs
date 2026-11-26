use crate::auth::AuthContext;
use crate::state::{record_audit, ApiError, AppState};
use agentdesk_ai::{
    ask_copilot, build_oauth_authorize_url, exchange_oauth_code, generate_pkce, AiAutonomyLevel,
    AiConfig, AiProviderKind, CopilotRequest,
};
use agentdesk_identity::hash_password;
use agentdesk_protocol::{
    AiChatRequest, AiChatResponse, AiExecuteActionRequest, AiProposedActionDto, AiSettingsDto,
    CalendarEventDto, CreateCalendarEventRequest, CreateTicketCommentRequest, CreateUserRequest,
    DocPageDto, MfaSetupResponse, MfaVerifyRequest, RmmCommandDto, RunCommandRequest,
    RunDiagnosticRequest, ServerToAgentMessage, TicketCommentDto, UpdateAiSettingsRequest, UserDto,
};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::Deserialize;
use serde_json::json;
use sha1::Sha1;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

type HmacSha1 = Hmac<Sha1>;

const ALLOWED_ROLES: [&str; 7] = [
    "instance_admin",
    "org_admin",
    "supervisor",
    "technician",
    "limited_technician",
    "auditor",
    "client",
];

fn compute_totp_at(secret_b32: &str, counter: u64) -> Result<String, ApiError> {
    let key = BASE32_NOPAD
        .decode(secret_b32.trim().as_bytes())
        .map_err(|_| ApiError::bad_request("invalid TOTP secret"))?;
    let mut mac = HmacSha1::new_from_slice(&key).map_err(|_| ApiError::internal("hmac init"))?;
    mac.update(&counter.to_be_bytes());
    let res = mac.finalize().into_bytes();
    let offset = (res[19] & 0x0f) as usize;
    let bin = (((res[offset] & 0x7f) as u32) << 24)
        | ((res[offset + 1] as u32) << 16)
        | ((res[offset + 2] as u32) << 8)
        | (res[offset + 3] as u32);
    Ok(format!("{:06}", bin % 1_000_000))
}

pub fn verify_totp_code(secret_b32: &str, code: &str) -> Result<bool, ApiError> {
    let clean = code.trim();
    if clean.len() != 6 || !clean.chars().all(|c| c.is_ascii_digit()) {
        return Ok(false);
    }
    let now_step = (Utc::now().timestamp().max(0) as u64) / 30;
    for delta in [-1i64, 0, 1] {
        let step = (now_step as i64 + delta).max(0) as u64;
        if compute_totp_at(secret_b32, step)? == clean {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Users, RBAC (7 roles) & TOTP MFA
// ---------------------------------------------------------------------------

pub async fn list_users(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, email, display_name, role, is_active, mfa_enabled, created_at
         FROM users WHERE organization_id = $1 ORDER BY created_at ASC",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| UserDto {
                id: r.get("id"),
                email: r.get("email"),
                display_name: r.get("display_name"),
                role: r.get("role"),
                is_active: r.get("is_active"),
                mfa_enabled: r.get("mfa_enabled"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

pub async fn create_user(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserDto>, ApiError> {
    if !matches!(auth.user.role.as_str(), "instance_admin" | "org_admin") {
        return Err(ApiError::forbidden("admin role required to create users"));
    }
    if !ALLOWED_ROLES.contains(&req.role.as_str()) {
        return Err(ApiError::bad_request("invalid RBAC role"));
    }
    if req.password.len() < 8 {
        return Err(ApiError::bad_request("password must be at least 8 characters"));
    }
    let uid = Uuid::new_v4();
    let pw_hash = hash_password(&req.password).map_err(|e| ApiError::internal(e.to_string()))?;
    let email = req.email.trim().to_lowercase();
    let r = sqlx::query(
        "INSERT INTO users (id, organization_id, email, display_name, password_hash, role)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING created_at",
    )
    .bind(uid)
    .bind(auth.user.organization_id)
    .bind(&email)
    .bind(req.display_name.trim())
    .bind(&pw_hash)
    .bind(&req.role)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "user.created",
        "user",
        &uid.to_string(),
        "success",
        json!({ "email": email, "role": req.role }),
    )
    .await;

    Ok(Json(UserDto {
        id: uid,
        email,
        display_name: req.display_name.trim().to_string(),
        role: req.role,
        is_active: true,
        mfa_enabled: false,
        created_at: r.get("created_at"),
    }))
}

#[derive(Deserialize)]
pub struct UpdateRoleBody {
    pub role: String,
}

pub async fn update_user_role(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateRoleBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !matches!(auth.user.role.as_str(), "instance_admin" | "org_admin") {
        return Err(ApiError::forbidden("admin role required"));
    }
    if !ALLOWED_ROLES.contains(&req.role.as_str()) {
        return Err(ApiError::bad_request("invalid RBAC role"));
    }
    let res = sqlx::query("UPDATE users SET role = $3 WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(auth.user.organization_id)
        .bind(&req.role)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("user not found"));
    }
    Ok(Json(json!({ "ok": true, "role": req.role })))
}

pub async fn mfa_setup(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<MfaSetupResponse>, ApiError> {
    let mut raw = [0u8; 20];
    rand::thread_rng().fill(&mut raw);
    let secret = BASE32_NOPAD.encode(&raw);
    sqlx::query("UPDATE users SET mfa_secret = $2 WHERE id = $1")
        .bind(auth.user.id)
        .bind(&secret)
        .execute(&state.pool)
        .await?;
    let uri = format!(
        "otpauth://totp/AgentDesk:{}?secret={secret}&issuer=AgentDesk",
        urlencoding::encode(&auth.user.email)
    );
    Ok(Json(MfaSetupResponse {
        secret_base32: secret,
        otpauth_uri: uri,
    }))
}

pub async fn mfa_verify(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<MfaVerifyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let secret: Option<String> = sqlx::query_scalar("SELECT mfa_secret FROM users WHERE id = $1")
        .bind(auth.user.id)
        .fetch_optional(&state.pool)
        .await?
        .flatten();
    let Some(sec) = secret else {
        return Err(ApiError::bad_request("call /api/v1/auth/mfa/setup first"));
    };
    if !verify_totp_code(&sec, &req.code)? {
        return Err(ApiError::unauthorized("invalid TOTP code"));
    }
    sqlx::query("UPDATE users SET mfa_enabled = true WHERE id = $1")
        .bind(auth.user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true, "mfa_enabled": true })))
}

// ---------------------------------------------------------------------------
// RMM Diagnostics & Remote Shell Terminal (RMM-01..03)
// ---------------------------------------------------------------------------

async fn dispatch_agent_rpc(
    state: &AppState,
    org_id: Uuid,
    device_id: Uuid,
    timeout_secs: u64,
    build_msg: impl FnOnce(Uuid) -> ServerToAgentMessage,
) -> Result<serde_json::Value, ApiError> {
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND organization_id = $2")
            .bind(device_id)
            .bind(org_id)
            .fetch_optional(&state.pool)
            .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("device not found in organization"));
    }

    let agent_tx = state
        .active_agents
        .read()
        .await
        .get(&device_id)
        .cloned()
        .ok_or_else(|| ApiError::conflict("el dispositivo no está conectado actualmente"))?;

    let req_id = Uuid::new_v4();
    let (tx_wait, rx_wait) = tokio::sync::oneshot::channel();
    state
        .pending_agent_requests
        .write()
        .await
        .insert(req_id, tx_wait);

    if agent_tx.send(build_msg(req_id)).is_err() {
        state.pending_agent_requests.write().await.remove(&req_id);
        return Err(ApiError::conflict("canal del agente cerrado"));
    }

    match tokio::time::timeout(Duration::from_secs(timeout_secs.clamp(3, 60)), rx_wait).await {
        Ok(Ok(val)) => Ok(val),
        _ => {
            state.pending_agent_requests.write().await.remove(&req_id);
            Err(ApiError::bad_request("timeout esperando respuesta del agente"))
        }
    }
}

pub async fn run_device_diagnostic(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
    Json(req): Json<RunDiagnosticRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = req.kind.trim().to_string();
    let val = dispatch_agent_rpc(&state, auth.user.organization_id, device_id, 20, |request_id| {
        ServerToAgentMessage::RunDiagnostic {
            request_id,
            kind: kind.clone(),
        }
    })
    .await?;

    let payload = val.get("data").cloned().unwrap_or_else(|| val.clone());
    let _ = sqlx::query(
        "INSERT INTO device_telemetry (organization_id, device_id, kind, payload) VALUES ($1, $2, $3, $4)",
    )
    .bind(auth.user.organization_id)
    .bind(device_id)
    .bind(&kind)
    .bind(&payload)
    .execute(&state.pool)
    .await;

    Ok(Json(val))
}

pub async fn list_device_telemetry(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, kind, payload, collected_at FROM device_telemetry
         WHERE device_id = $1 AND organization_id = $2
         ORDER BY collected_at DESC LIMIT 30",
    )
    .bind(device_id)
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.get::<i64, _>("id"),
                    "kind": r.get::<String, _>("kind"),
                    "payload": r.get::<serde_json::Value, _>("payload"),
                    "collected_at": r.get::<DateTime<Utc>, _>("collected_at"),
                })
            })
            .collect(),
    ))
}

pub async fn run_device_command(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
    Json(req): Json<RunCommandRequest>,
) -> Result<Json<RmmCommandDto>, ApiError> {
    if matches!(auth.user.role.as_str(), "auditor" | "client") {
        return Err(ApiError::forbidden("role cannot execute remote commands"));
    }
    let shell = if req.shell.eq_ignore_ascii_case("cmd") {
        "cmd".to_string()
    } else {
        "powershell".to_string()
    };
    let command = req.command.trim().to_string();
    if command.is_empty() {
        return Err(ApiError::bad_request("command is required"));
    }

    let val = dispatch_agent_rpc(
        &state,
        auth.user.organization_id,
        device_id,
        req.timeout_secs.clamp(5, 60),
        |request_id| ServerToAgentMessage::RunCommand {
            request_id,
            shell: shell.clone(),
            command: command.clone(),
            timeout_secs: req.timeout_secs.clamp(5, 60),
        },
    )
    .await?;

    let exit_code = val.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
    let stdout = val
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stderr = val
        .get("stderr")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let duration_ms = val.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0);

    let cmd_id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO rmm_commands (id, organization_id, device_id, requested_by, shell, command, exit_code, stdout, stderr, duration_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         RETURNING created_at",
    )
    .bind(cmd_id)
    .bind(auth.user.organization_id)
    .bind(device_id)
    .bind(auth.user.id)
    .bind(&shell)
    .bind(&command)
    .bind(exit_code)
    .bind(&stdout)
    .bind(&stderr)
    .bind(duration_ms)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "rmm.command_executed",
        "device",
        &device_id.to_string(),
        if exit_code == 0 { "success" } else { "nonzero" },
        json!({ "shell": shell, "exit_code": exit_code }),
    )
    .await;

    Ok(Json(RmmCommandDto {
        id: cmd_id,
        device_id,
        shell,
        command,
        exit_code,
        stdout,
        stderr,
        duration_ms,
        created_at: row.get("created_at"),
    }))
}

pub async fn list_device_commands(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
) -> Result<Json<Vec<RmmCommandDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, device_id, shell, command, exit_code, stdout, stderr, duration_ms, created_at
         FROM rmm_commands WHERE device_id = $1 AND organization_id = $2
         ORDER BY created_at DESC LIMIT 30",
    )
    .bind(device_id)
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| RmmCommandDto {
                id: r.get("id"),
                device_id: r.get("device_id"),
                shell: r.get("shell"),
                command: r.get("command"),
                exit_code: r.get("exit_code"),
                stdout: r.get("stdout"),
                stderr: r.get("stderr"),
                duration_ms: r.get("duration_ms"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// AI Copilot (ChatGPT OAuth PKCE / OpenAI API / Ollama) + MCP Endpoint
// ---------------------------------------------------------------------------

fn parse_provider(s: &str) -> AiProviderKind {
    match s {
        "openai_api" => AiProviderKind::OpenAiApi,
        "ollama_local" => AiProviderKind::OllamaLocal,
        "disabled" => AiProviderKind::Disabled,
        _ => AiProviderKind::ChatGptOAuth,
    }
}

fn provider_str(p: AiProviderKind) -> &'static str {
    match p {
        AiProviderKind::ChatGptOAuth => "chat_gpt_o_auth",
        AiProviderKind::OpenAiApi => "openai_api",
        AiProviderKind::OllamaLocal => "ollama_local",
        AiProviderKind::Disabled => "disabled",
    }
}

fn parse_autonomy(s: &str) -> AiAutonomyLevel {
    match s {
        "consult_only" => AiAutonomyLevel::ConsultOnly,
        "diagnostic_read" => AiAutonomyLevel::DiagnosticRead,
        "propose_actions" => AiAutonomyLevel::ProposeActions,
        _ => AiAutonomyLevel::HumanApprovedExec,
    }
}

fn autonomy_str(a: AiAutonomyLevel) -> &'static str {
    match a {
        AiAutonomyLevel::ConsultOnly => "consult_only",
        AiAutonomyLevel::DiagnosticRead => "diagnostic_read",
        AiAutonomyLevel::ProposeActions => "propose_actions",
        AiAutonomyLevel::HumanApprovedExec => "human_approved_exec",
    }
}

async fn load_org_ai_config(pool: &sqlx::PgPool, org_id: Uuid) -> Result<AiConfig, ApiError> {
    let row = sqlx::query(
        "SELECT provider, model, base_url, api_key, oauth_client_id, oauth_auth_url, oauth_token_url, oauth_access_token, oauth_refresh_token, autonomy_level, redact_pii
         FROM ai_settings WHERE organization_id = $1",
    )
    .bind(org_id)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some(r) => AiConfig {
            provider: parse_provider(r.get::<String, _>("provider").as_str()),
            model: r.get("model"),
            base_url: r.get("base_url"),
            api_key: r.get("api_key"),
            oauth_client_id: r.get("oauth_client_id"),
            oauth_auth_url: r.get("oauth_auth_url"),
            oauth_token_url: r.get("oauth_token_url"),
            oauth_access_token: r.get("oauth_access_token"),
            oauth_refresh_token: r.get("oauth_refresh_token"),
            autonomy_level: parse_autonomy(r.get::<String, _>("autonomy_level").as_str()),
            redact_pii: r.get("redact_pii"),
        },
        None => AiConfig::default(),
    })
}

pub async fn get_ai_settings(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<AiSettingsDto>, ApiError> {
    let cfg = load_org_ai_config(&state.pool, auth.user.organization_id).await?;
    Ok(Json(AiSettingsDto {
        provider: provider_str(cfg.provider).into(),
        model: cfg.model,
        base_url: cfg.base_url,
        has_api_key: !cfg.api_key.trim().is_empty(),
        oauth_client_id: cfg.oauth_client_id,
        oauth_auth_url: cfg.oauth_auth_url,
        oauth_token_url: cfg.oauth_token_url,
        oauth_connected: !cfg.oauth_access_token.trim().is_empty(),
        autonomy_level: autonomy_str(cfg.autonomy_level).into(),
        redact_pii: cfg.redact_pii,
    }))
}

pub async fn update_ai_settings(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<UpdateAiSettingsRequest>,
) -> Result<Json<AiSettingsDto>, ApiError> {
    let mut cfg = load_org_ai_config(&state.pool, auth.user.organization_id).await?;
    cfg.provider = parse_provider(&req.provider);
    if !req.model.trim().is_empty() {
        cfg.model = req.model.trim().to_string();
    }
    if !req.base_url.trim().is_empty() {
        cfg.base_url = req.base_url.trim().to_string();
    }
    if let Some(k) = req.api_key {
        if !k.trim().is_empty() {
            cfg.api_key = k.trim().to_string();
        }
    }
    if let Some(cid) = req.oauth_client_id {
        if !cid.trim().is_empty() {
            cfg.oauth_client_id = cid.trim().to_string();
        }
    }
    if let Some(aurl) = req.oauth_auth_url {
        if !aurl.trim().is_empty() {
            cfg.oauth_auth_url = aurl.trim().to_string();
        }
    }
    if let Some(turl) = req.oauth_token_url {
        if !turl.trim().is_empty() {
            cfg.oauth_token_url = turl.trim().to_string();
        }
    }
    if let Some(otok) = req.oauth_manual_token {
        if !otok.trim().is_empty() {
            cfg.oauth_access_token = otok.trim().to_string();
        }
    }
    cfg.autonomy_level = parse_autonomy(&req.autonomy_level);
    cfg.redact_pii = req.redact_pii;

    sqlx::query(
        "INSERT INTO ai_settings (organization_id, provider, model, base_url, api_key, oauth_client_id, oauth_auth_url, oauth_token_url, oauth_access_token, oauth_refresh_token, autonomy_level, redact_pii, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())
         ON CONFLICT (organization_id) DO UPDATE SET
           provider = EXCLUDED.provider,
           model = EXCLUDED.model,
           base_url = EXCLUDED.base_url,
           api_key = EXCLUDED.api_key,
           oauth_client_id = EXCLUDED.oauth_client_id,
           oauth_auth_url = EXCLUDED.oauth_auth_url,
           oauth_token_url = EXCLUDED.oauth_token_url,
           oauth_access_token = EXCLUDED.oauth_access_token,
           oauth_refresh_token = EXCLUDED.oauth_refresh_token,
           autonomy_level = EXCLUDED.autonomy_level,
           redact_pii = EXCLUDED.redact_pii,
           updated_at = now()",
    )
    .bind(auth.user.organization_id)
    .bind(provider_str(cfg.provider))
    .bind(&cfg.model)
    .bind(&cfg.base_url)
    .bind(&cfg.api_key)
    .bind(&cfg.oauth_client_id)
    .bind(&cfg.oauth_auth_url)
    .bind(&cfg.oauth_token_url)
    .bind(&cfg.oauth_access_token)
    .bind(&cfg.oauth_refresh_token)
    .bind(autonomy_str(cfg.autonomy_level))
    .bind(cfg.redact_pii)
    .execute(&state.pool)
    .await?;

    Ok(Json(AiSettingsDto {
        provider: provider_str(cfg.provider).into(),
        model: cfg.model,
        base_url: cfg.base_url,
        has_api_key: !cfg.api_key.trim().is_empty(),
        oauth_client_id: cfg.oauth_client_id,
        oauth_auth_url: cfg.oauth_auth_url,
        oauth_token_url: cfg.oauth_token_url,
        oauth_connected: !cfg.oauth_access_token.trim().is_empty(),
        autonomy_level: autonomy_str(cfg.autonomy_level).into(),
        redact_pii: cfg.redact_pii,
    }))
}

pub async fn ai_oauth_start(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cfg = load_org_ai_config(&state.pool, auth.user.organization_id).await?;
    let pkce = generate_pkce();
    let redirect_uri = format!(
        "{}/api/v1/ai/oauth/callback",
        state.public_url.trim_end_matches('/')
    );
    let authorize_url = build_oauth_authorize_url(&cfg, &redirect_uri, &pkce);
    state.pending_oauth_states.write().await.insert(
        pkce.state.clone(),
        (auth.user.organization_id, pkce.verifier, redirect_uri.clone()),
    );
    Ok(Json(json!({
        "authorize_url": authorize_url,
        "redirect_uri": redirect_uri,
        "state": pkce.state,
    })))
}

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub async fn ai_oauth_callback(
    State(state): State<AppState>,
    Query(q): Query<OAuthCallbackQuery>,
) -> Html<String> {
    if let Some(err) = q.error {
        return Html(format!(
            "<h2>Error OAuth ChatGPT</h2><p>{err}</p>"
        ));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return Html("<h2>Callback inválido</h2><p>Faltan parámetros code/state.</p>".into());
    };
    let pending = state.pending_oauth_states.write().await.remove(&st);
    let Some((org_id, verifier, redirect_uri)) = pending else {
        return Html("<h2>Estado OAuth expirado</h2><p>Vuelve a iniciar el flujo desde AgentDesk.</p>".into());
    };

    let Ok(mut cfg) = load_org_ai_config(&state.pool, org_id).await else {
        return Html("<h2>Error interno</h2>".into());
    };
    let http = reqwest::Client::new();
    match exchange_oauth_code(&http, &cfg, &code, &verifier, &redirect_uri).await {
        Ok(tok) => {
            cfg.provider = AiProviderKind::ChatGptOAuth;
            cfg.oauth_access_token = tok.access_token;
            if let Some(rt) = tok.refresh_token {
                cfg.oauth_refresh_token = rt;
            }
            let _ = sqlx::query(
                "INSERT INTO ai_settings (organization_id, provider, oauth_access_token, oauth_refresh_token, updated_at)
                 VALUES ($1, 'chat_gpt_o_auth', $2, $3, now())
                 ON CONFLICT (organization_id) DO UPDATE SET
                   provider = 'chat_gpt_o_auth',
                   oauth_access_token = EXCLUDED.oauth_access_token,
                   oauth_refresh_token = EXCLUDED.oauth_refresh_token,
                   updated_at = now()",
            )
            .bind(org_id)
            .bind(&cfg.oauth_access_token)
            .bind(&cfg.oauth_refresh_token)
            .execute(&state.pool)
            .await;
            Html("<h2>ChatGPT OAuth conectado</h2><p>Puedes cerrar esta pestaña y volver a AgentDesk.</p>".into())
        }
        Err(e) => Html(format!(
            "<h2>Error intercambiando token OAuth</h2><p>{e}</p>"
        )),
    }
}

pub async fn ai_chat(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<AiChatRequest>,
) -> Result<Json<AiChatResponse>, ApiError> {
    let cfg = load_org_ai_config(&state.pool, auth.user.organization_id).await?;
    let mut diag_snapshot = None;
    let mut screenshot_b64 = None;

    if let Some(did) = req.device_id {
        if req.include_diagnostics {
            if let Ok(val) = dispatch_agent_rpc(&state, auth.user.organization_id, did, 18, |rid| {
                ServerToAgentMessage::RunDiagnostic {
                    request_id: rid,
                    kind: "all".into(),
                }
            })
            .await
            {
                diag_snapshot = val.get("data").cloned().or(Some(val));
            }
        }
        if req.include_screenshot {
            if let Ok(val) = dispatch_agent_rpc(&state, auth.user.organization_id, did, 18, |rid| {
                ServerToAgentMessage::RunDiagnostic {
                    request_id: rid,
                    kind: "screenshot".into(),
                }
            })
            .await
            {
                screenshot_b64 = val
                    .get("screenshot_jpeg_b64")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    let machine_context = diag_snapshot
        .as_ref()
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_default());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap_or_default();

    let resp = ask_copilot(
        &http,
        &cfg,
        CopilotRequest {
            prompt: req.prompt.clone(),
            machine_context,
            screenshot_jpeg_b64: screenshot_b64,
        },
    )
    .await
    .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let actions_dto: Vec<AiProposedActionDto> = resp
        .proposed_actions
        .into_iter()
        .map(|a| AiProposedActionDto {
            id: a.id,
            title: a.title,
            shell: a.shell,
            command: a.command,
            risk_level: a.risk_level,
            rationale: a.rationale,
        })
        .collect();

    let _ = sqlx::query(
        "INSERT INTO ai_conversations (id, organization_id, user_id, device_id, ticket_id, prompt, reply, provider_used, proposed_actions)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(req.device_id)
    .bind(req.ticket_id)
    .bind(&req.prompt)
    .bind(&resp.reply)
    .bind(&resp.provider_used)
    .bind(serde_json::to_value(&actions_dto).unwrap_or_default())
    .execute(&state.pool)
    .await;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "ai.chat",
        "ai_copilot",
        &req.device_id.map(|d| d.to_string()).unwrap_or_else(|| "global".into()),
        "success",
        json!({ "provider": resp.provider_used, "actions_count": actions_dto.len() }),
    )
    .await;

    Ok(Json(AiChatResponse {
        reply: resp.reply,
        proposed_actions: actions_dto,
        provider_used: resp.provider_used,
        model_used: resp.model_used,
        redacted_context_applied: resp.redacted_context_applied,
        diagnostics_snapshot: diag_snapshot,
    }))
}

pub async fn ai_execute_approved_action(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<AiExecuteActionRequest>,
) -> Result<Json<RmmCommandDto>, ApiError> {
    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "ai.action_approved",
        "device",
        &req.device_id.to_string(),
        "approved",
        json!({ "rationale": req.rationale, "command": req.command }),
    )
    .await;

    run_device_command(
        State(state),
        auth,
        Path(req.device_id),
        Json(RunCommandRequest {
            shell: req.shell,
            command: req.command,
            timeout_secs: 20,
        }),
    )
    .await
}

#[derive(Deserialize)]
pub struct McpRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

/// MCP server endpoint (AI-06): exposes typed read/diagnostic tools scoped by AuthContext.
pub async fn mcp_rpc_handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<McpRpcRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = req.id.unwrap_or_else(|| json!(1));
    let result = match req.method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": { "name": "agentdesk-mcp", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "tools": {} }
        }),
        "tools/list" => json!({
            "tools": [
                {
                    "name": "list_devices",
                    "description": "List online and offline managed devices in the authenticated organization.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "list_tickets",
                    "description": "List active support tickets in the authenticated organization.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "inspect_device_diagnostics",
                    "description": "Run live read-only diagnostics (inventory/metrics/services/events/screenshot) on a connected device.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "device_id": { "type": "string" },
                            "kind": { "type": "string", "enum": ["metrics", "inventory", "services", "events", "all"] }
                        },
                        "required": ["device_id"]
                    }
                }
            ]
        }),
        "tools/call" => {
            let params = req.params.unwrap_or_default();
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_default();
            match name {
                "list_devices" => {
                    let rows = sqlx::query(
                        "SELECT id, public_code, hostname, status, os_name, private_mem_mb FROM devices WHERE organization_id = $1 ORDER BY last_heartbeat_at DESC",
                    )
                    .bind(auth.user.organization_id)
                    .fetch_all(&state.pool)
                    .await?;
                    let items: Vec<_> = rows
                        .into_iter()
                        .map(|r| {
                            json!({
                                "id": r.get::<Uuid, _>("id"),
                                "public_code": r.get::<String, _>("public_code"),
                                "hostname": r.get::<String, _>("hostname"),
                                "status": r.get::<String, _>("status"),
                                "os_name": r.get::<String, _>("os_name"),
                                "private_mem_mb": r.get::<f64, _>("private_mem_mb"),
                            })
                        })
                        .collect();
                    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&items).unwrap_or_default() }] })
                }
                "list_tickets" => {
                    let rows = sqlx::query(
                        "SELECT id, public_code, requester_name, description, status FROM support_requests WHERE organization_id = $1 ORDER BY created_at DESC LIMIT 30",
                    )
                    .bind(auth.user.organization_id)
                    .fetch_all(&state.pool)
                    .await?;
                    let items: Vec<_> = rows
                        .into_iter()
                        .map(|r| {
                            json!({
                                "id": r.get::<Uuid, _>("id"),
                                "public_code": r.get::<String, _>("public_code"),
                                "requester_name": r.get::<String, _>("requester_name"),
                                "description": r.get::<String, _>("description"),
                                "status": r.get::<String, _>("status"),
                            })
                        })
                        .collect();
                    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&items).unwrap_or_default() }] })
                }
                "inspect_device_diagnostics" => {
                    let did_str = args.get("device_id").and_then(|v| v.as_str()).unwrap_or("");
                    let did = Uuid::parse_str(did_str)
                        .map_err(|_| ApiError::bad_request("invalid device_id"))?;
                    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("metrics");
                    let val = dispatch_agent_rpc(&state, auth.user.organization_id, did, 18, |rid| {
                        ServerToAgentMessage::RunDiagnostic {
                            request_id: rid,
                            kind: kind.into(),
                        }
                    })
                    .await?;
                    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }] })
                }
                _ => return Err(ApiError::bad_request("unknown MCP tool")),
            }
        }
        _ => return Err(ApiError::bad_request("unsupported MCP method")),
    };

    Ok(Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })))
}

// ---------------------------------------------------------------------------
// Calendar, Ticket Comments, CSV Reports & Docs (CAL-01, TKT-02, REP-01, WEB-04)
// ---------------------------------------------------------------------------

pub async fn list_calendar_events(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<CalendarEventDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT c.id, c.organization_id, c.technician_id, u.display_name AS tech_name,
                c.device_id, c.title, c.client_name, c.starts_at, c.ends_at, c.notes
         FROM calendar_events c
         JOIN users u ON u.id = c.technician_id
         WHERE c.organization_id = $1
         ORDER BY c.starts_at ASC",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| CalendarEventDto {
                id: r.get("id"),
                organization_id: r.get("organization_id"),
                technician_id: r.get("technician_id"),
                technician_name: r.get("tech_name"),
                device_id: r.get("device_id"),
                title: r.get("title"),
                client_name: r.get("client_name"),
                starts_at: r.get("starts_at"),
                ends_at: r.get("ends_at"),
                notes: r.get("notes"),
            })
            .collect(),
    ))
}

pub async fn create_calendar_event(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<CreateCalendarEventRequest>,
) -> Result<Json<CalendarEventDto>, ApiError> {
    if req.ends_at <= req.starts_at {
        return Err(ApiError::bad_request("ends_at must be after starts_at"));
    }

    // Conflict detection for technician overlapping slots (CAL-01)
    let conflict_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM calendar_events
         WHERE organization_id = $1 AND technician_id = $2
           AND starts_at < $4 AND ends_at > $3",
    )
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(req.starts_at)
    .bind(req.ends_at)
    .fetch_one(&state.pool)
    .await?;

    if conflict_count > 0 {
        return Err(ApiError::conflict("conflicto de horario: ya existe una cita programada en ese rango"));
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO calendar_events (id, organization_id, technician_id, device_id, title, client_name, starts_at, ends_at, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(req.device_id)
    .bind(req.title.trim())
    .bind(req.client_name.trim())
    .bind(req.starts_at)
    .bind(req.ends_at)
    .bind(req.notes.trim())
    .execute(&state.pool)
    .await?;

    Ok(Json(CalendarEventDto {
        id,
        organization_id: auth.user.organization_id,
        technician_id: auth.user.id,
        technician_name: auth.user.display_name,
        device_id: req.device_id,
        title: req.title.trim().to_string(),
        client_name: req.client_name.trim().to_string(),
        starts_at: req.starts_at,
        ends_at: req.ends_at,
        notes: req.notes.trim().to_string(),
    }))
}

pub async fn delete_calendar_event(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let res = sqlx::query("DELETE FROM calendar_events WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(auth.user.organization_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("calendar event not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_ticket_comments(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(ticket_id): Path<Uuid>,
) -> Result<Json<Vec<TicketCommentDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, ticket_id, author_name, is_internal, body, created_at
         FROM ticket_comments WHERE ticket_id = $1 AND organization_id = $2
         ORDER BY created_at ASC",
    )
    .bind(ticket_id)
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| TicketCommentDto {
                id: r.get("id"),
                ticket_id: r.get("ticket_id"),
                author_name: r.get("author_name"),
                is_internal: r.get("is_internal"),
                body: r.get("body"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

pub async fn create_ticket_comment(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(ticket_id): Path<Uuid>,
    Json(req): Json<CreateTicketCommentRequest>,
) -> Result<Json<TicketCommentDto>, ApiError> {
    let cid = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO ticket_comments (id, organization_id, ticket_id, author_id, author_name, is_internal, body)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING created_at",
    )
    .bind(cid)
    .bind(auth.user.organization_id)
    .bind(ticket_id)
    .bind(auth.user.id)
    .bind(&auth.user.display_name)
    .bind(req.is_internal)
    .bind(req.body.trim())
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(TicketCommentDto {
        id: cid,
        ticket_id,
        author_name: auth.user.display_name,
        is_internal: req.is_internal,
        body: req.body.trim().to_string(),
        created_at: row.get("created_at"),
    }))
}

pub async fn export_sessions_csv(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Response, ApiError> {
    let rows = sqlx::query(
        "SELECT s.id, d.hostname, d.public_code, s.caller_label, s.allow_control, s.status, s.created_at, s.started_at, s.ended_at
         FROM remote_sessions s
         JOIN devices d ON d.id = s.device_id
         WHERE s.organization_id = $1
         ORDER BY s.created_at DESC LIMIT 500",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    let mut csv = String::from("session_id,hostname,connection_id,caller,allow_control,status,created_at,started_at,ended_at\n");
    for r in rows {
        let id: Uuid = r.get("id");
        let host: String = r.get("hostname");
        let code: String = r.get("public_code");
        let caller: String = r.get("caller_label");
        let ctrl: bool = r.get("allow_control");
        let st: String = r.get("status");
        let created: DateTime<Utc> = r.get("created_at");
        let started: Option<DateTime<Utc>> = r.get("started_at");
        let ended: Option<DateTime<Utc>> = r.get("ended_at");
        csv.push_str(&format!(
            "{id},\"{}\",\"{}\",\"{}\",{ctrl},{st},{},{},{}\n",
            host.replace('"', "\"\""),
            code.replace('"', "\"\""),
            caller.replace('"', "\"\""),
            created.to_rfc3339(),
            started.map(|t| t.to_rfc3339()).unwrap_or_default(),
            ended.map(|t| t.to_rfc3339()).unwrap_or_default(),
        ));
    }

    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"agentdesk-sessions.csv\"".to_string(),
            ),
        ],
        csv,
    )
        .into_response())
}

pub async fn list_docs(
    State(state): State<AppState>,
) -> Result<Json<Vec<DocPageDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT slug, title, body_markdown, updated_at FROM doc_pages ORDER BY slug ASC",
    )
    .fetch_all(&state.pool)
    .await?;

    if rows.is_empty() {
        return Ok(Json(vec![DocPageDto {
            slug: "inicio-rapido".into(),
            title: "Guía de Inicio Rápido AgentDesk".into(),
            body_markdown: "1. Abre AgentDesk Desktop para obtener tu ID numérico y contraseña aleatoria.\n2. Comparte el ID y contraseña o instala agentdesk-host.exe --service para acceso desatendido.\n3. Configura ChatGPT OAuth / OpenAI / Ollama en la pestaña Copiloto IA.".into(),
            updated_at: Utc::now(),
        }]));
    }

    Ok(Json(
        rows.into_iter()
            .map(|r| DocPageDto {
                slug: r.get("slug"),
                title: r.get("title"),
                body_markdown: r.get("body_markdown"),
                updated_at: r.get("updated_at"),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct UpsertDocBody {
    pub title: String,
    pub body_markdown: String,
}

pub async fn upsert_doc(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(slug): Path<String>,
    Json(req): Json<UpsertDocBody>,
) -> Result<Json<DocPageDto>, ApiError> {
    let row = sqlx::query(
        "INSERT INTO doc_pages (organization_id, slug, title, body_markdown, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (organization_id, slug) DO UPDATE SET
           title = EXCLUDED.title,
           body_markdown = EXCLUDED.body_markdown,
           updated_at = now()
         RETURNING updated_at",
    )
    .bind(auth.user.organization_id)
    .bind(slug.trim())
    .bind(req.title.trim())
    .bind(req.body_markdown.trim())
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "docs.updated",
        "doc_page",
        slug.trim(),
        "success",
        json!({}),
    )
    .await;

    Ok(Json(DocPageDto {
        slug: slug.trim().to_string(),
        title: req.title.trim().to_string(),
        body_markdown: req.body_markdown.trim().to_string(),
        updated_at: row.get("updated_at"),
    }))
}

// ---------------------------------------------------------------------------
// Device Groups, Sharing & Registration by Connection ID or User Email
// ---------------------------------------------------------------------------

async fn fetch_group_shares(
    pool: &sqlx::PgPool,
    group_id: Uuid,
) -> Result<Vec<agentdesk_protocol::DeviceGroupShareDto>, ApiError> {
    let rows = sqlx::query(
        "SELECT s.id, s.group_id, s.user_id, s.permission, s.created_at, u.email, u.display_name
         FROM device_group_shares s
         JOIN users u ON u.id = s.user_id
         WHERE s.group_id = $1
         ORDER BY u.email ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| agentdesk_protocol::DeviceGroupShareDto {
            id: r.get("id"),
            group_id: r.get("group_id"),
            user_id: r.get("user_id"),
            user_email: r.get("email"),
            user_display_name: r.get("display_name"),
            permission: r.get("permission"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn list_groups(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<agentdesk_protocol::DeviceGroupDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT g.id, g.organization_id, g.name, g.description, g.owner_user_id, g.created_at,
                u.email AS owner_email, s.permission AS share_perm,
                (SELECT COUNT(*) FROM address_book_entries a WHERE a.group_id = g.id) AS device_count
         FROM device_groups g
         JOIN users u ON u.id = g.owner_user_id
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
         WHERE g.organization_id = $1
           AND (g.owner_user_id = $2 OR s.user_id IS NOT NULL OR $3 IN ('admin', 'org_admin'))
         ORDER BY g.name ASC",
    )
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(&auth.user.role)
    .fetch_all(&state.pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let gid: Uuid = r.get("id");
        let owner_id: Uuid = r.get("owner_user_id");
        let share_perm: Option<String> = r.get("share_perm");
        let my_permission = if owner_id == auth.user.id || crate::state::is_admin_role(&auth.user.role) {
            "full_control".to_string()
        } else {
            share_perm.unwrap_or_else(|| "read_only".to_string())
        };
        let shares = fetch_group_shares(&state.pool, gid).await?;
        out.push(agentdesk_protocol::DeviceGroupDto {
            id: gid,
            organization_id: r.get("organization_id"),
            name: r.get("name"),
            description: r.get("description"),
            owner_user_id: owner_id,
            owner_email: r.get("owner_email"),
            my_permission,
            device_count: r.get("device_count"),
            shares,
            created_at: r.get("created_at"),
        });
    }
    Ok(Json(out))
}

pub async fn create_group(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<agentdesk_protocol::CreateDeviceGroupRequest>,
) -> Result<Json<agentdesk_protocol::DeviceGroupDto>, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("El nombre del grupo es obligatorio"));
    }
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO device_groups (id, organization_id, name, description, owner_user_id)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING created_at",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(name)
    .bind(req.description.trim())
    .bind(auth.user.id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            ApiError::conflict("Ya existe un grupo con ese nombre")
        } else {
            ApiError::from(e)
        }
    })?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "group.created",
        "device_group",
        &id.to_string(),
        "success",
        json!({ "name": name }),
    )
    .await;

    Ok(Json(agentdesk_protocol::DeviceGroupDto {
        id,
        organization_id: auth.user.organization_id,
        name: name.to_string(),
        description: req.description.trim().to_string(),
        owner_user_id: auth.user.id,
        owner_email: auth.user.email.clone(),
        my_permission: "full_control".to_string(),
        device_count: 0,
        shares: vec![],
        created_at: row.get("created_at"),
    }))
}

pub async fn delete_group(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = sqlx::query(
        "SELECT owner_user_id FROM device_groups WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("grupo no encontrado"))?;

    let owner_id: Uuid = row.get("owner_user_id");
    if owner_id != auth.user.id && !crate::state::is_admin_role(&auth.user.role) {
        return Err(ApiError::unauthorized("solo el propietario o admin puede eliminar el grupo"));
    }

    sqlx::query("DELETE FROM device_groups WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    Ok(Json(json!({ "ok": true })))
}

pub async fn share_group(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(group_id): Path<Uuid>,
    Json(req): Json<agentdesk_protocol::ShareDeviceGroupRequest>,
) -> Result<Json<agentdesk_protocol::DeviceGroupShareDto>, ApiError> {
    let perm = req.permission.trim();
    if !matches!(perm, "read_only" | "read_write" | "full_control") {
        return Err(ApiError::bad_request(
            "permiso inválido: use read_only, read_write o full_control",
        ));
    }

    let grp = sqlx::query(
        "SELECT g.owner_user_id, s.permission AS caller_perm
         FROM device_groups g
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
         WHERE g.id = $1 AND g.organization_id = $3",
    )
    .bind(group_id)
    .bind(auth.user.id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("grupo no encontrado"))?;

    let owner_id: Uuid = grp.get("owner_user_id");
    let caller_perm: Option<String> = grp.get("caller_perm");
    let allowed = owner_id == auth.user.id
        || crate::state::is_admin_role(&auth.user.role)
        || caller_perm.as_deref() == Some("full_control");
    if !allowed {
        return Err(ApiError::unauthorized("sin permisos para compartir este grupo"));
    }

    let target = sqlx::query(
        "SELECT id, email, display_name FROM users WHERE organization_id = $1 AND LOWER(email) = LOWER($2)",
    )
    .bind(auth.user.organization_id)
    .bind(req.user_email.trim())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("usuario destino no encontrado en la organización"))?;

    let target_id: Uuid = target.get("id");
    let share_id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO device_group_shares (id, group_id, user_id, permission)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (group_id, user_id) DO UPDATE SET permission = EXCLUDED.permission
         RETURNING id, created_at",
    )
    .bind(share_id)
    .bind(group_id)
    .bind(target_id)
    .bind(perm)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "group.shared",
        "device_group",
        &group_id.to_string(),
        "success",
        json!({ "target_email": target.get::<String, _>("email"), "permission": perm }),
    )
    .await;

    Ok(Json(agentdesk_protocol::DeviceGroupShareDto {
        id: row.get("id"),
        group_id,
        user_id: target_id,
        user_email: target.get("email"),
        user_display_name: target.get("display_name"),
        permission: perm.to_string(),
        created_at: row.get("created_at"),
    }))
}

pub async fn unshare_group(
    State(state): State<AppState>,
    auth: AuthContext,
    Path((group_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let grp = sqlx::query(
        "SELECT g.owner_user_id, s.permission AS caller_perm
         FROM device_groups g
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
         WHERE g.id = $1 AND g.organization_id = $3",
    )
    .bind(group_id)
    .bind(auth.user.id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("grupo no encontrado"))?;

    let owner_id: Uuid = grp.get("owner_user_id");
    let caller_perm: Option<String> = grp.get("caller_perm");
    let allowed = owner_id == auth.user.id
        || crate::state::is_admin_role(&auth.user.role)
        || caller_perm.as_deref() == Some("full_control");
    if !allowed {
        return Err(ApiError::unauthorized("sin permisos para modificar este grupo"));
    }

    sqlx::query("DELETE FROM device_group_shares WHERE group_id = $1 AND user_id = $2")
        .bind(group_id)
        .bind(target_user_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(json!({ "ok": true })))
}

pub async fn assign_device(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<agentdesk_protocol::AssignDeviceRequest>,
) -> Result<Json<agentdesk_protocol::AssignDeviceResponse>, ApiError> {
    let norm_code: String = req
        .connection_id
        .as_deref()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    if norm_code.is_empty() {
        return Err(ApiError::bad_request("Debe indicar el ID de conexión del equipo"));
    }

    let dev_row = sqlx::query(
        "SELECT id, organization_id, public_code, ed25519_pubkey, hostname, os_name, os_arch, agent_version,
                kind, screen_width, screen_height, private_mem_mb, status, last_heartbeat_at, personal_password_hash, owner_user_id
         FROM devices
         WHERE organization_id = $1
           AND regexp_replace(UPPER(public_code), '[^A-Z0-9]', '', 'g') = $2
         LIMIT 1",
    )
    .bind(auth.user.organization_id)
    .bind(&norm_code)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("No se encontró un equipo con ese ID en la organización"))?;

    let device_id: Uuid = dev_row.get("id");
    let mut owner_email_out: Option<String> = None;

    if let Some(em) = req.owner_email.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let user_row = sqlx::query(
            "SELECT id, email FROM users WHERE organization_id = $1 AND LOWER(email) = LOWER($2)",
        )
        .bind(auth.user.organization_id)
        .bind(em)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Usuario con ese email no encontrado"))?;

        let uid: Uuid = user_row.get("id");
        sqlx::query("UPDATE devices SET owner_user_id = $2 WHERE id = $1")
            .bind(device_id)
            .bind(uid)
            .execute(&state.pool)
            .await?;
        owner_email_out = Some(user_row.get("email"));
    } else if let Some(uid) = dev_row.get::<Option<Uuid>, _>("owner_user_id") {
        owner_email_out = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.pool)
            .await?;
    }

    let mut entry_dto = None;
    if req.group_id.is_some() || req.alias.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some() {
        let gname = if let Some(gid) = req.group_id {
            let gn: Option<String> = sqlx::query_scalar(
                "SELECT g.name FROM device_groups g
                 LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
                 WHERE g.id = $1 AND g.organization_id = $3
                   AND (g.owner_user_id = $2 OR $4 IN ('admin', 'org_admin') OR s.permission IN ('read_write', 'full_control'))",
            )
            .bind(gid)
            .bind(auth.user.id)
            .bind(auth.user.organization_id)
            .bind(&auth.user.role)
            .fetch_optional(&state.pool)
            .await?;
            gn.ok_or_else(|| ApiError::unauthorized("Grupo no encontrado o sin permiso de escritura"))?
        } else {
            "General".to_string()
        };

        let hostname: String = dev_row.get("hostname");
        let alias = req
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&hostname)
            .to_string();
        let notes = req.notes.as_deref().map(str::trim).unwrap_or("").to_string();

        let eid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO address_book_entries (id, organization_id, device_id, group_id, alias, client_company, contact_name, contact_email, group_name, is_favorite, notes, created_by)
             VALUES ($1, $2, $3, $4, $5, '', '', $6, $7, false, $8, $9)",
        )
        .bind(eid)
        .bind(auth.user.organization_id)
        .bind(device_id)
        .bind(req.group_id)
        .bind(&alias)
        .bind(owner_email_out.as_deref().unwrap_or(""))
        .bind(&gname)
        .bind(&notes)
        .bind(auth.user.id)
        .execute(&state.pool)
        .await?;

        let ar = sqlx::query(
            "SELECT a.id, a.organization_id, a.device_id, a.group_id, a.alias, a.client_company, a.contact_name,
                    a.contact_email, a.group_name, a.is_favorite, a.notes, a.scheduled_next_at,
                    a.created_at, a.updated_at,
                    d.public_code AS device_public_code, d.hostname AS device_hostname,
                    d.status AS device_status, d.os_name AS device_os, d.last_heartbeat_at AS device_last_heartbeat
             FROM address_book_entries a
             LEFT JOIN devices d ON d.id = a.device_id AND d.organization_id = a.organization_id
             WHERE a.id = $1",
        )
        .bind(eid)
        .fetch_one(&state.pool)
        .await?;

        entry_dto = Some(agentdesk_protocol::AddressBookEntryDto {
            id: ar.get("id"),
            organization_id: ar.get("organization_id"),
            device_id: ar.get("device_id"),
            group_id: ar.get("group_id"),
            alias: ar.get("alias"),
            client_company: ar.get("client_company"),
            contact_name: ar.get("contact_name"),
            contact_email: ar.get("contact_email"),
            group_name: ar.get("group_name"),
            is_favorite: ar.get("is_favorite"),
            notes: ar.get("notes"),
            scheduled_next_at: ar.get("scheduled_next_at"),
            created_at: ar.get("created_at"),
            updated_at: ar.get("updated_at"),
            device_public_code: ar.get("device_public_code"),
            device_hostname: ar.get("device_hostname"),
            device_status: ar.get("device_status"),
            device_os: ar.get("device_os"),
            device_last_heartbeat: ar.get("device_last_heartbeat"),
        });
    }

    let device = agentdesk_protocol::DeviceDto {
        id: device_id,
        organization_id: dev_row.get("organization_id"),
        public_code: dev_row.get("public_code"),
        ed25519_pubkey: dev_row.get("ed25519_pubkey"),
        hostname: dev_row.get("hostname"),
        os_name: dev_row.get("os_name"),
        os_arch: dev_row.get("os_arch"),
        agent_version: dev_row.get("agent_version"),
        kind: dev_row.get("kind"),
        screen_width: dev_row.get("screen_width"),
        screen_height: dev_row.get("screen_height"),
        private_mem_mb: dev_row.get("private_mem_mb"),
        status: dev_row.get("status"),
        last_heartbeat_at: dev_row.get("last_heartbeat_at"),
        owner_email: owner_email_out,
        has_personal_password: dev_row
            .get::<Option<String>, _>("personal_password_hash")
            .is_some(),
    };

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "device.assigned",
        "device",
        &device_id.to_string(),
        "success",
        json!({ "connection_id": device.public_code, "owner_email": device.owner_email, "group_id": req.group_id }),
    )
    .await;

    Ok(Json(agentdesk_protocol::AssignDeviceResponse {
        device,
        address_book_entry: entry_dto,
    }))
}
