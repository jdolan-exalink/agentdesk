use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use thiserror::Error;

const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const NOISE_MAX_CHUNK_PLAIN: usize = 60 * 1024;
const NOISE_MAX_PAYLOAD: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("cryptographic error: {0}")]
    Crypto(String),
    #[error("noise protocol error: {0}")]
    Noise(#[from] snow::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid framing")]
    InvalidFrame,
}

pub fn generate_high_entropy_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn generate_short_code(prefix: &str) -> String {
    let mut bytes = [0u8; 4];
    OsRng.fill_bytes(&mut bytes);
    let num = u32::from_be_bytes(bytes) % 900_000 + 100_000;
    format!("{prefix}-{num}")
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn hash_password(password: &str) -> Result<String, IdentityError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| IdentityError::Crypto(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, IdentityError> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| IdentityError::Crypto(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

// ---------------------------------------------------------------------------
// Device Ed25519 Identity + Windows DPAPI Persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct StoredIdentity {
    ed25519_seed_hex: String,
}

#[derive(Clone)]
pub struct DeviceIdentity {
    signing_key: SigningKey,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing_key.sign(message).to_bytes().to_vec()
    }

    pub fn verify_hex(
        pubkey_hex: &str,
        message: &[u8],
        signature_bytes: &[u8],
    ) -> Result<bool, IdentityError> {
        let pk_raw = hex::decode(pubkey_hex)
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let pk_arr: [u8; 32] = pk_raw
            .try_into()
            .map_err(|_| IdentityError::Crypto("invalid pubkey length".into()))?;
        let vk = VerifyingKey::from_bytes(&pk_arr)
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let sig_arr: [u8; 64] = signature_bytes
            .try_into()
            .map_err(|_| IdentityError::Crypto("invalid signature length".into()))?;
        let sig = Signature::from_bytes(&sig_arr);
        Ok(vk.verify(message, &sig).is_ok())
    }

    pub fn load_or_create(path: &Path) -> Result<Self, IdentityError> {
        if path.exists() {
            let raw = fs::read(path)?;
            let plain = dpapi_unprotect(&raw).unwrap_or(raw);
            let stored: StoredIdentity = serde_json::from_slice(&plain)?;
            let seed_bytes = hex::decode(stored.ed25519_seed_hex)
                .map_err(|e| IdentityError::Crypto(e.to_string()))?;
            let seed: [u8; 32] = seed_bytes
                .try_into()
                .map_err(|_| IdentityError::Crypto("invalid seed length".into()))?;
            return Ok(Self {
                signing_key: SigningKey::from_bytes(&seed),
            });
        }

        let ident = Self::generate();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let stored = StoredIdentity {
            ed25519_seed_hex: hex::encode(ident.signing_key.to_bytes()),
        };
        let plain = serde_json::to_vec(&stored)?;
        let protected = dpapi_protect(&plain).unwrap_or(plain);
        fs::write(path, protected)?;
        Ok(ident)
    }
}

#[cfg(windows)]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, IdentityError> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| IdentityError::Crypto(format!("CryptProtectData: {e}")))?;

        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let vec = slice.to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as _));
        Ok(vec)
    }
}

#[cfg(windows)]
fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>, IdentityError> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: cipher.len() as u32,
            pbData: cipher.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| IdentityError::Crypto(format!("CryptUnprotectData: {e}")))?;

        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let vec = slice.to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as _));
        Ok(vec)
    }
}

#[cfg(not(windows))]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, IdentityError> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>, IdentityError> {
    Ok(cipher.to_vec())
}

// ---------------------------------------------------------------------------
// Noise_XX E2EE Session Channel (with deterministic chunked framing)
// ---------------------------------------------------------------------------

pub struct NoiseHandshake {
    state: snow::HandshakeState,
}

impl NoiseHandshake {
    pub fn new_initiator() -> Result<Self, IdentityError> {
        let builder = snow::Builder::new(NOISE_PATTERN.parse()?);
        let kp = builder.generate_keypair()?;
        let state = builder.local_private_key(&kp.private).build_initiator()?;
        Ok(Self { state })
    }

    pub fn new_responder() -> Result<Self, IdentityError> {
        let builder = snow::Builder::new(NOISE_PATTERN.parse()?);
        let kp = builder.generate_keypair()?;
        let state = builder.local_private_key(&kp.private).build_responder()?;
        Ok(Self { state })
    }

    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; 4096];
        let n = self.state.write_message(payload, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn read_message(&mut self, msg: &[u8]) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; 4096];
        let n = self.state.read_message(msg, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    pub fn into_transport(self) -> Result<NoiseChannel, IdentityError> {
        Ok(NoiseChannel {
            transport: self.state.into_transport_mode()?,
        })
    }
}

