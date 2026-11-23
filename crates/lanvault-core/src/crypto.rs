//! Cryptographic primitives for LanVault zero-knowledge E2EE and peer trust.
//!
//! - Device identity & mutual authentication: Ed25519 (`ed25519-dalek` v2)
//! - Ephemeral session key agreement: X25519 (`x25519-dalek` v2) + BLAKE3 KDF
//! - Zero-knowledge chunk & channel encryption: XChaCha20-Poly1305 AEAD
//! - Sensitive key material implements `Zeroize` + `ZeroizeOnDrop`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use uuid::Uuid;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{LanVaultError, Result};

/// Length of an XChaCha20-Poly1305 192-bit nonce in bytes.
pub const XCHACHA_NONCE_LEN: usize = 24;
/// Length of a Poly1305 authentication tag in bytes.
pub const POLY1305_TAG_LEN: usize = 16;
/// Length of a 256-bit symmetric key in bytes.
pub const SYMMETRIC_KEY_LEN: usize = 32;
/// Length of an Ed25519 public or secret seed in bytes.
pub const ED25519_KEY_LEN: usize = 32;
/// Length of an Ed25519 signature in bytes.
pub const ED25519_SIG_LEN: usize = 64;

const SESSION_KDF_CONTEXT: &str = "lanvault.session.xchacha20poly1305.v1";
const VAULT_CHUNK_KDF_CONTEXT: &str = "lanvault.vault.chunk.key.v1";
const CHUNK_NONCE_KDF_CONTEXT: &str = "lanvault.chunk.nonce.v1";

/// 256-bit symmetric key that is securely zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymmetricKey([u8; SYMMETRIC_KEY_LEN]);

impl SymmetricKey {
    /// Generates a cryptographically secure random 256-bit key.
    pub fn generate() -> Self {
        let mut bytes = [0u8; SYMMETRIC_KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Constructs a key from an exact 32-byte array.
    pub fn from_bytes(bytes: [u8; SYMMETRIC_KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Parses a key from an untrusted byte slice at a system boundary.
    pub fn from_slice(slice: &[u8]) -> Result<Self> {
        let arr: [u8; SYMMETRIC_KEY_LEN] =
            slice.try_into().map_err(|_| LanVaultError::InvalidKeyLength {
                expected: SYMMETRIC_KEY_LEN,
                actual: slice.len(),
            })?;
        Ok(Self(arr))
    }

    /// Exposes the raw 32-byte key material.
    pub fn as_bytes(&self) -> &[u8; SYMMETRIC_KEY_LEN] {
        &self.0
    }

    /// Derives a vault-scoped chunk encryption key from a master key using BLAKE3 KDF.
    pub fn derive_vault_chunk_key(&self, vault_id: &str) -> Self {
        let mut input = Zeroizing::new(Vec::with_capacity(SYMMETRIC_KEY_LEN + vault_id.len()));
        input.extend_from_slice(&self.0);
        input.extend_from_slice(vault_id.as_bytes());
        let derived = blake3::derive_key(VAULT_CHUNK_KDF_CONTEXT, &input);
        Self(derived)
    }
}

/// Device long-term Ed25519 identity keypair.
pub struct DeviceIdentity {
    signing_key: SigningKey,
}

impl DeviceIdentity {
    /// Generates a new random Ed25519 device identity.
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; ED25519_KEY_LEN]);
        OsRng.fill_bytes(seed.as_mut());
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Loads a persisted 32-byte Ed25519 secret seed from `path`, or generates and atomically
    /// persists a new identity with restrictive permissions (`0600` on Unix) if absent.
    pub fn load_or_generate(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let seed = Zeroizing::new(bytes);
                Self::from_seed(&seed)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let identity = Self::generate();
                let seed = identity.seed_bytes();

                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                fs::create_dir_all(parent)?;

                let tmp_path = parent.join(format!(".device.key.{}.tmp", Uuid::new_v4()));
                write_secret_file(&tmp_path, seed.as_ref())?;
                fs::rename(&tmp_path, path)?;

                Ok(identity)
            }
            Err(err) => Err(LanVaultError::Io(err)),
        }
    }

