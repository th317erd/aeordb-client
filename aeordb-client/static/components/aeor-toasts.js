'use strict';

import { showToast } from '../aeor/components/aeor-toast.js';

/**
 * <aeor-toasts> — SSE-driven toast notification bridge.
 *
 * Connects to the client's SSE event stream, debounces sync activity
 * events over a 2-second window, groups by relationship, and shows
 * summarized toasts via the shared showToast() function.
 *
 * Also exposes window.aeorToast for programmatic use by other components.
 */
class AeorToasts extends HTMLElement {
  constructor() {
    super();
    this._eventSource = null;
    this._pendingEvents = [];
    this._debounceTimer = null;
    this._lastErrors = {};
  }

  connectedCallback() {
    // Expose global toast function using the shared toast system
    window.aeorToast = (message, type = 'info', duration = 6000) => {
      showToast(message, type, duration);
    };

    this._connectSSE();
  }

  disconnectedCallback() {
    if (this._eventSource) {
      this._eventSource.close();
      this._eventSource = null;
    }
    if (this._debounceTimer) {
      clearTimeout(this._debounceTimer);
    }
  }

  _connectSSE() {
    this._eventSource = new EventSource('/api/v1/events');

    this._eventSource.addEventListener('sync_activity', (event) => {
      try {
        const data = JSON.parse(event.data);
        this._bufferEvent(data);
      } catch (e) {
        // ignore parse errors
      }
    });

    this._eventSource.onerror = () => {
      // Reconnect is automatic with EventSource
    };
  }

  _bufferEvent(event) {
    this._pendingEvents.push(event);

    if (this._debounceTimer)
      clearTimeout(this._debounceTimer);

    this._debounceTimer = setTimeout(() => this._flushEvents(), 2000);
  }

  _flushEvents() {
    const events = this._pendingEvents;
    this._pendingEvents = [];
    this._debounceTimer = null;

    if (events.length === 0) return;

    // Group by relationship
    const grouped = {};
    for (const event of events) {
      const key = event.relationship_name || 'Unknown';
      if (!grouped[key]) grouped[key] = [];
      grouped[key].push(event);
    }

    for (const [name, relEvents] of Object.entries(grouped)) {
      let totalPulled = 0;
      let totalPushed = 0;
      let totalSynced = 0;
      let hasErrors = false;
      const errors = [];

      for (const event of relEvents) {
        if (event.event_type === 'error') {
          hasErrors = true;
          errors.push(event.summary);
        } else if (event.files_affected > 0) {
          if (event.event_type === 'pull') totalPulled += event.files_affected;
          else if (event.event_type === 'push') totalPushed += event.files_affected;
          else totalSynced += event.files_affected;
        }
      }

      // Show error toast — suppress if same error repeated
      if (hasErrors) {
        const errorMsg = errors[0];
        if (this._lastErrors[name] !== errorMsg) {
          this._lastErrors[name] = errorMsg;
          showToast(`${name}: ${errorMsg}`, 'error', 10000);
        }
      } else {
        delete this._lastErrors[name];
      }

      // Show summary toast for file operations
      const parts = [];
      if (totalPulled > 0) parts.push(`${totalPulled} pulled`);
      if (totalPushed > 0) parts.push(`${totalPushed} pushed`);
      if (totalSynced > 0) parts.push(`${totalSynced} synced`);

      if (parts.length > 0) {
        showToast(`${name}: ${parts.join(', ')}`, 'success');
      }
    }
  }
}

if (!customElements.get('aeor-toasts'))
  customElements.define('aeor-toasts', AeorToasts);

export { AeorToasts };
