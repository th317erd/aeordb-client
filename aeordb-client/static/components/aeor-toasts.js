'use strict';

class AeorToasts extends HTMLElement {
  constructor() {
    super();
    this._toasts = [];
    this._counter = 0;
    this._lastActivityTimestamp = Date.now();
    this._pollInterval = null;
  }

  connectedCallback() {
    this.innerHTML = '<div class="toast-container"></div>';

    // Expose global toast function
    window.aeorToast = (message, type = 'info', duration = 4000) => {
      this._addToast(message, type, duration);
    };

    // Start polling for background sync events
    this._startActivityPoll();
  }

  disconnectedCallback() {
    if (this._pollInterval) {
      clearInterval(this._pollInterval);
    }
  }

  _addToast(message, type, duration) {
    this._counter++;
    const id = this._counter;

    const container = this.querySelector('.toast-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.dataset.id = id;

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

    // Trigger enter animation
    requestAnimationFrame(() => toast.classList.add('toast-visible'));

    // Auto-dismiss
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

  _startActivityPoll() {
    // Poll every 10 seconds for new sync activity across all relationships
    this._pollInterval = setInterval(() => this._pollActivity(), 10000);
  }

  async _pollActivity() {
    try {
      const response = await fetch('/api/v1/sync');
      const relationships = await response.json();

      for (const rel of relationships) {
        const activityResponse = await fetch(`/api/v1/sync/${rel.id}/activity`);
        const events = await activityResponse.json();

        if (events.length === 0) continue;

        const latest = events[0]; // newest first
        if (latest.timestamp > this._lastActivityTimestamp) {
          this._lastActivityTimestamp = latest.timestamp;
          this._toastForEvent(rel.name, latest);
        }
      }
    } catch (error) {
      // Non-critical — silently skip
    }
  }

  _toastForEvent(relationshipName, event) {
    switch (event.event_type) {
      case 'pull':
      case 'push':
      case 'full_sync': {
        if (event.files_affected === 0) return; // skip no-ops
        const type = (event.errors && event.errors.length > 0) ? 'warning' : 'success';
        const action = (event.event_type === 'pull') ? 'Pulled' :
                       (event.event_type === 'push') ? 'Pushed' : 'Synced';
        window.aeorToast(
          `${relationshipName}: ${action} ${event.files_affected} files`,
          type,
        );
        break;
      }
      case 'error':
        window.aeorToast(
          `${relationshipName}: ${event.summary}`,
          'error',
          6000,
        );
        break;
    }
  }
}

customElements.define('aeor-toasts', AeorToasts);

export { AeorToasts };
