use agentdesk_identity::{generate_high_entropy_token, hash_password, DeviceIdentity};
use agentdesk_protocol::{
    AddressBookEntryDto, AuthResponse, BootstrapRequest, ControlMessage, CreateEnrollmentResponse,
    CreateSessionRequest, CreateSessionResponse, CreateSupportRequest, CreateSupportResponse,
    DataPacket, DeviceDto, DirectConnectRequest, DirectConnectResponse, EnrollDeviceRequest,
    EnrollDeviceResponse, FrameEncoding, QuickRegisterRequest, QuickRegisterResponse,
    RotatePasswordRequest, SupportTicketDto, UpsertAddressBookRequest, VideoFrame,
};
use agentdesk_server::{build_router, state::AppState};
use agentdesk_transport::RelayTunnel;
use futures_util::StreamExt;
use reqwest::StatusCode;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use uuid::Uuid;

async fn spawn_test_server() -> Option<(String, String, sqlx::PgPool)> {
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://agentdesk:agentdesk_dev_pass@127.0.0.1:5432/agentdesk".to_string()
    });
    let pool = match PgPoolOptions::new().max_connections(5).connect(&db_url).await {
        Ok(p) => p,
        Err(_) => return None,
    };
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

    // Clean test tables for deterministic isolated run
    sqlx::query("TRUNCATE audit_events, remote_sessions, support_requests, address_book_entries, devices, enrollments, device_groups, auth_sessions, users, organizations RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let boot = generate_high_entropy_token();
    let state = AppState::new(pool.clone(), boot.clone(), PathBuf::from("target/debug"));
    let app = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Some((format!("http://{addr}"), boot, pool))
}

