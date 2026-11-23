/**
 * LanVault REST API Client (11 Contract Endpoints)
 *
 * Reference basis:
 * - /working_dir/c_0c10370daccfa96e/lanvault/ARCHITECTURE_CONTRACTS.md (Section 3.1)
 *
 * Boundary-only validation: HTTP status & JSON parsing occur strictly here.
 * Persists 429 Retry-After lockout deadline in localStorage so page refreshes
 * cannot bypass the 60-second anti-bruteforce countdown.
 */
(function initLanVaultApi(global) {
  'use strict';

  const LOCKOUT_STORAGE_KEY = 'lanvault_lockout_until_ms';

  class ApiError extends Error {
    constructor(status, code, message, extra = {}) {
      super(message);
      this.name = 'ApiError';
      this.status = status;
      this.code = code;
      this.remainingAttempts = extra.remainingAttempts ?? null;
      this.retryAfterSecs = extra.retryAfterSecs ?? null;
    }
  }

  let unauthorizedHandler = null;
  let lockoutHandler = null;

  function configureApiCallbacks({ onUnauthorized, onLockout }) {
    unauthorizedHandler = onUnauthorized;
    lockoutHandler = onLockout;
  }

  function getPersistedLockoutUntil() {
    const raw = global.localStorage.getItem(LOCKOUT_STORAGE_KEY);
    if (!raw) return 0;
    const untilMs = Number(raw);
    if (!Number.isFinite(untilMs) || untilMs <= Date.now()) {
      global.localStorage.removeItem(LOCKOUT_STORAGE_KEY);
      return 0;
    }
    return untilMs;
  }

  function persistLockoutSecs(secs) {
    const clamped = Math.max(1, Math.ceil(secs));
    const untilMs = Date.now() + clamped * 1000;
    global.localStorage.setItem(LOCKOUT_STORAGE_KEY, String(untilMs));
    return untilMs;
  }

  function clearPersistedLockout() {
    global.localStorage.removeItem(LOCKOUT_STORAGE_KEY);
  }

  async function parseJsonBoundary(response) {
    const text = await response.text();
    if (!text) return {};
    const contentType = (response.headers.get('Content-Type') || '').toLowerCase();
    const isJson = contentType.includes('application/json') || contentType.includes('+json');
    if (!isJson) {
      return { message: text.trim() };
    }
    try {
      return JSON.parse(text);
    } catch {
      return { message: text.trim() };
    }
  }

  async function request(method, path, body = undefined, { suppressAuthRedirect = false } = {}) {
    const init = {
      method,
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
      },
    };

    if (body !== undefined) {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }

    const response = await fetch(path, init);
    const data = await parseJsonBoundary(response);

    if (response.status === 429) {
      const headerVal = Number(response.headers.get('Retry-After'));
      const bodyVal = Number(data.retry_after_secs);
      const retryAfterSecs = Number.isFinite(headerVal) && headerVal > 0
        ? headerVal
        : (Number.isFinite(bodyVal) && bodyVal > 0 ? bodyVal : 60);
      const untilMs = persistLockoutSecs(retryAfterSecs);
      if (lockoutHandler) lockoutHandler(untilMs, retryAfterSecs);
      throw new ApiError(429, 'locked_out', `连续尝试失败过多，已锁定 ${retryAfterSecs} 秒`, {
        retryAfterSecs,
      });
    }

    if (response.status === 401) {
      if (!suppressAuthRedirect && unauthorizedHandler) {
        unauthorizedHandler();
      }
      const remaining = typeof data.remaining_attempts === 'number' ? data.remaining_attempts : null;
      const msg = remaining !== null
        ? `密码错误，还可尝试 ${remaining} 次`
        : '会话已失效，请重新登录';
      throw new ApiError(401, data.error || 'unauthorized', msg, {
        remainingAttempts: remaining,
      });
    }

    if (!response.ok) {
      throw new ApiError(
        response.status,
        data.error || 'http_error',
        data.message || data.error || `请求失败（HTTP ${response.status}）`,
      );
    }

    return data;
  }

  /* 1. POST /api/auth/login */
  async function login(password) {
    const result = await request('POST', '/api/auth/login', { password }, { suppressAuthRedirect: true });
    clearPersistedLockout();
    return result;
  }

  /* 2. POST /api/auth/logout */
  function logout() {
    return request('POST', '/api/auth/logout');
  }

  /* 3. GET /api/system/status */
  function getSystemStatus() {
    return request('GET', '/api/system/status');
  }

  /* 4. POST /api/system/restart */
  function restartSystem() {
    return request('POST', '/api/system/restart');
  }

  /* 5. GET /api/vaults */
  function listVaults() {
    return request('GET', '/api/vaults');
  }

  /* 6. POST /api/vaults */
  function createVault(payload) {
    return request('POST', '/api/vaults', payload);
  }

  /* 7. GET /api/snapshots?vault_id=<id> */
  function listSnapshots(vaultId) {
    const query = new URLSearchParams({ vault_id: vaultId });
    return request('GET', `/api/snapshots?${query.toString()}`);
  }

  /* Helper URL for single-file rollback download via /api/snapshots/:id/file */
  function buildSnapshotRollbackUrl(vaultId, snapshotId, filePath) {
    return `/api/snapshots/${encodeURIComponent(snapshotId)}/file?vault_id=${encodeURIComponent(vaultId)}&path=${encodeURIComponent(filePath)}`;
  }

  /* 8. POST /api/scrub/trigger */
  function triggerScrub() {
    return request('POST', '/api/scrub/trigger');
  }

  /* 9. GET /api/peers */
  function listPeers() {
    return request('GET', '/api/peers');
  }

  /* 10. DELETE /api/peers/:device_id */
  function revokePeer(deviceId) {
    return request('DELETE', `/api/peers/${encodeURIComponent(deviceId)}`);
  }

  /* 11. POST /api/clipboard */
  function sendClipboard(payload) {
    return request('POST', '/api/clipboard', payload);
  }

  /* 12. Debug Mode & Unified Telemetry Logs */
  function getDebugConfig() {
    return request('GET', '/api/debug/config');
  }

  function setDebugConfig(enabled) {
    return request('POST', '/api/debug/config', { enabled: Boolean(enabled) });
  }

  function getDebugLogs(limit = 200) {
    return request('GET', `/api/debug/logs?limit=${encodeURIComponent(limit)}`);
  }

  function clearDebugLogs() {
    return request('DELETE', '/api/debug/logs');
  }

  function reportClientLog(level, source, message, detail = null) {
    return request('POST', '/api/debug/logs/client', { level, source, message, detail });
  }

  global.LanVault = global.LanVault || {};
  global.LanVault.api = {
    ApiError,
    configureApiCallbacks,
    getPersistedLockoutUntil,
    persistLockoutSecs,
    clearPersistedLockout,
    login,
    logout,
    getSystemStatus,
    restartSystem,
    listVaults,
    createVault,
    listSnapshots,
    buildSnapshotRollbackUrl,
    triggerScrub,
    listPeers,
    revokePeer,
    sendClipboard,
    getDebugConfig,
    setDebugConfig,
    getDebugLogs,
    clearDebugLogs,
    reportClientLog,
  };
})(window);
