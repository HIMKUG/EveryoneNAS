/**
 * LanVault GFS Snapshot Timeline Renderer
 *
 * Reference basis:
 * - /working_dir/c_0c10370daccfa96e/lanvault/ARCHITECTURE_CONTRACTS.md (Section 3.1 #7)
 * - /working_dir/c_0c10370daccfa96e/lanvault/vendor_refs/web-neumorphism-ui/src/scss/neumorphism/components/_list-group.scss
 *
 * Supports filtering by GFS retention_tier (hourly/daily/weekly/monthly/pinned)
 * and single-file rollback triggering via Pointer Events (no hover-only controls).
 */
(function initLanVaultTimeline(global) {
  'use strict';

  const TIER_LABELS = {
    hourly: '每小时',
    daily: '每日',
    weekly: '每周',
    monthly: '每月',
    pinned: '永久固定',
  };

  function formatBytes(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    const units = ['KB', 'MB', 'GB', 'TB'];
    let value = bytes / 1024;
    let unitIdx = 0;
    while (value >= 1024 && unitIdx < units.length - 1) {
      value /= 1024;
      unitIdx += 1;
    }
    const precision = value >= 100 ? 0 : 1;
    return `${value.toFixed(precision)} ${units[unitIdx]}`;
  }

  function formatTimestamp(unixSecs) {
    const ms = unixSecs > 1e12 ? unixSecs : unixSecs * 1000;
    const date = new Date(ms);
    return date.toLocaleString('zh-CN', {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    });
  }

  class SnapshotTimeline {
    constructor(containerEl, filterGroupEl, onRollback) {
      this.containerEl = containerEl;
      this.filterGroupEl = filterGroupEl;
      this.onRollback = onRollback;
      this.snapshots = [];
      this.activeTier = 'all';

      this.#bindFilterEvents();
    }

    setSnapshots(snapshots) {
      this.snapshots = [...snapshots].sort((a, b) => b.created_at - a.created_at);
      this.render();
    }

    setTier(tier) {
      this.activeTier = tier;
      const buttons = this.filterGroupEl.querySelectorAll('button[data-tier]');
      for (const btn of buttons) {
        btn.setAttribute('aria-pressed', String(btn.dataset.tier === tier));
      }
      this.render();
    }

    render() {
      this.containerEl.textContent = '';

      const visible = this.activeTier === 'all'
        ? this.snapshots
        : this.snapshots.filter((item) => item.retention_tier === this.activeTier);

      if (visible.length === 0) {
        const empty = document.createElement('p');
        empty.className = 'empty-state';
        empty.textContent = this.snapshots.length === 0
          ? '当前存储空间暂无快照'
          : `当前筛选层级（${TIER_LABELS[this.activeTier]}）下无快照`;
        this.containerEl.appendChild(empty);
        return;
      }

      const fragment = document.createDocumentFragment();
      for (const snap of visible) {
        fragment.appendChild(this.#createSnapshotRow(snap));
      }
      this.containerEl.appendChild(fragment);
    }

    #bindFilterEvents() {
      let pressedBtn = null;

      this.filterGroupEl.addEventListener('pointerdown', (event) => {
        if (event.button !== 0) {
          pressedBtn = null;
          return;
        }
        pressedBtn = event.target.closest('button[data-tier]');
      });

      this.filterGroupEl.addEventListener('pointercancel', () => {
        pressedBtn = null;
      });

      this.filterGroupEl.addEventListener('pointerleave', () => {
        pressedBtn = null;
      });

      this.filterGroupEl.addEventListener('pointerup', (event) => {
        const btn = event.target.closest('button[data-tier]');
        const matched = btn && btn === pressedBtn && event.button === 0;
        pressedBtn = null;
        if (!matched) return;
        this.setTier(btn.dataset.tier);
      });

      this.filterGroupEl.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        const btn = event.target.closest('button[data-tier]');
        if (!btn) return;
        event.preventDefault();
        this.setTier(btn.dataset.tier);
      });
    }

    #createSnapshotRow(snap) {
      const article = document.createElement('article');
      article.className = 'timeline-item';

      const top = document.createElement('div');
      top.className = 'timeline-item__top';

      const title = document.createElement('span');
      title.className = 'timeline-item__title mono';
      const tierText = TIER_LABELS[snap.retention_tier] || snap.retention_tier;
      title.textContent = `${formatTimestamp(snap.created_at)} · ${tierText}`;

      const toggleBtn = document.createElement('button');
      toggleBtn.type = 'button';
      toggleBtn.className = 'neu-btn neu-btn--compact';
      toggleBtn.textContent = '单文件回滚';
      toggleBtn.setAttribute('aria-expanded', 'false');

      top.append(title, toggleBtn);

      const meta = document.createElement('div');
      meta.className = 'timeline-item__meta';
      meta.textContent = `快照 ${snap.snapshot_id.slice(0, 12)} · 设备 ${snap.device_id} · ${snap.file_count} 个文件 · ${formatBytes(snap.total_bytes)}`;

      const rollbackBar = document.createElement('form');
      rollbackBar.className = 'rollback-bar';
      rollbackBar.hidden = true;

      const pathInput = document.createElement('input');
      pathInput.type = 'text';
      pathInput.className = 'neu-input mono';
      pathInput.placeholder = '输入快照内相对路径，例如：Photos/2026/IMG_001.jpg';
      pathInput.required = true;
      pathInput.setAttribute('aria-label', '要回滚的文件相对路径');

      const submitBtn = document.createElement('button');
      submitBtn.type = 'submit';
      submitBtn.className = 'neu-btn neu-btn--primary neu-btn--compact';
      submitBtn.textContent = '下载回滚版本';

      rollbackBar.append(pathInput, submitBtn);

      const toggleRollback = () => {
        const willOpen = rollbackBar.hidden;
        rollbackBar.hidden = !willOpen;
        toggleBtn.setAttribute('aria-expanded', String(willOpen));
        if (willOpen) {
          pathInput.focus();
        }
      };

      let pressed = false;
      toggleBtn.addEventListener('pointerdown', (event) => {
        pressed = event.button === 0 && !toggleBtn.disabled;
      });
      toggleBtn.addEventListener('pointercancel', () => {
        pressed = false;
      });
      toggleBtn.addEventListener('pointerleave', () => {
        pressed = false;
      });
      toggleBtn.addEventListener('pointerup', (event) => {
        const shouldFire = pressed && event.button === 0 && !toggleBtn.disabled;
        pressed = false;
        if (shouldFire) {
          toggleRollback();
        }
      });

      toggleBtn.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          toggleRollback();
        }
      });

      rollbackBar.addEventListener('submit', (event) => {
        event.preventDefault();
        const relPath = pathInput.value.trim();
        if (!relPath) return;
        this.onRollback(snap, relPath);
      });

      article.append(top, meta, rollbackBar);
      return article;
    }
  }

  global.LanVault = global.LanVault || {};
  global.LanVault.timeline = {
    SnapshotTimeline,
    formatBytes,
    formatTimestamp,
  };
})(window);
