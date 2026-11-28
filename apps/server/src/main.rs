use agentdesk_identity::{generate_high_entropy_token, hash_password};
use agentdesk_server::{build_router, config::ServerConfig, state::AppState};
use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "agentdesk-server", about = "AgentDesk Central Coordination & E2EE Relay Server")]
struct Cli {
    /// Run as a Windows Service (invoked by Windows SCM)
    #[arg(long, default_value_t = false)]
    service: bool,

    /// Explicit JSON configuration file path
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override listen address (e.g. 0.0.0.0:8080)
    #[arg(long)]
    bind: Option<String>,

    /// Write resolved configuration to the specified path and exit
    #[arg(long)]
    write_default_config: Option<PathBuf>,
}

fn load_effective_config(config_path: Option<&PathBuf>, bind_override: Option<&str>) -> (ServerConfig, Option<PathBuf>) {
    let (mut cfg, loaded_from) = ServerConfig::load(config_path.map(|p| p.as_path()));
    if let Some(b) = bind_override {
        if !b.trim().is_empty() {
            cfg.bind_addr = b.trim().to_string();
        }
    }
    (cfg, loaded_from)
}

async fn run_server(
    cfg: ServerConfig,
    loaded_from: Option<PathBuf>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if let Some(path) = &loaded_from {
        tracing::info!(config_path = %path.display(), "Loaded server configuration");
    } else {
        tracing::info!("Using default/env server configuration");
    }

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&cfg.database_url)
        .await?;

    sqlx::migrate!("../../migrations").run(&pool).await?;

    let bootstrap_token = cfg
        .bootstrap_token
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(generate_high_entropy_token);

    let (lan_ip, public_url) = cfg.resolve_public_url();

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;
    if user_count == 0 {
        if cfg.auto_seed_admin {
            let org_id = Uuid::new_v4();
            let user_id = Uuid::new_v4();
            let pw_hash = hash_password(&cfg.default_admin_password)
                .map_err(|e| anyhow::anyhow!("failed to hash default admin password: {e}"))?;
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
                .bind(org_id)
                .bind(cfg.default_org_name.trim())
                .bind(cfg.default_org_slug.trim().to_lowercase())
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO users (id, organization_id, email, display_name, password_hash, role)
                 VALUES ($1, $2, $3, $4, $5, 'instance_admin')",
            )
            .bind(user_id)
            .bind(org_id)
            .bind(cfg.default_admin_email.trim().to_lowercase())
            .bind(cfg.default_admin_name.trim())
            .bind(&pw_hash)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            tracing::info!(
                email = %cfg.default_admin_email,
                organization = %cfg.default_org_name,
                "Auto-seeded default administrator account"
            );
        } else {
            tracing::info!(
                bootstrap_token = %bootstrap_token,
                setup_url = %format!("{public_url}/setup"),
                "Instance not bootstrapped yet. Use this one-time bootstrap_token at /setup."
            );
        }
    }

    let artifacts_dir = cfg.resolve_artifacts_dir();
    tracing::info!(artifacts_dir = %artifacts_dir.display(), "Artifact downloads directory configured");

    let bind_addr = cfg.bind_addr.clone();
    let state = AppState::new(pool, bootstrap_token, artifacts_dir).with_network_info(
        bind_addr.clone(),
        lan_ip.clone(),
        public_url.clone(),
    );
    let metrics_interval_secs = std::env::var("AGENTDESK_RMM_METRICS_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300)
        .clamp(60, 86_400);
    agentdesk_server::rmm_ai_api::spawn_metrics_collector(
        state.clone(),
        Duration::from_secs(metrics_interval_secs),
    );
    tracing::info!(metrics_interval_secs, "scheduled RMM metric collection enabled");
    agentdesk_server::stun::spawn_stun_responder();
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(
        %bind_addr,
        lan_ip = lan_ip.as_deref().unwrap_or("none"),
        %public_url,
        "AgentDesk server listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let ctrl_c = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let service_stop = async {
                while !*shutdown.borrow() {
                    if shutdown.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            };
            tokio::select! {
                _ = ctrl_c => {}
                _ = service_stop => {}
            }
            tracing::info!("Shutdown signal received, stopping AgentDesk server");
        })
        .await?;

    Ok(())
}

#[cfg(windows)]
mod win_service {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    pub const SERVICE_NAME: &str = "AgentDeskServer";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    static SERVICE_ARGS: OnceLock<(Option<PathBuf>, Option<String>)> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn dispatch(config: Option<PathBuf>, bind: Option<String>) -> anyhow::Result<()> {
        let _ = SERVICE_ARGS.set((config, bind));
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    fn init_service_logging() {
        let log_path = std::env::var("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("AgentDesk")
            .join("logs")
            .join("server.log");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("agentdesk_server=info,tower_http=info"));
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init();
        } else {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .try_init();
        }
    }

    fn set_status(
        handle: &service_control_handler::ServiceStatusHandle,
        state: ServiceState,
        controls_accepted: ServiceControlAccept,
        exit_code: ServiceExitCode,
        wait_hint_secs: u64,
    ) {
        let _ = handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted,
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::from_secs(wait_hint_secs),
            process_id: None,
        });
    }

    fn service_main(_arguments: Vec<OsString>) {
        init_service_logging();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let status_handle = match service_control_handler::register(SERVICE_NAME, move |control| {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = shutdown_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        }) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "Failed to register service control handler");
                return;
            }
        };

        set_status(
            &status_handle,
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            10,
        );

        let (cfg_path, bind_override) = SERVICE_ARGS
            .get()
            .cloned()
            .unwrap_or((None, None));
        let (cfg, loaded_from) = load_effective_config(cfg_path.as_ref(), bind_override.as_deref());

        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "Failed to build Tokio runtime for service");
                set_status(
                    &status_handle,
                    ServiceState::Stopped,
                    ServiceControlAccept::empty(),
                    ServiceExitCode::ServiceSpecific(1),
                    0,
                );
                return;
            }
        };

        set_status(
            &status_handle,
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ServiceExitCode::Win32(0),
            0,
        );

        let result = rt.block_on(run_server(cfg, loaded_from, shutdown_rx));

        set_status(
            &status_handle,
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            5,
        );

        let exit_code = match result {
            Ok(()) => ServiceExitCode::Win32(0),
            Err(e) => {
                tracing::error!(error = %e, "AgentDesk server service exited with error");
                ServiceExitCode::ServiceSpecific(2)
            }
        };

        set_status(
            &status_handle,
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            exit_code,
            0,
        );
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(target_path) = cli.write_default_config {
        let (cfg, _) = load_effective_config(cli.config.as_ref(), cli.bind.as_deref());
        cfg.save(&target_path)?;
        println!("Saved server configuration to {}", target_path.display());
        return Ok(());
    }

    if cli.service {
        #[cfg(windows)]
        {
            return win_service::dispatch(cli.config, cli.bind);
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("--service is only supported on Windows");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("agentdesk_server=info,tower_http=info")),
        )
        .init();

    let (cfg, loaded_from) = load_effective_config(cli.config.as_ref(), cli.bind.as_deref());
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_server(cfg, loaded_from, shutdown_rx))
}
