pub mod api;
pub mod auth;
pub mod config;
pub mod relay;
pub mod rmm_ai_api;
pub mod state;
pub mod stun;
pub mod web;

use crate::state::AppState;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Public Web Portal
        .route("/", get(web::landing_page))
        .route("/downloads", get(web::downloads_page))
        .route("/docs", get(web::docs_page))
        .route("/support", get(web::support_page))
        .route("/support/status", get(web::status_page))
        .route("/setup", get(web::setup_page))
        .route("/login", get(web::login_page))
        .route("/app", get(web::app_dashboard_page))
        // Health & Artifacts
        .route("/health/live", get(api::health_live))
        .route("/health/ready", get(api::health_ready))
        .route("/api/v1/downloads", get(api::list_downloads))
        .route("/downloads/file/{filename}", get(api::download_artifact_file))
        .route("/api/v1/updates/{product}", get(api::update_manifest))
        // Setup & Auth
        .route("/api/v1/setup/status", get(auth::setup_status))
        .route("/api/v1/setup/bootstrap", post(auth::bootstrap))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/change-password", post(auth::change_password))
        // Enrollments & Devices & Instant Peer Registration
        .route("/api/v1/enrollments", get(api::list_enrollments).post(api::create_enrollment))
        .route("/api/v1/enrollments/{id}", delete(api::revoke_enrollment))
        .route("/api/v1/devices/enroll", post(api::enroll_device))
        .route("/api/v1/devices/heartbeat", post(api::device_heartbeat))
        .route("/api/v1/devices/assign", post(rmm_ai_api::assign_device))
        .route("/api/v1/devices/{id}/group", patch(api::update_device_group))
        .route("/api/v1/devices/{id}/appearance", patch(api::update_device_appearance))
        .route("/api/v1/devices/{id}/wol", post(api::wake_device))
        .route("/api/v1/devices", get(api::list_devices))
        .route("/api/v1/clients/register", post(api::quick_register_client))
        .route("/api/v1/clients/lookup", get(api::lookup_client_by_id))
        .route("/api/v1/clients/password", post(api::rotate_client_password))
        .route("/api/v1/clients/personal-password", post(api::set_personal_password))
        .route("/api/v1/sessions/connect-by-id", post(api::connect_by_id))
        // Device Groups & Sharing
        .route("/api/v1/groups", get(rmm_ai_api::list_groups).post(rmm_ai_api::create_group))
        .route("/api/v1/groups/{id}", patch(rmm_ai_api::update_group).delete(rmm_ai_api::delete_group))
        .route("/api/v1/groups/{id}/shares", post(rmm_ai_api::share_group))
        .route("/api/v1/groups/{id}/shares/{user_id}", delete(rmm_ai_api::unshare_group))
        // Address Book (Clientes Agendados)
        .route("/api/v1/address-book", get(api::list_address_book).post(api::create_address_book_entry))
        .route(
            "/api/v1/address-book/{id}",
            patch(api::update_address_book_entry).delete(api::delete_address_book_entry),
        )
        // Web Support & Tickets
        .route("/api/v1/support/requests", post(api::create_support_request))
        .route("/api/v1/support/requests/{code}/status", get(api::get_support_status))
        .route("/api/v1/support/requests/{code}/cancel", post(api::cancel_support_request))
        .route("/api/v1/tickets", get(api::list_tickets))
        .route("/api/v1/tickets/{id}/claim", post(api::claim_ticket))
        .route("/api/v1/tickets/{id}/close", post(api::close_ticket))
        .route("/api/v1/tickets/{id}/resolve", post(api::resolve_ticket))
        // Remote Sessions
        .route("/api/v1/sessions", get(api::list_sessions).post(api::create_session))
        .route("/api/v1/sessions/{id}/terminate", post(api::terminate_session))
        // Audit & SSE
        .route("/api/v1/audit", get(api::list_audit))
        .route("/api/v1/events", get(api::sse_events))
        // Users, RBAC & MFA TOTP
        .route("/api/v1/users", get(rmm_ai_api::list_users).post(rmm_ai_api::create_user))
        .route("/api/v1/users/{id}/role", patch(rmm_ai_api::update_user_role))
        .route("/api/v1/auth/mfa/setup", post(rmm_ai_api::mfa_setup))
        .route("/api/v1/auth/mfa/verify", post(rmm_ai_api::mfa_verify))
        // RMM Telemetry & Remote Terminal
        .route("/api/v1/devices/{id}/diagnostics", post(rmm_ai_api::run_device_diagnostic))
        .route("/api/v1/devices/{id}/telemetry", get(rmm_ai_api::list_device_telemetry))
        .route("/api/v1/devices/{id}/alerts", get(rmm_ai_api::list_device_alerts))
        .route("/api/v1/alerts/{id}/acknowledge", post(rmm_ai_api::acknowledge_device_alert))
        .route("/api/v1/rmm/alert-settings", get(rmm_ai_api::get_rmm_alert_settings).put(rmm_ai_api::update_rmm_alert_settings))
        .route(
            "/api/v1/devices/{id}/command",
            get(rmm_ai_api::list_device_commands).post(rmm_ai_api::run_device_command),
        )
        // AI Copilot (ChatGPT OAuth PKCE / OpenAI / Ollama) & MCP
        .route("/api/v1/ai/settings", get(rmm_ai_api::get_ai_settings).put(rmm_ai_api::update_ai_settings))
        .route("/api/v1/ai/models", get(rmm_ai_api::list_ai_models))
        .route("/api/v1/ai/oauth/start", get(rmm_ai_api::ai_oauth_start))
        .route("/api/v1/ai/oauth/device/start", post(rmm_ai_api::ai_oauth_device_start))
        .route("/api/v1/ai/oauth/callback", get(rmm_ai_api::ai_oauth_callback))
        .route("/api/v1/ai/oauth/complete", post(rmm_ai_api::ai_oauth_complete))
        .route("/api/v1/ai/oauth/disconnect", post(rmm_ai_api::ai_oauth_disconnect))
        .route("/api/v1/ai/chat", post(rmm_ai_api::ai_chat))
        .route("/api/v1/ai/sessions", get(rmm_ai_api::list_ai_chat_sessions))
        .route("/api/v1/ai/sessions/{session_id}/messages", get(rmm_ai_api::list_ai_chat_messages))
        .route("/api/v1/ai/execute-action", post(rmm_ai_api::ai_execute_approved_action))
        .route("/api/v1/mcp", post(rmm_ai_api::mcp_rpc_handler))
        // Calendar, Ticket Comments, CSV Reports & Docs
        .route("/api/v1/calendar", get(rmm_ai_api::list_calendar_events).post(rmm_ai_api::create_calendar_event))
        .route("/api/v1/calendar/{id}", delete(rmm_ai_api::delete_calendar_event))
        .route(
            "/api/v1/tickets/{id}/comments",
            get(rmm_ai_api::list_ticket_comments).post(rmm_ai_api::create_ticket_comment),
        )
        .route("/api/v1/reports/sessions.csv", get(rmm_ai_api::export_sessions_csv))
        .route("/api/v1/docs", get(rmm_ai_api::list_docs))
        .route("/api/v1/docs/{slug}", post(rmm_ai_api::upsert_doc).put(rmm_ai_api::upsert_doc))
        // WebSockets (Agent Control + Blind E2EE Relay)
        .route("/ws/agent", get(relay::ws_agent_handler))
        .route("/ws/relay/{session_id}", get(relay::ws_relay_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
