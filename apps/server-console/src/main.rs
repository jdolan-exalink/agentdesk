#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_LOG_LINES: usize = 600;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_database_url() -> String {
    "postgres://agentdesk:agentdesk_dev_pass@127.0.0.1:5432/agentdesk".to_string()
}
fn default_true() -> bool {
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

impl Default for ConsoleServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            database_url: default_database_url(),
            artifacts_dir: exe_dir(),
            bootstrap_token: Some("agentdesk-local-setup-2026".to_string()),
            public_url: None,
            auto_seed_admin: true,
            default_org_name: default_org_name(),
            default_org_slug: default_org_slug(),
            default_admin_email: default_admin_email(),
            default_admin_name: default_admin_name(),
            default_admin_password: default_admin_password(),
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
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
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

fn build_icon_rgba(online: bool) -> Vec<u8> {
    let w = 32u32;
    let h = 32u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            let dx = (x as f32) - 15.5;
            let dy = (y as f32) - 15.5;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= 14.5 {
                // Deep blue rounded badge + white monitor glyph + status dot
                let mut r = 37u8;
                let mut g = 99u8;
                let mut b = 235u8;
                let in_screen = (7..=24).contains(&x) && (8..=19).contains(&y);
                let in_inner = (9..=22).contains(&x) && (10..=17).contains(&y);
                let in_stand = (13..=18).contains(&x) && (20..=23).contains(&y);
                if (in_screen && !in_inner) || in_stand {
                    r = 255;
                    g = 255;
                    b = 255;
                }
                // Status dot bottom-right
                let sdx = (x as f32) - 24.5;
                let sdy = (y as f32) - 24.5;
                if (sdx * sdx + sdy * sdy).sqrt() <= 4.5 {
                    if online {
                        r = 34;
                        g = 197;
                        b = 94;
                    } else {
                        r = 245;
                        g = 158;
                        b = 11;
                    }
                }
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }
    rgba
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
    HealthStatus {
        online: bool,
        public_url: Option<String>,
        lan_ip: Option<String>,
    },
    FirewallStatus(String),
    Tray(TrayAction),
    Toast(String),
}

struct ServerConsoleApp {
    rt: tokio::runtime::Runtime,
    tx: Sender<ConsoleEvent>,
    rx: Receiver<ConsoleEvent>,
    cfg: ConsoleServerConfig,
    cfg_path: PathBuf,
    port_input: String,
    public_url_input: String,

    child: Option<Child>,
    online: bool,
    external_running: bool,
    lan_ip: Option<String>,
    public_url: String,
    firewall_status: String,
    status_msg: String,
    logs: VecDeque<String>,
    last_poll: Instant,
    allow_quit: bool,
    auto_started_once: bool,
}

impl ServerConsoleApp {
    fn new(cc: &eframe::CreationContext<'_>, tx: Sender<ConsoleEvent>, rx: Receiver<ConsoleEvent>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (cfg, cfg_path) = ConsoleServerConfig::load_or_default();
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

        #[cfg(windows)]
        spawn_windows_tray(cc.egui_ctx.clone(), tx.clone());

        let mut app = Self {
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
            tx,
            rx,
            port_input: port.to_string(),
            public_url_input: cfg.public_url.clone().unwrap_or_default(),
            cfg,
            cfg_path,
            child: None,
            online: false,
            external_running: false,
            lan_ip,
            public_url,
            firewall_status: "Sin verificar".into(),
            status_msg: String::new(),
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
            last_poll: Instant::now() - Duration::from_secs(10),
            allow_quit: false,
            auto_started_once: false,
        };

        app.push_log(format!(
            "[console] Config: {} | LAN IP: {}",
            app.cfg_path.display(),
            app.lan_ip.as_deref().unwrap_or("no detectada")
        ));
        app.check_firewall_status();
        app.probe_and_autostart();
        app
    }

    fn push_log(&mut self, line: String) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    fn server_binary_path(&self) -> Option<PathBuf> {
        let cand = exe_dir()?.join("agentdesk-server.exe");
        if cand.exists() {
            Some(cand)
        } else {
            None
        }
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
        if fallback.exists() {
            Some(fallback)
        } else {
            None
        }
    }

    fn probe_and_autostart(&mut self) {
        let port = self.cfg.port();
        let already_up = self.rt.block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_millis(800))
                .build()
                .ok()?;
            client
                .get(format!("http://127.0.0.1:{port}/health/ready"))
                .send()
                .await
                .ok()
                .filter(|r| r.status().is_success())
        });

        if already_up.is_some() {
            self.online = true;
            self.external_running = true;
            self.auto_started_once = true;
            self.push_log(format!(
                "[console] Servidor detectado en puerto {port} (proceso externo o Servicio Windows)."
            ));
            self.poll_health();
        } else if !self.auto_started_once {
            self.auto_started_once = true;
            self.start_local_server();
        }
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
            self.push_log(format!("[warn] No se pudo guardar {}: {e}", self.cfg_path.display()));
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
                        let reader = BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            let _ = tx.send(ConsoleEvent::LogLine(line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            let _ = tx.send(ConsoleEvent::LogLine(format!("[stderr] {line}")));
                        }
                    });
                }
                self.child = Some(child);
                self.external_running = false;
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
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            self.online = false;
            self.status_msg = "Servidor local detenido".into();
            self.push_log("[console] Proceso local de servidor detenido.".into());
        }
    }

    fn poll_health(&mut self) {
        let port = self.cfg.port();
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap_or_default();
            match client
                .get(format!("http://127.0.0.1:{port}/health/ready"))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.json::<serde_json::Value>().await.unwrap_or_default();
                    let pub_url = body
                        .get("public_url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let lan = body
                        .get("lan_ip")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let _ = tx.send(ConsoleEvent::HealthStatus {
                        online: true,
                        public_url: pub_url,
                        lan_ip: lan,
                    });
                }
                _ => {
                    let _ = tx.send(ConsoleEvent::HealthStatus {
                        online: false,
                        public_url: None,
                        lan_ip: None,
                    });
                }
            }
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
                Ok(out) if out.status.success() => {
                    format!("Regla activa: {rule}")
                }
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
                format!(
                    "& '{}' -Port {port} {bin_arg}",
                    sp.display()
                )
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
                    let _ = tx.send(ConsoleEvent::Toast(
                        "Reglas de Firewall aplicadas correctamente".into(),
                    ));
                    let _ = tx.send(ConsoleEvent::FirewallStatus(format!(
                        "Regla activa: AgentDesk Server TCP {port}"
                    )));
                }
                Ok(s) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!(
                        "Firewall no actualizado (código {s})"
                    )));
                }
                Err(e) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!(
                        "Error ejecutando PowerShell UAC: {e}"
                    )));
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
            let inner = format!(
                "& '{}' -BindAddr '{}' {}",
                script.display(),
                bind_addr,
                bin_arg
            );
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
                    let _ = tx.send(ConsoleEvent::Toast(
                        "Servicio Windows 'AgentDeskServer' instalado e iniciado".into(),
                    ));
                }
                Ok(s) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!(
                        "Instalador de servicio finalizó con código {s}"
                    )));
                }
                Err(e) => {
                    let _ = tx.send(ConsoleEvent::Toast(format!(
                        "Error lanzando UAC servicio: {e}"
                    )));
                }
            }
        });
    }

    fn open_web_portal(&self) {
        let url = format!("{}/app", self.public_url.trim_end_matches('/'));
        #[cfg(windows)]
        {
            let _ = Command::new("cmd")
                .args(["/C", "start", "", &url])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        if let Some(child) = &mut self.child {
            if let Ok(Some(code)) = child.try_wait() {
                self.push_log(format!("[console] Servidor finalizó con estado: {code}"));
                self.child = None;
                self.online = false;
            }
        }

        if self.last_poll.elapsed() >= Duration::from_secs(3) {
            self.last_poll = Instant::now();
            self.poll_health();
        }

        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                ConsoleEvent::LogLine(line) => self.push_log(line),
                ConsoleEvent::HealthStatus {
                    online,
                    public_url,
                    lan_ip,
                } => {
                    self.online = online;
                    if let Some(url) = public_url {
                        self.public_url = url;
                    }
                    if let Some(ip) = lan_ip {
                        self.lan_ip = Some(ip);
                    }
                    if !online && self.child.is_none() {
                        self.external_running = false;
                    }
                }
                ConsoleEvent::FirewallStatus(msg) => {
                    self.firewall_status = msg;
                }
                ConsoleEvent::Toast(msg) => {
                    self.push_log(format!("[info] {msg}"));
                    self.status_msg = msg;
                }
                ConsoleEvent::Tray(action) => match action {
                    TrayAction::ShowWindow => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::OpenPortal => self.open_web_portal(),
                    TrayAction::StartOrRestartServer => {
                        if self.external_running && self.child.is_none() {
                            self.push_log(
                                "[warn] Hay un servidor externo corriendo (Servicio/daemon). Detén ese servicio primero o toma el control."
                                    .into(),
                            );
                        } else {
                            self.start_local_server();
                        }
                    }
                    TrayAction::StopServer => self.stop_local_child(),
                    TrayAction::AllowFirewall => self.allow_firewall_uac(),
                    TrayAction::QuitAll => {
                        self.stop_local_child();
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                },
            }
        }
    }
}