    /// Restores a device identity from a 32-byte secret seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self> {
        let arr: [u8; ED25519_KEY_LEN] =
            seed.try_into().map_err(|_| LanVaultError::InvalidKeyLength {
                expected: ED25519_KEY_LEN,
                actual: seed.len(),
            })?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&arr),
        })
    }

    /// Exports the 32-byte secret seed wrapped in `Zeroizing`.
    pub fn seed_bytes(&self) -> Zeroizing<[u8; ED25519_KEY_LEN]> {
        Zeroizing::new(self.signing_key.to_bytes())
    }

    /// Returns the 32-byte Ed25519 public key.
    pub fn public_key_bytes(&self) -> [u8; ED25519_KEY_LEN] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Computes a short deterministic hex fingerprint of the public key (first 16 bytes of BLAKE3).
    pub fn fingerprint_hex(&self) -> String {
        public_key_fingerprint(&self.public_key_bytes())
    }

    /// Signs arbitrary message bytes with Ed25519.
    pub fn sign(&self, message: &[u8]) -> [u8; ED25519_SIG_LEN] {
        self.signing_key.sign(message).to_bytes()
    }
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Computes the 32-char hex fingerprint (16 bytes of BLAKE3) for an Ed25519 public key.
pub fn public_key_fingerprint(pubkey: &[u8; ED25519_KEY_LEN]) -> String {
    let hash = blake3::hash(pubkey);
    hex::encode(&hash.as_bytes()[..16])
}

/// Constant-time byte slice equality comparison.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Verifies an Ed25519 signature strictly against a peer's raw 32-byte public key.
pub fn verify_ed25519(pubkey_bytes: &[u8], message: &[u8], signature_bytes: &[u8]) -> Result<()> {
    let pk_arr: [u8; ED25519_KEY_LEN] =
        pubkey_bytes
            .try_into()
            .map_err(|_| LanVaultError::InvalidKeyLength {
                expected: ED25519_KEY_LEN,
                actual: pubkey_bytes.len(),
            })?;
    let sig_arr: [u8; ED25519_SIG_LEN] =
        signature_bytes
            .try_into()
            .map_err(|_| LanVaultError::InvalidKeyLength {
                expected: ED25519_SIG_LEN,
                actual: signature_bytes.len(),
            })?;

    let verifying_key =
        VerifyingKey::from_bytes(&pk_arr).map_err(|e| LanVaultError::Crypto(e.to_string()))?;
    let signature = Signature::from_bytes(&sig_arr);
    verifying_key
        .verify_strict(message, &signature)
        .map_err(|_| LanVaultError::InvalidSignature)
}

/// X25519 key-exchange handle for establishing an ephemeral session key.
pub struct SessionKeyExchange {
    secret: StaticSecret,
    public: X25519PublicKey,
}

impl SessionKeyExchange {
    /// Generates a fresh X25519 keypair using `OsRng`.
    pub fn new() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = X25519PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Returns the 32-byte X25519 public key to send to the peer.
    pub fn public_key_bytes(&self) -> [u8; SYMMETRIC_KEY_LEN] {
        self.public.to_bytes()
    }

