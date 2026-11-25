/**
 * LanVault REST API Client (v1 + v2 Contract Endpoints)
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

  const DEFAULT_REQUEST_TIMEOUT_MS = 15000;

  async function request(
    method,
    path,
    body = undefined,
    { suppressAuthRedirect = false, timeoutMs = DEFAULT_REQUEST_TIMEOUT_MS, signal = null } = {},
  ) {
    const controller = new AbortController();
    let timedOut = false;
    let timeoutId = null;

    const onExternalAbort = () => controller.abort(signal?.reason);
    if (signal) {
      if (signal.aborted) {
        controller.abort(signal.reason);
      } else {
        signal.addEventListener('abort', onExternalAbort, { once: true });
      }
    }

    if (Number.isFinite(timeoutMs) && timeoutMs > 0) {
      timeoutId = global.setTimeout(() => {
        timedOut = true;
        controller.abort(new DOMException('Request timed out', 'TimeoutError'));
      }, timeoutMs);
    }

    const init = {
      method,
      credentials: 'same-origin',
      signal: controller.signal,
      headers: {
        Accept: 'application/json',
      },
    };

    if (body !== undefined) {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }

    let response;
    let data;
    try {
      response = await fetch(path, init);
      data = await parseJsonBoundary(response);
    } catch (err) {
      if (timedOut) {
        throw new ApiError(0, 'timeout', '请求超时，请检查网络连接后重试');
      }
      if (signal?.aborted) {
        throw new ApiError(0, 'aborted', '请求已取消');
      }
      throw new ApiError(0, 'network_error', '无法连接到服务器，请检查网络');
    } finally {
      if (timeoutId !== null) {
        global.clearTimeout(timeoutId);
      }
      if (signal) {
        signal.removeEventListener('abort', onExternalAbort);
      }
    }

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

  /* 13. v2 Protocol Capabilities */
  function getCapabilities() {
    return request('GET', '/api/capabilities');
  }

  /* 14. v2 Device Groups & Membership */
  function listGroups() {
    return request('GET', '/api/groups');
  }

  function createGroup(name) {
    return request('POST', '/api/groups', { name });
  }

  function renameGroup(groupId, name) {
    return request('PATCH', `/api/groups/${encodeURIComponent(groupId)}`, { name });
  }

  function deleteGroup(groupId) {
    return request('DELETE', `/api/groups/${encodeURIComponent(groupId)}`);
  }

  function listGroupMembers(groupId) {
    return request('GET', `/api/groups/${encodeURIComponent(groupId)}/members`);
  }

  function addGroupMember(groupId, deviceId, role = 'member') {
    return request('POST', `/api/groups/${encodeURIComponent(groupId)}/members`, {
      device_id: deviceId,
      role,
    });
  }

  function removeGroupMember(groupId, deviceId) {
    return request(
      'DELETE',
      `/api/groups/${encodeURIComponent(groupId)}/members/${encodeURIComponent(deviceId)}`,
    );
  }

  /* 15. v2 Group Sync Rules & Target Bindings */
  function listGroupRules(groupId) {
    return request('GET', `/api/groups/${encodeURIComponent(groupId)}/rules`);
  }

  function listRuleBindings(ruleId) {
    return request('GET', `/api/rules/${encodeURIComponent(ruleId)}/bindings`);
  }

  /* 16. v2 Incremental Catalog & Version History */
  function listBindingCatalog(bindingId, afterPathKey = null, limit = 200, sourceDeletedOnly = false) {
    const query = new URLSearchParams();
    if (afterPathKey) {
      query.set('after_path_key', afterPathKey);
    }
    if (limit !== undefined && limit !== null) {
      query.set('limit', String(limit));
    }
    if (sourceDeletedOnly) {
      query.set('source_deleted_only', 'true');
    }
    const qs = query.toString();
    return request('GET', `/api/bindings/${encodeURIComponent(bindingId)}/catalog${qs ? `?${qs}` : ''}`);
  }

  function listBindingVersions(
    bindingId,
    pathKey = null,
    afterArchivedAt = null,
    afterVersionId = null,
    limit = 200,
  ) {
    const query = new URLSearchParams();
    if (pathKey) {
      query.set('path_key', pathKey);
    }
    if (afterArchivedAt !== undefined && afterArchivedAt !== null) {
      query.set('after_archived_at', String(afterArchivedAt));
    }
    if (afterVersionId) {
      query.set('after_version_id', afterVersionId);
    }
    if (limit !== undefined && limit !== null) {
      query.set('limit', String(limit));
    }
    const qs = query.toString();
    return request('GET', `/api/bindings/${encodeURIComponent(bindingId)}/versions${qs ? `?${qs}` : ''}`);
  }

  function buildVersionDownloadUrl(bindingId, versionId) {
    return `/api/bindings/${encodeURIComponent(bindingId)}/versions/${encodeURIComponent(versionId)}/download`;
  }

  /* 17. v2 Idempotent Multi-Target Clipboard & History */
  function sendClipboardV2(payload) {
    return request('POST', '/api/clipboard', payload);
  }

  function getClipboardHistory(limit = 50) {
    const query = new URLSearchParams({ limit: String(limit) });
    return request('GET', `/api/clipboard/history?${query.toString()}`);
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
    getCapabilities,
    listGroups,
    createGroup,
    renameGroup,
    deleteGroup,
    listGroupMembers,
    addGroupMember,
    removeGroupMember,
    listGroupRules,
    listRuleBindings,
    listBindingCatalog,
    listBindingVersions,
    buildVersionDownloadUrl,
    sendClipboardV2,
    getClipboardHistory,
  };
})(window);
