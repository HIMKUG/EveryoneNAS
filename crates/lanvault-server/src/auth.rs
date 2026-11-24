//! Argon2id password authentication, per-IP 3-strike / 60-second brute-force lockout,
//! and `lv_session` (`HttpOnly; SameSite=Strict; Path=/`) cookie session management.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::OsRng as ArgonOsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use dashmap::mapref::entry::Entry;
use dashmap::{DashMap, DashSet};
use lanvault_core::{verify_ed25519, DeviceIdentity, LanVaultError, Result};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tower_cookies::cookie::time::Duration as CookieDuration;
use tower_cookies::cookie::SameSite;
use tower_cookies::Cookie;

/// Name of the session cookie issued upon login.
pub const SESSION_COOKIE_NAME: &str = "lv_session";
/// Maximum consecutive failed login attempts before IP lockout.
pub const MAX_FAILED_ATTEMPTS: u32 = 3;
/// Lockout duration in seconds after 3 consecutive failed logins.
pub const LOCKOUT_DURATION_SECS: u64 = 60;
/// Session lifetime in seconds (24 hours).
pub const SESSION_TTL_SECS: u64 = 86_400;
const MAX_SESSIONS: usize = 256;
const MAX_IP_LOCKS: usize = 4096;

/// Per-IP login failure tracking state.
#[derive(Debug, Clone)]
pub struct LockState {
    pub failed_attempts: u32,
    pub locked_until: Option<Instant>,
}

/// Outcome of a login attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Authenticated { token: String, expires_in_secs: u64 },
    InvalidPassword { remaining_attempts: u32 },
    LockedOut { retry_after_secs: u64 },
}

/// Outcome of a rate-limited pairing PIN verification attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    Allowed,
    Rejected { remaining_attempts: u32 },
    LockedOut { retry_after_secs: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub is_admin: bool,
    pub device_id: Option<String>,
}

struct SessionRecord {
    expires_at: Instant,
    info: SessionInfo,
}

#[derive(Serialize, Deserialize)]
struct SignedSession {
    version: u8,
    expires_at_secs: u64,
    nonce_hex: String,
    info: SessionInfo,
}

/// Manages Argon2id password verification, per-IP rate limiting, and session cookies.
pub struct AuthManager {
    password_hash: String,
    identity: Arc<DeviceIdentity>,
    ip_locks: DashMap<IpAddr, LockState>,
    pair_locks: DashMap<IpAddr, LockState>,
    login_gates: DashMap<IpAddr, Arc<Mutex<()>>>,
    sessions: DashMap<String, SessionRecord>,
    revoked_tokens: DashMap<String, Instant>,
    revoked_devices: DashSet<String>,
}

impl AuthManager {
    /// Creates an `AuthManager` from an Argon2id PHC hash string.
    pub fn from_password_hash(
        password_hash: String,
        identity: Arc<DeviceIdentity>,
    ) -> Result<Self> {
        PasswordHash::new(&password_hash)
            .map_err(|e| LanVaultError::Auth(format!("invalid Argon2id hash: {e}")))?;
        Ok(Self {
            password_hash,
            identity,
            ip_locks: DashMap::new(),
            pair_locks: DashMap::new(),
            login_gates: DashMap::new(),
            sessions: DashMap::new(),
            revoked_tokens: DashMap::new(),
            revoked_devices: DashSet::new(),
        })
    }

    /// Creates an `AuthManager` by hashing a plaintext password with Argon2id.
    pub fn from_plaintext_password(
        password: &str,
        identity: Arc<DeviceIdentity>,
    ) -> Result<Self> {
        let hash = hash_password(password)?;
        Self::from_password_hash(hash, identity)
    }

    /// Returns remaining lockout seconds if `client_ip` is currently locked out from pairing.
    pub fn pair_lockout_remaining(&self, client_ip: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let entry = self.pair_locks.get(&client_ip)?;
        let until = entry.locked_until?;
        if until > now {
            Some(until.duration_since(now).as_secs().max(1))
        } else {
            None
        }
    }