impl eframe::App for ServerConsoleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.status_msg = "Consola minimizada a la bandeja del sistema (Systray)".into();
            return;
        }

        ctx.request_repaint_after(Duration::from_millis(500));

        egui::TopBottomPanel::top("srv_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("AgentDesk Server Console");
                ui.separator();
                if self.online {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "● EN LÍNEA");
                    ui.label(if self.child.is_some() {
                        "(Proceso consola)"
                    } else {
                        "(Externo / Servicio Windows)"
                    });
                } else {
                    ui.colored_label(egui::Color32::YELLOW, "● DETENIDO");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Salir y Detener").clicked() {
                        self.stop_local_child();
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Ocultar a Systray").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("URL Pública (LAN):");
                    ui.monospace(&self.public_url);
                    if ui.button("Copiar URL").clicked() {
                        ctx.copy_text(self.public_url.clone());
                        self.status_msg = format!("URL copiada: {}", self.public_url);
                    }
                    if ui.button("Abrir Portal Web").clicked() {
                        self.open_web_portal();
                    }
                    ui.separator();
                    ui.label(format!(
                        "IP LAN: {} | Local: http://127.0.0.1:{}",
                        self.lan_ip.as_deref().unwrap_or("N/A"),
                        self.cfg.port()
                    ));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Admin por defecto:");
                    ui.monospace(format!(
                        "{} / {}",
                        self.cfg.default_admin_email, self.cfg.default_admin_password
                    ));
                    ui.separator();
                    ui.strong("Firewall:");
                    ui.label(&self.firewall_status);
                });
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if self.child.is_none() && !self.online {
                    if ui.button("▶ Iniciar Servidor").clicked() {
                        self.start_local_server();
                    }
                }
                if self.child.is_some() {
                    if ui.button("↻ Reiniciar Servidor").clicked() {
                        self.start_local_server();
                    }
                    if ui.button("⏹ Detener Servidor").clicked() {
                        self.stop_local_child();
                    }
                }
                if ui.button("🛡 Permitir en Firewall (UAC)").clicked() {
                    self.allow_firewall_uac();
                }
                if ui.button("⚙ Instalar Servicio Windows (UAC)").clicked() {
                    self.install_service_uac();
                }
                if ui.button("⟳ Verificar estado").clicked() {
                    self.check_firewall_status();
                    self.poll_health();
                }
            });

            ui.add_space(6.0);
            egui::CollapsingHeader::new("Configuración de Red y Publicación")
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Puerto TCP:");
                        ui.add(egui::TextEdit::singleline(&mut self.port_input).desired_width(80.0));
                        ui.label("URL Pública (vacío = IP LAN automática):");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.public_url_input)
                                .hint_text("http://10.1.1.156:8080")
                                .desired_width(260.0),
                        );
                        if ui.button("Guardar y aplicar").clicked() {
                            let port: u16 = self.port_input.trim().parse().unwrap_or(8080);
                            self.cfg.bind_addr = format!("0.0.0.0:{port}");
                            self.cfg.public_url = if self.public_url_input.trim().is_empty() {
                                None
                            } else {
                                Some(
                                    self.public_url_input
                                        .trim()
                                        .trim_end_matches('/')
                                        .to_string(),
                                )
                            };
                            let _ = self.cfg.save(&self.cfg_path);
                            if self.child.is_some() {
                                self.start_local_server();
                            } else {
                                self.status_msg = "Configuración guardada".into();
                            }
                        }
                    });
                    ui.small(format!("Archivo: {}", self.cfg_path.display()));
                });

            if !self.status_msg.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_BLUE, &self.status_msg);
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("Consola de Logs en Vivo");
                if ui.button("Limpiar").clicked() {
                    self.logs.clear();
                }
                if ui.button("Copiar todo").clicked() {
                    let joined = self.logs.iter().cloned().collect::<Vec<_>>().join("\n");
                    ctx.copy_text(joined);
                    self.status_msg = "Logs copiados al portapapeles".into();
                }
            });

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.monospace(line);
                    }
                });
        });
    }
}

