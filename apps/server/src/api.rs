use crate::auth::AuthContext;
use crate::state::{record_audit, ApiError, AppState};
use agentdesk_identity::{
    generate_high_entropy_token, generate_short_code, hash_password, sha256_hex, verify_password,
};
use agentdesk_protocol::{
    AddressBookEntryDto, AuditEventDto, ClientLookupResponse, CreateEnrollmentRequest,
    CreateEnrollmentResponse, CreateSessionRequest, CreateSessionResponse, CreateSupportRequest,
    CreateSupportResponse, DeviceDto, DirectConnectRequest, DirectConnectResponse,
    DownloadArtifactDto, EnrollDeviceRequest, EnrollDeviceResponse, EnrollmentDto,
    HeartbeatRequest, QuickRegisterRequest, QuickRegisterResponse, ResolveTicketRequest,
    RotatePasswordRequest, ServerToAgentMessage, SessionDto, SetPersonalPasswordRequest,
    SupportTicketDto, UpdateDeviceGroupRequest, UpsertAddressBookRequest, WolRequest,
    WolResponse,
};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::convert::Infallible;
use tokio::fs;
use uuid::Uuid;

pub async fn health_live() -> Json<serde_json::Value> {
    Json(json!({
        "status": "live",
        "service": "agentdesk-server",
    }))
}

pub async fn health_ready(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(json!({
        "status": "ready",
        "service": "agentdesk-server",
        "database": "connected",
        "bind_addr": state.bind_addr,
        "lan_ip": state.lan_ip,
        "public_url": state.public_url,
    })))
}

// ---------------------------------------------------------------------------
// Enrollments & Devices
// ---------------------------------------------------------------------------

pub async fn list_enrollments(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<EnrollmentDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, organization_id, label, max_uses, used_count, expires_at, revoked_at, created_at
         FROM enrollments WHERE organization_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| EnrollmentDto {
                id: r.get("id"),
                organization_id: r.get("organization_id"),
                label: r.get("label"),
                max_uses: r.get("max_uses"),
                used_count: r.get("used_count"),
                expires_at: r.get("expires_at"),
                revoked_at: r.get("revoked_at"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

pub async fn create_enrollment(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<CreateEnrollmentRequest>,
) -> Result<Json<CreateEnrollmentResponse>, ApiError> {
    if req.label.trim().is_empty() {
        return Err(ApiError::bad_request("label is required"));
    }
    let id = Uuid::new_v4();
    let raw_token = generate_high_entropy_token();
    let token_hash = sha256_hex(raw_token.as_bytes());
    let max_uses = req.max_uses.unwrap_or(25).clamp(1, 10000);
    let expires_at = Utc::now() + Duration::hours(req.expires_hours.unwrap_or(72).clamp(1, 8760));

    let row = sqlx::query(
        "INSERT INTO enrollments (id, organization_id, token_hash, label, max_uses, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING created_at",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(&token_hash)
    .bind(req.label.trim())
    .bind(max_uses)
    .bind(expires_at)
    .bind(auth.user.id)
    .fetch_one(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "enrollment.created",
        "enrollment",
        &id.to_string(),
        "success",
        json!({ "label": req.label.trim(), "max_uses": max_uses }),
    )
    .await;

    Ok(Json(CreateEnrollmentResponse {
        enrollment: EnrollmentDto {
            id,
            organization_id: auth.user.organization_id,
            label: req.label.trim().to_string(),
            max_uses,
            used_count: 0,
            expires_at,
            revoked_at: None,
            created_at: row.get("created_at"),
        },
        enrollment_token: raw_token,
    }))
}

pub async fn revoke_enrollment(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let res = sqlx::query(
        "UPDATE enrollments SET revoked_at = now() WHERE id = $1 AND organization_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("enrollment not found"));
    }
    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "enrollment.revoked",
        "enrollment",
        &id.to_string(),
        "success",
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn enroll_device(
    State(state): State<AppState>,
    Json(req): Json<EnrollDeviceRequest>,
) -> Result<Json<EnrollDeviceResponse>, ApiError> {
    if req.ed25519_pubkey.len() != 64 {
        return Err(ApiError::bad_request("invalid ed25519 pubkey hex"));
    }
    if req.kind != "host" && req.kind != "quicksupport" {
        return Err(ApiError::bad_request("kind must be host or quicksupport"));
    }

    let mut tx = state.pool.begin().await?;

    let (org_id, linked_ticket_id): (Uuid, Option<Uuid>) = if let Some(ref etok) =
        req.enrollment_token
    {
        let thash = sha256_hex(etok.as_bytes());
        let erow = sqlx::query(
            "UPDATE enrollments
             SET used_count = used_count + 1
             WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now() AND used_count < max_uses
             RETURNING organization_id",
        )
        .bind(&thash)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid, expired, or exhausted enrollment token"))?;
        (erow.get("organization_id"), None)
    } else if let Some(ref scode) = req.support_code {
        let trow = sqlx::query(
            "SELECT id, organization_id FROM support_requests
             WHERE public_code = $1 AND status IN ('queued', 'assigned')",
        )
        .bind(scode.trim())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid or closed support code"))?;
        (trow.get("organization_id"), Some(trow.get("id")))
    } else {
        return Err(ApiError::bad_request(
            "enrollment_token or support_code required",
        ));
    };

    let device_token = generate_high_entropy_token();
    let device_token_hash = sha256_hex(device_token.as_bytes());

    let existing = sqlx::query(
        "SELECT id, public_code FROM devices WHERE organization_id = $1 AND ed25519_pubkey = $2",
    )
    .bind(org_id)
    .bind(&req.ed25519_pubkey)
    .fetch_optional(&mut *tx)
    .await?;

    let (device_id, public_code) = if let Some(row) = existing {
        let did: Uuid = row.get("id");
        let pcode: String = row.get("public_code");
        sqlx::query(
            "UPDATE devices SET device_token_hash = $2, hostname = $3, os_name = $4, os_arch = $5,
             agent_version = $6, kind = $7, screen_width = $8, screen_height = $9, private_mem_mb = $10,
             mac_addresses = CASE WHEN cardinality($11::text[]) > 0 THEN $11 ELSE mac_addresses END,
             lan_ips = CASE WHEN cardinality($12::text[]) > 0 THEN $12 ELSE lan_ips END,
             status = 'online', last_heartbeat_at = now()
             WHERE id = $1",
        )
        .bind(did)
        .bind(&device_token_hash)
        .bind(&req.hostname)
        .bind(&req.os_name)
        .bind(&req.os_arch)
        .bind(&req.agent_version)
        .bind(&req.kind)
        .bind(req.screen_width)
        .bind(req.screen_height)
        .bind(req.private_mem_mb)
        .bind(&req.mac_addresses)
        .bind(&req.lan_ips)
        .execute(&mut *tx)
        .await?;
        (did, pcode)
    } else {
        let did = Uuid::new_v4();
        let pcode = generate_short_code("AD");
        sqlx::query(
            "INSERT INTO devices (id, organization_id, public_code, ed25519_pubkey, device_token_hash, hostname, os_name, os_arch, agent_version, kind, screen_width, screen_height, private_mem_mb, mac_addresses, lan_ips, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 'online')",
        )
        .bind(did)
        .bind(org_id)
        .bind(&pcode)
        .bind(&req.ed25519_pubkey)
        .bind(&device_token_hash)
        .bind(&req.hostname)
        .bind(&req.os_name)
        .bind(&req.os_arch)
        .bind(&req.agent_version)
        .bind(&req.kind)
        .bind(req.screen_width)
        .bind(req.screen_height)
        .bind(req.private_mem_mb)
        .bind(&req.mac_addresses)
        .bind(&req.lan_ips)
        .execute(&mut *tx)
        .await?;
        (did, pcode)
    };

    if let Some(tid) = linked_ticket_id {
        sqlx::query("UPDATE support_requests SET device_id = $2, updated_at = now() WHERE id = $1")
            .bind(tid)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    record_audit(
        &state.pool,
        Some(org_id),
        Some(device_id),
        "device",
        "device.enrolled",
        "device",
        &device_id.to_string(),
        "success",
        json!({ "kind": req.kind, "hostname": req.hostname, "public_code": public_code }),
    )
    .await;

    state.emit_event(
        "device.enrolled",
        org_id,
        device_id.to_string(),
        format!("Dispositivo {} ({}) registrado", req.hostname, public_code),
    );

    Ok(Json(EnrollDeviceResponse {
        device_id,
        organization_id: org_id,
        public_code,
        device_token,
    }))
}

pub async fn device_heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<HeartbeatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let raw = headers
        .get("x-device-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing x-device-token"))?;
    let thash = sha256_hex(raw.as_bytes());
    let res = sqlx::query(
        "UPDATE devices SET status = 'online', screen_width = $3, screen_height = $4, private_mem_mb = $5, last_heartbeat_at = now()
         WHERE id = $1 AND device_token_hash = $2",
    )
    .bind(req.device_id)
    .bind(&thash)
    .bind(req.screen_width)
    .bind(req.screen_height)
    .bind(req.private_mem_mb)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::unauthorized("invalid device credentials"));
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_devices(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<DeviceDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT d.id, d.organization_id, d.public_code, d.ed25519_pubkey, d.hostname, d.os_name, d.os_arch,
                d.agent_version, d.kind, d.screen_width, d.screen_height, d.private_mem_mb, d.status,
                d.last_heartbeat_at, d.personal_password_hash, d.group_id, u.email AS owner_email,
                d.icon_kind, d.accent_color, d.mac_addresses, d.lan_ips
         FROM devices d
         LEFT JOIN users u ON u.id = d.owner_user_id
         LEFT JOIN device_groups g ON g.id = d.group_id AND g.organization_id = d.organization_id
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
         WHERE d.organization_id = $1
           AND (d.group_id IS NULL OR g.owner_user_id = $2 OR s.user_id IS NOT NULL OR $3 IN ('instance_admin', 'admin', 'org_admin'))
         ORDER BY d.status DESC, d.last_heartbeat_at DESC",
    )
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(&auth.user.role)
    .fetch_all(&state.pool)
    .await?;

    let active_ids: std::collections::HashSet<Uuid> = {
        let agents = state.active_agents.read().await;
        agents.keys().copied().collect()
    };

    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let id: Uuid = r.get("id");
                let status = if active_ids.contains(&id) {
                    "online".to_string()
                } else {
                    "offline".to_string()
                };
                DeviceDto {
                    id,
                    organization_id: r.get("organization_id"),
                    public_code: r.get("public_code"),
                    ed25519_pubkey: r.get("ed25519_pubkey"),
                    hostname: r.get("hostname"),
                    os_name: r.get("os_name"),
                    os_arch: r.get("os_arch"),
                    agent_version: r.get("agent_version"),
                    kind: r.get("kind"),
                    screen_width: r.get("screen_width"),
                    screen_height: r.get("screen_height"),
                    private_mem_mb: r.get("private_mem_mb"),
                    status,
                    last_heartbeat_at: r.get("last_heartbeat_at"),
                    group_id: r.get("group_id"),
                    owner_email: r.get("owner_email"),
                    has_personal_password: r
                        .get::<Option<String>, _>("personal_password_hash")
                        .is_some(),
                    icon_kind: r.get("icon_kind"),
                    accent_color: r.get("accent_color"),
                    mac_addresses: r.get("mac_addresses"),
                    lan_ips: r.get("lan_ips"),
                }
            })
            .collect(),
    ))
}

