#![cfg_attr(windows, windows_subsystem = "windows")]

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Icon, Sizable, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_LOG_LINES: usize = 600;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Palette {
    canvas: u32,
    panel: u32,
    raised: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    active: u32,
    success: u32,
    warning: u32,
    danger: u32,
    sidebar_bg: u32,
    sidebar_active: u32,
    sidebar_text: u32,
    sidebar_muted: u32,
    badge_ok_bg: u32,
    badge_ok_fg: u32,
}

const DARK: Palette = Palette {
    canvas: 0x0E131D,
    panel: 0x151B28,
    raised: 0x1D2536,
    border: 0x283247,
    text: 0xEDF2F7,
    muted: 0x94A3B8,
    accent: 0x3B82F6,
    active: 0x1E2F54,
    success: 0x22B983,
    warning: 0xF59E0B,
    danger: 0xEF4444,
    sidebar_bg: 0x0A1638,
    sidebar_active: 0x1D4ED8,
    sidebar_text: 0xF8FAFC,
    sidebar_muted: 0x93A8D6,
    badge_ok_bg: 0x123524,
    badge_ok_fg: 0x34D399,
};

const LIGHT: Palette = Palette {
    canvas: 0xFFFFFF,
    panel: 0xF7F9FC,
    raised: 0xEEF2F7,
    border: 0xE2E8F0,
    text: 0x111827,
    muted: 0x64748B,
    accent: 0x1D4ED8,
    active: 0xEFF6FF,
    success: 0x16A34A,
    warning: 0xD97706,
    danger: 0xDC2626,
    sidebar_bg: 0x0E2A8A,
    sidebar_active: 0x1E40AF,
    sidebar_text: 0xFFFFFF,
    sidebar_muted: 0xB8C7F0,
    badge_ok_bg: 0xDCFCE7,
    badge_ok_fg: 0x15803D,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SavedWindowBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConsoleServerConfig {
    #[serde(default = "default_bind_addr")]
    bind_addr: String,
    #[serde(default = "default_database_url")]
    database_url: String,
    #[serde(default)]
    artifacts_dir: Option<PathBuf>,
    #[serde(default)]
    bootstrap_token: Option<String>,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default = "default_true")]
    auto_seed_admin: bool,
    #[serde(default = "default_org_name")]
    default_org_name: String,
    #[serde(default = "default_org_slug")]
    default_org_slug: String,
    #[serde(default = "default_admin_email")]
    default_admin_email: String,
    #[serde(default = "default_admin_name")]
    default_admin_name: String,
    #[serde(default = "default_admin_password")]
    default_admin_password: String,
    #[serde(default = "default_true")]
    dark_mode: bool,
    #[serde(default)]
    window_bounds: Option<SavedWindowBounds>,
}

fn default_bind_addr() -> String { "0.0.0.0:8080".into() }
fn default_database_url() -> String {
    "postgres://agentdesk:agentdesk_dev_pass@127.0.0.1:5432/agentdesk".into()
}
fn default_true() -> bool { true }
fn default_org_name() -> String { "AgentDesk".into() }
fn default_org_slug() -> String { "agentdesk".into() }
fn default_admin_email() -> String { "juan@dolan.com.ar".into() }
fn default_admin_name() -> String { "Juan Dolan".into() }
fn default_admin_password() -> String { "admin123".into() }

impl Default for ConsoleServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            database_url: default_database_url(),
            artifacts_dir: exe_dir(),
            bootstrap_token: Some("agentdesk-local-setup-2026".into()),
            public_url: None,
            auto_seed_admin: true,
            default_org_name: default_org_name(),
            default_org_slug: default_org_slug(),
            default_admin_email: default_admin_email(),
            default_admin_name: default_admin_name(),
            default_admin_password: default_admin_password(),
            dark_mode: true,
            window_bounds: None,
        }
    }
}

