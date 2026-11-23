//! Argon2id password authentication, per-IP 3-strike / 60-second brute-force lockout,
//! and `lv_session` (`HttpOnly; SameSite=Strict; Path=/`) cookie session management.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::OsRng as ArgonOsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use lanvault_core::{LanVaultError, Result};
use rand::rngs::OsRng;
use rand::RngCore;
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

/// Manages Argon2id password verification, per-IP rate limiting, and session cookies.
pub struct AuthManager {
    password_hash: String,
    ip_locks: DashMap<IpAddr, LockState>,
    sessions: DashMap<String, Instant>,
}

impl AuthManager {
    /// Creates an `AuthManager` from an Argon2id PHC hash string.
    pub fn from_password_hash(password_hash: String) -> Result<Self> {
        PasswordHash::new(&password_hash)
            .map_err(|e| LanVaultError::Auth(format!("invalid Argon2id hash: {e}")))?;
        Ok(Self {
            password_hash,
            ip_locks: DashMap::new(),
            sessions: DashMap::new(),
        })
    }

    /// Creates an `AuthManager` by hashing a plaintext password with Argon2id.
    pub fn from_plaintext_password(password: &str) -> Result<Self> {
        let hash = hash_password(password)?;
        Self::from_password_hash(hash)
    }

    /// Checks per-IP lockout status and records a pairing PIN attempt using the shared
    /// 3-strike / 60-second `ip_locks` policy.
    pub fn check_and_record_pair_attempt(&self, client_ip: IpAddr, matched: bool) -> AuthOutcome {
        if self.ip_locks.len() >= MAX_IP_LOCKS {
            self.prune_expired();
        }
        let now = Instant::now();
        match self.ip_locks.entry(client_ip) {
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
        match self.check_and_record_pair_attempt(client_ip, verified) {
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
        self.sessions.retain(|_, expires_at| *expires_at > now);
        self.ip_locks.retain(|_, state| {
            state.locked_until.map(|u| u > now).unwrap_or(state.failed_attempts > 0)
        });
        if self.ip_locks.len() > MAX_IP_LOCKS {
            self.ip_locks.retain(|_, state| {
                state.locked_until.map(|u| u > now).unwrap_or(false)
            });
        }
    }

    pub fn issue_session(&self) -> String {
        let token = generate_session_token();
        let expires_at = Instant::now() + Duration::from_secs(SESSION_TTL_SECS);
        if self.sessions.len() >= MAX_SESSIONS {
            self.prune_expired();
        }
        self.sessions.insert(token.clone(), expires_at);
        token
    }

    /// Validates an active `lv_session` token, evicting it if expired.
    pub fn validate_session(&self, token: &str) -> bool {
        let now = Instant::now();
        if let Some(expires_at) = self.sessions.get(token) {
            if *expires_at > now {
                return true;
            }
        }
        self.sessions.remove(token);
        false
    }

    /// Invalidates a session token on logout.
    pub fn revoke_session(&self, token: &str) {
        self.sessions.remove(token);
    }
}

/// Builds the `lv_session` cookie (`HttpOnly; SameSite=Strict; Path=/`).
pub fn build_session_cookie(token: String) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, token);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_path("/");
    cookie.set_max_age(CookieDuration::seconds(SESSION_TTL_SECS as i64));
    cookie
}

/// Builds an expired `lv_session` cookie to clear the client's browser state on logout.
pub fn build_clear_session_cookie() -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, "");
    cookie.set_http_only(true);
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
        let auth = AuthManager::from_plaintext_password("secret-pass").unwrap();
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
}