    /// Checks per-IP lockout status for pairing PIN attempts.
    pub fn check_and_record_pair_attempt(&self, client_ip: IpAddr, matched: bool) -> AuthOutcome {
        self.check_and_record_lockout(&self.pair_locks, client_ip, matched)
    }

    fn check_and_record_lockout(
        &self,
        locks: &DashMap<IpAddr, LockState>,
        client_ip: IpAddr,
        matched: bool,
    ) -> AuthOutcome {
        if locks.len() >= MAX_IP_LOCKS {
            self.prune_expired();
        }
        let now = Instant::now();
        match locks.entry(client_ip) {
            Entry::Occupied(mut entry) => {
                if let Some(until) = entry.get().locked_until {
                    if until > now {
                        let secs = until.duration_since(now).as_secs().max(1);
                        return AuthOutcome::LockedOut {
                            retry_after_secs: secs,
                        };
                    }
                    let state = entry.get_mut();
                    state.locked_until = None;
                    state.failed_attempts = 0;
                }

                if matched {
                    entry.remove();
                    return AuthOutcome::Allowed;
                }

                let state = entry.get_mut();
                state.failed_attempts += 1;
                if state.failed_attempts >= MAX_FAILED_ATTEMPTS {
                    state.locked_until = Some(now + Duration::from_secs(LOCKOUT_DURATION_SECS));
                    state.failed_attempts = 0;
                    AuthOutcome::LockedOut {
                        retry_after_secs: LOCKOUT_DURATION_SECS,
                    }
                } else {
                    AuthOutcome::Rejected {
                        remaining_attempts: MAX_FAILED_ATTEMPTS - state.failed_attempts,
                    }
                }
            }
            Entry::Vacant(vacant) => {
                if matched {
                    return AuthOutcome::Allowed;
                }

                if MAX_FAILED_ATTEMPTS <= 1 {
                    vacant.insert(LockState {
                        failed_attempts: 0,
                        locked_until: Some(now + Duration::from_secs(LOCKOUT_DURATION_SECS)),
                    });
                    AuthOutcome::LockedOut {
                        retry_after_secs: LOCKOUT_DURATION_SECS,
                    }
                } else {
                    vacant.insert(LockState {
                        failed_attempts: 1,
                        locked_until: None,
                    });
                    AuthOutcome::Rejected {
                        remaining_attempts: MAX_FAILED_ATTEMPTS - 1,
                    }
                }
            }
        }
    }

    /// Attempts to authenticate `password` from `client_ip`.
    ///
    /// Enforces `ARCHITECTURE_CONTRACTS.md` §3.1:
    /// - 1st & 2nd consecutive failure -> `InvalidPassword { remaining_attempts }` (HTTP 401)
    /// - 3rd consecutive failure -> immediate 60s IP lockout -> `LockedOut { retry_after_secs: 60 }` (HTTP 429)
    pub fn login(&self, client_ip: IpAddr, password: &str) -> LoginOutcome {
        let gate = self
            .login_gates
            .entry(client_ip)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _login_guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();

        // Phase 1: Atomically check active lockout and evict expired lockout entries
        // before spending CPU on Argon2id verification.
        if let Entry::Occupied(entry) = self.ip_locks.entry(client_ip) {
            if let Some(until) = entry.get().locked_until {
                if until > now {
                    let secs = until.duration_since(now).as_secs().max(1);
                    return LoginOutcome::LockedOut {
                        retry_after_secs: secs,
                    };
                }
                entry.remove();
            }
        }

        let verified = verify_password(password, &self.password_hash);
        match self.check_and_record_lockout(&self.ip_locks, client_ip, verified) {
            AuthOutcome::Allowed => {
                let token = self.issue_session();
                LoginOutcome::Authenticated {
                    token,
                    expires_in_secs: SESSION_TTL_SECS,
                }
            }
            AuthOutcome::Rejected { remaining_attempts } => {
                LoginOutcome::InvalidPassword { remaining_attempts }
            }
            AuthOutcome::LockedOut { retry_after_secs } => {
                LoginOutcome::LockedOut { retry_after_secs }
            }
        }
    }