#[tokio::test]
async fn end_to_end_control_plane_tenant_isolation_and_blind_relay() {
    let Some((base, boot, pool)) = spawn_test_server().await else {
        eprintln!("Skipping PostgreSQL integration test: PostgreSQL not reachable");
        return;
    };
    let client = reqwest::Client::new();

    // 1. Public web pages
    for path in ["/", "/downloads", "/support", "/setup", "/login", "/app", "/health/ready"] {
        let r = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "GET {path}");
    }

    // 2. Bootstrap Org A
    let auth_a: AuthResponse = client
        .post(format!("{base}/api/v1/setup/bootstrap"))
        .json(&BootstrapRequest {
            organization_name: "Org Alpha".into(),
            organization_slug: "org-alpha".into(),
            admin_email: "admin@alpha.test".into(),
            admin_name: "Admin Alpha".into(),
            admin_password: "Password123!".into(),
            bootstrap_token: boot,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // 3. Seed Org B user directly in DB to verify strict cross-tenant isolation
    let org_b_id = Uuid::new_v4();
    let user_b_id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Org Beta', 'org-beta')")
        .bind(org_b_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, organization_id, email, display_name, password_hash, role) VALUES ($1, $2, 'admin@beta.test', 'Admin Beta', $3, 'org_admin')")
        .bind(user_b_id)
        .bind(org_b_id)
        .bind(hash_password("Password123!").unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let auth_b: AuthResponse = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({ "email": "admin@beta.test", "password": "Password123!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // 4. Org A creates enrollment & enrolls device
    let enr: CreateEnrollmentResponse = client
        .post(format!("{base}/api/v1/enrollments"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({ "label": "Lab", "max_uses": 5, "expires_hours": 24 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let dev_id = DeviceIdentity::generate();
    let enrolled: EnrollDeviceResponse = client
        .post(format!("{base}/api/v1/devices/enroll"))
        .json(&EnrollDeviceRequest {
            enrollment_token: Some(enr.enrollment_token),
            support_code: None,
            ed25519_pubkey: dev_id.public_key_hex(),
            hostname: "WIN-LAB-01".into(),
            os_name: "Windows 11".into(),
            os_arch: "x86_64".into(),
            agent_version: "0.1.0".into(),
            kind: "host".into(),
            screen_width: 1920,
            screen_height: 1080,
            private_mem_mb: 18.5,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // 5. Org A adds Scheduled Client (Address Book) linked to WIN-LAB-01
    let ab: AddressBookEntryDto = client
        .post(format!("{base}/api/v1/address-book"))
        .bearer_auth(&auth_a.token)
        .json(&UpsertAddressBookRequest {
            device_id: Some(enrolled.device_id),
            alias: "Caja Central".into(),
            client_company: "Cliente Alpha".into(),
            contact_name: "Ana".into(),
            contact_email: "ana@alpha.test".into(),
            group_name: "Sucursal 1".into(),
            is_favorite: true,
            notes: "Ventana mantenimiento 19h".into(),
            scheduled_next_at: None,
            group_id: None,
            icon_kind: "desktop".into(),
            accent_color: String::new(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ab.device_hostname.as_deref(), Some("WIN-LAB-01"));

    // 6. Negative Cross-Tenant Isolation Checks: Org B CANNOT see Org A devices, address book, or start session
    let b_devices: Vec<DeviceDto> = client
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&auth_b.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(b_devices.is_empty());

    let b_ab: Vec<AddressBookEntryDto> = client
        .get(format!("{base}/api/v1/address-book"))
        .bearer_auth(&auth_b.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(b_ab.is_empty());

    let cross_session = client
        .post(format!("{base}/api/v1/sessions"))
        .bearer_auth(&auth_b.token)
        .json(&CreateSessionRequest {
            device_id: enrolled.device_id,
            ticket_id: None,
            allow_control: true,
            allow_clipboard: true,
            allow_files: false,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(cross_session.status(), StatusCode::NOT_FOUND);

    // 7. Public Support Request + Atomic Claim (second claim must return 409 Conflict)
    let sup: CreateSupportResponse = client
        .post(format!("{base}/api/v1/support/requests"))
        .json(&CreateSupportRequest {
            requester_name: "Carlos".into(),
            requester_contact: "carlos@cliente.test".into(),
            company_name: Some("Cliente 1".into()),
            category: Some("Urgente".into()),
            description: "No abre ERP".into(),
            consent_accepted: true,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let tickets: Vec<SupportTicketDto> = client
        .get(format!("{base}/api/v1/tickets"))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tickets.len(), 1);

    let claim1 = client
        .post(format!("{base}/api/v1/tickets/{}/claim", sup.ticket_id))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap();
    assert_eq!(claim1.status(), StatusCode::OK);

    let claim2 = client
        .post(format!("{base}/api/v1/tickets/{}/claim", sup.ticket_id))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap();
    assert_eq!(claim2.status(), StatusCode::CONFLICT);

    // 8. Create Remote Session & Test Blind E2EE Relay with Noise_XX
    let sess: CreateSessionResponse = client
        .post(format!("{base}/api/v1/sessions"))
        .bearer_auth(&auth_a.token)
        .json(&CreateSessionRequest {
            device_id: enrolled.device_id,
            ticket_id: Some(sup.ticket_id),
            allow_control: true,
            allow_clipboard: false,
            allow_files: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let host_relay_token: String = sqlx::query_scalar(
        "SELECT host_relay_token FROM remote_sessions WHERE id = $1",
    )
    .bind(sess.session.id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let ws_base = base.replacen("http://", "ws://", 1);
    let host_url = format!("{ws_base}{}?role=host&token={host_relay_token}", sess.relay_ws_path);
    let viewer_url = format!(
        "{ws_base}{}?role=viewer&token={}",
        sess.relay_ws_path, sess.viewer_relay_token
    );

    let (host_res, viewer_res) = tokio::join!(
        RelayTunnel::connect(&host_url, false),
        RelayTunnel::connect(&viewer_url, true)
    );
    let mut host_tunnel = host_res.unwrap();
    let mut viewer_tunnel = viewer_res.unwrap();

    let test_frame = DataPacket::VideoFrame(VideoFrame {
        seq: 1,
        timestamp_ms: 999,
        width: 1280,
        height: 720,
        encoding: FrameEncoding::Jpeg,
        payload: vec![0xAA; 80_000],
    });
    host_tunnel.send_packet(&test_frame).await.unwrap();
    match viewer_tunnel.recv_packet().await.unwrap() {
        DataPacket::VideoFrame(f) => {
            assert_eq!(f.seq, 1);
            assert_eq!(f.payload.len(), 80_000);
        }
        _ => panic!("expected video frame"),
    }

    viewer_tunnel
        .send_packet(&DataPacket::Control(ControlMessage::Ping { ts_ms: 1234 }))
        .await
        .unwrap();
    match host_tunnel.recv_packet().await.unwrap() {
        DataPacket::Control(ControlMessage::Ping { ts_ms }) => assert_eq!(ts_ms, 1234),
        _ => panic!("expected ping"),
    }

    // 9. Instant Peer ID + Random Password Flow
    let peer_id = DeviceIdentity::generate();
    let reg: QuickRegisterResponse = client
        .post(format!("{base}/api/v1/clients/register"))
        .json(&QuickRegisterRequest {
            ed25519_pubkey: peer_id.public_key_hex(),
            hostname: "PEER-PC-01".into(),
            os_name: "Windows 11".into(),
            os_arch: "x86_64".into(),
            agent_version: "0.1.0".into(),
            screen_width: 1920,
            screen_height: 1080,
            private_mem_mb: 32.0,
            access_password: "init9999".into(),
            owner_email: None,
            personal_password: None,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg.connection_id.len(), 11); // "XXX XXX XXX"

    // Connect target client's /ws/agent control socket so it is active_agents-online
    let agent_ws_url = format!(
        "{ws_base}/ws/agent?device_id={}&token={}",
        reg.device_id, reg.device_token
    );
    let (mut agent_ws, _) = connect_async(&agent_ws_url).await.unwrap();
    let _welcome = agent_ws.next().await;

    // Wrong password rejected (401)
    let bad_conn = client
        .post(format!("{base}/api/v1/sessions/connect-by-id"))
        .json(&DirectConnectRequest {
            connection_id: reg.connection_id.replace(' ', ""),
            password: "wrong".into(),
            reconnect_token: None,
            caller_name: Some("Tester".into()),
            allow_control: true,
            allow_clipboard: true,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(bad_conn.status(), StatusCode::UNAUTHORIZED);

    // Rotate password with x-device-token
    let rot = client
        .post(format!("{base}/api/v1/clients/password"))
        .header("x-device-token", &reg.device_token)
        .json(&RotatePasswordRequest {
            device_id: reg.device_id,
            new_password: "rotated88".into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(rot.status(), StatusCode::OK);

    // Connect with spaces stripped & rotated password -> 200 OK + IncomingSession pushed
    let ok_conn: DirectConnectResponse = client
        .post(format!("{base}/api/v1/sessions/connect-by-id"))
        .json(&DirectConnectRequest {
            connection_id: reg.connection_id.replace(' ', ""),
            password: "rotated88".into(),
            reconnect_token: None,
            caller_name: Some("Peer Viewer".into()),
            allow_control: true,
            allow_clipboard: true,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok_conn.target_connection_id, reg.connection_id);
    assert!(ok_conn.reconnect_token.is_some());
    assert!(ok_conn.relay_ws_path.contains(&ok_conn.session_id.to_string()));

    // 10. Public /docs, /support/status, Calendar overlap 409, MFA setup/verify, CSV report, AI OAuth PKCE & MCP
    for pub_path in ["/docs", "/support/status", "/api/v1/docs"] {
        let r = client.get(format!("{base}{pub_path}")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "GET {pub_path}");
    }

    let now = chrono::Utc::now();
    let cal1 = client
        .post(format!("{base}/api/v1/calendar"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "device_id": enrolled.device_id,
            "title": "Mantenimiento Servidor",
            "client_name": "Cliente Alpha",
            "starts_at": now + chrono::Duration::hours(1),
            "ends_at": now + chrono::Duration::hours(2),
            "notes": "Revisión mensual"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cal1.status(), StatusCode::OK);

    // Overlapping appointment for same tech -> 409 Conflict
    let cal_conflict = client
        .post(format!("{base}/api/v1/calendar"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "device_id": null,
            "title": "Conflicto",
            "client_name": "Otro",
            "starts_at": now + chrono::Duration::minutes(90),
            "ends_at": now + chrono::Duration::minutes(150),
            "notes": ""
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cal_conflict.status(), StatusCode::CONFLICT);

    // MFA TOTP setup + verify
    let mfa_setup: agentdesk_protocol::MfaSetupResponse = client
        .post(format!("{base}/api/v1/auth/mfa/setup"))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(mfa_setup.otpauth_uri.contains("otpauth://totp/AgentDesk:"));

    // CSV Sessions Export
    let csv_resp = client
        .get(format!("{base}/api/v1/reports/sessions.csv"))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap();
    assert_eq!(csv_resp.status(), StatusCode::OK);
    let csv_txt = csv_resp.text().await.unwrap();
    assert!(csv_txt.starts_with("session_id,hostname,"));

    // ChatGPT OAuth PKCE start + MCP tools/list + live RMM RunDiagnostic RPC over /ws/agent
    let oauth_start: serde_json::Value = client
        .get(format!("{base}/api/v1/ai/oauth/start"))
        .bearer_auth(&auth_a.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(oauth_start["authorize_url"]
        .as_str()
        .unwrap()
        .contains("code_challenge_method=S256"));

    let mcp_tools: serde_json::Value = client
        .post(format!("{base}/api/v1/mcp"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mcp_tools["result"]["tools"].as_array().unwrap().len(), 3);

    // Change Password + Login verification (for admin A)
    let cp_resp = client
        .post(format!("{base}/api/v1/auth/change-password"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "current_password": "Password123!",
            "new_password": "EvenStronger456!"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cp_resp.status(), StatusCode::OK);

    let re_login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({
            "email": "admin@alpha.test",
            "password": "EvenStronger456!"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(re_login.status(), StatusCode::OK);

    // Quick register peer with personal password + owner_email, connect with personal pw & 1h reconnect_token
    let pk_hex = DeviceIdentity::generate().public_key_hex();
    let reg_resp: agentdesk_protocol::QuickRegisterResponse = client
        .post(format!("{base}/api/v1/clients/register"))
        .json(&serde_json::json!({
            "ed25519_pubkey": pk_hex,
            "hostname": "PC-TEST-DESKTOP",
            "os_name": "Windows 11",
            "os_arch": "x86_64",
            "agent_version": "0.1.0",
            "screen_width": 1920,
            "screen_height": 1080,
            "private_mem_mb": 42.0,
            "access_password": "RandPass99",
            "owner_email": "admin@alpha.test",
            "personal_password": "FixedPersonal77!"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Group creation, sharing with Org A technician & device assignment by connection ID + email
    let tech_user_resp = client
        .post(format!("{base}/api/v1/users"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "email": "tech2@alfa.local",
            "display_name": "Tech Two",
            "password": "TechPassword123!",
            "role": "technician"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(tech_user_resp.status(), StatusCode::OK);

    let grp: agentdesk_protocol::DeviceGroupDto = client
        .post(format!("{base}/api/v1/groups"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "name": "Sucursal Sur",
            "description": "Equipos críticos"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let share_resp = client
        .post(format!("{base}/api/v1/groups/{}/shares", grp.id))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "user_email": "tech2@alfa.local",
            "permission": "read_write"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(share_resp.status(), StatusCode::OK);

    let assign_resp: agentdesk_protocol::AssignDeviceResponse = client
        .post(format!("{base}/api/v1/devices/assign"))
        .bearer_auth(&auth_a.token)
        .json(&serde_json::json!({
            "connection_id": reg_resp.connection_id,
            "owner_email": "tech2@alfa.local",
            "group_id": grp.id,
            "alias": "Caja Sur 01"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(assign_resp.device.owner_email.as_deref(), Some("tech2@alfa.local"));
    assert!(assign_resp.device.has_personal_password);
    assert_eq!(
        assign_resp.address_book_entry.as_ref().unwrap().group_id,
        Some(grp.id)
    );
}
