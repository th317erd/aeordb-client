'use strict';

class AeorConflicts extends HTMLElement {
  constructor() {
    super();
    this._conflicts = [];
  }

  connectedCallback() {
    this.render();
    this._fetchConflicts();
  }

  render() {
    this.innerHTML = `
      <div class="page-header">
        <h1>Conflicts</h1>
      </div>

      ${(this._conflicts.length === 0)
        ? '<div class="empty-state"><div class="empty-icon">&#10003;</div>No conflicts. Everything is in sync.</div>'
        : this._renderConflictsView()
      }
    `;

    this._bindEvents();
  }

  _renderConflictsView() {
    return `
      <div class="info-section" style="margin-bottom: 20px;">
        <p style="color: var(--text-secondary); line-height: 1.7;">
          These files were changed on <strong style="color: var(--text-primary);">both sides</strong> since the last sync.
          AeorDB automatically picked a <strong style="color: var(--success);">winner</strong> (most recent edit wins),
          but the <strong style="color: var(--text-secondary);">loser</strong> version is preserved in history. You can override the auto-pick if needed.
        </p>
        <ul style="color: var(--text-secondary); margin: 12px 0 12px 20px; line-height: 1.9;">
          <li><strong style="color: var(--success);">Accept</strong> &mdash; Keep the auto-selected winner. The loser remains in version history.</li>
          <li><strong style="color: var(--accent);">Pick Loser</strong> &mdash; Override the auto-pick and promote the losing version instead.</li>
        </ul>
      </div>

      ${this._renderTable()}

      ${(this._conflicts.length > 1)
        ? `<div style="margin-top: 20px; display: flex; align-items: center; gap: 12px;">
            <span style="color: var(--text-secondary); font-size: 13px;">
              Resolve all ${this._conflicts.length} conflict(s):
            </span>
            <button class="success small" id="dismiss-all">Accept All Winners</button>
          </div>`
        : ''
      }
    `;
  }

  _renderTable() {
    const rows = this._conflicts.map((conflict) => {
      const winner = conflict.winner || {};
      const loser  = conflict.loser || {};

      return `
        <tr class="conflict-row" data-path="${conflict.path}">
          <td>
            <div style="font-weight: 500;">${conflict.path}</div>
            <div class="mono muted" style="margin-top: 4px; font-size: 11px;">
              ${conflict.conflict_type || 'modify/modify'}
            </div>
          </td>
          <td>
            <div style="color: var(--success); font-weight: 500;">Winner</div>
            <div class="mono muted" style="font-size: 11px;">${(winner.hash || '').substring(0, 12)}...</div>
            <div class="muted" style="font-size: 12px;">${this._formatSize(winner.size)} &middot; node ${winner.node_id || '?'}</div>
          </td>
          <td>
            <div style="color: var(--text-secondary);">Loser</div>
            <div class="mono muted" style="font-size: 11px;">${(loser.hash || '').substring(0, 12)}...</div>
            <div class="muted" style="font-size: 12px;">${this._formatSize(loser.size)} &middot; node ${loser.node_id || '?'}</div>
          </td>
          <td>${new Date(conflict.created_at).toLocaleString()}</td>
          <td class="actions">
            <button class="success small dismiss-btn" data-path="${conflict.path}">Accept</button>
            <button class="primary small resolve-btn" data-path="${conflict.path}" data-pick="loser">Pick Loser</button>
          </td>
        </tr>
      `;
    }).join('');

    return `
      <table>
        <thead>
          <tr><th>File</th><th>Winner</th><th>Loser</th><th>Detected</th><th>Actions</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _bindEvents() {
    this.querySelectorAll('.dismiss-btn').forEach((button) => {
      button.addEventListener('click', () => this._dismissConflict(button.dataset.path));
    });

    this.querySelectorAll('.resolve-btn').forEach((button) => {
      button.addEventListener('click', () => this._resolveConflict(button.dataset.path, button.dataset.pick));
    });

    const dismissAllButton = this.querySelector('#dismiss-all');
    if (dismissAllButton)
      dismissAllButton.addEventListener('click', () => this._dismissAll());
  }

  async _fetchConflicts() {
    try {
      const response  = await fetch('/api/v1/conflicts');
      this._conflicts = await response.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch conflicts:', error);
    }
  }

  async _dismissConflict(path) {
    try {
      await fetch('/api/v1/conflicts/dismiss', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path }),
      });
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to dismiss conflict:', error);
    }
  }

  async _resolveConflict(path, pick) {
    try {
      await fetch('/api/v1/conflicts/resolve', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path, pick }),
      });
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to resolve conflict:', error);
    }
  }

  async _dismissAll() {
    if (!confirm(`Accept all ${this._conflicts.length} auto-winners?\n\nLosing versions remain in version history.`))
      return;

    try {
      await fetch('/api/v1/conflicts/dismiss-all', { method: 'POST' });
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to dismiss all:', error);
    }
  }

  _formatSize(bytes) {
    if (bytes == null)
      return '?';

    if (bytes < 1024)
      return `${bytes} B`;

    if (bytes < 1024 * 1024)
      return `${(bytes / 1024).toFixed(1)} KB`;

    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
}

customElements.define('aeor-conflicts', AeorConflicts);

export { AeorConflicts };
