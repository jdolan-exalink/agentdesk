//! Actualizador del cliente: consulta el manifiesto firmado del servidor, muestra
//! el changelog y, SOLO cuando el usuario acepta, descarga, verifica (firma
//! ed25519 + SHA-256 + tamaño), reemplaza el ejecutable y reinicia.

use crate::theme::Palette;
use crate::ui::{ghost_btn, icon_btn, primary_btn, tip};
use crate::Client;
use agentdesk_identity::release::{is_newer, SignedUpdateManifest, UpdateInfo};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Clave pública de firma de releases (la privada está en la PC de compilación,
/// `%LOCALAPPDATA%\AgentDesk\release-signing.key`, protegida con DPAPI).
pub const RELEASE_PUBKEY_HEX: &str = "64299d5e222291646ab16c088d14553d205086b046ac7695ac5a74ccf766c249";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const PRODUCT: &str = "desktop";
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// Mientras el cliente está abierto consulta cada minuto (pedido de Juan).
const CHECK_EVERY: Duration = Duration::from_secs(60);
const MAX_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading(UpdateInfo),
    Installing(UpdateInfo),
    /// No hay permiso para reemplazar el exe: queda verificado en esta ruta.
    Manual(UpdateInfo, PathBuf),
    Failed(String),
}

pub struct Updater {
    pub state: UpdateState,
    /// "Más tarde": oculta el aviso del topbar hasta el próximo reinicio.
    pub dismissed: bool,
    pub progress: Arc<AtomicU64>,
    pub last_check: Option<chrono::DateTime<chrono::Local>>,
    /// Hay una consulta en curso (la automática no pasa por `Checking`).
    pub in_flight: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            state: UpdateState::Idle,
            dismissed: false,
            progress: Arc::new(AtomicU64::new(0)),
            last_check: None,
            in_flight: false,
        }
    }
}

