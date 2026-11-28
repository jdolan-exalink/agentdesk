//! Manifiesto firmado de actualizaciones del cliente.
//!
//! El manifiesto lo genera y firma `agentdesk-release-sign` en la PC de
//! compilación (clave ed25519 protegida con DPAPI, nunca sale de esa PC). El
//! servidor solo lo sirve tal cual: el cliente verifica la firma contra la clave
//! pública compilada en el binario, así que un servidor comprometido no puede
//! ofrecer un ejecutable propio.

use crate::{DeviceIdentity, IdentityError};
use serde::{Deserialize, Serialize};

/// Separador de dominio: una firma de actualización no sirve para otra cosa.
pub const UPDATE_SIGNING_DOMAIN: &[u8] = b"agentdesk-update-v1\n";

/// Lo que publica el servidor en `/api/v1/updates/{producto}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedUpdateManifest {
    /// JSON de [`UpdateInfo`], firmado byte a byte.
    pub payload: String,
    /// Firma ed25519 en hex sobre `UPDATE_SIGNING_DOMAIN || payload`.
    pub signature: String,
    /// Clave pública que firmó (solo informativa; el cliente usa la suya).
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// "desktop" (cliente de técnicos).
    pub product: String,
    pub version: String,
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// Ruta relativa al servidor, siempre bajo `/downloads/file/`.
    pub download_path: String,
    /// Unix seconds.
    pub published_at: i64,
    #[serde(default)]
    pub changelog: Vec<ChangelogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub version: String,
    #[serde(default)]
    pub date: String,
    pub items: Vec<String>,
}

impl SignedUpdateManifest {
    pub fn sign(info: &UpdateInfo, key: &DeviceIdentity) -> Result<Self, IdentityError> {
        let payload = serde_json::to_string(info)?;
        let mut msg = UPDATE_SIGNING_DOMAIN.to_vec();
        msg.extend_from_slice(payload.as_bytes());
        Ok(Self {
            payload,
            signature: hex::encode(key.sign(&msg)),
            key: key.public_key_hex(),
        })
    }

    /// Verifica la firma con `trusted_pubkey_hex` y devuelve el contenido.
    pub fn verify(&self, trusted_pubkey_hex: &str) -> Result<UpdateInfo, IdentityError> {
        let sig = hex::decode(self.signature.trim())
            .map_err(|e| IdentityError::Crypto(format!("firma inválida: {e}")))?;
        let mut msg = UPDATE_SIGNING_DOMAIN.to_vec();
        msg.extend_from_slice(self.payload.as_bytes());
        if !DeviceIdentity::verify_hex(trusted_pubkey_hex, &msg, &sig)? {
            return Err(IdentityError::Crypto("firma de actualización no válida".into()));
        }
        let info: UpdateInfo = serde_json::from_str(&self.payload)?;
        if !info.download_path.starts_with("/downloads/file/")
            || info.download_path.contains("..")
            || info.sha256.len() != 64
        {
            return Err(IdentityError::Crypto("manifiesto mal formado".into()));
        }
        Ok(info)
    }
}

/// "0.1.8" -> (0, 1, 8). Ignora sufijos tipo "-beta".
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

/// `candidate` es estrictamente más nueva que `current` (nunca downgrade).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Lee un CHANGELOG.md con secciones `## 0.1.8 — 2026-11-28` y viñetas `- ...`.
pub fn parse_changelog_md(text: &str) -> Vec<ChangelogEntry> {
    let mut out: Vec<ChangelogEntry> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(head) = l.strip_prefix("## ") {
            let head = head.trim();
            let mut parts = head.splitn(2, |c: char| c.is_whitespace());
            let ver = parts.next().unwrap_or("").trim_start_matches('v').to_string();
            if parse_version(&ver).is_none() {
                continue;
            }
            let date = parts
                .next()
                .unwrap_or("")
                .trim()
                .trim_start_matches(['—', '-', '–', '·'])
                .trim()
                .to_string();
            out.push(ChangelogEntry { version: ver, date, items: Vec::new() });
        } else if let Some(item) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
            if let Some(cur) = out.last_mut() {
                cur.items.push(item.trim().to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.1.8", "0.1.7"));
        assert!(is_newer("0.2.0", "0.1.17"));
        assert!(!is_newer("0.1.7", "0.1.7"));
        assert!(!is_newer("0.1.6", "0.1.7"));
        assert!(!is_newer("basura", "0.1.7"));
    }

    #[test]
    fn sign_and_verify() {
        let key = DeviceIdentity::generate();
        let info = UpdateInfo {
            product: "desktop".into(),
            version: "0.1.8".into(),
            filename: "agentdesk-desktop-v0.1.8-x64.exe".into(),
            sha256: "a".repeat(64),
            size_bytes: 10,
            download_path: "/downloads/file/agentdesk-desktop-v0.1.8-x64.exe".into(),
            published_at: 0,
            changelog: vec![],
        };
        let m = SignedUpdateManifest::sign(&info, &key).unwrap();
        assert_eq!(m.verify(&key.public_key_hex()).unwrap().version, "0.1.8");
        let other = DeviceIdentity::generate();
        assert!(m.verify(&other.public_key_hex()).is_err());
        let mut tampered = m.clone();
        tampered.payload = tampered.payload.replace("0.1.8", "0.1.9");
        assert!(tampered.verify(&key.public_key_hex()).is_err());
    }

    #[test]
    fn changelog() {
        let md = "# Cambios\n\n## 0.1.8 — 2026-11-28\n- Uno\n- Dos\n\n## 0.1.7\n* Tres\n";
        let c = parse_changelog_md(md);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].date, "2026-11-28");
        assert_eq!(c[0].items, vec!["Uno", "Dos"]);
        assert_eq!(c[1].items, vec!["Tres"]);
    }
}