pub(crate) async fn ensure_device_group_access(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    user_id: Uuid,
    user_role: &str,
    device_id: Uuid,
    require_write: bool,
) -> Result<(), ApiError> {
    let row = sqlx::query(
        "SELECT d.group_id, g.owner_user_id, s.permission
         FROM devices d
         LEFT JOIN device_groups g ON g.id = d.group_id AND g.organization_id = d.organization_id
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $3
         WHERE d.id = $1 AND d.organization_id = $2",
    )
    .bind(device_id)
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("device not found in organization"))?;

    let group_id: Option<Uuid> = row.get("group_id");
    if group_id.is_none() {
        return Ok(());
    }

    let owner_id: Option<Uuid> = row.get("owner_user_id");
    if owner_id == Some(user_id) || crate::state::is_admin_role(user_role) {
        return Ok(());
    }

    let permission: Option<String> = row.get("permission");
    let allowed = match (permission.as_deref(), require_write) {
        (Some("read_only"), false) => true,
        (Some("read_write" | "full_control"), _) => true,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(ApiError::not_found("device not found in accessible groups"))
    }
}

pub async fn update_device_group(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
    Json(req): Json<UpdateDeviceGroupRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let current_group_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT group_id FROM devices WHERE id = $1 AND organization_id = $2",
    )
    .bind(device_id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("equipo no encontrado en la organización"))?;

    if current_group_id != req.group_id {
        if let Some(group_id) = current_group_id {
            resolve_group_name_for_write(
                &state.pool,
                auth.user.organization_id,
                auth.user.id,
                &auth.user.role,
                Some(group_id),
                "",
            )
            .await?;
        }
        let target_group_name = if let Some(group_id) = req.group_id {
            resolve_group_name_for_write(
                &state.pool,
                auth.user.organization_id,
                auth.user.id,
                &auth.user.role,
                Some(group_id),
                "",
            )
            .await?
        } else {
            "General".to_string()
        };
        sqlx::query("UPDATE devices SET group_id = $2 WHERE id = $1 AND organization_id = $3")
            .bind(device_id)
            .bind(req.group_id)
            .bind(auth.user.organization_id)
            .execute(&state.pool)
            .await?;

        sqlx::query(
            "UPDATE address_book_entries SET group_id = $2, group_name = $3, updated_at = now()
             WHERE device_id = $1 AND organization_id = $4",
        )
        .bind(device_id)
        .bind(req.group_id)
        .bind(&target_group_name)
        .bind(auth.user.organization_id)
        .execute(&state.pool)
        .await?;

        record_audit(
            &state.pool,
            Some(auth.user.organization_id),
            Some(auth.user.id),
            "user",
            "device.group_updated",
            "device",
            &device_id.to_string(),
            "success",
            json!({ "from_group_id": current_group_id, "to_group_id": req.group_id }),
        )
        .await;
    }

    Ok(Json(json!({ "ok": true, "group_id": req.group_id })))
}

fn normalize_icon_kind(raw: &str) -> Result<String, ApiError> {
    let icon = raw.trim().to_ascii_lowercase();
    let norm = if icon.is_empty() {
        "desktop"
    } else {
        icon.as_str()
    };
    if matches!(
        norm,
        "desktop" | "server" | "laptop" | "database" | "network" | "workstation" | "cloud" | "camera"
    ) {
        Ok(norm.to_string())
    } else {
        Err(ApiError::bad_request("icono inválido"))
    }
}

fn normalize_accent_color(raw: &str) -> Result<String, ApiError> {
    let c = raw.trim();
    if c.is_empty() {
        return Ok(String::new());
    }
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(c.to_ascii_uppercase())
    } else {
        Err(ApiError::bad_request("color hexadecimal inválido (ej: #3B82F6)"))
    }
}

pub async fn update_device_appearance(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
    Json(req): Json<agentdesk_protocol::UpdateDeviceAppearanceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let icon_kind = normalize_icon_kind(&req.icon_kind)?;
    let accent_color = normalize_accent_color(&req.accent_color)?;

    ensure_device_group_access(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        device_id,
        true,
    )
    .await?;

    sqlx::query(
        "UPDATE devices SET icon_kind = $2, accent_color = $3 WHERE id = $1 AND organization_id = $4",
    )
    .bind(device_id)
    .bind(&icon_kind)
    .bind(&accent_color)
    .bind(auth.user.organization_id)
    .execute(&state.pool)
    .await?;

    sqlx::query(
        "UPDATE address_book_entries SET icon_kind = $2, accent_color = $3, updated_at = now()
         WHERE device_id = $1 AND organization_id = $4",
    )
    .bind(device_id)
    .bind(&icon_kind)
    .bind(&accent_color)
    .bind(auth.user.organization_id)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "ok": true,
        "icon_kind": icon_kind,
        "accent_color": accent_color,
    })))
}

