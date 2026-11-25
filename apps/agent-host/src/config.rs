use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub device_id: Option<Uuid>,
    #[serde(default)]
    pub public_code: Option<String>,
    #[serde(default)]
    pub device_token: Option<String>,
    #[serde(default = "default_auto_accept")]
    pub auto_accept: bool,
    #[serde(default = "default_fps")]
    pub fps: u8,
    #[serde(default = "default_quality")]
    pub quality: u8,
}

fn detect_lan_ip() -> Option<Ipv4Addr> {
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

fn default_server_url() -> String {
    match detect_lan_ip() {
        Some(ip) => format!("http://{ip}:8080"),
        None => "http://127.0.0.1:8080".to_string(),
    }
}

fn default_auto_accept() -> bool {
    true
}

fn default_fps() -> u8 {
    30
}

fn default_quality() -> u8 {
    70
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            server_url: default_server_url(),
            device_id: None,
            public_code: None,
            device_token: None,
            auto_accept: default_auto_accept(),
            fps: default_fps(),
            quality: default_quality(),
        }
    }
}

pub fn normalize_server_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return default_server_url();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join("agentdesk-host.json");
            if portable.exists() {
                return portable;
            }
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("host_config.json")
}

impl HostConfig {
    pub fn load() -> (Self, PathBuf) {
        let path = config_path();
        let mut cfg = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| {
                    let clean = raw.trim_start_matches('\u{feff}');
                    serde_json::from_str::<HostConfig>(clean).ok()
                })
                .unwrap_or_default()
        } else {
            Self::default()
        };
        cfg.server_url = normalize_server_url(&cfg.server_url);
        (cfg, path)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut normalized = self.clone();
        normalized.server_url = normalize_server_url(&normalized.server_url);
        let json = serde_json::to_string_pretty(&normalized)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn sanitized_json(&self) -> String {
        let mut redacted = self.clone();
        if redacted.device_token.is_some() {
            redacted.device_token = Some("***REDACTED***".to_string());
        }
        serde_json::to_string_pretty(&redacted).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentdesk_host_cfg_{}", Uuid::new_v4()));
        let path = dir.join("host_config.json");
        let did = Uuid::new_v4();
        let cfg = HostConfig {
            server_url: "10.1.1.156:8080/".into(),
            device_id: Some(did),
            public_code: Some("AD-123456".into()),
            device_token: Some("secret-tok".into()),
            auto_accept: true,
            fps: 25,
            quality: 80,
        };
        cfg.save(&path).expect("save host config");
        let raw = std::fs::read_to_string(&path).expect("read host config");
        let loaded: HostConfig = serde_json::from_str(&raw).expect("parse host config");
        assert_eq!(loaded.server_url, "http://10.1.1.156:8080");
        assert_eq!(loaded.device_id, Some(did));
        assert_eq!(loaded.public_code.as_deref(), Some("AD-123456"));
        assert_eq!(loaded.device_token.as_deref(), Some("secret-tok"));
        assert_eq!(loaded.fps, 25);
        assert_eq!(loaded.quality, 80);
        assert!(loaded.sanitized_json().contains("***REDACTED***"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
