//! Host-side session diagnostics: where the host process runs (session, desktop, elevation),
//! what the viewer sends and what Windows lets through. Written to
//! `%LOCALAPPDATA%\AgentDesk\logs\host-session.log` and summarized for the viewer when
//! something is off.

use std::io::Write;

/// `%LOCALAPPDATA%\AgentDesk\logs`.
pub fn log_dir_path() -> std::path::PathBuf {
    log_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir)
}

fn log_path() -> std::path::PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("AgentDesk").join("logs").join("host-session.log")
}

/// Appends one timestamped line to the host session log (best effort, capped at ~2 MB).
pub fn log(line: &str) {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writeln!(f, "{ts} {line}");
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostEnvironment {
    pub process_session: u32,
    pub console_session: u32,
    pub elevated: Option<bool>,
    pub thread_desktop: String,
    /// `None` when the input desktop cannot even be opened (secure desktop, other session).
    pub input_desktop: Option<String>,
}

impl HostEnvironment {
    /// Human-readable problems that stop remote input/whiteboard, if any.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.process_session != self.console_session {
            out.push(format!(
                "AgentDesk corre en la sesión {} pero la pantalla física es la sesión {} (¿quedó abierto desde Escritorio remoto o como servicio?)",
                self.process_session, self.console_session
            ));
        }
        match &self.input_desktop {
            None => out.push("Windows no deja abrir el escritorio de entrada (pantalla bloqueada, UAC o sesión ajena)".into()),
            Some(d) if !d.eq_ignore_ascii_case(&self.thread_desktop) => out.push(format!(
                "el escritorio activo es «{d}» y AgentDesk está en «{}»",
                self.thread_desktop
            )),
            _ => {}
        }
        out
    }

    pub fn summary(&self) -> String {
        format!(
            "proc_session={} console_session={} elevated={:?} thread_desktop={} input_desktop={:?}",
            self.process_session, self.console_session, self.elevated, self.thread_desktop, self.input_desktop
        )
    }
}

#[cfg(windows)]
pub fn host_environment() -> HostEnvironment {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, HDESK,
        DESKTOP_ACCESS_FLAGS, UOI_NAME,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcessToken,
    };

    unsafe fn desk_name(h: HDESK) -> String {
        let mut buf = [0u16; 128];
        let mut needed = 0u32;
        if GetUserObjectInformationW(
            HANDLE(h.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
        .is_ok()
        {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..n])
        } else {
            "?".into()
        }
    }

    unsafe {
        let mut env = HostEnvironment::default();
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut env.process_session);
        env.console_session = WTSGetActiveConsoleSessionId();
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_ok() {
            let mut el = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            if GetTokenInformation(
                tok,
                TokenElevation,
                Some(&mut el as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok()
            {
                env.elevated = Some(el.TokenIsElevated != 0);
            }
            let _ = CloseHandle(tok);
        }
        env.thread_desktop = GetThreadDesktop(GetCurrentThreadId())
            .map(|h| desk_name(h))
            .unwrap_or_else(|_| "?".into());
        // DESKTOP_READOBJECTS is enough to read the name.
        if let Ok(h) = OpenInputDesktop(Default::default(), false, DESKTOP_ACCESS_FLAGS(0x0001)) {
            env.input_desktop = Some(desk_name(h));
            let _ = CloseDesktop(h);
        }
        env
    }
}

#[cfg(not(windows))]
pub fn host_environment() -> HostEnvironment {
    HostEnvironment { thread_desktop: "Default".into(), input_desktop: Some("Default".into()), ..Default::default() }
}