    /// Issues and registers a new `lv_session` token (used after password login or PIN pairing).
    /// Evicts expired sessions and stale IP lockout entries to bound memory growth.
    pub fn prune_expired(&self) {
        let now = Instant::now();
        self.sessions
            .retain(|_, session| session.expires_at > now);
        self.revoked_tokens
            .retain(|_, expires_at| *expires_at > now);
        self.ip_locks.retain(|_, state| {
            state.locked_until.map(|u| u > now).unwrap_or(state.failed_attempts > 0)
        });
        while self.ip_locks.len() > MAX_IP_LOCKS {
            let Some(key) = self
                .ip_locks
                .iter()
                .next()
                .map(|entry| *entry.key())
            else {
                break;
            };
            self.ip_locks.remove(&key);
        }
        self.pair_locks.retain(|_, state| {
            state.locked_until.map(|until| until > now).unwrap_or(state.failed_attempts > 0)
        });
        while self.pair_locks.len() > MAX_IP_LOCKS {
            let Some(key) = self
                .pair_locks
                .iter()
                .next()
                .map(|entry| *entry.key())
            else {
                break;
            };
            self.pair_locks.remove(&key);
        }
        while self.login_gates.len() > MAX_IP_LOCKS {
            let Some(key) = self
                .login_gates
                .iter()
                .next()
                .map(|entry| *entry.key())
            else {
                break;
            };
            self.login_gates.remove(&key);
        }
    }

    pub fn issue_session(&self) -> String {
        self.issue_scoped_session(SessionInfo {
            is_admin: true,
            device_id: None,
        })
    }

    pub fn issue_peer_session(&self, device_id: String) -> String {
        self.issue_scoped_session(SessionInfo {
            is_admin: false,
            device_id: Some(device_id),
        })
    }

    fn issue_scoped_session(&self, info: SessionInfo) -> String {
        let expires_at = Instant::now() + Duration::from_secs(SESSION_TTL_SECS);
        let expires_at_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + SESSION_TTL_SECS;
        let payload = SignedSession {
            version: 1,
            expires_at_secs,
            nonce_hex: generate_session_token(),
            info: info.clone(),
        };
        let payload_bytes = serde_json::to_vec(&payload)
            .expect("signed session payload is always serializable");
        let signature = self.identity.sign(&payload_bytes);
        let token = format!("{}.{}", hex::encode(payload_bytes), hex::encode(signature));
        if self.sessions.len() >= MAX_SESSIONS {
            self.prune_expired();
        }
        if self.sessions.len() >= MAX_SESSIONS {
            if let Some((oldest, oldest_expires_at)) = self
                .sessions
                .iter()
                .min_by_key(|entry| entry.value().expires_at)
                .map(|entry| (entry.key().clone(), entry.value().expires_at))
            {
                self.sessions.remove(&oldest);
                self.revoked_tokens.insert(oldest, oldest_expires_at);
            }
        }
        self.sessions.insert(
            token.clone(),
            SessionRecord { expires_at, info },
        );
        token
    }

    /// Validates an active `lv_session` token, evicting it if expired.
    pub fn validate_session(&self, token: &str) -> Option<SessionInfo> {
        if self.revoked_tokens.contains_key(token) {
            return None;
        }
        let (payload_hex, signature_hex) = token.split_once('.')?;
        let payload_bytes = hex::decode(payload_hex).ok()?;
        let signature = hex::decode(signature_hex).ok()?;
        verify_ed25519(&self.identity.public_key_bytes(), &payload_bytes, &signature).ok()?;
        let payload: SignedSession = serde_json::from_slice(&payload_bytes).ok()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if payload.version != 1 || payload.expires_at_secs <= now {
            return None;
        }
        if payload
            .info
            .device_id
            .as_ref()
            .is_some_and(|device_id| self.revoked_devices.contains(device_id))
        {
            return None;
        }
        Some(payload.info)
    }