impl ConsoleServerConfig {
    fn port(&self) -> u16 {
        self.bind_addr
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8080)
    }

    fn load_or_default() -> (Self, PathBuf) {
        let exe_cfg = exe_dir().map(|d| d.join("agentdesk-server.json"));
        let pd_cfg = std::env::var("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("AgentDesk")
            .join("server_config.json");

        for cand in [exe_cfg.as_ref(), Some(&pd_cfg)].into_iter().flatten() {
            if cand.exists() {
                if let Ok(raw) = std::fs::read_to_string(cand) {
                    let clean = raw.trim_start_matches('\u{feff}');
                    if let Ok(parsed) = serde_json::from_str::<ConsoleServerConfig>(clean) {
                        return (parsed, cand.clone());
                    }
                }
            }
        }
        let target = exe_cfg.unwrap_or(pd_cfg);
        (Self::default(), target)
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
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

fn build_server_tray_rgba(online: bool) -> Vec<u8> {
    let mut rgba = image::load_from_memory(include_bytes!("../../../logos/agentdesk-server-icono.png"))
        .expect("logo AgentDesk Server válido")
        .resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let status = if online {
        image::Rgba([34, 197, 94, 255])
    } else {
        image::Rgba([245, 158, 11, 255])
    };
    for y in 20..30 {
        for x in 20..30 {
            let dx = x as f32 - 24.5;
            let dy = y as f32 - 24.5;
            if (dx * dx + dy * dy).sqrt() <= 4.5 {
                rgba.put_pixel(x, y, status);
            }
        }
    }
    rgba.into_raw()
}

fn load_brand_logo_image(png_bytes: &[u8]) -> Arc<RenderImage> {
    let mut img_buf = image::load_from_memory(png_bytes)
        .expect("logo AgentDesk Server válido")
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    for px in img_buf.pixels_mut() {
        px.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
}

fn apply_theme(cx: &mut App, mode: ThemeMode) {
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = "Inter".into();
        theme.font_size = px(14.);
    });
}

fn open_url_shell(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let res = unsafe {
            ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), None, None, SW_SHOWNORMAL)
        };
        if (res.0 as isize) <= 32 {
            return Err(format!("ShellExecuteW devolvió {}", res.0 as isize));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum TrayAction {
    ShowWindow,
    OpenPortal,
    StartOrRestartServer,
    StopServer,
    AllowFirewall,
    QuitAll,
}

enum ConsoleEvent {
    LogLine(String),
    InitialProbe {
        process_online: bool,
        ready: bool,
    },
    HealthStatus {
        process_online: bool,
        online: bool,
        database_status: String,
        public_url: Option<String>,
        lan_ip: Option<String>,
    },
    FirewallStatus(String),
    Tray(TrayAction),
    Toast(String),
}

struct ServerConsole {
    brand_logo: Arc<RenderImage>,
    tx: Sender<ConsoleEvent>,
    rx: Receiver<ConsoleEvent>,
    cfg: ConsoleServerConfig,
    cfg_path: PathBuf,
    port_input: Entity<InputState>,
    public_url_input: Entity<InputState>,
    show_network_cfg: bool,

    child: Arc<Mutex<Option<Child>>>,
    process_online: bool,
    online: bool,
    database_status: String,
    external_running: bool,
    lan_ip: Option<String>,
    public_url: String,
    firewall_status: String,
    status_msg: String,
    logs: VecDeque<String>,
    last_poll: Instant,
    auto_started_once: bool,
    hwnd: isize,
    allow_quit: Arc<AtomicBool>,
}

impl ServerConsole {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        tx: Sender<ConsoleEvent>,
        rx: Receiver<ConsoleEvent>,
        child: Arc<Mutex<Option<Child>>>,
        hwnd: isize,
        allow_quit: Arc<AtomicBool>,
    ) -> Self {
        let (cfg, cfg_path) = ConsoleServerConfig::load_or_default();
        apply_theme(
            cx,
            if cfg.dark_mode { ThemeMode::Dark } else { ThemeMode::Light },
        );

        let lan_ip = detect_lan_ip().map(|ip| ip.to_string());
        let port = cfg.port();
        let public_url = cfg
            .public_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| match &lan_ip {
                Some(ip) => format!("http://{ip}:{port}"),
                None => format!("http://127.0.0.1:{port}"),
            });

        let port_str = port.to_string();
        let pub_str = cfg.public_url.clone().unwrap_or_default();

        let port_input = cx.new(|cx| InputState::new(window, cx).default_value(port_str));
        let public_url_input = cx.new(|cx| {
            let mut st = InputState::new(window, cx).placeholder("http://10.1.1.156:8080");
            if !pub_str.is_empty() {
                st = st.default_value(pub_str);
            }
            st
        });

        let mut app = Self {
            brand_logo: load_brand_logo_image(include_bytes!("../../../logos/agentdesk-server-icono.png")),
            tx,
            rx,
            cfg,
            cfg_path,
            port_input,
            public_url_input,
            show_network_cfg: false,
            child,
            process_online: false,
            online: false,
            database_status: "Sin comprobar".into(),
            external_running: false,
            lan_ip,
            public_url,
            firewall_status: "Verificando…".into(),
            status_msg: String::new(),
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
            last_poll: Instant::now() - Duration::from_secs(10),
            auto_started_once: false,
            hwnd,
            allow_quit,
        };

        app.push_log(format!(
            "[console] Config: {} | LAN IP: {}",
            app.cfg_path.display(),
            app.lan_ip.as_deref().unwrap_or("no detectada")
        ));
        app.check_firewall_status();
        app.probe_and_autostart();

        // Background pump: funciona incluso con la ventana minimizada a bandeja
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let keep = cx.update(|cx| {
                let Some(ent) = weak.upgrade() else { return false };
                ent.update(cx, |this, cx| this.process_events(cx));
                true
            });
            if !keep {
                break;
            }
        })
        .detach();

        app
    }

    fn palette(&self) -> Palette {
        if self.cfg.dark_mode { DARK } else { LIGHT }
    }

    fn push_log(&mut self, line: String) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    fn server_binary_path(&self) -> Option<PathBuf> {
        let cand = exe_dir()?.join("agentdesk-server.exe");
        if cand.exists() { Some(cand) } else { None }
    }

    fn find_installer_script(&self, name: &str) -> Option<PathBuf> {
        if let Some(dir) = exe_dir() {
            let direct = dir.join(name);
            if direct.exists() {
                return Some(direct);
            }
            if let Some(target_dir) = dir.parent() {
                if let Some(repo_root) = target_dir.parent() {
                    let repo_script = repo_root.join("installers").join("windows").join(name);
                    if repo_script.exists() {
                        return Some(repo_script);
                    }
                }
            }
        }
        let fallback = PathBuf::from(r"d:\DEVs\AgentDesk\installers\windows").join(name);
        if fallback.exists() { Some(fallback) } else { None }
    }

    fn probe_and_autostart(&mut self) {
        let port = self.cfg.port();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(900))
                .build()
                .ok();
            let (proc_on, ready) = match client {
                Some(c) => {
                    let live = c
                        .get(format!("http://127.0.0.1:{port}/health/live"))
                        .send()
                        .ok()
                        .is_some_and(|r| r.status().is_success());
                    if !live {
                        (false, false)
                    } else {
                        let rd = c
                            .get(format!("http://127.0.0.1:{port}/health/ready"))
                            .send()
                            .ok()
                            .is_some_and(|r| r.status().is_success());
                        (true, rd)
                    }
                }
                None => (false, false),
            };
            let _ = tx.send(ConsoleEvent::InitialProbe {
                process_online: proc_on,
                ready,
            });
        });
    }

    fn has_local_child(&self) -> bool {
        self.child.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn start_local_server(&mut self) {
        self.stop_local_child();

        let Some(server_exe) = self.server_binary_path() else {
            self.status_msg = "No se encontró agentdesk-server.exe junto a la consola".into();
            self.push_log("[error] No se encontró agentdesk-server.exe".into());
            return;
        };

        if self.cfg.artifacts_dir.is_none() {
            self.cfg.artifacts_dir = exe_dir();
        }
        if let Err(e) = self.cfg.save(&self.cfg_path) {
            self.push_log(format!("[warn] No se pudo guardar config: {e}"));
        }

        let mut cmd = Command::new(&server_exe);
        cmd.arg("--config")
            .arg(&self.cfg_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            let _ = tx.send(ConsoleEvent::LogLine(line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            let _ = tx.send(ConsoleEvent::LogLine(format!("[stderr] {line}")));
                        }
                    });
                }
                if let Ok(mut guard) = self.child.lock() {
                    *guard = Some(child);
                }
                self.external_running = false;
                self.process_online = true;
                self.status_msg = "Servidor iniciado desde consola".into();
                self.push_log(format!(
                    "[console] Iniciado {} --config {}",
                    server_exe.display(),
                    self.cfg_path.display()
                ));
                self.last_poll = Instant::now() - Duration::from_secs(10);
            }
            Err(e) => {
                self.status_msg = format!("Error al iniciar servidor: {e}");
                self.push_log(format!("[error] No se pudo iniciar servidor: {e}"));
            }
        }
    }

    fn stop_local_child(&mut self) {
        let taken = if let Ok(mut guard) = self.child.lock() {
            guard.take()
        } else {
            None
        };
        if let Some(mut c) = taken {
            let _ = c.kill();
            std::thread::spawn(move || {
                let _ = c.wait();
            });
            self.online = false;
            self.process_online = false;
            self.database_status = "Sin comprobar".into();
            self.status_msg = "Servidor local detenido".into();
            self.push_log("[console] Proceso local de servidor detenido.".into());
        }
    }

    fn poll_health(&mut self) {
        let port = self.cfg.port();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap_or_default();
            let process_online = client
                .get(format!("http://127.0.0.1:{port}/health/live"))
                .send()
                .ok()
                .is_some_and(|r| r.status().is_success());
            let ready_resp = if process_online {
                client
                    .get(format!("http://127.0.0.1:{port}/health/ready"))
                    .send()
                    .ok()
            } else {
                None
            };
            let online = ready_resp.as_ref().is_some_and(|r| r.status().is_success());
            let body = match ready_resp {
                Some(r) if r.status().is_success() => r.json::<serde_json::Value>().unwrap_or_default(),
                _ => serde_json::Value::Null,
            };
            let public_url = body.get("public_url").and_then(|v| v.as_str()).map(str::to_string);
            let lan_ip = body.get("lan_ip").and_then(|v| v.as_str()).map(str::to_string);
            let database_status = if online {
                "Conectada"
            } else if process_online {
                "Sin conexión"
            } else {
                "Sin comprobar"
            };
            let _ = tx.send(ConsoleEvent::HealthStatus {
                process_online,
                online,
                database_status: database_status.into(),
                public_url,
                lan_ip,
            });
        });
    }

    fn check_firewall_status(&self) {
        let port = self.cfg.port();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let rule = format!("AgentDesk Server TCP {port}");
            let mut cmd = Command::new("netsh");
            cmd.args(["advfirewall", "firewall", "show", "rule"])
                .arg(format!("name={rule}"));
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let status = match cmd.output() {
                Ok(out) if out.status.success() => format!("Regla activa: {rule}"),
                _ => format!("Falta regla '{rule}' (usar botón Firewall)"),
            };
            let _ = tx.send(ConsoleEvent::FirewallStatus(status));
        });
    }

    fn allow_firewall_uac(&self) {
        let port = self.cfg.port();
        let tx = self.tx.clone();
        let script = self.find_installer_script("allow-firewall.ps1");
        let bin = self.server_binary_path();

        std::thread::spawn(move || {
            let ps_script = if let Some(sp) = script {
                let bin_arg = bin
                    .as_ref()
                    .map(|b| format!("-BinaryPath '{}'", b.display()))
                    .unwrap_or_default();
                format!("& '{}' -Port {port} {bin_arg}", sp.display())
            } else {
                let rule = format!("AgentDesk Server TCP {port}");
                let app_cmd = bin
                    .as_ref()
                    .map(|b| {
                        format!(
                            "Get-NetFirewallRule -DisplayName 'AgentDesk Server App' -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue; New-NetFirewallRule -DisplayName 'AgentDesk Server App' -Direction Inbound -Program '{}' -Action Allow -Profile Any | Out-Null;",
                            b.display()
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "Get-NetFirewallRule -DisplayName '{rule}' -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue; New-NetFirewallRule -DisplayName '{rule}' -Direction Inbound -Protocol TCP -LocalPort {port} -Action Allow -Profile Any | Out-Null; {app_cmd}"
                )
            };

            let wrapped = format!(
                "Start-Process powershell -Verb RunAs -Wait -ArgumentList @('-ExecutionPolicy','Bypass','-Command',\"{}\")",
                ps_script.replace('"', "`\"")
            );
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-ExecutionPolicy", "Bypass", "-Command", &wrapped]);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            match cmd.status() {
                Ok(s) if s.success() => {
                    let _ = tx.send(ConsoleEvent::Toast("Reglas de Firewall aplicadas correctamente".into()));
                    let _ = tx.send(ConsoleEvent::FirewallStatus(format!("Regla activa: AgentDesk Server TCP {port}")));
                }
                Ok(s) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!("Firewall no actualizado (código {s})")));
                }
                Err(e) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!("Error ejecutando PowerShell UAC: {e}")));
                }
            }
        });
    }

    fn install_service_uac(&mut self) {
        let tx = self.tx.clone();
        let Some(script) = self.find_installer_script("install-server-service.ps1") else {
            self.push_log("[warn] No se encontró installers/windows/install-server-service.ps1".into());
            return;
        };
        let port = self.cfg.port();
        let bind_addr = format!("0.0.0.0:{port}");
        let bin_arg = self
            .server_binary_path()
            .map(|b| format!("-BinaryPath '{}'", b.display()))
            .unwrap_or_default();
        std::thread::spawn(move || {
            let inner = format!("& '{}' -BindAddr '{}' {}", script.display(), bind_addr, bin_arg);
            let wrapped = format!(
                "Start-Process powershell -Verb RunAs -Wait -ArgumentList @('-ExecutionPolicy','Bypass','-Command',\"{}\")",
                inner.replace('"', "`\"")
            );
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-ExecutionPolicy", "Bypass", "-Command", &wrapped]);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            match cmd.status() {
                Ok(s) if s.success() => {
                    let _ = tx.send(ConsoleEvent::Toast("Servicio Windows 'AgentDeskServer' instalado e iniciado".into()));
                }
                Ok(s) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!("Instalador finalizó con código {s}")));
                }
                Err(e) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!("Error UAC servicio: {e}")));
                }
            }
        });
    }

    fn open_web_portal(&mut self) {
        let url = format!("{}/app", self.public_url.trim_end_matches('/'));
        let tx = self.tx.clone();
        self.status_msg = format!("Abriendo portal: {url}");
        std::thread::spawn(move || {
            if let Err(e) = open_url_shell(&url) {
                let _ = tx.send(ConsoleEvent::Toast(format!("Error abriendo portal: {e}")));
            }
        });
    }

    fn quit_all(&mut self) {
        self.stop_local_child();
        self.allow_quit.store(true, Ordering::Relaxed);
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
            let _ = PostMessageW(HWND(self.hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn hide_to_tray(&mut self, _cx: &mut App) {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
            let _ = ShowWindow(HWND(self.hwnd as *mut _), SW_HIDE);
        }
        self.status_msg = "Consola minimizada a Systray".into();
        self.push_log("[tray] Consola oculta en bandeja del sistema.".into());
    }

    fn process_events(&mut self, cx: &mut Context<Self>) {
        let mut dirty = false;

        let exited = if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                if let Ok(Some(code)) = child.try_wait() {
                    *guard = None;
                    Some(code)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(code) = exited {
            self.push_log(format!("[console] Servidor finalizó con estado: {code}"));
            self.online = false;
            self.process_online = false;
            dirty = true;
        }

        if self.last_poll.elapsed() >= Duration::from_secs(3) {
            self.last_poll = Instant::now();
            self.poll_health();
        }

        while let Ok(ev) = self.rx.try_recv() {
            dirty = true;
            match ev {
                ConsoleEvent::LogLine(line) => self.push_log(line),
                ConsoleEvent::InitialProbe { process_online, ready } => {
                    let port = self.cfg.port();
                    if process_online {
                        self.process_online = true;
                        self.online = ready;
                        self.external_running = true;
                        self.auto_started_once = true;
                        self.database_status = if ready { "Conectada" } else { "Sin conexión" }.into();
                        self.push_log(format!(
                            "[console] Servidor detectado en puerto {port} (externo / Servicio Windows); PostgreSQL {}.",
                            self.database_status.to_lowercase()
                        ));
                        self.poll_health();
                    } else if !self.auto_started_once {
                        self.auto_started_once = true;
                        self.start_local_server();
                    }
                }
                ConsoleEvent::HealthStatus {
                    process_online,
                    online,
                    database_status,
                    public_url,
                    lan_ip,
                } => {
                    self.process_online = process_online;
                    self.online = online;
                    self.database_status = database_status;
                    if let Some(u) = public_url {
                        self.public_url = u;
                    }
                    if let Some(ip) = lan_ip {
                        self.lan_ip = Some(ip);
                    }
                    if !process_online && !self.has_local_child() {
                        self.external_running = false;
                    }
                }
                ConsoleEvent::FirewallStatus(msg) => self.firewall_status = msg,
                ConsoleEvent::Toast(msg) => {
                    self.push_log(format!("[info] {msg}"));
                    self.status_msg = msg;
                }
                ConsoleEvent::Tray(action) => match action {
                    TrayAction::ShowWindow => {
                        #[cfg(windows)]
                        unsafe {
                            use windows::Win32::Foundation::HWND;
                            use windows::Win32::UI::WindowsAndMessaging::{
                                SetForegroundWindow, ShowWindow, SW_RESTORE,
                            };
                            let h = HWND(self.hwnd as *mut _);
                            let _ = ShowWindow(h, SW_RESTORE);
                            let _ = SetForegroundWindow(h);
                        }
                    }
                    TrayAction::OpenPortal => self.open_web_portal(),
                    TrayAction::StartOrRestartServer => {
                        if self.external_running && !self.has_local_child() {
                            self.push_log(
                                "[warn] Servidor corriendo externamente (Servicio/daemon). Detenelo primero para controlarlo desde consola."
                                    .into(),
                            );
                        } else {
                            self.start_local_server();
                        }
                    }
                    TrayAction::StopServer => self.stop_local_child(),
                    TrayAction::AllowFirewall => self.allow_firewall_uac(),
                    TrayAction::QuitAll => self.quit_all(),
                },
            }
        }

        if dirty {
            cx.notify();
        }
    }

    fn btn(id: &'static str, label: impl Into<SharedString>, bg: u32, fg: u32) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .text_xs()
            .font_semibold()
            .child(label.into())
    }
}

impl Render for ServerConsole {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let has_child = self.has_local_child();
        let status_label = if self.process_online {
            if self.online { "● OPERATIVO" } else { "● DEGRADADO" }
        } else {
            "● SERVIDOR DETENIDO"
        };
        let status_color = if self.process_online {
            if self.online { p.success } else { p.warning }
        } else {
            p.muted
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(p.canvas))
            .text_color(rgb(p.text))
            .child(
                // Top Bar
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(54.))
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        img(self.brand_logo.clone())
                                            .size(px(32.))
                                            .rounded_lg(),
                                    )
                                    .child(div().text_lg().font_semibold().child("AgentDesk Server")),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_full()
                                    .bg(rgb(if self.online { p.badge_ok_bg } else { p.raised }))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(rgb(if self.online { p.badge_ok_fg } else { status_color }))
                                    .child(status_label),
                            )
                            .child(div().text_xs().text_color(rgb(p.muted)).child(if self.process_online {
                                if has_child { "Proceso consola" } else { "Externo / Servicio Windows" }
                            } else {
                                "Detenido"
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.muted))
                                    .child(format!("PostgreSQL: {}", self.database_status)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("srv-theme-toggle")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(34.))
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .cursor_pointer()
                                    .bg(rgb(p.raised))
                                    .text_color(rgb(p.text))
                                    .hover(|s| s.bg(rgb(p.active)))
                                    .child(Icon::new(IconName::SunMoon).size(px(19.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cfg.dark_mode = !this.cfg.dark_mode;
                                        let _ = this.cfg.save(&this.cfg_path);
                                        apply_theme(
                                            cx,
                                            if this.cfg.dark_mode { ThemeMode::Dark } else { ThemeMode::Light },
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Self::btn("srv-hide-tray", "Ocultar a Systray", p.raised, p.text)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.hide_to_tray(cx);
                                    })),
                            )
                            .child(
                                Self::btn("srv-quit-stop", "Salir y Detener", p.danger, 0xFFFFFF)
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.quit_all();
                                    })),
                            ),
                    ),
            )
            .child(
                // Body: Left Sidebar actions + Right Dashboard & Logs
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        // Sidebar
                        div()
                            .flex()
                            .flex_col()
                            .w(px(220.))
                            .p_3()
                            .gap_2()
                            .bg(rgb(p.sidebar_bg))
                            .text_color(rgb(p.sidebar_text))
                            .child(div().text_xs().font_semibold().text_color(rgb(p.sidebar_muted)).child("CONTROL DEL SERVIDOR"))
                            .when(!has_child && !self.process_online, |this| {
                                this.child(
                                    Self::btn("sb-start", "▶ Iniciar Servidor", p.sidebar_active, p.sidebar_text)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_local_server();
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(has_child, |this| {
                                this.child(
                                    Self::btn("sb-restart", "↻ Reiniciar Servidor", p.sidebar_active, p.sidebar_text)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_local_server();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Self::btn("sb-stop", "⏹ Detener Servidor", p.danger, 0xFFFFFF)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.stop_local_child();
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Self::btn("sb-portal", "Abrir Portal Web (/app)", p.sidebar_active, p.sidebar_text)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_web_portal();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Self::btn("sb-copy-url", "Copiar URL pública", p.sidebar_bg, p.sidebar_text)
                                    .border_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            this.public_url.clone(),
                                        ));
                                        this.status_msg = format!("URL copiada: {}", this.public_url);
                                        cx.notify();
                                    })),
                            )
                            .child(div().pt_2().border_t_1().border_color(rgb(p.sidebar_active)).text_xs().font_semibold().text_color(rgb(p.sidebar_muted)).child("WINDOWS & RED"))
                            .child(
                                Self::btn("sb-fw", "Permitir Firewall (UAC)", p.sidebar_bg, p.sidebar_text)
                                    .border_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.allow_firewall_uac();
                                    })),
                            )
                            .child(
                                Self::btn("sb-svc", "Instalar Servicio (UAC)", p.sidebar_bg, p.sidebar_text)
                                    .border_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.install_service_uac();
                                    })),
                            )
                            .child(
                                Self::btn("sb-poll", "Verificar estado ahora", p.sidebar_bg, p.sidebar_text)
                                    .border_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.check_firewall_status();
                                        this.poll_health();
                                    })),
                            )
                            .child(
                                Self::btn("sb-netcfg", if self.show_network_cfg { "Ocultar Config Red" } else { "Configuración de Red" }, p.sidebar_bg, p.sidebar_text)
                                    .border_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_network_cfg = !this.show_network_cfg;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .pt_2()
                                    .border_t_1()
                                    .border_color(rgb(p.sidebar_active))
                                    .text_xs()
                                    .text_color(rgb(p.sidebar_muted))
                                    .child(format!("Admin: {}", self.cfg.default_admin_email)),
                            ),
                    )
                    .child(
                        // Right Main Pane
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .p_4()
                            .gap_3()
                            .child(
                                // Summary info card
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .p_4()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .bg(rgb(p.panel))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("URL Pública (LAN)"))
                                            .child(div().font_semibold().text_color(rgb(p.accent)).child(self.public_url.clone())),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("IP LAN / Local"))
                                            .child(div().text_sm().child(format!(
                                                "{} | 127.0.0.1:{}",
                                                self.lan_ip.as_deref().unwrap_or("N/A"),
                                                self.cfg.port()
                                            ))),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(p.muted)).child("Firewall Windows"))
                                            .child(div().text_sm().child(self.firewall_status.clone())),
                                    ),
                            )
                            .when(self.show_network_cfg, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .p_4()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(p.border))
                                        .bg(rgb(p.panel))
                                        .child(div().font_semibold().text_sm().child("Configuración de Red y Publicación"))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Input::new(&self.port_input).w(px(100.)))
                                                .child(Input::new(&self.public_url_input).flex_1())
                                                .child(
                                                    Self::btn("apply-net", "Guardar y aplicar", p.accent, 0xFFFFFF)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let port: u16 = this.port_input.read(cx).value().trim().parse().unwrap_or(8080);
                                                            let pub_v = this.public_url_input.read(cx).value().trim().trim_end_matches('/').to_string();
                                                            this.cfg.bind_addr = format!("0.0.0.0:{port}");
                                                            this.cfg.public_url = if pub_v.is_empty() { None } else { Some(pub_v) };
                                                            let _ = this.cfg.save(&this.cfg_path);
                                                            if this.has_local_child() {
                                                                this.start_local_server();
                                                            } else {
                                                                this.status_msg = "Configuración guardada".into();
                                                            }
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .child(div().text_xs().text_color(rgb(p.muted)).child(format!("Archivo: {}", self.cfg_path.display()))),
                                )
                            })
                            .when(!self.status_msg.is_empty(), |this| {
                                this.child(div().text_xs().text_color(rgb(p.accent)).child(self.status_msg.clone()))
                            })
                            .child(
                                // Live Logs Panel
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .p_4()
                                    .gap_2()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .bg(rgb(p.panel))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(div().font_semibold().text_sm().child("Consola de Logs en Vivo"))
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_2()
                                                    .child(
                                                        Self::btn("logs-clear", "Limpiar", p.raised, p.text)
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.logs.clear();
                                                                cx.notify();
                                                            })),
                                                    )
                                                    .child(
                                                        Self::btn("logs-copy", "Copiar todo", p.raised, p.text)
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                let joined = this.logs.iter().cloned().collect::<Vec<_>>().join("\n");
                                                                cx.write_to_clipboard(ClipboardItem::new_string(joined));
                                                                this.status_msg = "Logs copiados al portapapeles".into();
                                                                cx.notify();
                                                            })),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .p_3()
                                            .rounded_md()
                                            .bg(rgb(p.canvas))
                                            .overflow_y_scrollbar()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(self.logs.iter().map(|line| {
                                                div().text_xs().text_color(rgb(p.muted)).child(line.clone())
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(windows)]
fn focus_existing_window(title: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn spawn_windows_tray(
    tx: Sender<ConsoleEvent>,
    hwnd_val: isize,
    tray_ok: Arc<AtomicBool>,
    allow_quit: Arc<AtomicBool>,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostMessageW, SetForegroundWindow, ShowWindow,
        TranslateMessage, MSG, SW_RESTORE, WM_CLOSE,
    };

    std::thread::spawn(move || {
        let mut built = None;
        let mut last_err = String::new();
        for attempt in 0..5 {
            let menu = Menu::new();
            let item_open = MenuItem::new("Abrir Consola AgentDesk", true, None);
            let item_portal = MenuItem::new("Abrir Portal Web (LAN)", true, None);
            let item_restart = MenuItem::new("Iniciar / Reiniciar Servidor", true, None);
            let item_stop = MenuItem::new("Detener Servidor", true, None);
            let item_fw = MenuItem::new("Configurar Firewall (UAC)", true, None);
            let item_quit = MenuItem::new("Salir y Detener", true, None);

            let _ = menu.append(&item_open);
            let _ = menu.append(&item_portal);
            let _ = menu.append(&PredefinedMenuItem::separator());
            let _ = menu.append(&item_restart);
            let _ = menu.append(&item_stop);
            let _ = menu.append(&item_fw);
            let _ = menu.append(&PredefinedMenuItem::separator());
            let _ = menu.append(&item_quit);

            let id_open = item_open.id().clone();
            let id_portal = item_portal.id().clone();
            let id_restart = item_restart.id().clone();
            let id_stop = item_stop.id().clone();
            let id_fw = item_fw.id().clone();
            let id_quit = item_quit.id().clone();

            let Ok(icon) = tray_icon::Icon::from_rgba(build_server_tray_rgba(true), 32, 32) else {
                let _ = tx.send(ConsoleEvent::LogLine("[tray] No se pudo crear el ícono de systray.".into()));
                return;
            };
            match TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("AgentDesk Server Console")
                .with_icon(icon)
                .build()
            {
                Ok(t) => {
                    built = Some((t, id_open, id_portal, id_restart, id_stop, id_fw, id_quit));
                    break;
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < 4 {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
        let Some((_tray, id_open, id_portal, id_restart, id_stop, id_fw, id_quit)) = built else {
            let _ = tx.send(ConsoleEvent::LogLine(format!(
                "[tray] No se pudo registrar ícono en systray: {last_err}"
            )));
            return;
        };
        tray_ok.store(true, Ordering::Relaxed);
        let _ = tx.send(ConsoleEvent::LogLine("[tray] Ícono de systray registrado.".into()));

        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            let hwnd = HWND(hwnd_val as *mut _);
            if ev.id == id_open {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::ShowWindow));
            } else if ev.id == id_portal {
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::OpenPortal));
            } else if ev.id == id_restart {
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::StartOrRestartServer));
            } else if ev.id == id_stop {
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::StopServer));
            } else if ev.id == id_fw {
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::AllowFirewall));
            } else if ev.id == id_quit {
                allow_quit.store(true, Ordering::Relaxed);
                let _ = menu_tx.send(ConsoleEvent::Tray(TrayAction::QuitAll));
                unsafe {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }));

        TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
            if matches!(
                ev,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let hwnd = HWND(hwnd_val as *mut _);
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }));

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(windows)]
fn acquire_single_instance() -> Result<Option<windows::Win32::Foundation::HANDLE>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\AgentDesk.ServerConsole.Instance"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
        .map_err(|e| format!("CreateMutexW error: {e}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { let _ = CloseHandle(handle); }
        focus_existing_window("AgentDesk Server Console");
        return Ok(None);
    }
    Ok(Some(handle))
}

