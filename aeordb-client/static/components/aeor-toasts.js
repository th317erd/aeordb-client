'use strict';

class AeorToasts extends HTMLElement {
  constructor() {
    super();
    this._counter = 0;
    this._eventSource = null;
    this._pendingEvents = [];  // debounce buffer
    this._debounceTimer = null;
  }

  connectedCallback() {
    this.innerHTML = '<div class="toast-container"></div>';

    // Expose global toast function
    window.aeorToast = (message, type = 'info', duration = 4000) => {
      this._addToast(message, type, duration);
    };

    // Connect to SSE for real-time sync events
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

  _addToast(message, type, duration) {
    this._counter++;

    const container = this.querySelector('.toast-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.dataset.id = this._counter;

    const messageSpan = document.createElement('span');
    messageSpan.className = 'toast-message';
    messageSpan.textContent = message;
    toast.appendChild(messageSpan);

    const dismissButton = document.createElement('button');
    dismissButton.className = 'toast-dismiss';
    dismissButton.textContent = '\u2715';
    dismissButton.addEventListener('click', () => {
      this._removeToast(toast);
    });
    toast.appendChild(dismissButton);

    container.appendChild(toast);

    requestAnimationFrame(() => toast.classList.add('toast-visible'));

    if (duration > 0) {
      setTimeout(() => this._removeToast(toast), duration);
    }
  }

  _removeToast(toast) {
    if (!toast || !toast.parentNode) return;
    toast.classList.remove('toast-visible');
    toast.classList.add('toast-exit');
    setTimeout(() => toast.remove(), 300);
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

  // Collect events over a 2-second window, then show summarized toasts
  _bufferEvent(event) {
    this._pendingEvents.push(event);

    if (this._debounceTimer) {
      clearTimeout(this._debounceTimer);
    }

    this._debounceTimer = setTimeout(() => {
      this._flushEvents();
    }, 2000);
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

      // Show error toast if any errors
      if (hasErrors) {
        window.aeorToast(`${name}: ${errors[0]}`, 'error', 6000);
      }

      // Show summary toast for file operations
      const parts = [];
      if (totalPulled > 0) parts.push(`${totalPulled} pulled`);
      if (totalPushed > 0) parts.push(`${totalPushed} pushed`);
      if (totalSynced > 0) parts.push(`${totalSynced} synced`);

      if (parts.length > 0) {
        window.aeorToast(`${name}: ${parts.join(', ')}`, 'success');
      }
    }
  }
}

customElements.define('aeor-toasts', AeorToasts);

export { AeorToasts };