pub async fn wake_device(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(device_id): Path<Uuid>,
    body: Option<Json<WolRequest>>,
) -> Result<Json<WolResponse>, ApiError> {
    ensure_device_group_access(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        device_id,
        false,
    )
    .await?;

    let row = sqlx::query(
        "SELECT hostname, mac_addresses, lan_ips
         FROM devices
         WHERE id = $1 AND organization_id = $2",
    )
    .bind(device_id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("equipo no encontrado"))?;

    let hostname: String = row.get("hostname");
    let mut mac_addresses: Vec<String> = row.get("mac_addresses");
    let lan_ips: Vec<String> = row.get("lan_ips");

    if let Some(Json(req)) = body {
        if let Some(raw_mac) = req.mac_address.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let parsed = agentdesk_transport::parse_mac(raw_mac)
                .ok_or_else(|| ApiError::bad_request("Dirección MAC inválida"))?;
            let formatted = agentdesk_transport::format_mac(parsed);
            if !mac_addresses.iter().any(|m| m.eq_ignore_ascii_case(&formatted)) {
                mac_addresses.push(formatted);
                let _ = sqlx::query(
                    "UPDATE devices SET mac_addresses = $2 WHERE id = $1 AND organization_id = $3",
                )
                .bind(device_id)
                .bind(&mac_addresses)
                .bind(auth.user.organization_id)
                .execute(&state.pool)
                .await;
            }
        }
    }

    if mac_addresses.is_empty() {
        return Err(ApiError::bad_request(
            "El equipo aún no reportó su dirección MAC. Abrí AgentDesk v0.1.1 en ese equipo al menos una vez o ingresá la MAC.",
        ));
    }

    let server_sent = agentdesk_transport::send_wol_magic_packets(&mac_addresses, &lan_ips);

    let peer_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM devices WHERE organization_id = $1 AND id <> $2",
    )
    .bind(auth.user.organization_id)
    .bind(device_id)
    .fetch_all(&state.pool)
    .await?;

    let request_id = Uuid::new_v4();
    let mut relayed_agents = 0usize;
    {
        let agents = state.active_agents.read().await;
        for peer_id in peer_ids {
            if let Some(tx) = agents.get(&peer_id) {
                if tx
                    .send(ServerToAgentMessage::SendWakeOnLan {
                        request_id,
                        mac_addresses: mac_addresses.clone(),
                        broadcast_ips: lan_ips.clone(),
                    })
                    .is_ok()
                {
                    relayed_agents += 1;
                }
            }
        }
    }

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "device.wol",
        "device",
        &device_id.to_string(),
        "success",
        json!({
            "hostname": hostname,
            "mac_addresses": mac_addresses,
            "server_sent": server_sent,
            "relayed_agents": relayed_agents,
        }),
    )
    .await;

    let msg = format!(
        "Paquete mágico Wake-on-LAN enviado a {hostname} ({}) · servidor: {server_sent}, pares LAN: {relayed_agents}",
        mac_addresses.join(", ")
    );

    Ok(Json(WolResponse {
        ok: true,
        mac_addresses,
        server_sent,
        relayed_agents,
        message: msg,
    }))
}

// ---------------------------------------------------------------------------
// Address Book / Clientes Agendados (IAM-04 + Grupos Compartidos)
// ---------------------------------------------------------------------------

pub async fn list_address_book(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<AddressBookEntryDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT a.id, a.organization_id, a.device_id, a.group_id, a.alias, a.client_company, a.contact_name,
                a.contact_email, a.group_name, a.is_favorite, a.notes, a.scheduled_next_at,
                a.created_at, a.updated_at, a.icon_kind, a.accent_color,
                d.public_code AS device_public_code, d.hostname AS device_hostname,
                d.status AS device_status, d.os_name AS device_os, d.last_heartbeat_at AS device_last_heartbeat
         FROM address_book_entries a
         LEFT JOIN devices d ON d.id = a.device_id AND d.organization_id = a.organization_id
         LEFT JOIN device_groups g ON g.id = a.group_id
         LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
         WHERE a.organization_id = $1
           AND (
             a.group_id IS NULL
             OR g.owner_user_id = $2
             OR s.user_id IS NOT NULL
             OR $3 IN ('instance_admin', 'admin', 'org_admin')
           )
         ORDER BY a.is_favorite DESC, a.scheduled_next_at ASC NULLS LAST, a.alias ASC",
    )
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(&auth.user.role)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| AddressBookEntryDto {
                id: r.get("id"),
                organization_id: r.get("organization_id"),
                device_id: r.get("device_id"),
                group_id: r.get("group_id"),
                alias: r.get("alias"),
                client_company: r.get("client_company"),
                contact_name: r.get("contact_name"),
                contact_email: r.get("contact_email"),
                group_name: r.get("group_name"),
                is_favorite: r.get("is_favorite"),
                notes: r.get("notes"),
                scheduled_next_at: r.get("scheduled_next_at"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
                device_public_code: r.get("device_public_code"),
                device_hostname: r.get("device_hostname"),
                device_status: r.get("device_status"),
                device_os: r.get("device_os"),
                device_last_heartbeat: r.get("device_last_heartbeat"),
                icon_kind: r.get("icon_kind"),
                accent_color: r.get("accent_color"),
            })
            .collect(),
    ))
}

async fn resolve_group_name_for_write(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    user_id: Uuid,
    user_role: &str,
    group_id: Option<Uuid>,
    fallback_name: &str,
) -> Result<String, ApiError> {
    if let Some(gid) = group_id {
        let row = sqlx::query(
            "SELECT g.name, g.owner_user_id, s.permission
             FROM device_groups g
             LEFT JOIN device_group_shares s ON s.group_id = g.id AND s.user_id = $2
             WHERE g.id = $1 AND g.organization_id = $3",
        )
        .bind(gid)
        .bind(user_id)
        .bind(org_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("grupo no encontrado"))?;

        let owner_id: Option<Uuid> = row.get("owner_user_id");
        let share_perm: Option<String> = row.get("permission");
        let can_write = owner_id == Some(user_id)
            || crate::state::is_admin_role(user_role)
            || matches!(
                share_perm.as_deref(),
                Some("read_write") | Some("full_control")
            );
        if !can_write {
            return Err(ApiError::unauthorized(
                "sin permiso de escritura sobre este grupo",
            ));
        }
        return Ok(row.get("name"));
    }
    Ok(if fallback_name.trim().is_empty() {
        "General".to_string()
    } else {
        fallback_name.trim().to_string()
    })
}