    /// Invalidates a session token on logout.
    pub fn revoke_session(&self, token: &str) {
        let expires_at = self
            .sessions
            .remove(token)
            .map(|(_, record)| record.expires_at)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(SESSION_TTL_SECS));
        if self.revoked_tokens.len() >= MAX_SESSIONS {
            let now = Instant::now();
            self.revoked_tokens.retain(|_, exp| *exp > now);
        }
        self.revoked_tokens.insert(token.to_string(), expires_at);
    }

    /// Removes a previously revoked device ID when the peer legitimately re-pairs.
    pub fn unrevoke_device(&self, device_id: &str) {
        self.revoked_devices.remove(device_id);
    }

    pub fn validate_session_device_active(&self, device_id: &str) -> bool {
        !self.revoked_devices.contains(device_id)
    }

    pub fn revoke_device_sessions(&self, device_id: &str) {
        self.revoked_devices.insert(device_id.to_string());
        self.sessions.retain(|_, session| {
            session
                .info
                .device_id
                .as_deref()
                .is_none_or(|session_device_id| session_device_id != device_id)
        });
    }
}

/// Builds the `lv_session` cookie (`HttpOnly; SameSite=Strict; Path=/`).
pub fn build_session_cookie(token: String, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, token);
    cookie.set_http_only(true);
    cookie.set_secure(secure);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::seconds(SESSION_TTL_SECS as i64));
    cookie
}

/// Builds an expired `lv_session` cookie to clear the client's browser state on logout.
pub fn build_clear_session_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, "");
    cookie.set_http_only(true);
    cookie.set_secure(secure);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::ZERO);
    cookie
}

/// Hashes a plaintext password using Argon2id with a random salt.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut ArgonOsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| LanVaultError::Auth(format!("Argon2id hashing failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verifies a plaintext password against a stored Argon2id PHC string.
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn pair_attempts_enforce_three_strike_lockout() {
        let auth = AuthManager::from_plaintext_password(
            "secret-pass",
            Arc::new(DeviceIdentity::generate()),
        )
        .unwrap();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42));

        assert_eq!(
            auth.check_and_record_pair_attempt(ip, false),
            AuthOutcome::Rejected {
                remaining_attempts: 2
            }
        );
        assert_eq!(
            auth.check_and_record_pair_attempt(ip, false),
            AuthOutcome::Rejected {
                remaining_attempts: 1
            }
        );
        assert_eq!(
            auth.check_and_record_pair_attempt(ip, false),
            AuthOutcome::LockedOut {
                retry_after_secs: LOCKOUT_DURATION_SECS
            }
        );
        // Even a matching PIN is rejected while locked out.
        assert!(matches!(
            auth.check_and_record_pair_attempt(ip, true),
            AuthOutcome::LockedOut { .. }
        ));
    }

    #[test]
    fn unrevoke_device_restores_session_validation_after_repair() {
        let auth = AuthManager::from_plaintext_password(
            "secret-pass",
            Arc::new(DeviceIdentity::generate()),
        )
        .unwrap();
        let dev_id = "lv-0123456789ab";
        let t1 = auth.issue_peer_session(dev_id.to_string());
        assert!(auth.validate_session(&t1).is_some());

        auth.revoke_device_sessions(dev_id);
        let t2 = auth.issue_peer_session(dev_id.to_string());
        assert!(auth.validate_session(&t2).is_none());

        auth.unrevoke_device(dev_id);
        let t3 = auth.issue_peer_session(dev_id.to_string());
        assert!(auth.validate_session(&t3).is_some());
    }
}
