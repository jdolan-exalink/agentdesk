use crate::state::{record_audit, ApiError, AppState};
use agentdesk_identity::{
    generate_high_entropy_token, hash_password, sha256_hex, verify_password,
};
use agentdesk_protocol::{AuthResponse, AuthUserInfo, BootstrapRequest, LoginRequest};
use axum::extract::{FromRequestParts, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub const SESSION_COOKIE_NAME: &str = "agentdesk_session";

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user: AuthUserInfo,
    pub raw_token: String,
}

impl FromRequestParts<AppState> for AuthContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers)
            .ok_or_else(|| ApiError::unauthorized("missing authentication token"))?;
        let token_hash = sha256_hex(token.as_bytes());

        let row = sqlx::query(
            "SELECT u.id, u.organization_id, o.name AS org_name, u.email, u.display_name, u.role, u.is_active
             FROM auth_sessions s
             JOIN users u ON u.id = s.user_id
             JOIN organizations o ON o.id = u.organization_id
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now()",
        )
        .bind(&token_hash)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid or expired session"))?;

        let is_active: bool = row.get("is_active");
        if !is_active {
            return Err(ApiError::forbidden("account disabled"));
        }

        Ok(AuthContext {
            user: AuthUserInfo {
                id: row.get("id"),
                organization_id: row.get("organization_id"),
                organization_name: row.get("org_name"),
                email: row.get("email"),
                display_name: row.get("display_name"),
                role: row.get("role"),
            },
            raw_token: token,
        })
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth_val) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(bearer) = auth_val.strip_prefix("Bearer ") {
            let trimmed = bearer.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(cookie_hdr) = headers.get(COOKIE).and_then(|v| v.to_str().ok()) {
        for part in cookie_hdr.split(';') {
            let kv = part.trim();
            if let Some(val) = kv.strip_prefix(&format!("{SESSION_COOKIE_NAME}=")) {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

pub async fn setup_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(json!({
        "bootstrapped": count > 0,
        "product": "AgentDesk",
        "lan_ip": state.lan_ip,
        "public_url": state.public_url,
    })))
}

pub async fn bootstrap(
    State(state): State<AppState>,
    Json(req): Json<BootstrapRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.bootstrap_token != state.bootstrap_token {
        return Err(ApiError::forbidden("invalid bootstrap token"));
    }
    if req.admin_password.len() < 8 {
        return Err(ApiError::bad_request("password must be at least 8 characters"));
    }

    let existing_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    if existing_users > 0 {
        return Err(ApiError::conflict("instance already bootstrapped"));
    }

    let org_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let pw_hash = hash_password(&req.admin_password).map_err(|e| ApiError::internal(e.to_string()))?;

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)",
    )
    .bind(org_id)
    .bind(req.organization_name.trim())
    .bind(req.organization_slug.trim().to_lowercase())
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO users (id, organization_id, email, display_name, password_hash, role)
         VALUES ($1, $2, $3, $4, $5, 'instance_admin')",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(req.admin_email.trim().to_lowercase())
    .bind(req.admin_name.trim())
    .bind(&pw_hash)
    .execute(&mut *tx)
    .await?;

    let raw_token = generate_high_entropy_token();
    let token_hash = sha256_hex(raw_token.as_bytes());
    let expires_at = Utc::now() + Duration::days(7);

    sqlx::query(
        "INSERT INTO auth_sessions (token_hash, user_id, organization_id, expires_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&token_hash)
    .bind(user_id)
    .bind(org_id)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    record_audit(
        &state.pool,
        Some(org_id),
        Some(user_id),
        "user",
        "instance.bootstrapped",
        "organization",
        &org_id.to_string(),
        "success",
        json!({ "email": req.admin_email.trim().to_lowercase() }),
    )
    .await;

    let cookie = format!(
        "{SESSION_COOKIE_NAME}={raw_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"
    );
    let body = AuthResponse {
        token: raw_token,
        expires_at,
        user: AuthUserInfo {
            id: user_id,
            organization_id: org_id,
            organization_name: req.organization_name.trim().to_string(),
            email: req.admin_email.trim().to_lowercase(),
            display_name: req.admin_name.trim().to_string(),
            role: "instance_admin".into(),
        },
    };
    Ok(([(SET_COOKIE, cookie)], Json(body)))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let email = req.email.trim().to_lowercase();
    let row = sqlx::query(
        "SELECT u.id, u.organization_id, o.name AS org_name, u.email, u.display_name, u.password_hash, u.role, u.is_active
         FROM users u
         JOIN organizations o ON o.id = u.organization_id
         WHERE u.email = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("invalid credentials"))?;

    let is_active: bool = row.get("is_active");
    if !is_active {
        return Err(ApiError::forbidden("account disabled"));
    }
    let pw_hash: String = row.get("password_hash");
    let valid = verify_password(&req.password, &pw_hash).unwrap_or(false);
    if !valid {
        return Err(ApiError::unauthorized("invalid credentials"));
    }

    let user_id: Uuid = row.get("id");
    let org_id: Uuid = row.get("organization_id");
    let raw_token = generate_high_entropy_token();
    let token_hash = sha256_hex(raw_token.as_bytes());
    let expires_at = Utc::now() + Duration::days(7);

    sqlx::query(
        "INSERT INTO auth_sessions (token_hash, user_id, organization_id, expires_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&token_hash)
    .bind(user_id)
    .bind(org_id)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(org_id),
        Some(user_id),
        "user",
        "auth.login",
        "user",
        &user_id.to_string(),
        "success",
        json!({}),
    )
    .await;

    let cookie = format!(
        "{SESSION_COOKIE_NAME}={raw_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"
    );
    let body = AuthResponse {
        token: raw_token,
        expires_at,
        user: AuthUserInfo {
            id: user_id,
            organization_id: org_id,
            organization_name: row.get("org_name"),
            email: row.get("email"),
            display_name: row.get("display_name"),
            role: row.get("role"),
        },
    };
    Ok(([(SET_COOKIE, cookie)], Json(body)))
}

pub async fn logout(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<impl IntoResponse, ApiError> {
    let token_hash = sha256_hex(auth.raw_token.as_bytes());
    sqlx::query("UPDATE auth_sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&state.pool)
        .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "auth.logout",
        "user",
        &auth.user.id.to_string(),
        "success",
        json!({}),
    )
    .await;

    let clear_cookie = format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    Ok(([(SET_COOKIE, clear_cookie)], Json(json!({ "ok": true }))))
}

pub async fn me(auth: AuthContext) -> Json<AuthUserInfo> {
    Json(auth.user)
}
