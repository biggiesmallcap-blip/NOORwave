//! Symmetric token encryption for service credentials.
//!
//! NOTE: Today only the new Last.fm session key is routed through this helper.
//! TIDAL/Spotify tokens still live as plaintext UTF-8 JSON in the historically
//! mis-named `service_auth.access_token_enc` BLOB column. Migrating those to
//! this helper is intentional follow-up work — see `MIGRATION` note below.
//!
//! Master-key lifecycle:
//! - On first boot we generate 32 random bytes and write them to
//!   `<noor.db dir>/.noor_secret`.
//! - On subsequent boots we read them back.
//! - On Unix the file is chmod'd to 0o600. On Windows the user-profile
//!   directory's default ACL already restricts read access to the owning user;
//!   we do not invoke `icacls` to avoid the platform-dep blast radius.
//! - There is no key rotation. Wiping `.noor_secret` invalidates every
//!   encrypted blob on disk; users will have to reconnect affected services.
//!
//! MIGRATION (follow-up): swap `services/tidal/auth.rs` and `services/spotify`
//! token reads/writes to `MasterKey::encrypt`/`decrypt` and rename the column.

use aes_gcm::{
    AeadCore, Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng},
};
use anyhow::{Context, Result, anyhow};
use rand::RngCore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SECRET_FILENAME: &str = ".noor_secret";
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct MasterKey {
    cipher: Arc<Aes256Gcm>,
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never reveal the key material in logs / debug dumps.
        f.write_str("MasterKey(<redacted>)")
    }
}

impl MasterKey {
    /// Read the master key from `<dir>/.noor_secret`, generating it on first
    /// run. The file is owner-only on Unix.
    pub fn load_or_generate(dir: &Path) -> Result<Self> {
        if !dir.exists() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("create dir {}", dir.display()))?;
        }
        let path = dir.join(SECRET_FILENAME);
        let bytes = if path.exists() {
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            if bytes.len() != 32 {
                return Err(anyhow!(
                    "{} is {} bytes, expected 32 — refusing to use a malformed key",
                    path.display(),
                    bytes.len()
                ));
            }
            bytes
        } else {
            let mut buf = vec![0u8; 32];
            OsRng.fill_bytes(&mut buf);
            write_secret_file(&path, &buf)?;
            tracing::info!(
                "Generated new master key at {} (owner-only on Unix)",
                path.display()
            );
            buf
        };
        let key = Key::<Aes256Gcm>::from_slice(&bytes);
        Ok(Self {
            cipher: Arc::new(Aes256Gcm::new(key)),
        })
    }

    /// Encrypt arbitrary bytes. Output layout: `nonce(12) || ciphertext+tag`.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| anyhow!("AES-GCM encrypt failed: {e}"))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt bytes produced by `encrypt`. Errors on tampering or wrong key.
    pub fn decrypt(&self, blob: &[u8]) -> Result<Vec<u8>> {
        if blob.len() <= NONCE_LEN {
            return Err(anyhow!("ciphertext too short"));
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce, ct)
            .map_err(|e| anyhow!("AES-GCM decrypt failed: {e}"))
    }
}

#[cfg(unix)]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    use std::io::Write;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("create {}", path.display()))
}

/// Resolve the directory that should hold `.noor_secret` — the same dir as
/// the SQLite db, so it travels with the install.
pub fn secret_dir_for_db(db_path: &str) -> PathBuf {
    Path::new(db_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempdir();
        let key = MasterKey::load_or_generate(&dir).unwrap();
        let plain = b"hunter2-very-long-session-key";
        let blob = key.encrypt(plain).unwrap();
        assert_ne!(blob, plain, "ciphertext must differ from plaintext");
        let back = key.decrypt(&blob).unwrap();
        assert_eq!(back, plain);
    }

    #[test]
    fn key_persists_across_loads() {
        let dir = tempdir();
        let k1 = MasterKey::load_or_generate(&dir).unwrap();
        let k2 = MasterKey::load_or_generate(&dir).unwrap();
        let blob = k1.encrypt(b"x").unwrap();
        assert_eq!(k2.decrypt(&blob).unwrap(), b"x");
    }

    #[test]
    fn ciphertext_does_not_leak_plaintext() {
        let dir = tempdir();
        let key = MasterKey::load_or_generate(&dir).unwrap();
        let secret = b"super-secret-session-key-7c4f";
        let blob = key.encrypt(secret).unwrap();
        // Hard requirement from the plan: the raw stored blob must not
        // contain the plaintext anywhere as a substring.
        assert!(
            !blob.windows(secret.len()).any(|w| w == secret),
            "raw ciphertext leaked plaintext bytes"
        );
    }

    #[test]
    fn tampering_is_detected() {
        let dir = tempdir();
        let key = MasterKey::load_or_generate(&dir).unwrap();
        let mut blob = key.encrypt(b"hello").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(key.decrypt(&blob).is_err());
    }

    fn tempdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("noor-crypto-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