    /// Completes Diffie-Hellman agreement with the peer's 32-byte X25519 public key
    /// and derives a 256-bit XChaCha20-Poly1305 session key bound to `transcript_context`.
    pub fn agree(self, peer_pubkey: &[u8], transcript_context: &[u8]) -> Result<SymmetricKey> {
        let peer_arr: [u8; SYMMETRIC_KEY_LEN] =
            peer_pubkey
                .try_into()
                .map_err(|_| LanVaultError::InvalidKeyLength {
                    expected: SYMMETRIC_KEY_LEN,
                    actual: peer_pubkey.len(),
                })?;
        let peer_pk = X25519PublicKey::from(peer_arr);
        let shared = self.secret.diffie_hellman(&peer_pk);

        // Reject all-zero low-order shared secrets.
        if shared.as_bytes().iter().all(|&b| b == 0) {
            return Err(LanVaultError::Crypto(
                "X25519 shared secret is all-zero (low-order public key)".to_string(),
            ));
        }

        let mut ikm =
            Zeroizing::new(Vec::with_capacity(SYMMETRIC_KEY_LEN + transcript_context.len()));
        ikm.extend_from_slice(shared.as_bytes());
        ikm.extend_from_slice(transcript_context);
        let derived = blake3::derive_key(SESSION_KDF_CONTEXT, &ikm);
        Ok(SymmetricKey(derived))
    }
}

impl Default for SessionKeyExchange {
    fn default() -> Self {
        Self::new()
    }
}

