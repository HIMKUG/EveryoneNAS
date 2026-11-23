/**
 * LanVault Web Console Main Controller
 *
 * Reference basis:
 * - /working_dir/c_0c10370daccfa96e/lanvault/ARCHITECTURE_CONTRACTS.md
 * - /working_dir/c_0c10370daccfa96e/lanvault/vendor_refs/web-neumorphism-ui/src/scss/neumorphism/components/_buttons.scss
 *
 * Uses Pointer Events for tactile feedback and actions (never hover-only).
 * Coordinates the 4 cognitive blocks:
 *   1. Service & Keep-Alive + One-Click Graceful Restart
 *   2. Storage Vaults (single/striped/mirror_raid1) + GFS Timeline + Bit-Rot Scrub
 *   3. Paired Devices + 6-Digit PIN / QR + Revoke
 *   4. Safe Logout + LAN Clipboard Quick-Transfer
 */
(function initLanVaultApp(global) {
  'use strict';

  const {
    ApiError,
    buildSnapshotRollbackUrl,
    clearPersistedLockout,
    configureApiCallbacks,
    createVault,
    getPersistedLockoutUntil,
    getSystemStatus,
    listPeers,
    listSnapshots,
    listVaults,
    login,
    logout,
    restartSystem,
    revokePeer,
    sendClipboard,
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
  const renderPairingQrSvg = (uri) => window.LanVault.qr.renderPairingQrSvg(uri);

  function renderPeers(data) {
    state.peers = data.peers;
    state.pairingPin = data.pairing_pin;
    state.pairingQrUri = data.pairing_qr_uri;

    el.pairingPin.textContent = data.pairing_pin;
    el.pairingUri.textContent = data.pairing_qr_uri;
    el.pairingQr.innerHTML = renderPairingQrSvg(data.pairing_qr_uri);

    el.peerList.textContent = '';
    el.clipTarget.textContent = '';
    const allOpt = document.createElement('option');
    allOpt.value = '';
    allOpt.textContent = '全部在线已配对设备';
    el.clipTarget.appendChild(allOpt);

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
  }

  /* Block 4: LAN Clipboard Live Feed */
  function handleClipboardReceivedEvent(payload) {
    const empty = el.clipboardFeed.querySelector('.empty-state');
    if (empty) empty.remove();

    const item = document.createElement('article');
    item.className = 'clip-item';

    const head = document.createElement('div');
    head.className = 'clip-item__head mono';

    const source = document.createElement('span');
    const fileLabel = payload.kind === 'file' && payload.filename ? ` · 文件 ${payload.filename}` : '';
    source.textContent = `来自 ${payload.from_device_id}${fileLabel} · ${formatTimestamp(payload.timestamp)}`;

    const copyBtn = document.createElement('button');
    copyBtn.type = 'button';
    copyBtn.className = 'neu-btn neu-btn--compact';
    copyBtn.textContent = '复制';
    copyBtn.disabled = payload.encrypted === true;
    if (copyBtn.disabled) copyBtn.textContent = '需客户端解密';
    bindPressAction(copyBtn, async () => {
      if (payload.encrypted === true) return;
      const ok = await copyTextToClipboard(payload.content);
      setFeedback(
        el.clipFeedback,
        ok ? '已复制收到的剪贴板内容' : '复制失败，请手动选中复制',
        ok ? 'status-ok' : 'status-danger',
      );
    });

    head.append(source, copyBtn);

    const body = document.createElement('pre');
    body.className = 'clip-item__body';
    body.textContent = payload.encrypted === true
      ? '内容已端到端加密，请在已配对的原生客户端中查看。'
      : payload.content;

    item.append(head, body);
    el.clipboardFeed.prepend(item);
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

  async function loadAllDashboardData() {
    const [status, vaultRes, peerRes] = await Promise.all([
      getSystemStatus(),
      listVaults(),
      listPeers(),
    ]);
    applySystemStatus(status);
    renderVaults(vaultRes.vaults);
    renderPeers(peerRes);
    await refreshSnapshotsForSelectedVault();
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

    el.clipboardForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const targetRaw = el.clipTarget.value;
      const kind = el.clipKind.value;
      const filenameRaw = el.clipFilename.value.trim();
      const payload = {
        target_device_id: targetRaw || null,
        kind,
        content: el.clipContent.value,
        filename: kind === 'file' ? (filenameRaw || 'snippet.txt') : null,
      };

      try {
        const res = await sendClipboard(payload);
        setFeedback(el.clipFeedback, `已推送至 ${res.delivered_to} 台在线设备`, 'status-ok');
        el.clipContent.value = '';
      } catch (err) {
        setFeedback(el.clipFeedback, err.message, 'status-danger');
      }
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