pub async fn create_address_book_entry(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<UpsertAddressBookRequest>,
) -> Result<Json<AddressBookEntryDto>, ApiError> {
    if req.alias.trim().is_empty() {
        return Err(ApiError::bad_request("alias is required"));
    }
    if let Some(did) = req.device_id {
        let owns: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND organization_id = $2")
                .bind(did)
                .bind(auth.user.organization_id)
                .fetch_optional(&state.pool)
                .await?;
        if owns.is_none() {
            return Err(ApiError::not_found(
                "linked device not found in organization",
            ));
        }
    }

    let resolved_group_name = resolve_group_name_for_write(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        req.group_id,
        &req.group_name,
    )
    .await?;

    let icon_kind = normalize_icon_kind(&req.icon_kind)?;
    let accent_color = normalize_accent_color(&req.accent_color)?;

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO address_book_entries (id, organization_id, device_id, group_id, alias, client_company, contact_name, contact_email, group_name, is_favorite, notes, scheduled_next_at, icon_kind, accent_color, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(req.device_id)
    .bind(req.group_id)
    .bind(req.alias.trim())
    .bind(req.client_company.trim())
    .bind(req.contact_name.trim())
    .bind(req.contact_email.trim())
    .bind(&resolved_group_name)
    .bind(req.is_favorite)
    .bind(req.notes.trim())
    .bind(req.scheduled_next_at)
    .bind(&icon_kind)
    .bind(&accent_color)
    .bind(auth.user.id)
    .execute(&state.pool)
    .await?;

    if let Some(did) = req.device_id {
        if !icon_kind.is_empty() || !accent_color.is_empty() {
            let _ = sqlx::query(
                "UPDATE devices SET icon_kind = $2, accent_color = $3 WHERE id = $1 AND organization_id = $4",
            )
            .bind(did)
            .bind(&icon_kind)
            .bind(&accent_color)
            .bind(auth.user.organization_id)
            .execute(&state.pool)
            .await;
        }
    }

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "address_book.created",
        "address_book_entry",
        &id.to_string(),
        "success",
        json!({ "alias": req.alias.trim(), "group": resolved_group_name }),
    )
    .await;

    fetch_address_book_entry(&state.pool, id, auth.user.organization_id)
        .await
        .map(Json)
}

pub async fn update_address_book_entry(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
    Json(req): Json<UpsertAddressBookRequest>,
) -> Result<Json<AddressBookEntryDto>, ApiError> {
    if let Some(did) = req.device_id {
        let owns: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND organization_id = $2")
                .bind(did)
                .bind(auth.user.organization_id)
                .fetch_optional(&state.pool)
                .await?;
        if owns.is_none() {
            return Err(ApiError::not_found(
                "linked device not found in organization",
            ));
        }
    }

    let resolved_group_name = resolve_group_name_for_write(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        req.group_id,
        &req.group_name,
    )
    .await?;

    let icon_kind = normalize_icon_kind(&req.icon_kind)?;
    let accent_color = normalize_accent_color(&req.accent_color)?;

    let res = sqlx::query(
        "UPDATE address_book_entries
         SET device_id = $3, group_id = $4, alias = $5, client_company = $6, contact_name = $7, contact_email = $8,
             group_name = $9, is_favorite = $10, notes = $11, scheduled_next_at = $12, icon_kind = $13, accent_color = $14, updated_at = now()
         WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(req.device_id)
    .bind(req.group_id)
    .bind(req.alias.trim())
    .bind(req.client_company.trim())
    .bind(req.contact_name.trim())
    .bind(req.contact_email.trim())
    .bind(&resolved_group_name)
    .bind(req.is_favorite)
    .bind(req.notes.trim())
    .bind(req.scheduled_next_at)
    .bind(&icon_kind)
    .bind(&accent_color)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("address book entry not found"));
    }

    if let Some(did) = req.device_id {
        let _ = sqlx::query(
            "UPDATE devices SET icon_kind = $2, accent_color = $3 WHERE id = $1 AND organization_id = $4",
        )
        .bind(did)
        .bind(&icon_kind)
        .bind(&accent_color)
        .bind(auth.user.organization_id)
        .execute(&state.pool)
        .await;
    }

    fetch_address_book_entry(&state.pool, id, auth.user.organization_id)
        .await
        .map(Json)
}