/// Encrypts `plaintext` with XChaCha20-Poly1305 AEAD and optional `aad` using a random 192-bit nonce.
///
/// Wire format: `[24-byte random nonce || ciphertext || 16-byte Poly1305 tag]`.
pub fn encrypt_aead(key: &SymmetricKey, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let mut nonce_bytes = [0u8; XCHACHA_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    encrypt_aead_with_nonce(key, &nonce_bytes, plaintext, aad)
}

/// Encrypts `plaintext` with XChaCha20-Poly1305 AEAD using a deterministic SIV-like 192-bit nonce
/// derived from `(key, aad, plaintext)` via keyed BLAKE3.
///
/// Identical `(key, plaintext, aad)` inputs always produce identical ciphertext and stored BLAKE3
/// hash, enabling zero-knowledge content-addressed deduplication across chunks.
///
/// Wire format: `[24-byte deterministic nonce || ciphertext || 16-byte Poly1305 tag]`.
pub fn encrypt_aead_deterministic(
    key: &SymmetricKey,
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let nonce_key = Zeroizing::new(blake3::derive_key(CHUNK_NONCE_KDF_CONTEXT, key.as_bytes()));
    let mut hasher = blake3::Hasher::new_keyed(&nonce_key);
    hasher.update(&(aad.len() as u64).to_le_bytes());
    hasher.update(aad);
    hasher.update(plaintext);
    let digest = hasher.finalize();

    let mut nonce_bytes = [0u8; XCHACHA_NONCE_LEN];
    nonce_bytes.copy_from_slice(&digest.as_bytes()[..XCHACHA_NONCE_LEN]);
    encrypt_aead_with_nonce(key, &nonce_bytes, plaintext, aad)
}

fn encrypt_aead_with_nonce(
    key: &SymmetricKey,
    nonce_bytes: &[u8; XCHACHA_NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let nonce = XNonce::from_slice(nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad })
        .map_err(|e| LanVaultError::Crypto(format!("AEAD encryption failed: {e}")))?;

    let mut out = Vec::with_capacity(XCHACHA_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts an XChaCha20-Poly1305 wire buffer (`[24-byte nonce || ciphertext || 16-byte tag]`).
pub fn decrypt_aead(key: &SymmetricKey, wire: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if wire.len() < XCHACHA_NONCE_LEN + POLY1305_TAG_LEN {
        return Err(LanVaultError::Crypto(format!(
            "ciphertext too short: {} bytes",
            wire.len()
        )));
    }
    let (nonce_bytes, ciphertext) = wire.split_at(XCHACHA_NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let nonce = XNonce::from_slice(nonce_bytes);

    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| {
            LanVaultError::Crypto("AEAD authentication failed (wrong key or tampered data)".into())
        })
}

/// Computes a keyed BLAKE3 MAC for 6-digit pairing PIN verification.
pub fn compute_pin_mac(pin: &str, nonce: &[u8], device_id: &str) -> [u8; 32] {
    let pin_key = blake3::derive_key("lanvault.pairing.pin.v1", pin.as_bytes());
    let mut hasher = blake3::Hasher::new_keyed(&pin_key);
    hasher.update(nonce);
    hasher.update(device_id.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Constant-time verification of a pairing PIN MAC.
pub fn verify_pin_mac(pin: &str, nonce: &[u8], device_id: &str, expected_mac: &[u8]) -> bool {
    if expected_mac.len() != 32 {
        return false;
    }
    let actual = compute_pin_mac(pin, nonce, device_id);
    constant_time_eq(&actual, expected_mac)
}



/// Reusable per-vault deterministic AEAD cipher context.
/// Hoists BLAKE3 nonce subkey derivation, XChaCha20-Poly1305 key schedule expansion,
/// and AAD prefix hashing once per vault instead of repeating per chunk.
pub struct ChunkSealContext {
    cipher: XChaCha20Poly1305,
    aad_hasher_prefix: blake3::Hasher,
    aad: Vec<u8>,
}

impl ChunkSealContext {
    pub fn new(vault_chunk_key: &SymmetricKey, aad: &[u8]) -> Self {
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.crypto",
            "crypto.seal_context_init",
            None,
        );
        let nonce_subkey = blake3::derive_key("lanvault.chunk.nonce.v1", vault_chunk_key.as_bytes());
        let mut aad_hasher_prefix = blake3::Hasher::new_keyed(&nonce_subkey);
        aad_hasher_prefix.update(&(aad.len() as u64).to_le_bytes());
        aad_hasher_prefix.update(aad);
        Self {
            cipher: XChaCha20Poly1305::new(vault_chunk_key.as_bytes().into()),
            aad_hasher_prefix,
            aad: aad.to_vec(),
        }
    }

    pub fn encrypt_deterministic(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut probe = crate::debug_telemetry::ProbeGuard::start(
            "core.crypto",
            "crypto.deterministic_seal",
            None,
        );
        probe.add_bytes(plaintext.len() as u64);
        let mut hasher = self.aad_hasher_prefix.clone();
        hasher.update(plaintext);
        let digest = hasher.finalize();
        let mut nonce_bytes = [0u8; 24];
        nonce_bytes.copy_from_slice(&digest.as_bytes()[..24]);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: &self.aad,
                },
            )
            .map_err(|_| LanVaultError::Crypto("XChaCha20-Poly1305 deterministic encrypt failed".to_string()))?;
        let mut out = Vec::with_capacity(24 + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_aead_produces_stable_ciphertext_and_roundtrips() {
        let key = SymmetricKey::generate();
        let plaintext = b"identical backup chunk payload";
        let aad = b"vault-123";

        let c1 = encrypt_aead_deterministic(&key, plaintext, aad).unwrap();
        let c2 = encrypt_aead_deterministic(&key, plaintext, aad).unwrap();
        assert_eq!(c1, c2);

        let c_other_aad = encrypt_aead_deterministic(&key, plaintext, b"vault-456").unwrap();
        assert_ne!(c1, c_other_aad);

        let decrypted = decrypt_aead(&key, &c1, aad).unwrap();
        assert_eq!(decrypted, plaintext);
        assert!(decrypt_aead(&key, &c1, b"wrong-vault").is_err());
    }

    #[test]
    fn device_identity_load_or_generate_persists_fingerprint() {
        let dir = std::env::temp_dir().join(format!("lanvault-id-test-{}", Uuid::new_v4()));
        let key_path = dir.join("device.key");

        let first = DeviceIdentity::load_or_generate(&key_path).unwrap();
        let second = DeviceIdentity::load_or_generate(&key_path).unwrap();
        assert_eq!(first.fingerprint_hex(), second.fingerprint_hex());
        assert_eq!(first.public_key_bytes(), second.public_key_bytes());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn constant_time_eq_checks_exact_equality() {
        assert!(constant_time_eq(b"123456", b"123456"));
        assert!(!constant_time_eq(b"123456", b"123457"));
        assert!(!constant_time_eq(b"123456", b"12345"));
    }
}