impl Updater {
    pub fn available(&self) -> Option<&UpdateInfo> {
        match &self.state {
            UpdateState::Available(i) => Some(i),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Lógica sin UI (corre en el background executor)
// ---------------------------------------------------------------------------

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())
}

/// `Ok(None)` = no hay nada más nuevo (o el servidor aún no publica manifiesto).
pub fn fetch_update(base: &str) -> Result<Option<UpdateInfo>, String> {
    fetch_update_with(base, RELEASE_PUBKEY_HEX, CURRENT_VERSION)
}

fn fetch_update_with(base: &str, pubkey: &str, current: &str) -> Result<Option<UpdateInfo>, String> {
    let r = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?
        .get(format!("{base}/api/v1/updates/{PRODUCT}"))
        .send()
        .map_err(|e| format!("sin conexión con el servidor: {e}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status()));
    }
    let manifest: SignedUpdateManifest = r.json().map_err(|e| format!("manifiesto ilegible: {e}"))?;
    let info = manifest
        .verify(pubkey)
        .map_err(|e| format!("manifiesto rechazado: {e}"))?;
    if info.product != PRODUCT || info.size_bytes == 0 || info.size_bytes > MAX_SIZE {
        return Err("manifiesto rechazado: producto o tamaño inválido".into());
    }
    Ok(is_newer(&info.version, current).then_some(info))
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".agentdesk-write-test");
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

fn old_exe_path(exe: &Path) -> PathBuf {
    PathBuf::from(format!("{}.old", exe.display()))
}

/// Descarga a un temporal hasheando en streaming; solo deja el archivo final si
/// el tamaño y el SHA-256 coinciden con el manifiesto firmado.
pub fn download_verified(base: &str, info: &UpdateInfo, progress: &AtomicU64) -> Result<PathBuf, String> {
    let exe = current_exe()?;
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    let dir = if dir_writable(&exe_dir) { exe_dir } else { std::env::temp_dir() };
    download_verified_to(&dir, base, info, progress)
}

fn download_verified_to(dir: &Path, base: &str, info: &UpdateInfo, progress: &AtomicU64) -> Result<PathBuf, String> {
    let part = dir.join(format!("{}.download", info.filename));
    let done = dir.join(format!("agentdesk-desktop-v{}.new.exe", info.version));

    let mut resp = http()?
        .get(format!("{base}{}", info.download_path))
        .send()
        .map_err(|e| format!("descarga: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("descarga: HTTP {}", resp.status()));
    }
    let mut file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    let res: Result<(), String> = (|| loop {
        let n = resp.read(&mut buf).map_err(|e| format!("descarga: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        total += n as u64;
        if total > info.size_bytes {
            return Err("el archivo es más grande que lo anunciado".into());
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress.store(total * 100 / info.size_bytes, Ordering::Relaxed);
    })();
    drop(file);
    let check = res.and_then(|_| {
        if total != info.size_bytes {
            return Err(format!("descarga incompleta ({total} de {} bytes)", info.size_bytes));
        }
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(&info.sha256) {
            return Err("SHA-256 no coincide: el archivo fue alterado o está corrupto".into());
        }
        Ok(())
    });
    if let Err(e) = check {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let _ = std::fs::remove_file(&done);
    std::fs::rename(&part, &done).map_err(|e| e.to_string())?;
    Ok(done)
}

pub enum InstallError {
    /// Sin permiso sobre la carpeta del exe: el usuario tiene que ejecutarlo a mano.
    NoPermission,
    Other(String),
}

/// Reemplaza el exe en uso (Windows permite renombrar un exe que está
/// corriendo) y lanza la versión nueva, que espera a que este proceso termine.
pub fn install_and_relaunch(new_exe: &Path) -> Result<(), InstallError> {
    let exe = current_exe().map_err(InstallError::Other)?;
    swap_in(&exe, new_exe)?;
    let old = old_exe_path(&exe);

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--after-update").arg(std::process::id().to_string());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    if let Err(e) = cmd.spawn() {
        // Volver atrás: que el usuario no quede sin cliente.
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::rename(&old, &exe);
        return Err(InstallError::Other(format!("no pude iniciar la versión nueva: {e}")));
    }
    Ok(())
}

/// Aparta `exe` como `exe.old` y pone `new_exe` en su lugar (o deshace).
fn swap_in(exe: &Path, new_exe: &Path) -> Result<(), InstallError> {
    let old = old_exe_path(exe);
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
            InstallError::NoPermission
        } else {
            InstallError::Other(format!("no pude apartar el ejecutable actual: {e}"))
        });
    }
    let placed = std::fs::rename(new_exe, &exe).or_else(|_| std::fs::copy(new_exe, &exe).map(|_| ()));
    if let Err(e) = placed {
        let _ = std::fs::rename(&old, &exe);
        return Err(InstallError::Other(format!("no pude colocar la versión nueva: {e}")));
    }
    let _ = std::fs::remove_file(new_exe);
    Ok(())
}

/// Llamar al principio de `main`, antes del mutex de instancia única: si nos
/// lanzó el actualizador, espera a que la versión anterior cierre y borra su exe.
pub fn after_update_startup() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pid) = args
        .iter()
        .position(|a| a == "--after-update")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u32>().ok())
    {
        wait_for_process_exit(pid, Duration::from_secs(30));
    }
    if let Ok(exe) = current_exe() {
        let old = old_exe_path(&exe);
        if old.exists() {
            std::thread::spawn(move || {
                for _ in 0..20 {
                    if std::fs::remove_file(&old).is_ok() || !old.exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
    }
}

#[cfg(windows)]
fn wait_for_process_exit(pid: u32, timeout: Duration) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, timeout.as_millis() as u32);
            let _ = CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_process_exit(_pid: u32, timeout: Duration) {
    std::thread::sleep(timeout.min(Duration::from_secs(2)));
}

fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

// ---------------------------------------------------------------------------
// Integración con el cliente
// ---------------------------------------------------------------------------

impl Client {
    /// Primera consulta a los 20 s y luego cada minuto. Nunca instala sola.
    pub fn schedule_update_checks(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut wait = FIRST_CHECK;
            loop {
                cx.background_executor().timer(wait).await;
                wait = CHECK_EVERY;
                if this.update(cx, |this, cx| this.check_updates(false, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn check_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        match self.updater.state {
            UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Installing(_) => return,
            // La consulta de cada minuto no pisa el aviso de "ejecutalo a mano".
            UpdateState::Manual(..) if !manual => return,
            _ => {}
        }
        if self.updater.in_flight {
            return;
        }
        self.updater.in_flight = true;
        let base = self.base_url(cx);
        // La consulta automática corre en silencio: el panel no parpadea cada minuto.
        let prev = if manual {
            let prev = std::mem::replace(&mut self.updater.state, UpdateState::Checking);
            cx.notify();
            prev
        } else {
            self.updater.state.clone()
        };
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { fetch_update(&base) })
                .await;
            this.update(cx, |this, cx| {
                this.updater.in_flight = false;
                this.updater.last_check = Some(chrono::Local::now());
                // Si mientras tanto el usuario aceptó actualizar, no lo pisamos.
                if matches!(
                    this.updater.state,
                    UpdateState::Downloading(_) | UpdateState::Installing(_) | UpdateState::Manual(..)
                ) {
                    return;
                }
                this.updater.state = match res {
                    Ok(Some(info)) => UpdateState::Available(info),
                    Ok(None) => UpdateState::UpToDate,
                    // Una consulta automática fallida no borra un aviso previo.
                    Err(e) if !manual => match prev {
                        UpdateState::Available(i) => UpdateState::Available(i),
                        _ => UpdateState::Failed(e),
                    },
                    Err(e) => UpdateState::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Solo se llama desde el botón "Actualizar y reiniciar" (consentimiento).
    pub fn start_update(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.updater.available().cloned() else { return };
        let base = self.base_url(cx);
        let progress = self.updater.progress.clone();
        progress.store(0, Ordering::Relaxed);
        self.updater.state = UpdateState::Downloading(info.clone());
        cx.notify();

        // Refresco de la barra de progreso mientras descarga.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            let downloading = this
                .update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.updater.state, UpdateState::Downloading(_))
                })
                .unwrap_or(false);
            if !downloading {
                break;
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let dl_info = info.clone();
            let res = cx
                .background_executor()
                .spawn(async move { download_verified(&base, &dl_info, &progress) })
                .await;
            let new_exe = match res {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.updater.state = UpdateState::Failed(e);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |this, cx| {
                this.updater.state = UpdateState::Installing(info.clone());
                cx.notify();
            })
            .ok();
            let path = new_exe.clone();
            let res = cx
                .background_executor()
                .spawn(async move { install_and_relaunch(&path) })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(()) => {
                    let _ = this.cfg.save(&this.config_path);
                    std::process::exit(0);
                }
                Err(InstallError::NoPermission) => {
                    reveal_in_explorer(&new_exe);
                    this.updater.state = UpdateState::Manual(info.clone(), new_exe.clone());
                    cx.notify();
                }
                Err(InstallError::Other(e)) => {
                    this.updater.state = UpdateState::Failed(e);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Botón del topbar: solo aparece cuando hay una versión nueva.
    pub fn render_update_badge(&mut self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let p = self.palette();
        let busy = matches!(
            self.updater.state,
            UpdateState::Downloading(_) | UpdateState::Installing(_)
        );
        let label = match &self.updater.state {
            UpdateState::Available(i) if !self.updater.dismissed => format!("Actualización disponible: v{}", i.version),
            UpdateState::Downloading(i) | UpdateState::Installing(i) => format!("Actualizando a v{}…", i.version),
            _ => return None,
        };
        Some(
            icon_btn("topbar-update", IconName::ArrowDown, label.clone(), busy, p)
                .relative()
                .tooltip(tip(label))
                .child(
                    div()
                        .absolute()
                        .top(px(7.))
                        .right(px(7.))
                        .size(px(7.))
                        .rounded_full()
                        .bg(rgb(p.success)),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page = crate::Page::Settings;
                    this.settings_section = "about";
                    cx.notify();
                })),
        )
    }

    /// Panel "Actualizaciones" de Ajustes → Acerca de.
    pub fn render_update_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let st = self.updater.state.clone();
        let checked = self
            .updater
            .last_check
            .map(|t| format!("Última consulta: {}", t.format("%d/%m %H:%M")))
            .unwrap_or_else(|| "Todavía no se consultó.".into());

        let (icon, color, status): (IconName, u32, String) = match &st {
            UpdateState::Idle => (IconName::Info, p.muted, format!("v{CURRENT_VERSION} · {checked}")),
            UpdateState::Checking => (IconName::RefreshCw, p.muted, "Buscando actualizaciones…".into()),
            UpdateState::UpToDate => (IconName::Check, p.success, format!("Tenés la última versión (v{CURRENT_VERSION}). {checked}")),
            UpdateState::Available(i) => (IconName::ArrowDown, p.accent, format!("Nueva versión v{} disponible (tenés v{CURRENT_VERSION}).", i.version)),
            UpdateState::Downloading(i) => (
                IconName::ArrowDown,
                p.accent,
                format!("Descargando v{}… {}%", i.version, self.updater.progress.load(Ordering::Relaxed)),
            ),
            UpdateState::Installing(i) => (IconName::ShieldCheck, p.success, format!("Verificado v{}. Instalando y reiniciando…", i.version)),
            UpdateState::Manual(i, path) => (
                IconName::ShieldCheck,
                p.warning,
                format!(
                    "v{} descargada y verificada, pero no tengo permiso para reemplazar el cliente. Cerralo y ejecutá: {}",
                    i.version,
                    path.display()
                ),
            ),
            UpdateState::Failed(e) => (IconName::X, p.danger, format!("No se pudo actualizar: {e}")),
        };

        let busy = matches!(st, UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Installing(_));
        let mut panel = crate::workspace_panel("Actualizaciones", p).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(Icon::new(icon).size(px(18.)).text_color(rgb(color)))
                .child(div().flex_1().text_sm().child(status))
                .when(!busy, |d| {
                    d.child(
                        icon_btn("upd-check", IconName::RefreshCw, "Buscar actualizaciones", false, p)
                            .tooltip(tip("Buscar actualizaciones"))
                            .on_click(cx.listener(|this, _, _, cx| this.check_updates(true, cx))),
                    )
                }),
        );

        if let UpdateState::Downloading(_) = st {
            let pct = self.updater.progress.load(Ordering::Relaxed).min(100) as f32;
            panel = panel.child(
                div()
                    .h(px(6.))
                    .w_full()
                    .rounded_full()
                    .bg(rgb(p.raised))
                    .child(div().h_full().rounded_full().bg(rgb(p.accent)).w(relative(pct / 100.))),
            );
        }

        if let UpdateState::Available(info) = &st {
            panel = panel
                .child(changelog_view(info, p))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            primary_btn("upd-install", Some(IconName::ArrowDown), format!("Actualizar a v{} y reiniciar", info.version), p)
                                .on_click(cx.listener(|this, _, _, cx| this.start_update(cx))),
                        )
                        .child(
                            ghost_btn("upd-later", Some(IconName::Clock), "Más tarde", p).on_click(cx.listener(|this, _, _, cx| {
                                this.updater.dismissed = true;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(Icon::new(IconName::ShieldCheck).size(px(14.)))
                        .child(format!(
                            "Firmada por AgentDesk · SHA-256 {}… · {:.1} MB. Se cierran las sesiones abiertas al reiniciar.",
                            &info.sha256[..12],
                            info.size_bytes as f64 / 1_048_576.0
                        )),
                );
        }
        panel
    }
}

fn changelog_view(info: &UpdateInfo, p: Palette) -> Div {
    let mut list = div().flex().flex_col().gap_2();
    let entries: Vec<_> = info
        .changelog
        .iter()
        .filter(|c| is_newer(&c.version, CURRENT_VERSION))
        .collect();
    if entries.is_empty() {
        return list.child(div().text_xs().text_color(rgb(p.muted)).child("Sin notas para esta versión."));
    }
    for c in entries {
        let mut block = div().flex().flex_col().gap_1().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().font_semibold().child(format!("v{}", c.version)))
                .when(!c.date.is_empty(), |d| d.child(div().text_xs().text_color(rgb(p.muted)).child(c.date.clone()))),
        );
        for item in &c.items {
            block = block.child(
                div()
                    .flex()
                    .gap_2()
                    .text_sm()
                    .child(div().text_color(rgb(p.accent)).child("•"))
                    .child(div().flex_1().child(item.clone())),
            );
        }
        list = list.child(block);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdesk_identity::{sha256_hex, DeviceIdentity};
    use std::net::TcpListener;

    /// Servidor HTTP mínimo: responde `routes` y cierra.
    fn serve(routes: Vec<(String, Vec<u8>)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in l.incoming().flatten() {
                let mut s = stream;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| ("200 OK", b.clone()))
                    .unwrap_or(("404 Not Found", Vec::new()));
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        base
    }

    fn signed(key: &DeviceIdentity, version: &str, exe: &[u8]) -> (UpdateInfo, Vec<u8>) {
        let filename = format!("agentdesk-desktop-v{version}-x64.exe");
        let info = UpdateInfo {
            product: "desktop".into(),
            version: version.into(),
            download_path: format!("/downloads/file/{filename}"),
            filename,
            sha256: sha256_hex(exe),
            size_bytes: exe.len() as u64,
            published_at: 0,
            changelog: vec![],
        };
        let m = SignedUpdateManifest::sign(&info, key).unwrap();
        (info, serde_json::to_vec(&m).unwrap())
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdesk-upd-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[::core::prelude::v1::test]
    fn end_to_end_download_verifies() {
        let key = DeviceIdentity::generate();
        let exe = vec![7u8; 700_000];
        let (info, manifest) = signed(&key, "9.9.9", &exe);
        let base = serve(vec![
            ("/api/v1/updates/desktop".into(), manifest),
            (info.download_path.clone(), exe.clone()),
        ]);
        let got = fetch_update_with(&base, &key.public_key_hex(), "0.1.7").unwrap().unwrap();
        assert_eq!(got.version, "9.9.9");
        // Misma versión o más vieja: nada que hacer (nunca downgrade).
        assert!(fetch_update_with(&base, &key.public_key_hex(), "9.9.9").unwrap().is_none());
        // Otra clave: rechazado.
        let other = DeviceIdentity::generate();
        assert!(fetch_update_with(&base, &other.public_key_hex(), "0.1.7").is_err());

        let dir = tmpdir("ok");
        let progress = AtomicU64::new(0);
        let path = download_verified_to(&dir, &base, &got, &progress).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), exe);
        assert_eq!(progress.load(Ordering::Relaxed), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[::core::prelude::v1::test]
    fn tampered_binary_is_rejected() {
        let key = DeviceIdentity::generate();
        let (info, _) = signed(&key, "9.9.9", &[1u8; 1000]);
        let base = serve(vec![(info.download_path.clone(), vec![2u8; 1000])]);
        let dir = tmpdir("bad");
        let err = download_verified_to(&dir, &base, &info, &AtomicU64::new(0)).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no debe quedar nada descargado");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[::core::prelude::v1::test]
    fn swap_replaces_and_keeps_old() {
        let dir = tmpdir("swap");
        let exe = dir.join("agentdesk.exe");
        let new = dir.join("new.exe");
        std::fs::write(&exe, b"viejo").unwrap();
        std::fs::write(&new, b"nuevo").unwrap();
        assert!(swap_in(&exe, &new).is_ok());
        assert_eq!(std::fs::read(&exe).unwrap(), b"nuevo");
        assert_eq!(std::fs::read(old_exe_path(&exe)).unwrap(), b"viejo");
        assert!(!new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
