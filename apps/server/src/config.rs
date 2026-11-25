use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default)]
    pub artifacts_dir: Option<PathBuf>,
    #[serde(default)]
    pub bootstrap_token: Option<String>,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default = "default_auto_seed_admin")]
    pub auto_seed_admin: bool,
    #[serde(default = "default_org_name")]
    pub default_org_name: String,
    #[serde(default = "default_org_slug")]
    pub default_org_slug: String,
    #[serde(default = "default_admin_email")]
    pub default_admin_email: String,
    #[serde(default = "default_admin_name")]
    pub default_admin_name: String,
    #[serde(default = "default_admin_password")]
    pub default_admin_password: String,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_database_url() -> String {
    "postgres://agentdesk:agentdesk_dev_pass@127.0.0.1:5432/agentdesk".to_string()
}

fn default_auto_seed_admin() -> bool {
    true
}

fn default_org_name() -> String {
    "AgentDesk".to_string()
}

fn default_org_slug() -> String {
    "agentdesk".to_string()
}

fn default_admin_email() -> String {
    "juan@dolan.com.ar".to_string()
}

fn default_admin_name() -> String {
    "Juan Dolan".to_string()
}

fn default_admin_password() -> String {
    "admin123".to_string()
}

/// Detects the primary outbound LAN IPv4 address using a non-sending UDP socket route probe.
pub fn detect_lan_ip() -> Option<Ipv4Addr> {
    for target in ["8.8.8.8:80", "1.1.1.1:80"] {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(local) = sock.local_addr() {
                    if let std::net::IpAddr::V4(v4) = local.ip() {
                        if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() {
                            return Some(v4);
                        }
                    }
                }
            }
        }
    }
    None
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            database_url: default_database_url(),
            artifacts_dir: None,
            bootstrap_token: None,
            public_url: None,
            auto_seed_admin: default_auto_seed_admin(),
            default_org_name: default_org_name(),
            default_org_slug: default_org_slug(),
            default_admin_email: default_admin_email(),
            default_admin_name: default_admin_name(),
            default_admin_password: default_admin_password(),
        }
    }
}

impl ServerConfig {
    /// Resolves server configuration from explicit path, portable `agentdesk-server.json`
    /// next to executable, or `%PROGRAMDATA%\AgentDesk\server_config.json`, then overlays env vars.
    pub fn load(explicit_path: Option<&Path>) -> (Self, Option<PathBuf>) {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit_path {
            candidates.push(p.to_path_buf());
        } else {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    candidates.push(dir.join("agentdesk-server.json"));
                }
            }
            candidates.push(default_programdata_config_path());
        }

        let mut loaded_from = None;
        let mut cfg = Self::default();

        for path in candidates {
            if path.exists() {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    let clean = raw.trim_start_matches('\u{feff}');
                    if let Ok(parsed) = serde_json::from_str::<ServerConfig>(clean) {
                        cfg = parsed;
                        loaded_from = Some(path);
                        break;
                    }
                }
            }
        }

        if let Ok(v) = std::env::var("AGENTDESK_BIND") {
            if !v.trim().is_empty() {
                cfg.bind_addr = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("DATABASE_URL") {
            if !v.trim().is_empty() {
                cfg.database_url = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("AGENTDESK_ARTIFACTS_DIR") {
            if !v.trim().is_empty() {
                cfg.artifacts_dir = Some(PathBuf::from(v.trim()));
            }
        }
        if let Ok(v) = std::env::var("AGENTDESK_BOOTSTRAP_TOKEN") {
            if !v.trim().is_empty() {
                cfg.bootstrap_token = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("AGENTDESK_PUBLIC_URL") {
            if !v.trim().is_empty() {
                cfg.public_url = Some(v.trim().trim_end_matches('/').to_string());
            }
        }

        (cfg, loaded_from)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn bind_port(&self) -> u16 {
        self.bind_addr
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(8080)
    }

    pub fn resolve_public_url(&self) -> (Option<String>, String) {
        let lan = detect_lan_ip().map(|ip| ip.to_string());
        if let Some(explicit) = &self.public_url {
            let trimmed = explicit.trim().trim_end_matches('/');
            if !trimmed.is_empty() {
                return (lan, trimmed.to_string());
            }
        }
        let port = self.bind_port();
        let url = match &lan {
            Some(ip) => format!("http://{ip}:{port}"),
            None => format!("http://127.0.0.1:{port}"),
        };
        (lan, url)
    }

    pub fn resolve_artifacts_dir(&self) -> PathBuf {
        if let Some(dir) = &self.artifacts_dir {
            return dir.clone();
        }
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("target/release"))
    }
}

pub fn default_programdata_config_path() -> PathBuf {
    let base = std::env::var("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("server_config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentdesk_srv_cfg_{}", uuid::Uuid::new_v4()));
        let path = dir.join("server_config.json");
        let cfg = ServerConfig {
            bind_addr: "0.0.0.0:9090".into(),
            database_url: "postgres://user:pass@127.0.0.1:5432/db".into(),
            artifacts_dir: Some(PathBuf::from("C:\\AgentDesk\\bin")),
            bootstrap_token: Some("tok-123".into()),
            public_url: Some("http://10.0.0.5:9090".into()),
            auto_seed_admin: true,
            default_org_name: "Dolan".into(),
            default_org_slug: "dolan".into(),
            default_admin_email: "juan@dolan.com.ar".into(),
            default_admin_name: "Juan Dolan".into(),
            default_admin_password: "admin123".into(),
        };
        cfg.save(&path).expect("save server config");
        let (loaded, from) = ServerConfig::load(Some(&path));
        assert_eq!(from.as_deref(), Some(path.as_path()));
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_port_and_public_url_resolution() {
        let mut cfg = ServerConfig::default();
        cfg.bind_addr = "0.0.0.0:9443".into();
        assert_eq!(cfg.bind_port(), 9443);

        cfg.public_url = Some("https://soporte.dolan.com.ar/".into());
        let (_lan, url) = cfg.resolve_public_url();
        assert_eq!(url, "https://soporte.dolan.com.ar");

        cfg.public_url = None;
        let (lan2, url2) = cfg.resolve_public_url();
        assert!(url2.ends_with(":9443"));
        if let Some(ip) = lan2 {
            assert_eq!(url2, format!("http://{ip}:9443"));
        }
    }
}
