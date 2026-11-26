use crate::theme::ThemeMode;
use serde::{Deserialize, Serialize};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionHistoryEntry {
    pub connection_id: String,
    pub hostname: String,
    pub last_connected_unix: i64,
    #[serde(default)]
    pub reconnect_token: Option<String>,
    #[serde(default)]
    pub reconnect_expires_unix: Option<i64>,
}

impl ConnectionHistoryEntry {
    pub fn is_trust_valid(&self, now_unix: i64) -> bool {
        let within_hour = (now_unix - self.last_connected_unix) < 3600;
        let token_ok = self.reconnect_token.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
            && self.reconnect_expires_unix.unwrap_or(0) > now_unix;
        within_hour && token_ok
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    pub server_url: String,
    pub last_email: String,
    pub dark_mode: bool,
    #[serde(default)]
    pub theme: Option<ThemeMode>,
    #[serde(default)]
    pub connection_history: Vec<ConnectionHistoryEntry>,
    #[serde(default)]
    pub owner_email: String,
    #[serde(default)]
    pub personal_password_set: bool,
    #[serde(default)]
    pub start_with_windows: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        let server_url = match detect_lan_ip() {
            Some(ip) => format!("http://{ip}:8080"),
            None => "http://127.0.0.1:8080".to_string(),
        };
        Self {
            server_url,
            last_email: String::new(),
            dark_mode: false,
            theme: Some(ThemeMode::OpenVmsLight),
            connection_history: Vec::new(),
            owner_email: String::new(),
            personal_password_set: false,
            start_with_windows: false,
        }
    }
}

impl DesktopConfig {
    pub fn config_path() -> PathBuf {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("AgentDesk").join("desktop_config.json")
    }

    pub fn load() -> (Self, PathBuf) {
        let path = Self::config_path();
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&raw) {
                cfg.server_url = normalize_server_url(&cfg.server_url);
                return (cfg, path);
            }
        }
        let cfg = Self::default();
        let _ = cfg.save(&path);
        (cfg, path)
    }

    pub fn effective_theme(&self) -> ThemeMode {
        if let Some(t) = self.theme {
            t
        } else if self.dark_mode {
            ThemeMode::DraculaDark
        } else {
            ThemeMode::OpenVmsLight
        }
    }

    pub fn set_theme(&mut self, mode: ThemeMode) {
        self.theme = Some(mode);
        self.dark_mode = matches!(mode, ThemeMode::DraculaDark);
    }

    #[allow(dead_code)]
    pub fn record_history(
        &mut self,
        connection_id: String,
        hostname: String,
        now_unix: i64,
        reconnect_token: Option<String>,
        reconnect_expires_unix: Option<i64>,
    ) {
        let norm = normalize_conn_code(&connection_id);
        self.connection_history
            .retain(|h| normalize_conn_code(&h.connection_id) != norm);
        self.connection_history.insert(
            0,
            ConnectionHistoryEntry {
                connection_id,
                hostname,
                last_connected_unix: now_unix,
                reconnect_token,
                reconnect_expires_unix,
            },
        );
        self.connection_history.truncate(20);
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

pub fn normalize_conn_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

pub fn normalize_server_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return DesktopConfig::default().server_url;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

pub fn detect_lan_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let addr = sock.local_addr().ok()?;
    let ip = addr.ip().to_string();
    if ip == "127.0.0.1" || ip == "0.0.0.0" {
        None
    } else {
        Some(ip)
    }
}

#[cfg(windows)]
pub fn set_windows_autostart(enabled: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let output = if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let val = format!("\"{}\"", exe.display());
        Command::new("reg.exe")
            .args(["add", key, "/v", "AgentDeskDesktop", "/t", "REG_SZ", "/d", &val, "/f"])
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| e.to_string())?
    } else {
        Command::new("reg.exe")
            .args(["delete", key, "/v", "AgentDeskDesktop", "/f"])
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| e.to_string())?
    };

    if output.status.success() || !enabled {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(not(windows))]
pub fn set_windows_autostart(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub fn install_host_service_elevated(server_url: &str, enroll_token: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let host_exe = exe
        .parent()
        .map(|p| p.join("agentdesk-host.exe"))
        .unwrap_or_else(|| PathBuf::from("agentdesk-host.exe"));
    if !host_exe.exists() {
        return Err(format!("No se encontró {}", host_exe.display()));
    }
    if enroll_token.trim().is_empty() {
        return Err("Pegá un token de enrolamiento (pestaña Equipos > Generar token)".into());
    }

    let bin_path = format!(
        "\"{}\" --service --server {} --enroll-token {} --auto-accept",
        host_exe.display(),
        server_url.trim(),
        enroll_token.trim()
    );
    let script = format!(
        "sc.exe stop AgentDeskHost | Out-Null\r\nsc.exe delete AgentDeskHost | Out-Null\r\nStart-Sleep -Seconds 1\r\nsc.exe create AgentDeskHost binPath= '{}' start= auto DisplayName= 'AgentDesk Host (acceso desatendido)'\r\nsc.exe start AgentDeskHost\r\n",
        bin_path.replace('\'', "''")
    );
    let script_path = std::env::temp_dir().join("agentdesk_install_service.ps1");
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;

    let launcher = format!(
        "Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}'",
        script_path.display().to_string().replace('\'', "''")
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &launcher])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok("Solicitud UAC lanzada: se instalará el servicio AgentDeskHost (inicia antes del login)".into())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(not(windows))]
pub fn install_host_service_elevated(_server_url: &str, _enroll_token: &str) -> Result<String, String> {
    Err("Solo soportado en Windows".into())
}