pub struct NoiseChannel {
    transport: snow::TransportState,
}

impl NoiseChannel {
    /// Encrypts an arbitrarily sized plaintext payload into one or more
    /// length-prefixed Noise transport subframes so frames > 64 KiB work seamlessly.
    /// Wire format: `[u32 chunk_count][ (u32 cipher_len || ciphertext)... ]`
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, IdentityError> {
        if plaintext.len() > NOISE_MAX_PAYLOAD {
            return Err(IdentityError::InvalidFrame);
        }
        let chunks: Vec<&[u8]> = if plaintext.is_empty() {
            vec![&[]]
        } else {
            plaintext.chunks(NOISE_MAX_CHUNK_PLAIN).collect()
        };

        let mut out = Vec::with_capacity(plaintext.len() + chunks.len() * 24 + 4);
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());

        let mut tmp = vec![0u8; NOISE_MAX_CHUNK_PLAIN + 128];
        for chunk in chunks {
            let n = self.transport.write_message(chunk, &mut tmp)?;
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out.extend_from_slice(&tmp[..n]);
        }
        Ok(out)
    }

    pub fn decrypt(&mut self, packet: &[u8]) -> Result<Vec<u8>, IdentityError> {
        if packet.len() < 4 {
            return Err(IdentityError::InvalidFrame);
        }
        let chunk_count = u32::from_le_bytes(packet[0..4].try_into().unwrap()) as usize;
        if chunk_count == 0 || chunk_count > 512 {
            return Err(IdentityError::InvalidFrame);
        }

        let mut cursor = 4usize;
        let mut plain = Vec::new();
        let mut tmp = vec![0u8; NOISE_MAX_CHUNK_PLAIN + 128];

        for _ in 0..chunk_count {
            if cursor + 4 > packet.len() {
                return Err(IdentityError::InvalidFrame);
            }
            let clen =
                u32::from_le_bytes(packet[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            if clen == 0 || cursor + clen > packet.len() || clen > tmp.len() {
                return Err(IdentityError::InvalidFrame);
            }
            let n = self
                .transport
                .read_message(&packet[cursor..cursor + clen], &mut tmp)?;
            cursor += clen;
            if plain.len() + n > NOISE_MAX_PAYLOAD {
                return Err(IdentityError::InvalidFrame);
            }
            plain.extend_from_slice(&tmp[..n]);
        }
        if cursor != packet.len() {
            return Err(IdentityError::InvalidFrame);
        }
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_roundtrip() {
        let h = hash_password("S3cur3-AgentDesk!").unwrap();
        assert!(verify_password("S3cur3-AgentDesk!", &h).unwrap());
        assert!(!verify_password("wrong", &h).unwrap());
    }

    #[test]
    fn ed25519_sign_and_verify() {
        let id = DeviceIdentity::generate();
        let sig = id.sign(b"agentdesk-hello");
        assert!(DeviceIdentity::verify_hex(&id.public_key_hex(), b"agentdesk-hello", &sig).unwrap());
        assert!(!DeviceIdentity::verify_hex(&id.public_key_hex(), b"tampered", &sig).unwrap());
    }

    #[test]
    fn noise_xx_multi_chunk_e2ee_and_tamper_rejection() {
        let mut init = NoiseHandshake::new_initiator().unwrap();
        let mut resp = NoiseHandshake::new_responder().unwrap();

        let m1 = init.write_message(&[]).unwrap();
        resp.read_message(&m1).unwrap();
        let m2 = resp.write_message(&[]).unwrap();
        init.read_message(&m2).unwrap();
        let m3 = init.write_message(&[]).unwrap();
        resp.read_message(&m3).unwrap();

        assert!(init.is_finished() && resp.is_finished());
        let mut tx = init.into_transport().unwrap();
        let mut rx = resp.into_transport().unwrap();

        // 250 KB payload spans multiple 60 KB Noise subframes
        let big: Vec<u8> = (0..250_000).map(|i| (i % 251) as u8).collect();
        let mut cipher = tx.encrypt(&big).unwrap();
        let plain = rx.decrypt(&cipher).unwrap();
        assert_eq!(plain, big);

        // Tamper 1 byte -> must fail AEAD authentication
        let last = cipher.len() - 1;
        cipher[last] ^= 0xFF;
        assert!(rx.decrypt(&cipher).is_err());
    }
}