fn init_console_logging() {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let log_path = base.join("AgentDesk").join("server_console.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = tracing_subscriber::fmt().with_ansi(false).with_writer(Mutex::new(file)).try_init();
    } else {
        let _ = tracing_subscriber::fmt().with_ansi(false).try_init();
    }
}

fn main() {
    init_console_logging();

    #[cfg(windows)]
    let _guard = match acquire_single_instance() {
        Ok(Some(h)) => h,
        Ok(None) => return,
        Err(_) => return,
    };

    let (tx, rx) = mpsc::channel();
    let shared_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let child_cleanup = shared_child.clone();
    let (init_cfg, _) = ConsoleServerConfig::load_or_default();
    let init_bounds = if let Some(sb) = init_cfg.window_bounds {
        if sb.width >= 640.0
            && sb.height >= 460.0
            && sb.x > -3000.0
            && sb.y > -3000.0
            && sb.x < 8000.0
            && sb.y < 5000.0
        {
            let b = Bounds {
                origin: point(px(sb.x), px(sb.y)),
                size: size(px(sb.width), px(sb.height)),
            };
            if sb.maximized {
                WindowBounds::Maximized(b)
            } else {
                WindowBounds::Windowed(b)
            }
        } else {
            WindowBounds::Windowed(Bounds {
                origin: point(px(120.), px(90.)),
                size: size(px(940.), px(620.)),
            })
        }
    } else {
        WindowBounds::Windowed(Bounds {
            origin: point(px(120.), px(90.)),
            size: size(px(940.), px(620.)),
        })
    };

    application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.text_system()
                .add_fonts(vec![Cow::Borrowed(
                    include_bytes!("../../desktop/assets/fonts/Inter.ttf").as_slice(),
                )])
                .expect("no se pudo cargar la fuente Inter");
            apply_theme(cx, if init_cfg.dark_mode { ThemeMode::Dark } else { ThemeMode::Light });

            let tx_win = tx.clone();
            let mut rx_opt = Some(rx);
            let child_win = shared_child.clone();

            open_window(
                WindowOptions {
                    window_bounds: Some(init_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some("AgentDesk Server Console".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let rx_take = rx_opt.take().expect("single init");
                    let allow_quit = Arc::new(AtomicBool::new(false));
                    let tray_ok = Arc::new(AtomicBool::new(false));
                    let mut hwnd_val = 0isize;

                    #[cfg(windows)]
                    {
                        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                        use windows::core::PCWSTR;
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            LoadImageW, SendMessageW, SetClassLongPtrW, SetWindowTextW,
                            GCLP_HICON, GCLP_HICONSM, ICON_BIG, ICON_SMALL, IMAGE_ICON,
                            LR_DEFAULTCOLOR, WM_SETICON,
                        };
                        if let Ok(handle) = HasWindowHandle::window_handle(window) {
                            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                                hwnd_val = h.hwnd.get() as isize;
                                let title_wide: Vec<u16> = "AgentDesk Server Console"
                                    .encode_utf16()
                                    .chain(std::iter::once(0))
                                    .collect();
                                unsafe {
                                    let _ = SetWindowTextW(
                                        HWND(hwnd_val as *mut _),
                                        PCWSTR(title_wide.as_ptr()),
                                    );
                                    if let Ok(hmod) = GetModuleHandleW(None) {
                                        if let Ok(hbig) = LoadImageW(hmod, PCWSTR(1 as *const u16), IMAGE_ICON, 32, 32, LR_DEFAULTCOLOR) {
                                            let _ = SendMessageW(HWND(hwnd_val as *mut _), WM_SETICON, windows::Win32::Foundation::WPARAM(ICON_BIG as usize), windows::Win32::Foundation::LPARAM(hbig.0 as isize));
                                            let _ = SetClassLongPtrW(HWND(hwnd_val as *mut _), GCLP_HICON, hbig.0 as isize);
                                        }
                                        if let Ok(hsmall) = LoadImageW(hmod, PCWSTR(1 as *const u16), IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR) {
                                            let _ = SendMessageW(HWND(hwnd_val as *mut _), WM_SETICON, windows::Win32::Foundation::WPARAM(ICON_SMALL as usize), windows::Win32::Foundation::LPARAM(hsmall.0 as isize));
                                            let _ = SetClassLongPtrW(HWND(hwnd_val as *mut _), GCLP_HICONSM, hsmall.0 as isize);
                                        }
                                    }
                                }
                                spawn_windows_tray(
                                    tx_win.clone(),
                                    hwnd_val,
                                    tray_ok.clone(),
                                    allow_quit.clone(),
                                );
                            }
                        }
                    }

                    let root = cx.new(|cx| ServerConsole::new(window, cx, tx_win, rx_take, child_win, hwnd_val, allow_quit.clone()));
                    let weak_root = root.downgrade();
                    let aq = allow_quit.clone();
                    let tok = tray_ok.clone();
                    let ch = child_cleanup.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        let (bounds, maximized) = match window.window_bounds() {
                            WindowBounds::Windowed(b) => (b, false),
                            WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
                        };
                        let x: f32 = bounds.origin.x.into();
                        let y: f32 = bounds.origin.y.into();
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        if width >= 240.0 && height >= 180.0 && x > -10000.0 && y > -10000.0 {
                            let _ = weak_root.update(cx, |this, _| {
                                this.cfg.window_bounds = Some(SavedWindowBounds {
                                    x,
                                    y,
                                    width,
                                    height,
                                    maximized,
                                });
                                let _ = this.cfg.save(&this.cfg_path);
                            });
                        }
                        if aq.load(Ordering::Relaxed) || !tok.load(Ordering::Relaxed) {
                            if let Ok(mut g) = ch.lock() {
                                if let Some(mut c) = g.take() {
                                    let _ = c.kill();
                                    let _ = c.wait();
                                }
                            }
                            return true;
                        }
                        #[cfg(windows)]
                        unsafe {
                            use windows::Win32::Foundation::HWND;
                            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                            let _ = ShowWindow(HWND(hwnd_val as *mut _), SW_HIDE);
                        }
                        false
                    });
                    root
                },
            )
            .expect("no se pudo abrir AgentDesk Server Console");
        });
}