pub async fn delete_address_book_entry(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = sqlx::query(
        "SELECT group_id FROM address_book_entries WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("address book entry not found"))?;

    let group_id: Option<Uuid> = row.get("group_id");
    if group_id.is_some() {
        resolve_group_name_for_write(
            &state.pool,
            auth.user.organization_id,
            auth.user.id,
            &auth.user.role,
            group_id,
            "",
        )
        .await?;
    }

    sqlx::query("DELETE FROM address_book_entries WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(auth.user.organization_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn fetch_address_book_entry(
    pool: &sqlx::PgPool,
    id: Uuid,
    org_id: Uuid,
) -> Result<AddressBookEntryDto, ApiError> {
    let r = sqlx::query(
        "SELECT a.id, a.organization_id, a.device_id, a.group_id, a.alias, a.client_company, a.contact_name,
                a.contact_email, a.group_name, a.is_favorite, a.notes, a.scheduled_next_at,
                a.created_at, a.updated_at, a.icon_kind, a.accent_color,
                d.public_code AS device_public_code, d.hostname AS device_hostname,
                d.status AS device_status, d.os_name AS device_os, d.last_heartbeat_at AS device_last_heartbeat
         FROM address_book_entries a
         LEFT JOIN devices d ON d.id = a.device_id AND d.organization_id = a.organization_id
         WHERE a.id = $1 AND a.organization_id = $2",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("entry not found"))?;

    Ok(AddressBookEntryDto {
        id: r.get("id"),
        organization_id: r.get("organization_id"),
        device_id: r.get("device_id"),
        group_id: r.get("group_id"),
        alias: r.get("alias"),
        client_company: r.get("client_company"),
        contact_name: r.get("contact_name"),
        contact_email: r.get("contact_email"),
        group_name: r.get("group_name"),
        is_favorite: r.get("is_favorite"),
        notes: r.get("notes"),
        scheduled_next_at: r.get("scheduled_next_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        device_public_code: r.get("device_public_code"),
        device_hostname: r.get("device_hostname"),
        device_status: r.get("device_status"),
        device_os: r.get("device_os"),
        device_last_heartbeat: r.get("device_last_heartbeat"),
        icon_kind: r.get("icon_kind"),
        accent_color: r.get("accent_color"),
    })
}

// ---------------------------------------------------------------------------
// Public Support Requests & Technician Ticket Queue (SUP-01..06)
// ---------------------------------------------------------------------------

pub async fn create_support_request(
    State(state): State<AppState>,
    Json(req): Json<CreateSupportRequest>,
) -> Result<Json<CreateSupportResponse>, ApiError> {
    if !req.consent_accepted {
        return Err(ApiError::bad_request("data consent is required"));
    }
    if req.requester_name.trim().is_empty()
        || req.requester_contact.trim().is_empty()
        || req.description.trim().is_empty()
    {
        return Err(ApiError::bad_request(
            "name, contact, and description are required",
        ));
    }

    let org_id: Uuid =
        sqlx::query_scalar("SELECT id FROM organizations ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::bad_request("instance is not bootstrapped yet"))?;

    let ticket_id = Uuid::new_v4();
    let public_code = generate_short_code("SUP");
    let tracking_token = generate_high_entropy_token();
    let tracking_hash = sha256_hex(tracking_token.as_bytes());

    sqlx::query(
        "INSERT INTO support_requests (id, organization_id, public_code, tracking_token_hash, requester_name, requester_contact, company_name, category, description, consent_accepted, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, 'queued')",
    )
    .bind(ticket_id)
    .bind(org_id)
    .bind(&public_code)
    .bind(&tracking_hash)
    .bind(req.requester_name.trim())
    .bind(req.requester_contact.trim())
    .bind(req.company_name.as_deref().unwrap_or("").trim())
    .bind(req.category.as_deref().unwrap_or("Soporte General").trim())
    .bind(req.description.trim())
    .execute(&state.pool)
    .await?;

    record_audit(
        &state.pool,
        Some(org_id),
        None,
        "public_requester",
        "support.created",
        "support_request",
        &ticket_id.to_string(),
        "success",
        json!({ "public_code": public_code }),
    )
    .await;

    state.emit_event(
        "support.created",
        org_id,
        ticket_id.to_string(),
        format!(
            "Nueva solicitud {public_code} de {}",
            req.requester_name.trim()
        ),
    );

    Ok(Json(CreateSupportResponse {
        ticket_id,
        public_code: public_code.clone(),
        tracking_token,
        deep_link: format!("agentdesk://support/{public_code}"),
        status: "queued".into(),
    }))
}

#[derive(Deserialize)]
pub struct SupportTokenQuery {
    pub token: String,
}

pub async fn get_support_status(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<SupportTokenQuery>,
) -> Result<Json<SupportTicketDto>, ApiError> {
    let thash = sha256_hex(q.token.as_bytes());
    let r = sqlx::query(
        "SELECT id, organization_id, public_code, requester_name, requester_contact, company_name, category, description, status, device_id, assigned_to, created_at, updated_at
         FROM support_requests WHERE public_code = $1 AND tracking_token_hash = $2",
    )
    .bind(code.trim())
    .bind(&thash)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("support request not found or invalid token"))?;

    Ok(Json(row_to_ticket(r)))
}

pub async fn cancel_support_request(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<SupportTokenQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thash = sha256_hex(q.token.as_bytes());
    let res = sqlx::query(
        "UPDATE support_requests SET status = 'cancelled', updated_at = now()
         WHERE public_code = $1 AND tracking_token_hash = $2 AND status IN ('queued', 'assigned')",
    )
    .bind(code.trim())
    .bind(&thash)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("support request cannot be cancelled"));
    }
    Ok(Json(json!({ "ok": true, "status": "cancelled" })))
}

pub async fn list_tickets(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<SupportTicketDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, organization_id, public_code, requester_name, requester_contact, company_name, category, description, status, device_id, assigned_to, created_at, updated_at
         FROM support_requests WHERE organization_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(rows.into_iter().map(row_to_ticket).collect()))
}

pub async fn claim_ticket(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<SupportTicketDto>, ApiError> {
    // Atomic claim prevents double assignment (SUP-04)
    let r = sqlx::query(
        "UPDATE support_requests
         SET assigned_to = $3, status = 'assigned', updated_at = now()
         WHERE id = $1 AND organization_id = $2 AND assigned_to IS NULL AND status = 'queued'
         RETURNING id, organization_id, public_code, requester_name, requester_contact, company_name, category, description, status, device_id, assigned_to, created_at, updated_at",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::conflict("ticket already claimed or unavailable"))?;

    let dto = row_to_ticket(r);
    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "ticket.claimed",
        "support_request",
        &id.to_string(),
        "success",
        json!({ "public_code": dto.public_code }),
    )
    .await;

    state.emit_event(
        "ticket.assigned",
        auth.user.organization_id,
        id.to_string(),
        format!(
            "Ticket {} reclamado por {}",
            dto.public_code, auth.user.display_name
        ),
    );

    Ok(Json(dto))
}

pub async fn close_ticket(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<SupportTicketDto>, ApiError> {
    let r = sqlx::query(
        "UPDATE support_requests
         SET status = 'closed', updated_at = now()
         WHERE id = $1 AND organization_id = $2
         RETURNING id, organization_id, public_code, requester_name, requester_contact, company_name, category, description, status, device_id, assigned_to, created_at, updated_at",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("ticket not found"))?;

    let dto = row_to_ticket(r);
    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "ticket.closed",
        "support_request",
        &id.to_string(),
        "success",
        json!({ "public_code": dto.public_code }),
    )
    .await;
    state.emit_event(
        "ticket.closed",
        auth.user.organization_id,
        id.to_string(),
        format!(
            "Ticket {} cerrado por {}",
            dto.public_code, auth.user.display_name
        ),
    );
    Ok(Json(dto))
}

pub async fn resolve_ticket(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
    Json(req): Json<ResolveTicketRequest>,
) -> Result<Json<SupportTicketDto>, ApiError> {
    if matches!(auth.user.role.as_str(), "auditor" | "client") {
        return Err(ApiError::forbidden("role cannot resolve support tickets"));
    }
    let outcome = req.outcome.trim();
    if !matches!(outcome, "resolved" | "partial" | "unresolved") {
        return Err(ApiError::bad_request("invalid resolution outcome"));
    }
    let summary = req.resolution_summary.trim();
    if summary.is_empty() || summary.chars().count() > 4000 {
        return Err(ApiError::bad_request(
            "resolution summary must contain 1 to 4000 characters",
        ));
    }

    let mut tx = state.pool.begin().await?;
    let ticket_row = sqlx::query(
        "UPDATE support_requests
         SET status = 'closed', updated_at = now()
         WHERE id = $1 AND organization_id = $2 AND status IN ('queued', 'assigned')
         RETURNING id, organization_id, public_code, requester_name, requester_contact, company_name,
                   category, description, status, device_id, assigned_to, created_at, updated_at",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::conflict("ticket is already closed or unavailable"))?;

    let session_rows = sqlx::query(
        "SELECT s.id, s.status, s.started_at, s.ended_at, s.allow_control, d.hostname
         FROM remote_sessions s
         JOIN devices d ON d.id = s.device_id AND d.organization_id = s.organization_id
         WHERE s.ticket_id = $1 AND s.organization_id = $2 ORDER BY s.created_at ASC",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_all(&mut *tx)
    .await?;
    let session_lines: Vec<String> = session_rows
        .iter()
        .map(|session| {
            let session_id: Uuid = session.get("id");
            let status: String = session.get("status");
            let hostname: String = session.get("hostname");
            let allow_control: bool = session.get("allow_control");
            let mode = if allow_control { "control" } else { "solo vista" };
            let started_at: Option<chrono::DateTime<Utc>> = session.get("started_at");
            let ended_at: Option<chrono::DateTime<Utc>> = session.get("ended_at");
            let elapsed = started_at
                .zip(ended_at)
                .map(|(start, end)| format!(" · duración {} s", (end - start).num_seconds().max(0)))
                .unwrap_or_default();
            format!("{session_id} · {hostname} · {mode} · {status}{elapsed}")
        })
        .collect();
    let outcome_label = match outcome {
        "resolved" => "Resuelto",
        "partial" => "Parcial",
        _ => "Sin solución",
    };
    let resolution_note = format!(
        "Resultado: {outcome_label}\n\nResumen de resolución:\n{summary}\n\nSesiones vinculadas ({}):\n{}",
        session_lines.len(),
        if session_lines.is_empty() {
            "Sin sesiones remotas vinculadas".to_string()
        } else {
            session_lines.join("\n")
        },
    );
    sqlx::query(
        "INSERT INTO ticket_comments (id, organization_id, ticket_id, author_id, author_name, is_internal, body)
         VALUES ($1, $2, $3, $4, $5, true, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(auth.user.organization_id)
    .bind(id)
    .bind(auth.user.id)
    .bind(&auth.user.display_name)
    .bind(&resolution_note)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let dto = row_to_ticket(ticket_row);
    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "ticket.resolved",
        "support_request",
        &id.to_string(),
        "success",
        json!({
            "public_code": dto.public_code,
            "outcome": outcome,
            "linked_sessions": session_lines.len(),
            "summary_length": summary.chars().count(),
        }),
    )
    .await;
    state.emit_event(
        "ticket.closed",
        auth.user.organization_id,
        id.to_string(),
        format!(
            "Ticket {} cerrado por {} · {}",
            dto.public_code, auth.user.display_name, outcome_label
        ),
    );
    Ok(Json(dto))
}

fn row_to_ticket(r: sqlx::postgres::PgRow) -> SupportTicketDto {
    SupportTicketDto {
        id: r.get("id"),
        organization_id: r.get("organization_id"),
        public_code: r.get("public_code"),
        requester_name: r.get("requester_name"),
        requester_contact: r.get("requester_contact"),
        company_name: r.get("company_name"),
        category: r.get("category"),
        description: r.get("description"),
        status: r.get("status"),
        device_id: r.get("device_id"),
        assigned_to: r.get("assigned_to"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

// ---------------------------------------------------------------------------
// Remote Sessions
// ---------------------------------------------------------------------------

pub async fn list_sessions(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<SessionDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT s.id, s.organization_id, s.device_id, s.ticket_id, s.technician_id,
                s.allow_control, s.allow_clipboard, s.allow_files, s.status, s.created_at
         FROM remote_sessions s
         JOIN devices d ON d.id = s.device_id AND d.organization_id = s.organization_id
         LEFT JOIN device_groups g ON g.id = d.group_id AND g.organization_id = d.organization_id
         LEFT JOIN device_group_shares shares ON shares.group_id = g.id AND shares.user_id = $2
         WHERE s.organization_id = $1
           AND (d.group_id IS NULL OR g.owner_user_id = $2 OR shares.user_id IS NOT NULL OR $3 IN ('instance_admin', 'admin', 'org_admin'))
         ORDER BY s.created_at DESC LIMIT 100",
    )
    .bind(auth.user.organization_id)
    .bind(auth.user.id)
    .bind(&auth.user.role)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| SessionDto {
                id: r.get("id"),
                organization_id: r.get("organization_id"),
                device_id: r.get("device_id"),
                ticket_id: r.get("ticket_id"),
                technician_id: r.get("technician_id"),
                allow_control: r.get("allow_control"),
                allow_clipboard: r.get("allow_clipboard"),
                allow_files: r.get("allow_files"),
                status: r.get("status"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

pub async fn create_session(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let read_only_role = matches!(auth.user.role.as_str(), "client" | "auditor");
    if read_only_role && req.allow_control {
        return Err(ApiError::forbidden(
            "role cannot start control sessions",
        ));
    }
    if read_only_role && (req.allow_clipboard || req.allow_files) {
        return Err(ApiError::forbidden(
            "role cannot use clipboard or file transfer in remote sessions",
        ));
    }
    ensure_device_group_access(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        req.device_id,
        req.allow_control || req.allow_clipboard || req.allow_files,
    )
    .await?;
    if let Some(ticket_id) = req.ticket_id {
        let ticket_device_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT device_id FROM support_requests WHERE id = $1 AND organization_id = $2",
        )
        .bind(ticket_id)
        .bind(auth.user.organization_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket not found in organization"))?;
        if ticket_device_id.is_some_and(|ticket_device_id| ticket_device_id != req.device_id) {
            return Err(ApiError::bad_request(
                "session device must match the ticket device",
            ));
        }
    }
    let dev_row = sqlx::query(
        "SELECT id, ed25519_pubkey, status FROM devices WHERE id = $1 AND organization_id = $2",
    )
    .bind(req.device_id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("device not found in organization"))?;

    let device_pubkey: String = dev_row.get("ed25519_pubkey");

    let session_id = Uuid::new_v4();
    let host_relay_token = generate_high_entropy_token();
    let viewer_relay_token = generate_high_entropy_token();

    let row = sqlx::query(
        "INSERT INTO remote_sessions (id, organization_id, device_id, ticket_id, technician_id, allow_control, allow_clipboard, allow_files, host_relay_token, viewer_relay_token, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending')
         RETURNING created_at",
    )
    .bind(session_id)
    .bind(auth.user.organization_id)
    .bind(req.device_id)
    .bind(req.ticket_id)
    .bind(auth.user.id)
    .bind(req.allow_control)
    .bind(req.allow_clipboard)
    .bind(req.allow_files)
    .bind(&host_relay_token)
    .bind(&viewer_relay_token)
    .fetch_one(&state.pool)
    .await?;

    let relay_ws_path = format!("/ws/relay/{session_id}");

    // Notify online agent if connected via /ws/agent
    let agent_tx = {
        let agents = state.active_agents.read().await;
        agents.get(&req.device_id).cloned()
    };
    if let Some(tx) = agent_tx {
        let _ = tx.send(ServerToAgentMessage::IncomingSession {
            session_id,
            technician_name: auth.user.display_name.clone(),
            allow_control: req.allow_control,
            allow_clipboard: req.allow_clipboard,
            allow_files: req.allow_files,
            relay_path: relay_ws_path.clone(),
            host_relay_token,
        });
    }

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "session.started",
        "remote_session",
        &session_id.to_string(),
        "success",
        json!({ "device_id": req.device_id, "allow_control": req.allow_control }),
    )
    .await;

    state.emit_event(
        "session.started",
        auth.user.organization_id,
        session_id.to_string(),
        format!("Sesión iniciada por {}", auth.user.display_name),
    );

    Ok(Json(CreateSessionResponse {
        session: SessionDto {
            id: session_id,
            organization_id: auth.user.organization_id,
            device_id: req.device_id,
            ticket_id: req.ticket_id,
            technician_id: Some(auth.user.id),
            allow_control: req.allow_control,
            allow_clipboard: req.allow_clipboard,
            allow_files: req.allow_files,
            status: "pending".into(),
            created_at: row.get("created_at"),
        },
        viewer_relay_token,
        relay_ws_path,
        device_pubkey,
    }))
}

fn generate_numeric_connection_id() -> String {
    let mut rng = rand::thread_rng();
    let num: u32 = rand::Rng::gen_range(&mut rng, 100_000_000..=999_999_999);
    let a = num / 1_000_000;
    let b = (num / 1_000) % 1_000;
    let c = num % 1_000;
    format!("{a:03} {b:03} {c:03}")
}

fn normalize_connection_id(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

pub async fn quick_register_client(
    State(state): State<AppState>,
    Json(req): Json<QuickRegisterRequest>,
) -> Result<Json<QuickRegisterResponse>, ApiError> {
    if req.ed25519_pubkey.len() != 64 {
        return Err(ApiError::bad_request("invalid ed25519 pubkey hex"));
    }
    let trimmed_pw = req.access_password.trim();
    if trimmed_pw.len() < 4 {
        return Err(ApiError::bad_request(
            "access_password must be at least 4 characters",
        ));
    }

    let org_id: Uuid =
        sqlx::query_scalar("SELECT id FROM organizations ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::bad_request("instance is not bootstrapped yet"))?;

    let pw_hash = hash_password(trimmed_pw).map_err(|e| ApiError::internal(e.to_string()))?;
    let personal_hash = match req.personal_password.as_deref().map(str::trim) {
        Some(pp) if pp.len() >= 4 => {
            Some(hash_password(pp).map_err(|e| ApiError::internal(e.to_string()))?)
        }
        _ => None,
    };
    let owner_user_id: Option<Uuid> = match req.owner_email.as_deref().map(str::trim) {
        Some(em) if !em.is_empty() => {
            sqlx::query_scalar(
                "SELECT id FROM users WHERE organization_id = $1 AND LOWER(email) = LOWER($2)",
            )
            .bind(org_id)
            .bind(em)
            .fetch_optional(&state.pool)
            .await?
        }
        _ => None,
    };

    let device_token = generate_high_entropy_token();
    let device_token_hash = sha256_hex(device_token.as_bytes());

    let mut tx = state.pool.begin().await?;
    let existing = sqlx::query(
        "SELECT id, public_code FROM devices WHERE organization_id = $1 AND ed25519_pubkey = $2",
    )
    .bind(org_id)
    .bind(&req.ed25519_pubkey)
    .fetch_optional(&mut *tx)
    .await?;

    let (device_id, connection_id) = if let Some(row) = existing {
        let did: Uuid = row.get("id");
        let pcode: String = row.get("public_code");
        sqlx::query(
            "UPDATE devices SET device_token_hash = $2, access_password_hash = $3, hostname = $4,
             os_name = $5, os_arch = $6, agent_version = $7, kind = 'desktop',
             screen_width = $8, screen_height = $9, private_mem_mb = $10,
             personal_password_hash = COALESCE($11, personal_password_hash),
             owner_user_id = COALESCE($12, owner_user_id),
             mac_addresses = CASE WHEN cardinality($13::text[]) > 0 THEN $13 ELSE mac_addresses END,
             lan_ips = CASE WHEN cardinality($14::text[]) > 0 THEN $14 ELSE lan_ips END,
             status = 'online', last_heartbeat_at = now()
             WHERE id = $1",
        )
        .bind(did)
        .bind(&device_token_hash)
        .bind(&pw_hash)
        .bind(&req.hostname)
        .bind(&req.os_name)
        .bind(&req.os_arch)
        .bind(&req.agent_version)
        .bind(req.screen_width)
        .bind(req.screen_height)
        .bind(req.private_mem_mb)
        .bind(&personal_hash)
        .bind(owner_user_id)
        .bind(&req.mac_addresses)
        .bind(&req.lan_ips)
        .execute(&mut *tx)
        .await?;
        (did, pcode)
    } else {
        let did = Uuid::new_v4();
        let mut chosen_code = generate_numeric_connection_id();
        for _ in 0..8 {
            let exists: Option<i32> =
                sqlx::query_scalar("SELECT 1 FROM devices WHERE public_code = $1")
                    .bind(&chosen_code)
                    .fetch_optional(&mut *tx)
                    .await?;
            if exists.is_none() {
                break;
            }
            chosen_code = generate_numeric_connection_id();
        }
        sqlx::query(
            "INSERT INTO devices (id, organization_id, public_code, ed25519_pubkey, device_token_hash, access_password_hash, personal_password_hash, owner_user_id, hostname, os_name, os_arch, agent_version, kind, screen_width, screen_height, private_mem_mb, mac_addresses, lan_ips, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'desktop', $13, $14, $15, $16, $17, 'online')",
        )
        .bind(did)
        .bind(org_id)
        .bind(&chosen_code)
        .bind(&req.ed25519_pubkey)
        .bind(&device_token_hash)
        .bind(&pw_hash)
        .bind(&personal_hash)
        .bind(owner_user_id)
        .bind(&req.hostname)
        .bind(&req.os_name)
        .bind(&req.os_arch)
        .bind(&req.agent_version)
        .bind(req.screen_width)
        .bind(req.screen_height)
        .bind(req.private_mem_mb)
        .bind(&req.mac_addresses)
        .bind(&req.lan_ips)
        .execute(&mut *tx)
        .await?;
        (did, chosen_code)
    };

    tx.commit().await?;

    record_audit(
        &state.pool,
        Some(org_id),
        Some(device_id),
        "device",
        "client.registered",
        "device",
        &device_id.to_string(),
        "success",
        json!({ "hostname": req.hostname, "connection_id": connection_id }),
    )
    .await;

    state.emit_event(
        "client.registered",
        org_id,
        device_id.to_string(),
        format!("Cliente {} listo (ID: {})", req.hostname, connection_id),
    );

    Ok(Json(QuickRegisterResponse {
        device_id,
        organization_id: org_id,
        connection_id,
        device_token,
    }))
}

pub async fn rotate_client_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RotatePasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let raw = headers
        .get("x-device-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing x-device-token"))?;
    let trimmed_pw = req.new_password.trim();
    if trimmed_pw.len() < 4 {
        return Err(ApiError::bad_request(
            "new_password must be at least 4 characters",
        ));
    }

    let thash = sha256_hex(raw.as_bytes());
    let pw_hash = hash_password(trimmed_pw).map_err(|e| ApiError::internal(e.to_string()))?;

    let res = sqlx::query(
        "UPDATE devices SET access_password_hash = $3, last_heartbeat_at = now()
         WHERE id = $1 AND device_token_hash = $2",
    )
    .bind(req.device_id)
    .bind(&thash)
    .bind(&pw_hash)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::unauthorized("invalid device credentials"));
    }

    Ok(Json(json!({ "ok": true })))
}

pub async fn set_personal_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetPersonalPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let raw = headers
        .get("x-device-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing x-device-token"))?;
    let thash = sha256_hex(raw.as_bytes());

    let personal_hash = match req.personal_password.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(pw) if pw.len() < 4 => {
            return Err(ApiError::bad_request(
                "personal_password must be at least 4 characters",
            ))
        }
        Some(pw) => Some(hash_password(pw).map_err(|e| ApiError::internal(e.to_string()))?),
    };

    let res = sqlx::query(
        "UPDATE devices SET personal_password_hash = $3, last_heartbeat_at = now()
         WHERE id = $1 AND device_token_hash = $2",
    )
    .bind(req.device_id)
    .bind(&thash)
    .bind(&personal_hash)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::unauthorized("invalid device credentials"));
    }

    Ok(Json(
        json!({ "ok": true, "has_personal_password": personal_hash.is_some() }),
    ))
}

#[derive(Deserialize)]
pub struct LookupQuery {
    pub connection_id: String,
}

pub async fn lookup_client_by_id(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
) -> Result<Json<ClientLookupResponse>, ApiError> {
    let norm = normalize_connection_id(&q.connection_id);
    if norm.is_empty() {
        return Ok(Json(ClientLookupResponse {
            exists: false,
            online: false,
            connection_id: String::new(),
            hostname: String::new(),
        }));
    }

    let row = sqlx::query(
        "SELECT id, hostname, public_code
         FROM devices
         WHERE regexp_replace(UPPER(public_code), '[^A-Z0-9]', '', 'g') = $1
         LIMIT 1",
    )
    .bind(&norm)
    .fetch_optional(&state.pool)
    .await?;

    let Some(row) = row else {
        return Ok(Json(ClientLookupResponse {
            exists: false,
            online: false,
            connection_id: String::new(),
            hostname: String::new(),
        }));
    };

    let device_id: Uuid = row.get("id");
    let hostname: String = row.get("hostname");
    let public_code: String = row.get("public_code");
    let online = state.active_agents.read().await.contains_key(&device_id);

    Ok(Json(ClientLookupResponse {
        exists: true,
        online,
        connection_id: public_code,
        hostname,
    }))
}

pub async fn connect_by_id(
    State(state): State<AppState>,
    Json(req): Json<DirectConnectRequest>,
) -> Result<Json<DirectConnectResponse>, ApiError> {
    let norm = normalize_connection_id(&req.connection_id);
    if norm.is_empty() {
        return Err(ApiError::bad_request("connection_id requerido"));
    }
    let reconnect_tok = req
        .reconnect_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let raw_pw = req.password.trim();
    if raw_pw.is_empty() && reconnect_tok.is_none() {
        return Err(ApiError::bad_request(
            "contraseña o token de reconexión requerido",
        ));
    }

    let row = sqlx::query(
        "SELECT id, organization_id, hostname, public_code, ed25519_pubkey, access_password_hash, personal_password_hash
         FROM devices
         WHERE regexp_replace(UPPER(public_code), '[^A-Z0-9]', '', 'g') = $1
         LIMIT 1",
    )
    .bind(&norm)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("ID o contraseña incorrectos"))?;

    let device_id: Uuid = row.get("id");
    let org_id: Uuid = row.get("organization_id");
    let target_hostname: String = row.get("hostname");
    let target_connection_id: String = row.get("public_code");
    let device_pubkey: String = row.get("ed25519_pubkey");

    let mut authenticated = false;
    if !raw_pw.is_empty() {
        let access_hash: Option<String> = row.get("access_password_hash");
        let personal_hash: Option<String> = row.get("personal_password_hash");
        // Argon2 is deliberately slow: run it off the async workers and stop at the first match
        // (it used to always hash twice on the request path).
        let pw = raw_pw.to_string();
        authenticated = tokio::task::spawn_blocking(move || {
            [access_hash, personal_hash]
                .iter()
                .flatten()
                .any(|h| verify_password(&pw, h).unwrap_or(false))
        })
        .await
        .unwrap_or(false);
    } else if let Some(tok) = reconnect_tok {
        let thash = sha256_hex(tok.as_bytes());
        let valid: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM direct_reconnect_grants
             WHERE device_id = $1 AND token_hash = $2 AND expires_at > now()",
        )
        .bind(device_id)
        .bind(&thash)
        .fetch_optional(&state.pool)
        .await?;
        authenticated = valid.is_some();
        if !authenticated {
            return Err(ApiError::unauthorized(
                "La ventana de reconexión de 1 hora expiró. Ingresá la contraseña.",
            ));
        }
    }

    if !authenticated {
        return Err(ApiError::unauthorized("ID o contraseña incorrectos"));
    }

    let agent_tx = {
        let agents = state.active_agents.read().await;
        agents.get(&device_id).cloned()
    }
    .ok_or_else(|| ApiError::conflict("El equipo destino no está conectado al servidor"))?;

    let session_id = Uuid::new_v4();
    let host_relay_token = generate_high_entropy_token();
    let viewer_relay_token = generate_high_entropy_token();
    let caller_label = req
        .caller_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Cliente AgentDesk")
        .to_string();

    sqlx::query(
        "INSERT INTO remote_sessions (id, organization_id, device_id, ticket_id, technician_id, caller_label, allow_control, allow_clipboard, allow_files, host_relay_token, viewer_relay_token, status)
         VALUES ($1, $2, $3, NULL, NULL, $4, $5, $6, false, $7, $8, 'pending')",
    )
    .bind(session_id)
    .bind(org_id)
    .bind(device_id)
    .bind(&caller_label)
    .bind(req.allow_control)
    .bind(req.allow_clipboard)
    .bind(&host_relay_token)
    .bind(&viewer_relay_token)
    .execute(&state.pool)
    .await?;

    // Emitir token efímero de 1 hora para reconexiones rápidas / Reboot & Reconnect sin guardar contraseñas en disco
    let _ = sqlx::query("DELETE FROM direct_reconnect_grants WHERE expires_at < now()")
        .execute(&state.pool)
        .await;
    let fresh_reconnect_token = generate_high_entropy_token();
    let fresh_reconnect_hash = sha256_hex(fresh_reconnect_token.as_bytes());
    let reconnect_expires_at = Utc::now() + Duration::hours(1);
    let _ = sqlx::query(
        "INSERT INTO direct_reconnect_grants (id, device_id, token_hash, caller_label, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(device_id)
    .bind(&fresh_reconnect_hash)
    .bind(&caller_label)
    .bind(reconnect_expires_at)
    .execute(&state.pool)
    .await;

    let relay_ws_path = format!("/ws/relay/{session_id}");
    let _ = agent_tx.send(ServerToAgentMessage::IncomingSession {
        session_id,
        technician_name: caller_label.clone(),
        allow_control: req.allow_control,
        allow_clipboard: req.allow_clipboard,
        allow_files: false,
        relay_path: relay_ws_path.clone(),
        host_relay_token,
    });

    record_audit(
        &state.pool,
        Some(org_id),
        Some(device_id),
        "peer",
        "session.direct_connect",
        "remote_session",
        &session_id.to_string(),
        "success",
        json!({ "target_connection_id": target_connection_id, "caller": caller_label }),
    )
    .await;

    state.emit_event(
        "session.direct_connect",
        org_id,
        session_id.to_string(),
        format!(
            "Conexión directa a {} ({})",
            target_hostname, target_connection_id
        ),
    );

    Ok(Json(DirectConnectResponse {
        session_id,
        viewer_relay_token,
        relay_ws_path,
        target_hostname,
        target_connection_id,
        device_pubkey,
        reconnect_token: Some(fresh_reconnect_token),
        reconnect_expires_at: Some(reconnect_expires_at),
    }))
}

pub async fn terminate_session(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session_device_id: Uuid = sqlx::query_scalar(
        "SELECT device_id FROM remote_sessions WHERE id = $1 AND organization_id = $2",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("session not found"))?;
    ensure_device_group_access(
        &state.pool,
        auth.user.organization_id,
        auth.user.id,
        &auth.user.role,
        session_device_id,
        true,
    )
    .await?;
    let row = sqlx::query(
        "UPDATE remote_sessions SET status = 'ended', ended_at = now()
         WHERE id = $1 AND organization_id = $2
         RETURNING device_id",
    )
    .bind(id)
    .bind(auth.user.organization_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("session not found"))?;

    let device_id: Uuid = row.get("device_id");
    if let Some(tx) = state.active_agents.read().await.get(&device_id).cloned() {
        let _ = tx.send(ServerToAgentMessage::TerminateSession {
            session_id: id,
            reason: "terminated by technician".into(),
        });
    }
    state.relay_rooms.write().await.remove(&id);

    record_audit(
        &state.pool,
        Some(auth.user.organization_id),
        Some(auth.user.id),
        "user",
        "session.ended",
        "remote_session",
        &id.to_string(),
        "success",
        json!({}),
    )
    .await;

    Ok(Json(json!({ "ok": true, "status": "ended" })))
}

// ---------------------------------------------------------------------------
// Audit, SSE Events, Downloads
// ---------------------------------------------------------------------------

pub async fn list_audit(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<Vec<AuditEventDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, organization_id, actor_id, actor_type, action, resource_type, resource_id, outcome, metadata, occurred_at
         FROM audit_events WHERE organization_id = $1 ORDER BY occurred_at DESC LIMIT 100",
    )
    .bind(auth.user.organization_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| AuditEventDto {
                id: r.get("id"),
                organization_id: r.get("organization_id"),
                actor_id: r.get("actor_id"),
                actor_type: r.get("actor_type"),
                action: r.get("action"),
                resource_type: r.get("resource_type"),
                resource_id: r.get("resource_id"),
                outcome: r.get("outcome"),
                metadata: r.get("metadata"),
                occurred_at: r.get("occurred_at"),
            })
            .collect(),
    ))
}

