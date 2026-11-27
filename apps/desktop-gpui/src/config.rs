use serde::{Deserialize, Serialize};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    OpenVmsLight,
    DraculaDark,
}

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
        let token_ok = self
            .reconnect_token
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
            && self.reconnect_expires_unix.unwrap_or(0) > now_unix;
        within_hour && token_ok
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SavedWindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
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
    #[serde(default)]
    pub window_bounds: Option<SavedWindowBounds>,
    #[serde(default)]
    pub saved_auth_token: Option<String>,
    #[serde(default)]
    pub saved_auth_server_url: Option<String>,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        let server_url = "https://agentdesk.dolan.com.ar".to_string();
        Self {
            server_url,
            last_email: String::new(),
            dark_mode: true,
            theme: Some(ThemeMode::DraculaDark),
            connection_history: Vec::new(),
            owner_email: String::new(),
            personal_password_set: false,
            start_with_windows: false,
            window_bounds: None,
            saved_auth_token: None,
            saved_auth_server_url: None,
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
            let clean = raw.trim_start_matches('\u{feff}');
            if let Ok(mut cfg) = serde_json::from_str::<Self>(clean) {
                cfg.server_url = normalize_server_url(&cfg.server_url);
                return (cfg, path);
            }
        }
        let cfg = Self::default();
        let _ = cfg.save(&path);
        (cfg, path)
    }

    pub fn is_dark(&self) -> bool {
        match self.theme {
            Some(ThemeMode::DraculaDark) => true,
            Some(ThemeMode::OpenVmsLight) => false,
            None => self.dark_mode,
        }
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.dark_mode = dark;
        self.theme = Some(if dark {
            ThemeMode::DraculaDark
        } else {
            ThemeMode::OpenVmsLight
        });
    }

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

    pub fn set_saved_auth(&mut self, token: String, server_url: String) {
        self.saved_auth_token = Some(token);
        self.saved_auth_server_url = Some(normalize_server_url(&server_url));
    }

    pub fn clear_saved_auth(&mut self) {
        self.saved_auth_token = None;
        self.saved_auth_server_url = None;
    }

    pub fn saved_auth_for_server(&self, server_url: &str) -> Option<String> {
        let tok = self.saved_auth_token.as_deref()?.trim();
        if tok.is_empty() {
            return None;
        }
        let target = normalize_server_url(server_url);
        if let Some(saved_url) = self.saved_auth_server_url.as_deref() {
            if normalize_server_url(saved_url) != target {
                return None;
            }
        }
        Some(tok.to_string())
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

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn fmt_elapsed(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m < 1 {
        "hace instantes".to_string()
    } else if m < 60 {
        format!("hace {m} min")
    } else {
        format!("hace {} h {} min", m / 60, m % 60)
    }
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Alta/baja del inicio automático escribiendo la clave HKCU\...\Run con la API
/// del registro (sin lanzar reg.exe oculto, que los antivirus marcan como heurística).
#[cfg(windows)]
pub fn set_windows_autostart(enabled: bool) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = to_wide("AgentDeskDesktop");
    let mut hkey = HKEY::default();
    unsafe {
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc.is_err() {
            return Err(format!("No se pudo abrir la clave Run ({:?})", rc));
        }
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let data = to_wide(&format!("\"{}\"", exe.display()));
            let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
            let rc = RegSetValueExW(hkey, PCWSTR(value_name.as_ptr()), 0, REG_SZ, Some(bytes));
            if rc.is_err() {
                Err(format!("No se pudo escribir el inicio automático ({:?})", rc))
            } else {
                Ok(())
            }
        } else {
            let rc = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
            if rc.is_err() && rc != ERROR_FILE_NOT_FOUND {
                Err(format!("No se pudo quitar el inicio automático ({:?})", rc))
            } else {
                Ok(())
            }
        };
        let _ = RegCloseKey(hkey);
        result
    }
}

#[cfg(not(windows))]
pub fn set_windows_autostart(_enabled: bool) -> Result<(), String> {
    Ok(())
}

/// Instala el servicio AgentDeskHost pidiendo elevación UAC de forma visible:
/// abre una consola elevada (ShellExecute "runas") que ejecuta sc.exe y queda a la
/// vista unos segundos. Sin scripts temporales ni PowerShell oculto.
#[cfg(windows)]
pub fn install_host_service_elevated(server_url: &str, enroll_token: &str) -> Result<String, String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let host_exe = exe
        .parent()
        .map(|p| p.join("agentdesk-host.exe"))
        .unwrap_or_else(|| PathBuf::from("agentdesk-host.exe"));
    if !host_exe.exists() {
        return Err(format!("No se encontró {}", host_exe.display()));
    }
    let token = enroll_token.trim();
    if token.is_empty() {
        return Err("Pegá un token de enrolamiento (Dispositivos → Generar token)".into());
    }
    let server = server_url.trim();
    let safe = |s: &str| {
        !s.is_empty()
            && s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ":/._-[]%?=".contains(c))
    };
    if !safe(server) || !safe(token) {
        return Err("La URL del servidor o el token contienen caracteres no válidos".into());
    }

    // sc.exe interpreta \" como comilla literal dentro de binPath=.
    let bin_path = format!(
        "\"\\\"{}\\\" --service --server {} --enroll-token {} --auto-accept\"",
        host_exe.display(),
        server,
        token
    );
    let params = format!(
        "/c \"echo Instalando el servicio AgentDeskHost... & sc.exe stop AgentDeskHost >nul 2>&1 & sc.exe delete AgentDeskHost >nul 2>&1 & timeout /t 2 /nobreak >nul & sc.exe create AgentDeskHost binPath= {bin_path} start= auto DisplayName= \"AgentDesk Host (acceso desatendido)\" & sc.exe start AgentDeskHost & timeout /t 5\""
    );
    let verb = to_wide("runas");
    let file = to_wide("cmd.exe");
    let params_w = to_wide(&params);
    let res = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW devuelve un valor > 32 si tuvo éxito.
    if res.0 as isize > 32 {
        Ok("Se pidió permiso de administrador para instalar el servicio AgentDeskHost".into())
    } else {
        Err("Se canceló la elevación UAC o no se pudo iniciar la instalación".into())
    }
}

#[cfg(not(windows))]
pub fn install_host_service_elevated(_server_url: &str, _enroll_token: &str) -> Result<String, String> {
    Err("Solo soportado en Windows".into())
}

#[cfg(test)]
mod tests {
    use super::DesktopConfig;

    #[test]
    fn saved_auth_persists_matches_normalized_server_and_clears() {
        let mut cfg = DesktopConfig::default();
        assert!(cfg.saved_auth_for_server("http://127.0.0.1:8080").is_none());

        cfg.set_saved_auth("tok-123".into(), "127.0.0.1:8080/".into());
        assert_eq!(
            cfg.saved_auth_for_server("http://127.0.0.1:8080"),
            Some("tok-123".to_string())
        );
        assert!(cfg.saved_auth_for_server("http://192.168.1.50:8080").is_none());

        cfg.clear_saved_auth();
        assert!(cfg.saved_auth_for_server("http://127.0.0.1:8080").is_none());
    }

    #[test]
    fn legacy_desktop_config_deserializes_without_saved_auth() {
        let raw = r#"{
            "server_url": "http://127.0.0.1:8080",
            "last_email": "juan@dolan.com.ar",
            "dark_mode": true
        }"#;
        let cfg: DesktopConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.last_email, "juan@dolan.com.ar");
        assert!(cfg.saved_auth_token.is_none());
        assert!(cfg.saved_auth_server_url.is_none());
    }
}
