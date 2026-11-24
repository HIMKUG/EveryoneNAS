/**
 * LanVault WebSocket Client (/ws)
 *
 * Handles automatic exponential-backoff reconnection and boundary validation
 * for contract events:
 *   - telemetry
 *   - server_restarting
 *   - bit_rot_alert
 *   - clipboard_received
 *   - clipboard_status
 *   - group_sync_update
 */
(function initLanVaultWs(global) {
  'use strict';

  const KNOWN_EVENTS = new Set([
    'telemetry',
    'server_restarting',
    'bit_rot_alert',
    'clipboard_received',
    'clipboard_status',
    'group_sync_update',
  ]);

  const RECONNECT_DELAYS_MS = [1000, 2000, 4000, 8000, 15000];

  class LanVaultSocket {
    constructor(handlers) {
      this.handlers = handlers;
      this.socket = null;
      this.shouldReconnect = false;
      this.attempt = 0;
      this.reconnectTimer = null;
    }

    connect() {
      this.shouldReconnect = true;
      this.#clearTimer();
      this.#openSocket();
    }

    disconnect() {
      this.shouldReconnect = false;
      this.#clearTimer();
      if (this.socket) {
        this.socket.close(1000, 'client_disconnect');
        this.socket = null;
      }
      this.handlers.onStateChange('closed');
    }

    scheduleReconnectAfter(delayMs) {
      if (!this.shouldReconnect) return;
      this.#clearTimer();
      this.handlers.onStateChange('reconnecting');
      this.reconnectTimer = global.setTimeout(() => {
        this.reconnectTimer = null;
        this.#openSocket();
      }, delayMs);
    }

    #openSocket() {
      if (this.socket && (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING)) {
        return;
      }

      const protocol = global.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const url = `${protocol}//${global.location.host}/ws`;
      this.handlers.onStateChange(this.attempt === 0 ? 'connecting' : 'reconnecting');

      const ws = new WebSocket(url);
      this.socket = ws;

      ws.onopen = () => {
        this.attempt = 0;
        this.handlers.onStateChange('open');
      };

      ws.onmessage = (event) => {
        this.#handleRawMessage(event.data);
      };

      ws.onerror = () => {
        // Browser fires onclose immediately after onerror; reconnect is handled there.
      };

      ws.onclose = (event) => {
        if (this.socket === ws) {
          this.socket = null;
        }

        if (!this.shouldReconnect) {
          this.handlers.onStateChange('closed');
          return;
        }

        if (event.code === 4401 || event.code === 1008) {
          this.shouldReconnect = false;
          this.handlers.onStateChange('closed');
          this.handlers.onAuthExpired();
          return;
        }

        if (event.code === 1006 || !event.wasClean) {
          this.#probeAuthBeforeReconnect();
          return;
        }

        this.#scheduleNextReconnect();
      };
    }

    async #probeAuthBeforeReconnect() {
      try {
        const response = await fetch('/api/system/status', {
          method: 'GET',
          credentials: 'same-origin',
          headers: { Accept: 'application/json' },
        });
        if (!this.shouldReconnect) return;
        if (response.status === 401) {
          this.shouldReconnect = false;
          this.#clearTimer();
          this.handlers.onStateChange('closed');
          this.handlers.onAuthExpired();
          return;
        }
      } catch {
        // Server unreachable or restarting; continue with backoff reconnect.
      }

      if (!this.shouldReconnect) return;
      this.#scheduleNextReconnect();
    }

    #scheduleNextReconnect() {
      const idx = Math.min(this.attempt, RECONNECT_DELAYS_MS.length - 1);
      const delayMs = RECONNECT_DELAYS_MS[idx];
      this.attempt += 1;
      this.scheduleReconnectAfter(delayMs);
    }

    #handleRawMessage(rawData) {
      if (typeof rawData !== 'string') return;

      let frame;
      try {
        frame = JSON.parse(rawData);
      } catch {
        console.error('[LanVault WS] Malformed JSON frame ignored');
        return;
      }

      if (!frame || typeof frame.type !== 'string' || !KNOWN_EVENTS.has(frame.type) || frame.payload === undefined) {
        console.warn('[LanVault WS] Unknown or incomplete frame ignored:', frame);
        return;
      }

      const handler = this.handlers[frame.type];
      if (handler) {
        handler(frame.payload);
      }
    }

    #clearTimer() {
      if (this.reconnectTimer !== null) {
        global.clearTimeout(this.reconnectTimer);
        this.reconnectTimer = null;
      }
    }
  }

  global.LanVault = global.LanVault || {};
  global.LanVault.ws = { LanVaultSocket };
})(window);