pub async fn sse_events(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.events_tx.subscribe();
    let org_id = auth.user.organization_id;
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.organization_id == org_id => {
                    if let Ok(json) = serde_json::to_string(&ev) {
                        yield Ok(Event::default().event(ev.kind).data(json));
                    }
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn list_downloads(State(state): State<AppState>) -> Json<Vec<DownloadArtifactDto>> {
    let version = env!("CARGO_PKG_VERSION");
    let desktop_file = format!("agentdesk-desktop-v{version}-x64.exe");
    let host_file = format!("agentdesk-host-v{version}-x64.exe");
    let quicksupport_file = format!("agentdesk-quicksupport-v{version}-x64.exe");
    let candidates = [
        (
            "desktop",
            "AgentDesk Desktop Client",
            desktop_file.as_str(),
            "Consola nativa de técnicos + visor remoto wgpu",
        ),
        (
            "host",
            "AgentDesk Host Agent",
            host_file.as_str(),
            "Agente residente Windows para acceso desatendido y captura DXGI",
        ),
        (
            "quicksupport",
            "AgentDesk QuickSupport (Portable)",
            quicksupport_file.as_str(),
            "Ejecutable portable sin instalación, inicia directamente en modo QuickSupport",
        ),
    ];
    let mut out = Vec::new();
    for (id, name, filename, notes) in candidates {
        let path = state.artifacts_dir.join(filename);
        let (available, size_bytes, sha256) = match fs::read(&path).await {
            Ok(bytes) => (true, bytes.len() as u64, sha256_hex(&bytes)),
            Err(_) => (false, 0, "no-compilado-aun".into()),
        };
        out.push(DownloadArtifactDto {
            id: id.into(),
            name: name.into(),
            filename: filename.into(),
            platform: "Windows".into(),
            architecture: "x64".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            available,
            size_bytes,
            sha256,
            download_url: format!("/downloads/file/{filename}"),
            notes: notes.into(),
        });
    }
    Json(out)
}

pub async fn download_artifact_file(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, ApiError> {
    let version = env!("CARGO_PKG_VERSION");
    let allowed = [
        format!("agentdesk-desktop-v{version}-x64.exe"),
        format!("agentdesk-host-v{version}-x64.exe"),
        format!("agentdesk-quicksupport-v{version}-x64.exe"),
    ];
    if !allowed.iter().any(|candidate| candidate == &filename) {
        return Err(ApiError::not_found("artifact not found"));
    }
    let path = state.artifacts_dir.join(&filename);
    let bytes = fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found("binary not built yet"))?;
    let disp = format!("attachment; filename=\"{filename}\"");
    Ok((
        [
            (CONTENT_TYPE, "application/octet-stream".to_string()),
            (CONTENT_DISPOSITION, disp),
        ],
        Body::from(bytes),
    )
        .into_response())
}
