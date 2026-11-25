pub mod api;
pub mod auth;
pub mod config;
pub mod relay;
pub mod state;
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
        .route("/support", get(web::support_page))
        .route("/setup", get(web::setup_page))
        .route("/login", get(web::login_page))
        .route("/app", get(web::app_dashboard_page))
        // Health & Artifacts
        .route("/health/ready", get(api::health_ready))
        .route("/api/v1/downloads", get(api::list_downloads))
        .route("/downloads/file/{filename}", get(api::download_artifact_file))
        // Setup & Auth
        .route("/api/v1/setup/status", get(auth::setup_status))
        .route("/api/v1/setup/bootstrap", post(auth::bootstrap))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        // Enrollments & Devices & Instant Peer Registration
        .route("/api/v1/enrollments", get(api::list_enrollments).post(api::create_enrollment))
        .route("/api/v1/enrollments/{id}", delete(api::revoke_enrollment))
        .route("/api/v1/devices/enroll", post(api::enroll_device))
        .route("/api/v1/devices/heartbeat", post(api::device_heartbeat))
        .route("/api/v1/devices", get(api::list_devices))
        .route("/api/v1/clients/register", post(api::quick_register_client))
        .route("/api/v1/clients/password", post(api::rotate_client_password))
        .route("/api/v1/sessions/connect-by-id", post(api::connect_by_id))
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
        // Remote Sessions
        .route("/api/v1/sessions", get(api::list_sessions).post(api::create_session))
        .route("/api/v1/sessions/{id}/terminate", post(api::terminate_session))
        // Audit & SSE
        .route("/api/v1/audit", get(api::list_audit))
        .route("/api/v1/events", get(api::sse_events))
        // WebSockets (Agent Control + Blind E2EE Relay)
        .route("/ws/agent", get(relay::ws_agent_handler))
        .route("/ws/relay/{session_id}", get(relay::ws_relay_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