#[cfg(windows)]
fn spawn_windows_tray(ctx: egui::Context, tx: Sender<ConsoleEvent>) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    std::thread::spawn(move || {
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

        let rgba = build_icon_rgba(true);
        let icon = match tray_icon::Icon::from_rgba(rgba, 32, 32) {
            Ok(i) => i,
            Err(_) => return,
        };

        let _tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("AgentDesk Server Console")
            .with_icon(icon)
            .build()
        {
            Ok(t) => t,
            Err(_) => return,
        };

        let tx_ev = tx.clone();
        let ctx_ev = ctx.clone();
        std::thread::spawn(move || loop {
            let mut woke = false;
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                woke = true;
                let action = if ev.id == id_open {
                    Some(TrayAction::ShowWindow)
                } else if ev.id == id_portal {
                    Some(TrayAction::OpenPortal)
                } else if ev.id == id_restart {
                    Some(TrayAction::StartOrRestartServer)
                } else if ev.id == id_stop {
                    Some(TrayAction::StopServer)
                } else if ev.id == id_fw {
                    Some(TrayAction::AllowFirewall)
                } else if ev.id == id_quit {
                    Some(TrayAction::QuitAll)
                } else {
                    None
                };
                if let Some(act) = action {
                    let _ = tx_ev.send(ConsoleEvent::Tray(act));
                }
            }
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } = ev
                {
                    woke = true;
                    let _ = tx_ev.send(ConsoleEvent::Tray(TrayAction::ShowWindow));
                }
            }
            if woke {
                ctx_ev.request_repaint();
            }
            std::thread::sleep(Duration::from_millis(80));
        });

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(windows)]
fn show_error_box(title: &str, body: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let b: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(b.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

fn init_console_logging() {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let log_path = base.join("AgentDesk").join("server_console.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt().with_ansi(false).try_init();
    }
}

fn main() {
    init_console_logging();

    let (tx, rx) = mpsc::channel();
    let icon = egui::IconData {
        rgba: build_icon_rgba(true),
        width: 32,
        height: 32,
    };

    let make_opts = |renderer: eframe::Renderer| eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_title("AgentDesk — Consola del Servidor")
            .with_icon(icon.clone())
            .with_active(true),
        ..Default::default()
    };

    let tx_clone = tx.clone();
    let mut rx_opt = Some(rx);
    let res = eframe::run_native(
        "AgentDesk Server Console",
        make_opts(eframe::Renderer::Glow),
        Box::new(move |cc| {
            let rx_take = rx_opt.take().expect("single init");
            Ok(Box::new(ServerConsoleApp::new(cc, tx_clone, rx_take)))
        }),
    );

    if let Err(glow_err) = res {
        tracing::warn!(error = %glow_err, "Glow renderer failed, falling back to Wgpu");
        let (tx2, rx2) = mpsc::channel();
        let mut rx2_opt = Some(rx2);
        if let Err(wgpu_err) = eframe::run_native(
            "AgentDesk Server Console",
            make_opts(eframe::Renderer::Wgpu),
            Box::new(move |cc| {
                let rx_take = rx2_opt.take().expect("single init");
                Ok(Box::new(ServerConsoleApp::new(cc, tx2, rx_take)))
            }),
        ) {
            let msg = format!("Glow error: {glow_err}\nWgpu error: {wgpu_err}");
            tracing::error!("{msg}");
            #[cfg(windows)]
            show_error_box("Error iniciando AgentDesk Server Console", &msg);
        }
    }
}
