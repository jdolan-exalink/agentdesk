use crate::theme::ThemeMode;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopConfig {
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default = "default_email")]
    pub last_email: String,
    #[serde(default)]
    pub dark_mode: bool,
    #[serde(default)]
    pub theme: Option<ThemeMode>,
}

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

fn default_server_url() -> String {
    match detect_lan_ip() {
        Some(ip) => format!("http://{ip}:8080"),
        None => "http://127.0.0.1:8080".to_string(),
    }
}

fn default_email() -> String {
    "juan@dolan.com.ar".to_string()
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            server_url: default_server_url(),
            last_email: default_email(),
            dark_mode: false,
            theme: Some(ThemeMode::OpenVmsLight),
        }
    }
}

impl DesktopConfig {
    pub fn effective_theme(&self) -> ThemeMode {
        if let Some(t) = self.theme {
            return t;
        }
        if self.dark_mode {
            ThemeMode::DraculaDark
        } else {
            ThemeMode::OpenVmsLight
        }
    }

    pub fn set_theme(&mut self, mode: ThemeMode) {
        self.theme = Some(mode);
        self.dark_mode = matches!(mode, ThemeMode::DraculaDark);
    }

    pub fn load() -> (Self, PathBuf) {
        let path = config_path();
        let mut cfg = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| {
                    let clean = raw.trim_start_matches('\u{feff}');
                    serde_json::from_str::<DesktopConfig>(clean).ok()
                })
                .unwrap_or_default()
        } else {
            Self::default()
        };

        if let Ok(env_srv) = std::env::var("AGENTDESK_SERVER") {
            if !env_srv.trim().is_empty() {
                cfg.server_url = env_srv;
            }
        }
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
            let portable = dir.join("agentdesk-desktop.json");
            if portable.exists() {
                return portable;
            }
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentDesk").join("desktop_config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_adds_scheme_and_strips_slash() {
        assert_eq!(normalize_server_url("10.1.1.156:8080/"), "http://10.1.1.156:8080");
        assert_eq!(normalize_server_url("https://desk.example.com/"), "https://desk.example.com");
        assert!(normalize_server_url("   ").starts_with("http://"));
    }

    #[test]
    fn desktop_config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentdesk_dt_cfg_{}", uuid::Uuid::new_v4()));
        let path = dir.join("desktop_config.json");
        let cfg = DesktopConfig {
            server_url: "192.168.1.50:8080/".into(),
            last_email: "juan@dolan.com.ar".into(),
            dark_mode: true,
            theme: Some(ThemeMode::DraculaDark),
        };
        cfg.save(&path).expect("save desktop config");
        let raw = std::fs::read_to_string(&path).expect("read");
        let parsed: DesktopConfig = serde_json::from_str(&raw).expect("parse");
        assert_eq!(parsed.server_url, "http://192.168.1.50:8080");
        assert_eq!(parsed.last_email, "juan@dolan.com.ar");
        assert_eq!(parsed.effective_theme(), ThemeMode::DraculaDark);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
