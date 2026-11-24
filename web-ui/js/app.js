/**
 * LanVault Web Console Main Controller
 *
 * Uses Pointer Events for tactile feedback and actions (never hover-only).
 * Coordinates:
 *   1. Service & Keep-Alive + One-Click Graceful Restart
 *   2. Storage Vaults (single/striped/mirror_raid1) + GFS Timeline + Bit-Rot Scrub
 *   3. Paired Devices + 6-Digit PIN / QR + Revoke
 *   4. v2 Device Groups, Group Sync Rules, Target Bindings & Catalog/Version Browser
 *   5. Safe Logout + Non-Blocking LAN Clipboard Quick-Transfer with Per-Target Retry
 */
(function initLanVaultApp(global) {
  'use strict';

  const {
    ApiError,
    addGroupMember,
    buildSnapshotRollbackUrl,
    buildVersionDownloadUrl,
    clearPersistedLockout,
    configureApiCallbacks,
    createGroup,
    createVault,
    deleteGroup,
    getCapabilities,
    getClipboardHistory,
    getPersistedLockoutUntil,
    getSystemStatus,
    listBindingCatalog,
    listBindingVersions,
    listGroupMembers,
    listGroupRules,
    listGroups,
    listPeers,
    listRuleBindings,
    listSnapshots,
    listVaults,
    login,
    logout,
    removeGroupMember,
    renameGroup,
    restartSystem,
    revokePeer,
    sendClipboardV2,
    triggerScrub,
  } = global.LanVault.api;
  const { SnapshotTimeline, formatBytes, formatTimestamp } = global.LanVault.timeline;
  const { LanVaultSocket } = global.LanVault.ws;

  const KEEP_ALIVE_LABELS = {
    LEVEL_3_MAGISK_DAEMON: 'Level 3 · Magisk 守护',
    LEVEL_2_ROOT_LIBSU: 'Level 2 · Root 守护',
    LEVEL_1_SHIZUKU: 'Level 1 · Shizuku 守护',
    LEVEL_0_FOREGROUND: 'Level 0 · 前台服务',
    WIN_SERVICE: 'Windows 系统服务',
  };

  const POOL_MODE_LABELS = {
    single: '单盘',
    striped: '多盘串联',
    mirror_raid1: '双盘镜像',
  };

  const BIT_ROT_LABELS = {
    healthy: { text: '正常', cls: 'status-ok' },
    degraded: { text: '需关注', cls: 'status-warn' },
    corrupted: { text: '发现损坏', cls: 'status-danger' },
  };

  const WS_STATE_LABELS = {
    connecting: { text: '连接中…', cls: 'status-warn' },
    open: { text: '实时已连接', cls: 'status-ok' },
    reconnecting: { text: '断线重连中…', cls: 'status-warn' },
    closed: { text: '已断开', cls: 'status-danger' },
  };

  const CLIP_STATUS_LABELS = {
    sending: { text: '发送中', cls: 'status-badge--sending' },
    sent: { text: '已发送', cls: 'status-badge--ok' },
    partial_failed: { text: '部分失败', cls: 'status-badge--warn' },
    failed: { text: '发送失败', cls: 'status-badge--danger' },
    received: { text: '已接收', cls: 'status-badge--neutral' },
  };

  const TARGET_STATUS_LABELS = {
    sending: '发送中',
    delivered: '已送达',
    accepted: '已接收',
    acked: '已确认',
    offline: '离线',
    revoked: '已吊销',
    failed: '失败',
  };

  const SYNC_MODE_LABELS = {
    diff_backup: { text: '差异备份', cls: 'status-badge--ok' },
    mirror_sync: { text: '镜像同步', cls: 'status-badge--warn' },
  };

  const BINDING_STATUS_LABELS = {
    synced: { text: '已同步', cls: 'status-badge--ok' },
    ok: { text: '正常', cls: 'status-badge--ok' },
    pending: { text: '待同步', cls: 'status-badge--neutral' },
    syncing: { text: '同步中', cls: 'status-badge--sending' },
    needs_binding_confirm: { text: '待确认绑定', cls: 'status-badge--warn' },
    error: { text: '异常', cls: 'status-badge--danger' },
    failed: { text: '失败', cls: 'status-badge--danger' },
  };

  const state = {
    vaults: [],
    selectedVaultId: '',
    peers: [],
    pairingPin: '',
    pairingQrUri: '',
    lockoutTimerId: null,
    lockoutUntilMs: 0,
    lockoutTotalSecs: 60,
    restartCountdownId: null,
    // Non-blocking clipboard message map keyed by client_msg_id
    clipboardMessages: new Map(),
    clipboardSeq: 0,
    // v2 Device Groups, Rules & Catalog browser state
    capabilities: null,
    groups: [],
    selectedGroupId: '',
    groupMembers: [],
    groupRules: [],
    ruleBindingsMap: new Map(),
    selectedBindingId: '',
    selectedBindingLabel: '',
    catalogEntries: [],
    catalogNextCursor: null,
    catalogSelectedPathKey: '',
    catalogVersions: [],
    versionsNextArchivedAt: null,
    versionsNextVersionId: null,
  };

  const el = {
    authOverlay: document.getElementById('auth-overlay'),
    appShell: document.getElementById('app-shell'),
    loginForm: document.getElementById('login-form'),
    loginPassword: document.getElementById('login-password'),
    loginSubmit: document.getElementById('login-submit'),
    loginError: document.getElementById('login-error'),
    lockoutBox: document.getElementById('lockout-box'),
    lockoutText: document.getElementById('lockout-text'),
    lockoutProgress: document.getElementById('lockout-progress'),
    wsState: document.getElementById('ws-state'),
    capKeepalive: document.getElementById('cap-keepalive'),
    capStorageText: document.getElementById('cap-storage-text'),
    capStorageFill: document.getElementById('cap-storage-fill'),
    capThroughput: document.getElementById('cap-throughput'),
    capBitrot: document.getElementById('cap-bitrot'),
    systemBanner: document.getElementById('system-banner'),
    systemBannerText: document.getElementById('system-banner-text'),
    systemBannerDismiss: document.getElementById('system-banner-dismiss'),
    btnRefreshStatus: document.getElementById('btn-refresh-status'),
    btnGracefulRestart: document.getElementById('btn-graceful-restart'),
    svcOs: document.getElementById('svc-os'),
    svcKeepalive: document.getElementById('svc-keepalive'),
    svcLan: document.getElementById('svc-lan'),
    svcUptime: document.getElementById('svc-uptime'),
    svcFeedback: document.getElementById('svc-feedback'),
    advMdns: document.getElementById('adv-mdns'),
    advCorrupted: document.getElementById('adv-corrupted'),
    btnTriggerScrub: document.getElementById('btn-trigger-scrub'),
    btnRefreshVaults: document.getElementById('btn-refresh-vaults'),
    vaultList: document.getElementById('vault-list'),
    createVaultDrawer: document.getElementById('create-vault-drawer'),
    createVaultForm: document.getElementById('create-vault-form'),
    vaultMode: document.getElementById('vault-mode'),
    extraVolumesSection: document.getElementById('extra-volumes-section'),
    extraVolumesList: document.getElementById('extra-volumes-list'),
    btnAddExtraVol: document.getElementById('btn-add-extra-vol'),
    vaultFeedback: document.getElementById('vault-feedback'),
    snapshotVaultSelect: document.getElementById('snapshot-vault-select'),
    tierFilters: document.getElementById('tier-filters'),
    snapshotTimeline: document.getElementById('snapshot-timeline'),
    timelineFeedback: document.getElementById('timeline-feedback'),
    scrubLog: document.getElementById('scrub-log'),
    btnRefreshPeers: document.getElementById('btn-refresh-peers'),
    pairingQr: document.getElementById('pairing-qr'),
    pairingPin: document.getElementById('pairing-pin'),
    pairingUri: document.getElementById('pairing-uri'),
    btnCopyPin: document.getElementById('btn-copy-pin'),
    btnCopyUri: document.getElementById('btn-copy-uri'),
    peerList: document.getElementById('peer-list'),
    peerFeedback: document.getElementById('peer-feedback'),
    // v2 Device Groups & Catalog elements
    v2CapabilitiesBadge: document.getElementById('v2-capabilities-badge'),
    btnRefreshGroups: document.getElementById('btn-refresh-groups'),
    createGroupForm: document.getElementById('create-group-form'),
    groupNameInput: document.getElementById('group-name-input'),
    groupFeedback: document.getElementById('group-feedback'),
    groupsList: document.getElementById('groups-list'),
    groupDetailPanel: document.getElementById('group-detail-panel'),
    groupDetailName: document.getElementById('group-detail-name'),
    groupDetailRev: document.getElementById('group-detail-rev'),
    renameGroupForm: document.getElementById('rename-group-form'),
    renameGroupInput: document.getElementById('rename-group-input'),
    btnDeleteGroup: document.getElementById('btn-delete-group'),
    groupMembersList: document.getElementById('group-members-list'),
    addGroupMemberForm: document.getElementById('add-group-member-form'),
    groupMemberDevice: document.getElementById('group-member-device'),
    groupMemberRole: document.getElementById('group-member-role'),
    groupRulesList: document.getElementById('group-rules-list'),
    catalogBrowserPanel: document.getElementById('catalog-browser-panel'),
    catalogBindingTitle: document.getElementById('catalog-binding-title'),
    catalogDeletedOnly: document.getElementById('catalog-deleted-only'),
    btnRefreshCatalog: document.getElementById('btn-refresh-catalog'),
    btnCloseCatalog: document.getElementById('btn-close-catalog'),
    catalogFeedback: document.getElementById('catalog-feedback'),
    catalogEntriesList: document.getElementById('catalog-entries-list'),
    btnCatalogLoadMore: document.getElementById('btn-catalog-load-more'),
    catalogVersionsTitle: document.getElementById('catalog-versions-title'),
    catalogVersionsList: document.getElementById('catalog-versions-list'),
    btnVersionsLoadMore: document.getElementById('btn-versions-load-more'),
    // Block 4: Security & Clipboard
    btnRefreshClipboard: document.getElementById('btn-refresh-clipboard'),
    btnLogout: document.getElementById('btn-logout'),
    clipboardForm: document.getElementById('clipboard-form'),
    clipTarget: document.getElementById('clip-target'),
    clipKind: document.getElementById('clip-kind'),
    clipFilenameField: document.getElementById('clip-filename-field'),
    clipFilename: document.getElementById('clip-filename'),
    clipContent: document.getElementById('clip-content'),
    clipFeedback: document.getElementById('clip-feedback'),
    clipboardFeed: document.getElementById('clipboard-feed'),
  };

  const timeline = new SnapshotTimeline(
    el.snapshotTimeline,
    el.tierFilters,
    handleSnapshotRollback,
  );

  const socket = new LanVaultSocket({
    onStateChange: renderWsState,
    onAuthExpired: showLockScreen,
    telemetry: applySystemStatus,
    server_restarting: handleServerRestartingEvent,
    bit_rot_alert: handleBitRotAlertEvent,
    clipboard_received: handleClipboardReceivedEvent,
    clipboard_status: handleClipboardStatusEvent,
    group_sync_update: handleGroupSyncUpdateEvent,
  });

  function formatUptime(secs) {
    const days = Math.floor(secs / 86400);
    const hours = Math.floor((secs % 86400) / 3600);
    const mins = Math.floor((secs % 3600) / 60);
    if (days > 0) return `${days} 天 ${hours} 小时 ${mins} 分`;
    if (hours > 0) return `${hours} 小时 ${mins} 分`;
    return `${mins} 分 ${secs % 60} 秒`;
  }

  function setFeedback(node, text, tone = '') {
    node.textContent = text;
    node.className = tone ? `feedback-line ${tone}` : 'feedback-line';
  }

  function generateClientMsgId() {
    if (global.crypto && typeof global.crypto.randomUUID === 'function') {
      return global.crypto.randomUUID();
    }
    const rand = Math.random().toString(16).slice(2, 10);
    return `msg-${Date.now()}-${rand}`;
  }

  function resolveDeviceName(deviceId) {
    if (!deviceId) return '未知设备';
    const match = state.peers.find((p) => p.device_id === deviceId);
    return match ? match.device_name : deviceId;
  }

  async function copyTextToClipboard(text) {
    if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch {
        // Fallback for non-secure LAN HTTP contexts where Clipboard API is restricted.
      }
    }

    const textarea = document.createElement('textarea');
    textarea.value = text;
    textarea.setAttribute('readonly', '');
    textarea.style.position = 'fixed';
    textarea.style.top = '-9999px';
    textarea.style.left = '-9999px';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.select();
    textarea.setSelectionRange(0, text.length);

    let copied = false;
    try {
      copied = document.execCommand('copy');
    } finally {
      textarea.remove();
    }
    return copied;
  }

  function showBanner(text, tone = 'warn') {
    el.systemBannerText.textContent = text;
    el.systemBanner.className = `system-banner system-banner--${tone}`;
    el.systemBanner.hidden = false;
  }

  function hideBanner() {
    el.systemBanner.hidden = true;
  }

  /* Pointer Events tactile press state for all .neu-btn controls */
  function bindGlobalPointerFeedback() {
    const release = (target) => {
      const btn = target instanceof Element ? target.closest('.neu-btn') : null;
      if (btn) btn.removeAttribute('data-pressed');
    };

    document.addEventListener('pointerdown', (event) => {
      const btn = event.target instanceof Element ? event.target.closest('.neu-btn') : null;
      if (!btn || btn.disabled || event.button !== 0) return;
      btn.setAttribute('data-pressed', 'true');
    });

    document.addEventListener('pointerup', (event) => release(event.target));
    document.addEventListener('pointercancel', (event) => release(event.target));
    document.addEventListener('pointerout', (event) => release(event.target));
  }

  function bindPressAction(button, handler) {
    let pressed = false;

    button.addEventListener('pointerdown', (event) => {
      pressed = event.button === 0 && !button.disabled;
    });

    button.addEventListener('pointercancel', () => {
      pressed = false;
    });

    button.addEventListener('pointerleave', () => {
      pressed = false;
    });

    button.addEventListener('pointerup', (event) => {
      const shouldFire = pressed && event.button === 0 && !button.disabled;
      pressed = false;
      if (!shouldFire) return;
      handler(event);
    });

    button.addEventListener('keydown', (event) => {
      if ((event.key === 'Enter' || event.key === ' ') && !button.disabled) {
        event.preventDefault();
        handler(event);
      }
    });
  }

  /* Auth & Persistent 60s Lockout Countdown */
  function showLockScreen({ syncLockout = true } = {}) {
    socket.disconnect();
    el.appShell.hidden = true;
    el.authOverlay.hidden = false;
    if (syncLockout) {
      syncLockoutCountdown();
    }
    if (!el.loginPassword.disabled) {
      el.loginPassword.focus();
    }
  }

  function showDashboard() {
    el.authOverlay.hidden = true;
    el.appShell.hidden = false;
    el.loginError.textContent = '';
    el.loginPassword.value = '';
  }

  function startLockoutCountdown(untilMs, totalSecs = 60) {
    const normalizedTotal = Math.max(1, Math.ceil(totalSecs));
    if (state.lockoutTimerId !== null && state.lockoutUntilMs === untilMs) {
      return;
    }

    state.lockoutUntilMs = untilMs;
    state.lockoutTotalSecs = normalizedTotal;
    el.lockoutProgress.parentElement.setAttribute('aria-valuemax', String(normalizedTotal));

    if (state.lockoutTimerId !== null) {
      global.clearInterval(state.lockoutTimerId);
      state.lockoutTimerId = null;
    }

    const tick = () => {
      const remainingMs = untilMs - Date.now();
      if (remainingMs <= 0) {
        global.clearInterval(state.lockoutTimerId);
        state.lockoutTimerId = null;
        state.lockoutUntilMs = 0;
        clearPersistedLockout();
        el.lockoutBox.hidden = true;
        el.loginPassword.disabled = false;
        el.loginSubmit.disabled = false;
        el.loginError.textContent = '';
        return;
      }

      const remainingSecs = Math.ceil(remainingMs / 1000);
      const ratio = Math.min(100, Math.max(0, (remainingMs / (state.lockoutTotalSecs * 1000)) * 100));
      el.lockoutBox.hidden = false;
      el.loginPassword.disabled = true;
      el.loginSubmit.disabled = true;
      el.lockoutText.textContent = `连续密码错误已锁定，请等待 ${remainingSecs} 秒`;
      el.lockoutProgress.style.width = `${ratio.toFixed(1)}%`;
      el.lockoutProgress.parentElement.setAttribute('aria-valuenow', String(remainingSecs));
    };

    tick();
    state.lockoutTimerId = global.setInterval(tick, 200);
  }

  function syncLockoutCountdown() {
    const untilMs = getPersistedLockoutUntil();
    if (untilMs > Date.now()) {
      if (state.lockoutTimerId !== null && state.lockoutUntilMs === untilMs) {
        return;
      }
      const remainingSecs = Math.ceil((untilMs - Date.now()) / 1000);
      startLockoutCountdown(untilMs, Math.max(60, remainingSecs));
    }
  }

  /* Status Capsule & Block 1: Service & Keep-Alive */
  function renderWsState(wsState) {
    const info = WS_STATE_LABELS[wsState];
    el.wsState.textContent = info.text;
    el.wsState.className = `capsule-item__value ${info.cls}`;
  }

  function applySystemStatus(status) {
    const keepAliveText = KEEP_ALIVE_LABELS[status.keep_alive_level] || status.keep_alive_level;
    const usedPct = status.vault_quota_bytes > 0
      ? Math.min(100, Math.round((status.vault_used_bytes / status.vault_quota_bytes) * 100))
      : 0;
    const health = BIT_ROT_LABELS[status.bit_rot_health] || BIT_ROT_LABELS.healthy;

    el.capKeepalive.textContent = keepAliveText;
    el.capStorageText.textContent = `${formatBytes(status.vault_used_bytes)} / ${formatBytes(status.vault_quota_bytes)} (${usedPct}%)`;
    el.capStorageFill.style.width = `${usedPct}%`;
    el.capStorageFill.className = usedPct >= 90
      ? 'neu-track__fill neu-track__fill--danger'
      : (usedPct >= 75 ? 'neu-track__fill neu-track__fill--warn' : 'neu-track__fill neu-track__fill--ok');
    el.capThroughput.textContent = `${formatBytes(status.throughput_bps)}/s`;
    el.capBitrot.textContent = status.corrupted_chunks > 0
      ? `${health.text}（${status.corrupted_chunks} 块）`
      : health.text;
    el.capBitrot.className = `capsule-item__value ${health.cls}`;

    el.svcOs.textContent = status.os.toUpperCase();
    el.svcKeepalive.textContent = keepAliveText;
    el.svcLan.textContent = `${status.lan_ip}:${status.port}`;
    el.svcUptime.textContent = formatUptime(status.uptime_secs);
    el.advMdns.textContent = status.mdns_active ? '已启用 (_lanvault._tcp.local.)' : '未启用';
    el.advCorrupted.textContent = String(status.corrupted_chunks);
  }

  function handleServerRestartingEvent(payload) {
    const drainSecs = typeof payload.drain_secs === 'number' ? payload.drain_secs : 3;
    if (state.restartCountdownId !== null) {
      global.clearInterval(state.restartCountdownId);
    }

    let left = drainSecs;
    showBanner(`服务正在优雅重启并刷盘，预计 ${left} 秒后自动重连…`, 'warn');
    state.restartCountdownId = global.setInterval(() => {
      left -= 1;
      if (left <= 0) {
        global.clearInterval(state.restartCountdownId);
        state.restartCountdownId = null;
        showBanner('服务重启指令已完成，正在等待重新连接…', 'warn');
        socket.scheduleReconnectAfter(1500);
        return;
      }
      showBanner(`服务正在优雅重启并刷盘，预计 ${left} 秒后自动重连…`, 'warn');
    }, 1000);
  }

  /* Block 2: Storage Vaults, Snapshots & Bit-Rot */
  function renderVaults(vaults) {
    state.vaults = vaults;
    el.vaultList.textContent = '';

    if (vaults.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = '尚未配置存储空间，可在下方新建单盘、串联或双盘镜像空间';
      el.vaultList.appendChild(empty);
    } else {
      const frag = document.createDocumentFragment();
      for (const vault of vaults) {
        const pct = vault.quota_bytes > 0
          ? Math.min(100, Math.round((vault.used_bytes / vault.quota_bytes) * 100))
          : 0;

        const card = document.createElement('article');
        card.className = 'vault-item';

        const head = document.createElement('div');
        head.className = 'vault-item__head';

        const name = document.createElement('h3');
        name.className = 'vault-item__name';
        name.textContent = vault.name;

        const mode = document.createElement('span');
        mode.className = 'field__label';
        mode.textContent = `${POOL_MODE_LABELS[vault.pool_mode] || vault.pool_mode} · ${vault.e2ee_enabled ? 'E2EE 已开启' : '未加密'}`;
        head.append(name, mode);

        const meta = document.createElement('div');
        meta.className = 'vault-item__meta mono';
        meta.textContent = `${formatBytes(vault.used_bytes)} / ${formatBytes(vault.quota_bytes)} (${pct}%)`;

        const track = document.createElement('div');
        track.className = 'neu-track';
        const fill = document.createElement('div');
        fill.className = pct >= 90 ? 'neu-track__fill neu-track__fill--danger' : 'neu-track__fill neu-track__fill--ok';
        fill.style.width = `${pct}%`;
        track.appendChild(fill);

        const volList = document.createElement('ul');
        volList.className = 'volume-list';
        for (const vol of vault.volumes) {
          const li = document.createElement('li');
          li.className = 'volume-row mono';

          const pathSpan = document.createElement('span');
          pathSpan.textContent = `${vol.role} · ${vol.mount_path}${vol.volume_guid ? ` (${vol.volume_guid})` : ''}`;

          const onlineSpan = document.createElement('span');
          onlineSpan.className = vol.is_online ? 'status-ok' : 'status-danger';
          onlineSpan.textContent = vol.is_online ? '在线' : '离线';

          li.append(pathSpan, onlineSpan);
          volList.appendChild(li);
        }

        card.append(head, meta, track, volList);
        frag.appendChild(card);
      }
      el.vaultList.appendChild(frag);
    }

    el.snapshotVaultSelect.textContent = '';
    for (const vault of vaults) {
      const opt = document.createElement('option');
      opt.value = vault.vault_id;
      opt.textContent = vault.name;
      el.snapshotVaultSelect.appendChild(opt);
    }

    if (vaults.length > 0) {
      const stillExists = vaults.some((v) => v.vault_id === state.selectedVaultId);
      state.selectedVaultId = stillExists ? state.selectedVaultId : vaults[0].vault_id;
      el.snapshotVaultSelect.value = state.selectedVaultId;
    } else {
      state.selectedVaultId = '';
      timeline.setSnapshots([]);
    }
  }

  function addExtraVolumeRow() {
    const defaultRole = el.vaultMode.value === 'mirror_raid1' ? 'mirror' : 'pool_ext';
    const row = document.createElement('div');
    row.className = 'extra-volume-row';

    const mountInput = document.createElement('input');
    mountInput.type = 'text';
    mountInput.className = 'neu-input mono';
    mountInput.placeholder = '附加卷挂载路径';
    mountInput.required = true;
    mountInput.dataset.field = 'mount_path';

    const guidInput = document.createElement('input');
    guidInput.type = 'text';
    guidInput.className = 'neu-input mono';
    guidInput.placeholder = 'Volume GUID（可选）';
    guidInput.dataset.field = 'volume_guid';

    const roleSelect = document.createElement('select');
    roleSelect.className = 'neu-select';
    roleSelect.dataset.field = 'role';
    roleSelect.innerHTML = '<option value="mirror">镜像盘 (mirror)</option><option value="pool_ext">串联盘 (pool_ext)</option>';
    roleSelect.value = defaultRole;

    const removeBtn = document.createElement('button');
    removeBtn.type = 'button';
    removeBtn.className = 'neu-btn neu-btn--danger neu-btn--compact';
    removeBtn.textContent = '移除';
    bindPressAction(removeBtn, () => row.remove());

    row.append(mountInput, guidInput, roleSelect, removeBtn);
    el.extraVolumesList.appendChild(row);
  }

  function appendScrubLog(message, tone = '') {
    const empty = el.scrubLog.querySelector('.empty-state');
    if (empty) empty.remove();

    const row = document.createElement('div');
    row.className = tone ? `log-entry ${tone}` : 'log-entry';

    const msgSpan = document.createElement('span');
    msgSpan.textContent = message;

    const timeSpan = document.createElement('span');
    timeSpan.textContent = new Date().toLocaleTimeString('zh-CN', { hour12: false });

    row.append(msgSpan, timeSpan);
    el.scrubLog.prepend(row);
  }

  function handleBitRotAlertEvent(payload) {
    const isCorrupted = payload.status === 'corrupted';
    const label = isCorrupted ? '检测到静默损坏块' : '损坏块已自愈修复';
    const tone = isCorrupted ? 'status-danger' : 'status-ok';
    appendScrubLog(`${label} · 空间 ${payload.vault_id} · 块 ${payload.chunk_hash.slice(0, 16)}`, tone);
    if (isCorrupted) {
      showBanner(`发现数据块静默损坏（${payload.chunk_hash.slice(0, 12)}），等待客户端重连自动修复`, 'danger');
    }
  }

  function handleSnapshotRollback(snapshot, filePath) {
    const url = buildSnapshotRollbackUrl(snapshot.vault_id, snapshot.snapshot_id, filePath);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filePath.split('/').pop() || 'rollback-file';
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    setFeedback(el.timelineFeedback, `已触发回滚下载：${filePath}`, 'status-ok');
  }

  /* Block 3: Paired Devices, 6-Digit PIN & Offline Pairing QR Matrix */
  const renderPairingQrSvg = (uri) => global.LanVault.qr.renderPairingQrSvg(uri);

  function syncGroupMemberDeviceSelect() {
    const prev = el.groupMemberDevice.value;
    el.groupMemberDevice.textContent = '';

    if (state.peers.length === 0) {
      const opt = document.createElement('option');
      opt.value = '';
      opt.textContent = '暂无已配对设备';
      el.groupMemberDevice.appendChild(opt);
      return;
    }

    for (const peer of state.peers) {
      const opt = document.createElement('option');
      opt.value = peer.device_id;
      opt.textContent = `${peer.device_name}（${peer.online ? '在线' : '离线'}）`;
      el.groupMemberDevice.appendChild(opt);
    }

    if (prev && state.peers.some((p) => p.device_id === prev)) {
      el.groupMemberDevice.value = prev;
    }
  }

  function renderPeers(data) {
    state.peers = data.peers;
    state.pairingPin = data.pairing_pin;
    state.pairingQrUri = data.pairing_qr_uri;

    el.pairingPin.textContent = data.pairing_pin;
    el.pairingUri.textContent = data.pairing_qr_uri;
    el.pairingQr.innerHTML = renderPairingQrSvg(data.pairing_qr_uri);

    const prevClipTarget = el.clipTarget.value;
    el.peerList.textContent = '';
    el.clipTarget.textContent = '';
    const allOpt = document.createElement('option');
    allOpt.value = '';
    allOpt.textContent = '全部在线已配对设备';
    el.clipTarget.appendChild(allOpt);

    syncGroupMemberDeviceSelect();

    if (data.peers.length === 0) {
      const li = document.createElement('li');
      li.className = 'empty-state';
      li.textContent = '暂无已配对设备，请在客户端输入上方 6 位配对码完成配对';
      el.peerList.appendChild(li);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const peer of data.peers) {
      const opt = document.createElement('option');
      opt.value = peer.device_id;
      opt.textContent = `${peer.device_name}（${peer.online ? '在线' : '离线'}）`;
      el.clipTarget.appendChild(opt);

      const li = document.createElement('li');
      li.className = 'peer-row';

      const info = document.createElement('div');
      info.className = 'peer-row__info';

      const name = document.createElement('span');
      name.className = 'peer-row__name';
      name.textContent = `${peer.device_name} · ${peer.os_type}`;

      const meta = document.createElement('span');
      meta.className = 'peer-row__meta mono';
      const addr = peer.last_ip ? `${peer.last_ip}:${peer.last_port}` : '暂无地址';
      const statusText = peer.online ? '在线' : '离线';
      meta.textContent = `${statusText} · ${addr} · 配对于 ${formatTimestamp(peer.paired_at)}`;
      info.append(name, meta);

      const revokeBtn = document.createElement('button');
      revokeBtn.type = 'button';
      revokeBtn.className = 'neu-btn neu-btn--danger neu-btn--compact';
      revokeBtn.textContent = '吊销信任';
      bindPressAction(revokeBtn, async () => {
        revokeBtn.disabled = true;
        try {
          await revokePeer(peer.device_id);
          setFeedback(el.peerFeedback, `已吊销设备：${peer.device_name}`, 'status-ok');
          await refreshPeers();
        } catch (err) {
          setFeedback(el.peerFeedback, err.message, 'status-danger');
          revokeBtn.disabled = false;
        }
      });

      li.append(info, revokeBtn);
      frag.appendChild(li);
    }
    el.peerList.appendChild(frag);

    if (prevClipTarget && data.peers.some((p) => p.device_id === prevClipTarget)) {
      el.clipTarget.value = prevClipTarget;
    }
  }

  /* Block 5: v2 Device Groups, Group Sync Rules & Catalog/Version Browser */
  function renderCapabilities(caps) {
    state.capabilities = caps;
    if (!caps) {
      el.v2CapabilitiesBadge.textContent = 'v2';
      return;
    }
    const ver = caps.protocol_version || 2;
    const featCount = Array.isArray(caps.features) ? caps.features.length : 0;
    el.v2CapabilitiesBadge.textContent = featCount > 0 ? `协议 v${ver} · ${featCount} 项能力` : `协议 v${ver}`;
  }

  function renderGroupsList() {
    el.groupsList.textContent = '';

    if (state.groups.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = '暂无设备组，可在上方新建设备组以管理成员与同步规则';
      el.groupsList.appendChild(empty);
      el.groupDetailPanel.hidden = true;
      closeCatalogBrowser();
      return;
    }

    const frag = document.createDocumentFragment();
    for (const group of state.groups) {
      const selected = group.group_id === state.selectedGroupId;
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'neu-btn neu-btn--pill neu-btn--compact';
      btn.setAttribute('role', 'listitem');
      btn.setAttribute('aria-pressed', selected ? 'true' : 'false');
      btn.setAttribute('aria-label', `选择设备组 ${group.name}`);
      btn.textContent = `${group.name} (rev ${group.revision})`;
      bindPressAction(btn, async () => {
        if (state.selectedGroupId === group.group_id) return;
        state.selectedGroupId = group.group_id;
        closeCatalogBrowser();
        renderGroupsList();
        try {
          await loadSelectedGroupDetails();
        } catch (err) {
          setFeedback(el.groupFeedback, err.message, 'status-danger');
        }
      });
      frag.appendChild(btn);
    }
    el.groupsList.appendChild(frag);

    const activeGroup = state.groups.find((g) => g.group_id === state.selectedGroupId);
    if (!activeGroup) {
      el.groupDetailPanel.hidden = true;
      return;
    }

    el.groupDetailPanel.hidden = false;
    el.groupDetailName.textContent = activeGroup.name;
    el.groupDetailRev.textContent = `rev ${activeGroup.revision}`;
    el.renameGroupInput.value = activeGroup.name;
  }

  function renderGroupMembers() {
    el.groupMembersList.textContent = '';

    if (state.groupMembers.length === 0) {
      const li = document.createElement('li');
      li.className = 'empty-state';
      li.textContent = '当前设备组暂无成员';
      el.groupMembersList.appendChild(li);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const member of state.groupMembers) {
      const li = document.createElement('li');
      li.className = 'member-row';

      const info = document.createElement('div');
      info.className = 'member-row__info';

      const name = document.createElement('span');
      name.className = 'member-row__name';
      const displayName = resolveDeviceName(member.device_id);
      const roleLabel = member.role === 'coordinator' ? '协调者' : '成员';
      name.textContent = `${displayName} · ${roleLabel}`;

      const meta = document.createElement('span');
      meta.className = 'member-row__meta mono';
      meta.textContent = `${member.device_id} · 加入于 ${formatTimestamp(member.added_at)}`;

      info.append(name, meta);

      const removeBtn = document.createElement('button');
      removeBtn.type = 'button';
      removeBtn.className = 'neu-btn neu-btn--danger neu-btn--compact';
      removeBtn.setAttribute('aria-label', `从设备组移除成员 ${displayName}`);
      removeBtn.textContent = '移除';
      bindPressAction(removeBtn, async () => {
        removeBtn.disabled = true;
        try {
          await removeGroupMember(state.selectedGroupId, member.device_id);
          setFeedback(el.groupFeedback, `已移除成员：${displayName}`, 'status-ok');
          await refreshGroups();
        } catch (err) {
          setFeedback(el.groupFeedback, err.message, 'status-danger');
          removeBtn.disabled = false;
        }
      });

      li.append(info, removeBtn);
      frag.appendChild(li);
    }
    el.groupMembersList.appendChild(frag);
  }

  function renderGroupRules() {
    el.groupRulesList.textContent = '';

    if (state.groupRules.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = '当前设备组暂无同步规则';
      el.groupRulesList.appendChild(empty);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const rule of state.groupRules) {
      const card = document.createElement('article');
      card.className = 'rule-card';

      const head = document.createElement('div');
      head.className = 'rule-card__head';

      const title = document.createElement('span');
      title.className = 'rule-card__name';
      title.textContent = rule.name || rule.rule_id;

      const modeInfo = SYNC_MODE_LABELS[rule.mode] || { text: rule.mode, cls: 'status-badge--neutral' };
      const modeBadge = document.createElement('span');
      modeBadge.className = `status-badge ${modeInfo.cls}`;
      modeBadge.textContent = modeInfo.text;

      head.append(title, modeBadge);

      const sourceLine = document.createElement('div');
      sourceLine.className = 'rule-card__path mono';
      sourceLine.textContent = `源：${resolveDeviceName(rule.source_device_id)} (${rule.source_root}) · 保留 ${rule.archive_retention_days} 天 · ${rule.enabled ? '已启用' : '已暂停'}`;

      card.append(head, sourceLine);

      const bindings = state.ruleBindingsMap.get(rule.rule_id) || [];
      const bindingList = document.createElement('div');
      bindingList.className = 'binding-list';

      if (bindings.length === 0) {
        const noBind = document.createElement('p');
        noBind.className = 'empty-state';
        noBind.textContent = '尚未绑定目标设备';
        bindingList.appendChild(noBind);
      } else {
        for (const binding of bindings) {
          const row = document.createElement('div');
          row.className = 'binding-row';

          const info = document.createElement('div');
          info.className = 'binding-row__info';

          const targetLine = document.createElement('div');
          targetLine.className = 'block-title-row';

          const targetName = document.createElement('span');
          targetName.className = 'binding-row__target';
          const targetLabel = resolveDeviceName(binding.target_device_id);
          const destDetail = binding.target_root || binding.target_vault_id || '默认存储';
          targetName.textContent = `→ ${targetLabel} (${destDetail})`;

          const stInfo = BINDING_STATUS_LABELS[binding.status] || {
            text: binding.status || '未知',
            cls: 'status-badge--neutral',
          };
          const statusBadge = document.createElement('span');
          statusBadge.className = `status-badge ${stInfo.cls}`;
          statusBadge.textContent = stInfo.text;

          targetLine.append(targetName, statusBadge);

          const meta = document.createElement('span');
          meta.className = 'binding-row__meta mono';
          const errPart = binding.last_error ? ` · 错误：${binding.last_error}` : '';
          meta.textContent = `rev ${binding.applied_revision} · 更新于 ${formatTimestamp(binding.updated_at)}${errPart}`;

          info.append(targetLine, meta);

          const browseBtn = document.createElement('button');
          browseBtn.type = 'button';
          browseBtn.className = 'neu-btn neu-btn--compact';
          browseBtn.setAttribute('aria-label', `浏览 ${targetLabel} 的目录与历史版本`);
          browseBtn.setAttribute('aria-pressed', state.selectedBindingId === binding.binding_id ? 'true' : 'false');
          browseBtn.textContent = '浏览目录与版本';
          bindPressAction(browseBtn, async () => {
            await openBindingCatalog(binding, rule);
          });

          row.append(info, browseBtn);
          bindingList.appendChild(row);
        }
      }

      card.appendChild(bindingList);
      frag.appendChild(card);
    }
    el.groupRulesList.appendChild(frag);
  }

  function closeCatalogBrowser() {
    state.selectedBindingId = '';
    state.selectedBindingLabel = '';
    state.catalogEntries = [];
    state.catalogNextCursor = null;
    state.catalogSelectedPathKey = '';
    state.catalogVersions = [];
    state.versionsNextArchivedAt = null;
    state.versionsNextVersionId = null;
    el.catalogBrowserPanel.hidden = true;
    el.btnCatalogLoadMore.hidden = true;
    el.btnVersionsLoadMore.hidden = true;
  }

  async function openBindingCatalog(binding, rule) {
    state.selectedBindingId = binding.binding_id;
    const targetName = resolveDeviceName(binding.target_device_id);
    state.selectedBindingLabel = `${rule.name || rule.rule_id} → ${targetName}`;
    state.catalogSelectedPathKey = '';
    state.catalogVersions = [];
    state.versionsNextArchivedAt = null;
    state.versionsNextVersionId = null;

    el.catalogBindingTitle.textContent = `目录与历史版本：${state.selectedBindingLabel}`;
    el.catalogBrowserPanel.hidden = false;
    el.catalogVersionsTitle.textContent = '历史归档版本';
    el.catalogVersionsList.innerHTML = '<p class="empty-state">点击左侧目录条目的“查看版本”以浏览历史版本</p>';
    el.btnVersionsLoadMore.hidden = true;

    renderGroupRules();
    await loadBindingCatalogEntries(true);
  }

  async function loadBindingCatalogEntries(reset = false) {
    if (!state.selectedBindingId) return;
    const afterPathKey = reset ? null : state.catalogNextCursor;
    const sourceDeletedOnly = el.catalogDeletedOnly.checked;

    try {
      const res = await listBindingCatalog(state.selectedBindingId, afterPathKey, 100, sourceDeletedOnly);
      const entries = Array.isArray(res.entries) ? res.entries : [];
      state.catalogEntries = reset ? entries : state.catalogEntries.concat(entries);
      state.catalogNextCursor = entries.length >= 100 ? (res.next_cursor || null) : null;
      renderCatalogEntries();
      setFeedback(
        el.catalogFeedback,
        `已加载 ${state.catalogEntries.length} 条目录记录${sourceDeletedOnly ? '（仅看源已删除）' : ''}`,
        'status-ok',
      );
    } catch (err) {
      setFeedback(el.catalogFeedback, err.message, 'status-danger');
    }
  }

  function renderCatalogEntries() {
    el.catalogEntriesList.textContent = '';
    el.btnCatalogLoadMore.hidden = !state.catalogNextCursor;

    if (state.catalogEntries.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = el.catalogDeletedOnly.checked
        ? '当前绑定没有标记为“源已删除”的条目'
        : '当前绑定暂无增量目录条目';
      el.catalogEntriesList.appendChild(empty);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const entry of state.catalogEntries) {
      const row = document.createElement('div');
      row.className = 'catalog-entry-row';

      const info = document.createElement('div');
      info.className = 'catalog-entry-row__info';

      const pathRow = document.createElement('div');
      pathRow.className = 'block-title-row';

      const pathSpan = document.createElement('span');
      pathSpan.className = 'catalog-entry-row__path mono';
      pathSpan.textContent = entry.display_path;
      pathRow.appendChild(pathSpan);

      if (entry.source_deleted) {
        const delBadge = document.createElement('span');
        delBadge.className = 'status-badge status-badge--danger';
        delBadge.textContent = '源已删除';
        pathRow.appendChild(delBadge);
      }

      const meta = document.createElement('span');
      meta.className = 'catalog-entry-row__meta mono';
      meta.textContent = `${entry.entry_kind} · ${formatBytes(entry.size)} · rev ${entry.updated_revision} · ${formatTimestamp(entry.updated_at)}`;

      info.append(pathRow, meta);

      const verBtn = document.createElement('button');
      verBtn.type = 'button';
      verBtn.className = 'neu-btn neu-btn--compact';
      verBtn.setAttribute('aria-label', `查看 ${entry.display_path} 的历史版本`);
      verBtn.setAttribute('aria-pressed', state.catalogSelectedPathKey === entry.path_key ? 'true' : 'false');
      verBtn.textContent = '查看版本';
      bindPressAction(verBtn, async () => {
        state.catalogSelectedPathKey = entry.path_key;
        el.catalogVersionsTitle.textContent = `历史版本：${entry.display_path}`;
        renderCatalogEntries();
        await loadBindingVersions(entry.path_key, true);
      });

      row.append(info, verBtn);
      frag.appendChild(row);
    }
    el.catalogEntriesList.appendChild(frag);
  }

  async function loadBindingVersions(pathKey, reset = false) {
    if (!state.selectedBindingId) return;
    const afterArchivedAt = reset ? null : state.versionsNextArchivedAt;
    const afterVersionId = reset ? null : state.versionsNextVersionId;

    try {
      const res = await listBindingVersions(
        state.selectedBindingId,
        pathKey || null,
        afterArchivedAt,
        afterVersionId,
        50,
      );
      const versions = Array.isArray(res.versions) ? res.versions : [];
      state.catalogVersions = reset ? versions : state.catalogVersions.concat(versions);
      state.versionsNextArchivedAt = versions.length >= 50 ? (res.next_after_archived_at ?? null) : null;
      state.versionsNextVersionId = versions.length >= 50 ? (res.next_after_version_id || null) : null;
      renderCatalogVersions();
    } catch (err) {
      setFeedback(el.catalogFeedback, err.message, 'status-danger');
    }
  }

  function renderCatalogVersions() {
    el.catalogVersionsList.textContent = '';
    el.btnVersionsLoadMore.hidden = !state.versionsNextVersionId;

    if (state.catalogVersions.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = '该路径暂无归档历史版本';
      el.catalogVersionsList.appendChild(empty);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const ver of state.catalogVersions) {
      const row = document.createElement('div');
      row.className = 'version-row';

      const info = document.createElement('div');
      info.className = 'version-row__info';

      const pathSpan = document.createElement('span');
      pathSpan.className = 'catalog-entry-row__path mono';
      pathSpan.textContent = `${ver.display_path} (${ver.archive_reason})`;

      const meta = document.createElement('span');
      meta.className = 'catalog-entry-row__meta mono';
      const recycledText = ver.recycled_at ? ` · 已回收于 ${formatTimestamp(ver.recycled_at)}` : '';
      meta.textContent = `${formatBytes(ver.size)} · 归档于 ${formatTimestamp(ver.archived_at)}${recycledText}`;

      info.append(pathSpan, meta);

      if (ver.recycled_at) {
        const badge = document.createElement('span');
        badge.className = 'status-badge status-badge--neutral';
        badge.textContent = '已回收';
        row.append(info, badge);
      } else {
        const downloadLink = document.createElement('a');
        downloadLink.className = 'neu-btn neu-btn--compact';
        downloadLink.href = buildVersionDownloadUrl(state.selectedBindingId, ver.version_id);
        downloadLink.setAttribute('download', '');
        downloadLink.setAttribute('rel', 'noopener noreferrer');
        downloadLink.setAttribute('aria-label', `下载历史版本 ${ver.display_path}`);
        downloadLink.textContent = '下载版本';
        row.append(info, downloadLink);
      }

      frag.appendChild(row);
    }
    el.catalogVersionsList.appendChild(frag);
  }

  async function loadSelectedGroupDetails() {
    if (!state.selectedGroupId) return;
    const groupId = state.selectedGroupId;
    const [membersRes, rulesRes] = await Promise.all([
      listGroupMembers(groupId),
      listGroupRules(groupId),
    ]);
    if (state.selectedGroupId !== groupId) return;

    state.groupMembers = Array.isArray(membersRes.members) ? membersRes.members : [];
    state.groupRules = Array.isArray(rulesRes.rules) ? rulesRes.rules : [];

    const bindingsResults = await Promise.all(
      state.groupRules.map((rule) =>
        listRuleBindings(rule.rule_id)
          .then((res) => ({ ruleId: rule.rule_id, bindings: Array.isArray(res.bindings) ? res.bindings : [] }))
          .catch(() => ({ ruleId: rule.rule_id, bindings: [] }))),
    );
    if (state.selectedGroupId !== groupId) return;

    state.ruleBindingsMap = new Map();
    for (const item of bindingsResults) {
      state.ruleBindingsMap.set(item.ruleId, item.bindings);
    }

    renderGroupMembers();
    renderGroupRules();
  }

  async function refreshGroups() {
    const res = await listGroups();
    state.groups = Array.isArray(res.groups) ? res.groups : [];
    if (state.groups.length > 0) {
      const stillExists = state.groups.some((g) => g.group_id === state.selectedGroupId);
      state.selectedGroupId = stillExists ? state.selectedGroupId : state.groups[0].group_id;
    } else {
      state.selectedGroupId = '';
    }
    renderGroupsList();
    if (state.selectedGroupId) {
      await loadSelectedGroupDetails();
    }
  }

  async function handleGroupSyncUpdateEvent(payload) {
    setFeedback(
      el.groupFeedback,
      `同步更新：规则 ${payload.rule_id.slice(0, 8)} 推进至 rev ${payload.new_revision}（${payload.applied_ops} 项变更）`,
      'status-ok',
    );
    try {
      if (state.selectedGroupId && payload.group_id === state.selectedGroupId) {
        await loadSelectedGroupDetails();
        if (state.selectedBindingId && payload.binding_id === state.selectedBindingId) {
          await loadBindingCatalogEntries(true);
        }
      } else if (state.groups.length === 0) {
        await refreshGroups();
      }
    } catch {
      // Non-fatal background refresh; manual refresh remains available.
    }
  }

  /* Block 4: Non-Blocking LAN Clipboard Composer & Live Feed */
  function isTargetDelivered(status) {
    return status === 'delivered' || status === 'accepted' || status === 'acked';
  }

  function computeMessageStatus(targets, deliveredTo) {
    if (!Array.isArray(targets) || targets.length === 0) {
      return deliveredTo > 0 ? 'sent' : 'failed';
    }
    let deliveredCount = 0;
    let failedCount = 0;
    let sendingCount = 0;
    for (const t of targets) {
      if (isTargetDelivered(t.status)) {
        deliveredCount += 1;
      } else if (t.status === 'sending') {
        sendingCount += 1;
      } else {
        failedCount += 1;
      }
    }
    if (sendingCount > 0) return 'sending';
    if (failedCount === 0 && deliveredCount > 0) return 'sent';
    if (deliveredCount > 0 && failedCount > 0) return 'partial_failed';
    return 'failed';
  }

  function mergeTargetsAck(existingTargets, incomingTargets) {
    const byId = new Map();
    if (Array.isArray(existingTargets)) {
      for (const t of existingTargets) {
        if (t && t.target_device_id) {
          byId.set(t.target_device_id, { ...t });
        }
      }
    }
    if (Array.isArray(incomingTargets)) {
      for (const t of incomingTargets) {
        if (t && t.target_device_id) {
          const prev = byId.get(t.target_device_id);
          // Preserve already-delivered target status during partial-target retries
          if (prev && isTargetDelivered(prev.status) && !isTargetDelivered(t.status)) {
            continue;
          }
          byId.set(t.target_device_id, {
            target_device_id: t.target_device_id,
            status: t.status || 'failed',
            error_code: t.error_code || null,
            error_message: t.error_message || null,
          });
        }
      }
    }
    return Array.from(byId.values());
  }

  function mapOutboxStatus(rawStatus) {
    if (rawStatus === 'delivered' || rawStatus === 'sent' || rawStatus === 'accepted') return 'sent';
    if (rawStatus === 'partial_failed') return 'partial_failed';
    if (rawStatus === 'sending' || rawStatus === 'queued') return 'sending';
    return 'failed';
  }

  function mergeClipboardHistory(historyRes) {
    if (!historyRes) return;
    const outbox = Array.isArray(historyRes.outbox) ? historyRes.outbox : [];
    const transfers = Array.isArray(historyRes.transfers) ? historyRes.transfers : [];

    for (const rec of outbox) {
      if (!rec.client_msg_id) continue;
      const existing = state.clipboardMessages.get(rec.client_msg_id);
      if (existing && existing.status === 'sending') {
        continue;
      }
      if (existing && existing.direction === 'outgoing') {
        continue;
      }
      state.clipboardMessages.set(rec.client_msg_id, {
        client_msg_id: rec.client_msg_id,
        local_seq: rec.local_seq || ++state.clipboardSeq,
        direction: 'outgoing',
        from_device_id: '本机管理台',
        kind: rec.kind || 'text',
        content: rec.content || rec.title || '',
        filename: rec.kind === 'file' ? (rec.title || null) : null,
        encrypted: false,
        timestamp: rec.created_at || Math.floor(Date.now() / 1000),
        status: mapOutboxStatus(rec.overall_status),
        frozen_targets: [],
        targets: [],
        delivered_to: 0,
        error_message: null,
      });
    }

    for (const tr of transfers) {
      const title = typeof tr.title === 'string' ? tr.title : '';
      const clientMsgId = title.startsWith('clipboard:')
        ? title.slice('clipboard:'.length)
        : `transfer-${tr.id}`;
      const existing = state.clipboardMessages.get(clientMsgId);
      if (existing && existing.status === 'sending') {
        continue;
      }
      if (existing) {
        continue;
      }
      const isOutgoing = tr.direction === 'send';
      state.clipboardMessages.set(clientMsgId, {
        client_msg_id: clientMsgId,
        local_seq: ++state.clipboardSeq,
        direction: isOutgoing ? 'outgoing' : 'incoming',
        from_device_id: tr.peer_name || '局域网设备',
        kind: tr.kind || 'text',
        content: tr.content || `${title}${tr.subtitle ? ` (${tr.subtitle})` : ''}`,
        filename: tr.kind === 'file' ? title : null,
        encrypted: false,
        timestamp: tr.created_at || Math.floor(Date.now() / 1000),
        status: isOutgoing ? 'sent' : 'received',
        frozen_targets: [],
        targets: [],
        delivered_to: 0,
        error_message: null,
      });
    }

    renderClipboardFeed();
  }

  function handleClipboardReceivedEvent(payload) {
    const key = payload.client_msg_id || `rx-${payload.timestamp || Date.now()}-${++state.clipboardSeq}`;
    const existing = state.clipboardMessages.get(key);
    // Never overwrite an in-flight sending item or our own outgoing plaintext entry
    if (existing && (existing.status === 'sending' || existing.direction === 'outgoing')) {
      return;
    }

    state.clipboardMessages.set(key, {
      client_msg_id: key,
      local_seq: ++state.clipboardSeq,
      direction: 'incoming',
      from_device_id: payload.from_device_id || '未知设备',
      kind: payload.kind || 'text',
      content: payload.content || '',
      filename: payload.filename || null,
      encrypted: payload.encrypted === true,
      timestamp: payload.timestamp || Math.floor(Date.now() / 1000),
      status: 'received',
      frozen_targets: [],
      targets: [],
      delivered_to: 0,
      error_message: null,
    });
    renderClipboardFeed();
  }

  function handleClipboardStatusEvent(payload) {
    if (!payload || !payload.client_msg_id) return;
    const existing = state.clipboardMessages.get(payload.client_msg_id);
    // Never overwrite an in-flight sending item
    if (existing && existing.status === 'sending') {
      return;
    }

    const mergedTargets = mergeTargetsAck(existing ? existing.targets : [], payload.targets);
    const deliveredTo = typeof payload.delivered_to === 'number'
      ? payload.delivered_to
      : mergedTargets.filter((t) => isTargetDelivered(t.status)).length;
    const nextStatus = computeMessageStatus(mergedTargets, deliveredTo);

    if (existing) {
      existing.targets = mergedTargets;
      existing.delivered_to = deliveredTo;
      existing.status = nextStatus;
      if (payload.server_ack_at) {
        existing.timestamp = payload.server_ack_at;
      }
      renderClipboardFeed();
      return;
    }

    state.clipboardMessages.set(payload.client_msg_id, {
      client_msg_id: payload.client_msg_id,
      local_seq: ++state.clipboardSeq,
      direction: 'outgoing',
      from_device_id: payload.from_device_id || '本机管理台',
      kind: 'text',
      content: `消息 ${payload.client_msg_id.slice(0, 8)}`,
      filename: null,
      encrypted: false,
      timestamp: payload.server_ack_at || Math.floor(Date.now() / 1000),
      status: nextStatus,
      frozen_targets: mergedTargets.map((t) => t.target_device_id),
      targets: mergedTargets,
      delivered_to: deliveredTo,
      error_message: null,
    });
    renderClipboardFeed();
  }

  async function dispatchClipboardSend(msg, targetIds) {
    const payload = {
      client_msg_id: msg.client_msg_id,
      target_device_ids: targetIds.length > 0 ? targetIds : null,
      kind: msg.kind,
      content: msg.content,
      filename: msg.filename,
    };

    try {
      const res = await sendClipboardV2(payload);
      const serverTargets = Array.isArray(res.targets) ? res.targets : [];
      msg.targets = mergeTargetsAck(msg.targets, serverTargets);
      const deliveredCount = msg.targets.length > 0
        ? msg.targets.filter((t) => isTargetDelivered(t.status)).length
        : (typeof res.delivered_to === 'number' ? res.delivered_to : 0);
      msg.delivered_to = deliveredCount;
      msg.timestamp = res.server_ack_at || res.timestamp || msg.timestamp;
      msg.status = computeMessageStatus(msg.targets, deliveredCount);
      msg.error_message = msg.targets.length === 0 && deliveredCount === 0
        ? '当前没有在线目标设备'
        : null;

      renderClipboardFeed();
      if (msg.status === 'sent') {
        setFeedback(el.clipFeedback, `已送达 ${deliveredCount} 台设备`, 'status-ok');
      } else if (msg.status === 'partial_failed') {
        setFeedback(el.clipFeedback, `部分目标失败（已送达 ${deliveredCount}/${msg.targets.length}），可点击“重试失败目标”`, 'status-warn');
      } else {
        setFeedback(el.clipFeedback, msg.error_message || '目标设备发送失败，可点击“重试失败目标”', 'status-danger');
      }
    } catch (err) {
      msg.targets = msg.targets.map((t) => (
        t.status === 'sending'
          ? { ...t, status: 'failed', error_code: err.code || 'network_error', error_message: err.message }
          : t
      ));
      msg.error_message = err.message;
      msg.status = computeMessageStatus(msg.targets, msg.delivered_to);
      if (msg.status === 'sending') {
        msg.status = 'failed';
      }
      renderClipboardFeed();
      setFeedback(el.clipFeedback, `发送失败：${err.message}`, 'status-danger');
    }
  }

  function retryFailedTargets(clientMsgId) {
    const msg = state.clipboardMessages.get(clientMsgId);
    if (!msg || msg.status === 'sending') return;

    const failedTargets = msg.targets
      .filter((t) => !isTargetDelivered(t.status))
      .map((t) => t.target_device_id);
    const retryList = failedTargets.length > 0 ? failedTargets : msg.frozen_targets.slice();

    msg.status = 'sending';
    msg.error_message = null;
    if (retryList.length > 0) {
      const retrySet = new Set(retryList);
      msg.targets = msg.targets.map((t) => (
        retrySet.has(t.target_device_id)
          ? { ...t, status: 'sending', error_code: null, error_message: null }
          : t
      ));
    }

    renderClipboardFeed();
    dispatchClipboardSend(msg, retryList);
  }

  function renderClipboardFeed() {
    el.clipboardFeed.textContent = '';

    const items = Array.from(state.clipboardMessages.values());
    if (items.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'empty-state';
      empty.textContent = '等待局域网设备推送剪贴板内容';
      el.clipboardFeed.appendChild(empty);
      return;
    }

    items.sort((a, b) => (b.timestamp - a.timestamp) || (b.local_seq - a.local_seq));

    const frag = document.createDocumentFragment();
    for (const msg of items) {
      const card = document.createElement('article');
      card.className = 'clip-item';
      card.dataset.clientMsgId = msg.client_msg_id;

      const head = document.createElement('div');
      head.className = 'clip-item__head mono';

      const metaGroup = document.createElement('div');
      metaGroup.className = 'clip-item__head-meta';

      const statusMeta = CLIP_STATUS_LABELS[msg.status] || CLIP_STATUS_LABELS.received;
      const badge = document.createElement('span');
      badge.className = `status-badge ${statusMeta.cls}`;
      badge.textContent = statusMeta.text;

      const source = document.createElement('span');
      const fileLabel = msg.kind === 'file' && msg.filename ? ` · 文件 ${msg.filename}` : '';
      const fromLabel = msg.direction === 'outgoing' ? '本机发送' : `来自 ${resolveDeviceName(msg.from_device_id)}`;
      source.textContent = `${fromLabel}${fileLabel} · ${formatTimestamp(msg.timestamp)}`;

      metaGroup.append(badge, source);

      const actions = document.createElement('div');
      actions.className = 'clip-item__actions';

      if (msg.status === 'failed' || msg.status === 'partial_failed') {
        const retryBtn = document.createElement('button');
        retryBtn.type = 'button';
        retryBtn.className = 'neu-btn neu-btn--danger neu-btn--compact clip-retry-btn';
        retryBtn.setAttribute('aria-label', '重试失败目标');
        retryBtn.textContent = '重试失败目标';
        bindPressAction(retryBtn, () => retryFailedTargets(msg.client_msg_id));
        actions.appendChild(retryBtn);
      }

      const copyBtn = document.createElement('button');
      copyBtn.type = 'button';
      copyBtn.className = 'neu-btn neu-btn--compact';
      copyBtn.setAttribute('aria-label', '复制剪贴板内容');
      copyBtn.textContent = msg.encrypted ? '需客户端解密' : '复制';
      copyBtn.disabled = msg.encrypted === true;
      bindPressAction(copyBtn, async () => {
        if (msg.encrypted === true) return;
        const ok = await copyTextToClipboard(msg.content);
        setFeedback(
          el.clipFeedback,
          ok ? '已复制剪贴板内容' : '复制失败，请手动选中复制',
          ok ? 'status-ok' : 'status-danger',
        );
      });
      actions.appendChild(copyBtn);

      head.append(metaGroup, actions);

      const body = document.createElement('pre');
      body.className = 'clip-item__body';
      body.textContent = msg.encrypted === true
        ? '内容已端到端加密，请在已配对的原生客户端中查看。'
        : msg.content;

      card.append(head, body);

      if ((Array.isArray(msg.targets) && msg.targets.length > 0) || msg.error_message) {
        const targetBox = document.createElement('div');
        targetBox.className = 'clip-targets mono';

        if (Array.isArray(msg.targets) && msg.targets.length > 0) {
          for (const t of msg.targets) {
            const tRow = document.createElement('div');
            tRow.className = 'clip-target-row';

            const left = document.createElement('span');
            left.textContent = `目标：${resolveDeviceName(t.target_device_id)}`;

            const right = document.createElement('span');
            const stText = TARGET_STATUS_LABELS[t.status] || t.status;
            const errDetail = t.error_message || t.error_code ? `（${t.error_message || t.error_code}）` : '';
            right.className = isTargetDelivered(t.status)
              ? 'status-ok'
              : (t.status === 'sending' ? '' : 'status-danger');
            right.textContent = `${stText}${errDetail}`;

            tRow.append(left, right);
            targetBox.appendChild(tRow);
          }
        } else if (msg.error_message) {
          const errRow = document.createElement('div');
          errRow.className = 'clip-target-row status-danger';
          errRow.textContent = msg.error_message;
          targetBox.appendChild(errRow);
        }

        card.appendChild(targetBox);
      }

      frag.appendChild(card);
    }

    el.clipboardFeed.appendChild(frag);
  }

  /* Data Loaders */
  async function refreshSystemStatus() {
    const status = await getSystemStatus();
    applySystemStatus(status);
  }

  async function refreshSnapshotsForSelectedVault() {
    if (!state.selectedVaultId) {
      timeline.setSnapshots([]);
      return;
    }
    const res = await listSnapshots(state.selectedVaultId);
    timeline.setSnapshots(res.snapshots);
  }

  async function refreshVaults() {
    const res = await listVaults();
    renderVaults(res.vaults);
    await refreshSnapshotsForSelectedVault();
  }

  async function refreshPeers() {
    const res = await listPeers();
    renderPeers(res);
  }

  async function refreshClipboardHistory() {
    const historyRes = await getClipboardHistory(50);
    mergeClipboardHistory(historyRes);
  }

  async function loadAllDashboardData() {
    const [status, vaultRes, peerRes, capsRes, groupsRes, historyRes] = await Promise.all([
      getSystemStatus(),
      listVaults(),
      listPeers(),
      getCapabilities().catch(() => null),
      listGroups().catch(() => ({ groups: [] })),
      getClipboardHistory(50).catch(() => null),
    ]);
    applySystemStatus(status);
    renderVaults(vaultRes.vaults);
    renderPeers(peerRes);
    renderCapabilities(capsRes);

    state.groups = Array.isArray(groupsRes.groups) ? groupsRes.groups : [];
    state.selectedGroupId = state.groups.length > 0 ? state.groups[0].group_id : '';
    renderGroupsList();

    if (historyRes) {
      mergeClipboardHistory(historyRes);
    }

    await Promise.all([
      refreshSnapshotsForSelectedVault(),
      state.selectedGroupId ? loadSelectedGroupDetails().catch(() => {}) : Promise.resolve(),
    ]);
  }

  /* Event Bindings */
  function bindAppEvents() {
    el.loginForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      if (getPersistedLockoutUntil() > Date.now()) {
        syncLockoutCountdown();
        return;
      }

      const password = el.loginPassword.value;
      el.loginSubmit.disabled = true;
      el.loginError.textContent = '';

      try {
        await login(password);
        showDashboard();
        await loadAllDashboardData();
        socket.connect();
      } catch (err) {
        el.loginError.textContent = err.message;
        if (!(err instanceof ApiError && err.status === 429)) {
          el.loginSubmit.disabled = false;
          el.loginPassword.select();
        }
      }
    });

    bindPressAction(el.systemBannerDismiss, hideBanner);

    bindPressAction(el.btnRefreshStatus, async () => {
      try {
        await refreshSystemStatus();
        setFeedback(el.svcFeedback, '状态已刷新', 'status-ok');
      } catch (err) {
        setFeedback(el.svcFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnGracefulRestart, async () => {
      el.btnGracefulRestart.disabled = true;
      try {
        const res = await restartSystem();
        setFeedback(el.svcFeedback, `已触发优雅重启，刷盘等待 ${res.drain_secs} 秒…`, 'status-warn');
        handleServerRestartingEvent({ drain_secs: res.drain_secs });
      } catch (err) {
        setFeedback(el.svcFeedback, err.message, 'status-danger');
      } finally {
        global.setTimeout(() => {
          el.btnGracefulRestart.disabled = false;
        }, 3000);
      }
    });

    bindPressAction(el.btnRefreshVaults, async () => {
      try {
        await refreshVaults();
        setFeedback(el.vaultFeedback, '存储空间与快照已刷新', 'status-ok');
      } catch (err) {
        setFeedback(el.vaultFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnTriggerScrub, async () => {
      el.btnTriggerScrub.disabled = true;
      try {
        const res = await triggerScrub();
        const tone = res.corrupted_chunks > 0 ? 'status-danger' : 'status-ok';
        appendScrubLog(
          `巡检 ${res.scrub_id}（${res.status}）· 校验 ${res.checked_chunks} 块 · 损坏 ${res.corrupted_chunks} · 自愈 ${res.healed_chunks}`,
          tone,
        );
      } catch (err) {
        appendScrubLog(`手动巡检失败：${err.message}`, 'status-danger');
      } finally {
        el.btnTriggerScrub.disabled = false;
      }
    });

    el.vaultMode.addEventListener('change', () => {
      const needsExtra = el.vaultMode.value !== 'single';
      el.extraVolumesSection.hidden = !needsExtra;
      if (needsExtra && el.extraVolumesList.children.length === 0) {
        addExtraVolumeRow();
      }
      if (!needsExtra) {
        el.extraVolumesList.textContent = '';
      }
    });

    bindPressAction(el.btnAddExtraVol, addExtraVolumeRow);

    el.createVaultForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const formData = new FormData(el.createVaultForm);
      const poolMode = String(formData.get('pool_mode'));
      const quotaGb = Number(formData.get('quota_gb'));
      const volumeGuidRaw = String(formData.get('volume_guid') || '').trim();

      const extraVolumes = [];
      if (poolMode !== 'single') {
        const rows = el.extraVolumesList.querySelectorAll('.extra-volume-row');
        for (const row of rows) {
          const mountPath = row.querySelector('[data-field="mount_path"]').value.trim();
          const guidVal = row.querySelector('[data-field="volume_guid"]').value.trim();
          const role = row.querySelector('[data-field="role"]').value;
          if (mountPath) {
            extraVolumes.push({
              mount_path: mountPath,
              volume_guid: guidVal || null,
              role,
            });
          }
        }
      }

      const payload = {
        name: String(formData.get('name')).trim(),
        mount_path: String(formData.get('mount_path')).trim(),
        volume_guid: volumeGuidRaw || null,
        pool_mode: poolMode,
        quota_bytes: Math.round(quotaGb * 1024 * 1024 * 1024),
        e2ee_enabled: el.createVaultForm.elements.e2ee_enabled.checked,
        extra_volumes: extraVolumes,
      };

      try {
        await createVault(payload);
        setFeedback(el.vaultFeedback, `已创建存储空间：${payload.name}`, 'status-ok');
        el.createVaultForm.reset();
        el.extraVolumesList.textContent = '';
        el.extraVolumesSection.hidden = true;
        el.createVaultDrawer.open = false;
        await refreshVaults();
      } catch (err) {
        setFeedback(el.vaultFeedback, err.message, 'status-danger');
      }
    });

    el.snapshotVaultSelect.addEventListener('change', async () => {
      state.selectedVaultId = el.snapshotVaultSelect.value;
      try {
        await refreshSnapshotsForSelectedVault();
      } catch (err) {
        setFeedback(el.timelineFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnRefreshPeers, async () => {
      try {
        await refreshPeers();
        setFeedback(el.peerFeedback, '设备列表与配对码已刷新', 'status-ok');
      } catch (err) {
        setFeedback(el.peerFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnCopyPin, async () => {
      if (!state.pairingPin) return;
      const ok = await copyTextToClipboard(state.pairingPin);
      setFeedback(
        el.peerFeedback,
        ok ? '6 位配对码已复制' : '复制失败，请手动选中复制',
        ok ? 'status-ok' : 'status-danger',
      );
    });

    bindPressAction(el.btnCopyUri, async () => {
      if (!state.pairingQrUri) return;
      const ok = await copyTextToClipboard(state.pairingQrUri);
      setFeedback(
        el.peerFeedback,
        ok ? '配对链接已复制' : '复制失败，请手动选中复制',
        ok ? 'status-ok' : 'status-danger',
      );
    });

    /* v2 Device Groups & Catalog Event Bindings */
    bindPressAction(el.btnRefreshGroups, async () => {
      try {
        await refreshGroups();
        setFeedback(el.groupFeedback, '设备组与同步规则已刷新', 'status-ok');
      } catch (err) {
        setFeedback(el.groupFeedback, err.message, 'status-danger');
      }
    });

    el.createGroupForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const name = el.groupNameInput.value.trim();
      if (!name) return;
      try {
        const created = await createGroup(name);
        el.groupNameInput.value = '';
        state.selectedGroupId = created.group_id;
        setFeedback(el.groupFeedback, `已创建设备组：${created.name}`, 'status-ok');
        await refreshGroups();
      } catch (err) {
        setFeedback(el.groupFeedback, err.message, 'status-danger');
      }
    });

    el.renameGroupForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      if (!state.selectedGroupId) return;
      const newName = el.renameGroupInput.value.trim();
      if (!newName) return;
      try {
        const updated = await renameGroup(state.selectedGroupId, newName);
        setFeedback(el.groupFeedback, `设备组已重命名为：${updated.name}`, 'status-ok');
        await refreshGroups();
      } catch (err) {
        setFeedback(el.groupFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnDeleteGroup, async () => {
      if (!state.selectedGroupId) return;
      const targetId = state.selectedGroupId;
      el.btnDeleteGroup.disabled = true;
      try {
        await deleteGroup(targetId);
        closeCatalogBrowser();
        setFeedback(el.groupFeedback, '已删除设备组', 'status-ok');
        await refreshGroups();
      } catch (err) {
        setFeedback(el.groupFeedback, err.message, 'status-danger');
      } finally {
        el.btnDeleteGroup.disabled = false;
      }
    });

    el.addGroupMemberForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      if (!state.selectedGroupId) return;
      const deviceId = el.groupMemberDevice.value;
      const role = el.groupMemberRole.value || 'member';
      if (!deviceId) {
        setFeedback(el.groupFeedback, '请先配对设备后再添加到设备组', 'status-warn');
        return;
      }
      try {
        await addGroupMember(state.selectedGroupId, deviceId, role);
        setFeedback(el.groupFeedback, `已添加成员：${resolveDeviceName(deviceId)}`, 'status-ok');
        await refreshGroups();
      } catch (err) {
        setFeedback(el.groupFeedback, err.message, 'status-danger');
      }
    });

    el.catalogDeletedOnly.addEventListener('change', () => {
      loadBindingCatalogEntries(true);
    });

    bindPressAction(el.btnRefreshCatalog, () => {
      loadBindingCatalogEntries(true);
    });

    bindPressAction(el.btnCloseCatalog, () => {
      closeCatalogBrowser();
      renderGroupRules();
    });

    bindPressAction(el.btnCatalogLoadMore, () => {
      loadBindingCatalogEntries(false);
    });

    bindPressAction(el.btnVersionsLoadMore, () => {
      loadBindingVersions(state.catalogSelectedPathKey, false);
    });

    /* Block 4: Security & Non-Blocking Clipboard */
    bindPressAction(el.btnRefreshClipboard, async () => {
      try {
        await refreshClipboardHistory();
        setFeedback(el.clipFeedback, '快传记录已刷新', 'status-ok');
      } catch (err) {
        setFeedback(el.clipFeedback, err.message, 'status-danger');
      }
    });

    bindPressAction(el.btnLogout, async () => {
      try {
        await logout();
      } finally {
        showLockScreen();
      }
    });

    el.clipKind.addEventListener('change', () => {
      el.clipFilenameField.hidden = el.clipKind.value !== 'file';
    });

    // Non-blocking submit: immediately capture, clear inputs, insert optimistic item, and dispatch
    el.clipboardForm.addEventListener('submit', (event) => {
      event.preventDefault();
      const content = el.clipContent.value;
      if (!content) return;

      const targetRaw = el.clipTarget.value;
      const kind = el.clipKind.value;
      const filenameRaw = el.clipFilename.value.trim();
      const filename = kind === 'file' ? (filenameRaw || 'snippet.txt') : null;
      const frozenTargets = targetRaw
        ? [targetRaw]
        : state.peers.filter((p) => p.online).map((p) => p.device_id);
      const clientMsgId = generateClientMsgId();

      // Clear composer inputs synchronously BEFORE async send so new drafts are never overwritten
      el.clipContent.value = '';
      el.clipFilename.value = '';

      const optimisticMsg = {
        client_msg_id: clientMsgId,
        local_seq: ++state.clipboardSeq,
        direction: 'outgoing',
        from_device_id: '本机管理台',
        kind,
        content,
        filename,
        encrypted: false,
        timestamp: Math.floor(Date.now() / 1000),
        status: 'sending',
        frozen_targets: frozenTargets,
        targets: frozenTargets.map((id) => ({
          target_device_id: id,
          status: 'sending',
          error_code: null,
          error_message: null,
        })),
        delivered_to: 0,
        error_message: null,
      };

      state.clipboardMessages.set(clientMsgId, optimisticMsg);
      renderClipboardFeed();
      setFeedback(el.clipFeedback, '消息发送中，可继续输入下一条…', '');

      dispatchClipboardSend(optimisticMsg, frozenTargets);
    });
  }

  /* Bootstrap */
  async function bootstrap() {
    bindGlobalPointerFeedback();
    bindAppEvents();

    configureApiCallbacks({
      onUnauthorized: showLockScreen,
      onLockout: (untilMs, secs) => {
        showLockScreen({ syncLockout: false });
        startLockoutCountdown(untilMs, secs);
      },
    });

    syncLockoutCountdown();

    try {
      await loadAllDashboardData();
      showDashboard();
      socket.connect();
    } catch (err) {
      if (err instanceof ApiError && (err.status === 401 || err.status === 429)) {
        showLockScreen();
        return;
      }
      showLockScreen();
      el.loginError.textContent = err.message;
    }
  }

  bootstrap();

  // Global Web UI error & unhandledrejection hooks -> unified Rust debug log
  window.addEventListener('error', (event) => {
    const msg = event.message || 'Unhandled window error';
    const detail = event.error && event.error.stack
      ? String(event.error.stack)
      : `${event.filename || ''}:${event.lineno || 0}`;
    if (window.LanVault && window.LanVault.api && typeof window.LanVault.api.reportClientLog === 'function') {
      window.LanVault.api.reportClientLog('ERROR', 'web.window', msg, detail).catch(() => {});
    }
  });

  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    const msg = reason instanceof Error ? reason.message : String(reason || 'Unhandled promise rejection');
    const detail = reason instanceof Error && reason.stack ? String(reason.stack) : null;
    if (window.LanVault && window.LanVault.api && typeof window.LanVault.api.reportClientLog === 'function') {
      window.LanVault.api.reportClientLog('ERROR', 'web.promise', msg, detail).catch(() => {});
    }
  });
})(window);
