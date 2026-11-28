//! Firma el manifiesto de actualización del cliente.
//!
//!   agentdesk-release-sign pubkey
//!   agentdesk-release-sign sign --exe <agentdesk-desktop-vX.Y.Z-x64.exe> --version X.Y.Z \
//!        --changelog CHANGELOG.md --out <dir>\agentdesk-desktop-update.json
//!
//! La clave privada vive en `%LOCALAPPDATA%\AgentDesk\release-signing.key`
//! (DPAPI de este usuario; se puede cambiar con AGENTDESK_RELEASE_KEY). Si se
//! pierde, los clientes instalados no aceptan más actualizaciones automáticas
//! hasta instalar a mano un cliente con la clave nueva: hacé un respaldo.

use agentdesk_identity::release::{parse_changelog_md, SignedUpdateManifest, UpdateInfo};
use agentdesk_identity::{sha256_hex, DeviceIdentity};
use std::path::PathBuf;

fn key_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTDESK_RELEASE_KEY") {
        return PathBuf::from(p);
    }
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("AgentDesk").join("release-signing.key")
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let key_file = key_path();
    match args.get(1).map(String::as_str) {
        Some("pubkey") => {
            let key = DeviceIdentity::load_or_create(&key_file)?;
            println!("{}", key.public_key_hex());
            eprintln!("clave: {}", key_file.display());
        }
        Some("sign") => {
            let exe = PathBuf::from(arg(&args, "--exe").ok_or("falta --exe")?);
            let version = arg(&args, "--version").ok_or("falta --version")?;
            let out = PathBuf::from(arg(&args, "--out").ok_or("falta --out")?);
            let product = arg(&args, "--product").unwrap_or_else(|| "desktop".into());
            if !key_file.exists() {
                return Err(format!("no existe la clave {}; corré `pubkey` primero", key_file.display()).into());
            }
            let key = DeviceIdentity::load_or_create(&key_file)?;
            let bytes = std::fs::read(&exe)?;
            let filename = exe
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("nombre de archivo inválido")?
                .to_string();
            let mut changelog = match arg(&args, "--changelog") {
                Some(p) => parse_changelog_md(&std::fs::read_to_string(p)?),
                None => Vec::new(),
            };
            changelog.truncate(15);
            if !changelog.iter().any(|c| c.version == version) {
                eprintln!("aviso: el changelog no tiene sección ## {version}");
            }
            let info = UpdateInfo {
                product,
                version,
                download_path: format!("/downloads/file/{filename}"),
                filename,
                sha256: sha256_hex(&bytes),
                size_bytes: bytes.len() as u64,
                published_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as i64,
                changelog,
            };
            let manifest = SignedUpdateManifest::sign(&info, &key)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&manifest)?)?;
            println!("{} v{} sha256={} -> {}", info.filename, info.version, info.sha256, out.display());
        }
        _ => {
            eprintln!("uso: agentdesk-release-sign pubkey | sign --exe <exe> --version X.Y.Z [--changelog CHANGELOG.md] --out <json>");
            std::process::exit(2);
        }
    }
    Ok(())
}
