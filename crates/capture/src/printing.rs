use agentdesk_protocol::PrinterInfo;
use std::path::Path;

#[cfg(windows)]
mod win_print {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Graphics::Printing::{
        EnumPrintersW, GetDefaultPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
        PRINTER_INFO_2W,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    unsafe fn pcwstr_to_string(p: PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *p.0.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len))
    }

    pub fn enumerate() -> Vec<PrinterInfo> {
        let mut default_buf = vec![0u16; 512];
        let mut default_len = default_buf.len() as u32;
        let default_name = unsafe {
            if GetDefaultPrinterW(PWSTR(default_buf.as_mut_ptr()), &mut default_len).as_bool()
                && default_len > 1
            {
                String::from_utf16_lossy(&default_buf[..(default_len as usize - 1)])
            } else {
                String::new()
            }
        };

        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let mut needed = 0u32;
        let mut returned = 0u32;
        unsafe {
            let _ = EnumPrintersW(flags, PCWSTR::null(), 2, None, &mut needed, &mut returned);
            if needed == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u8; needed as usize];
            if EnumPrintersW(
                flags,
                PCWSTR::null(),
                2,
                Some(buf.as_mut_slice()),
                &mut needed,
                &mut returned,
            )
            .is_err()
            {
                return Vec::new();
            }

            let infos = std::slice::from_raw_parts(
                buf.as_ptr() as *const PRINTER_INFO_2W,
                returned as usize,
            );
            let mut out = Vec::with_capacity(infos.len());
            for info in infos {
                let name = pcwstr_to_string(info.pPrinterName);
                if name.is_empty() {
                    continue;
                }
                let status = if info.Status == 0 {
                    "Lista".to_string()
                } else if (info.Status & 0x0000_0400) != 0 {
                    "Imprimiendo".to_string()
                } else if (info.Status & 0x0000_1000) != 0 {
                    "Sin conexión".to_string()
                } else {
                    format!("Estado {}", info.Status)
                };
                out.push(PrinterInfo {
                    is_default: !default_name.is_empty() && name.eq_ignore_ascii_case(&default_name),
                    name,
                    status,
                });
            }
            out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
            out
        }
    }

    pub fn print_file(path: &Path, printer_name: Option<&str>) -> Result<String, String> {
        if !path.exists() {
            return Err("El archivo a imprimir no existe".into());
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let target_label = printer_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("predeterminada");

        if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp") {
            let mut cmd = std::process::Command::new("mspaint.exe");
            if let Some(p) = printer_name.filter(|s| !s.trim().is_empty()) {
                cmd.arg("/pt").arg(path).arg(p);
            } else {
                cmd.arg("/p").arg(path);
            }
            match cmd.spawn() {
                Ok(_) => return Ok(format!("Imagen enviada a impresora ({target_label})")),
                Err(e) => return Err(format!("No se pudo lanzar impresión de imagen: {e}")),
            }
        }

        let file_w = to_wide(&path.to_string_lossy());
        if let Some(p) = printer_name.filter(|s| !s.trim().is_empty()) {
            let verb_w = to_wide("printto");
            let params_w = to_wide(&format!("\"{p}\""));
            let res = unsafe {
                ShellExecuteW(
                    None,
                    PCWSTR(verb_w.as_ptr()),
                    PCWSTR(file_w.as_ptr()),
                    PCWSTR(params_w.as_ptr()),
                    PCWSTR::null(),
                    SW_HIDE,
                )
            };
            if (res.0 as isize) > 32 {
                Ok(format!("Trabajo enviado a impresora {p}"))
            } else {
                Err(format!(
                    "Windows no pudo imprimir el documento en {p} (código {}).",
                    res.0 as isize
                ))
            }
        } else {
            let verb_w = to_wide("print");
            let res = unsafe {
                ShellExecuteW(
                    None,
                    PCWSTR(verb_w.as_ptr()),
                    PCWSTR(file_w.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    SW_HIDE,
                )
            };
            if (res.0 as isize) > 32 {
                Ok("Trabajo enviado a la impresora predeterminada".into())
            } else {
                Err(format!(
                    "Windows no pudo imprimir el documento (código {}).",
                    res.0 as isize
                ))
            }
        }
    }
}

pub fn enumerate_printers() -> Vec<PrinterInfo> {
    #[cfg(windows)]
    {
        win_print::enumerate()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn print_file_to_printer(path: &Path, printer_name: Option<&str>) -> Result<String, String> {
    #[cfg(windows)]
    {
        win_print::print_file(path, printer_name)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, printer_name);
        Err("La impresión remota solo está soportada en Windows".into())
    }
}
