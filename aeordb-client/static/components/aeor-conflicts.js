'use strict';

class AeorConflicts extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this._conflicts = [];
  }

  connectedCallback() {
    this.render();
    this._fetchConflicts();
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host { display: block; }
        h1 { font-size: 24px; font-weight: 600; margin-bottom: 24px; color: var(--text-primary, #e6edf3); display: flex; justify-content: space-between; align-items: center; }
        button { font-size: 14px; padding: 6px 16px; border-radius: 6px; border: 1px solid var(--border, #30363d); background-color: var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); cursor: pointer; font-weight: 500; }
        button:hover { background-color: var(--border, #30363d); }
        button.primary { background-color: var(--accent, #58a6ff); color: #000; border-color: var(--accent); }
        button.primary:hover { background-color: var(--accent-hover, #79c0ff); }
        button.danger { background-color: var(--error, #f85149); border-color: var(--error); }
        table { width: 100%; border-collapse: collapse; }
        th { text-align: left; padding: 8px 12px; border-bottom: 1px solid var(--border, #30363d); color: var(--text-secondary, #8b949e); font-weight: 600; font-size: 12px; text-transform: uppercase; }
        td { padding: 8px 12px; border-bottom: 1px solid var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); }
        .empty { color: var(--text-muted, #484f58); font-style: italic; padding: 40px; text-align: center; }
        .empty-icon { font-size: 48px; margin-bottom: 12px; }
        .actions { display: flex; gap: 8px; }
        .hash { font-family: var(--font-mono, monospace); font-size: 11px; color: var(--text-secondary, #8b949e); }
      </style>

      <h1>
        Conflicts
        ${(this._conflicts.length > 0)
          ? `<button class="danger" id="resolve-all-btn">Resolve All</button>`
          : ''
        }
      </h1>

      ${(this._conflicts.length === 0)
        ? '<div class="empty"><div class="empty-icon">&#10003;</div>No conflicts. Everything is in sync.</div>'
        : this._renderTable()
      }
    `;

    const resolveAllButton = this.shadowRoot.querySelector('#resolve-all-btn');
    if (resolveAllButton)
      resolveAllButton.addEventListener('click', () => this._resolveAll());

    this._bindTableEvents();
  }

  _renderTable() {
    const rows = this._conflicts.map((conflict) => `
      <tr>
        <td>${conflict.file_path}</td>
        <td class="hash">${conflict.local_hash.substring(0, 12)}...</td>
        <td class="hash">${conflict.remote_hash.substring(0, 12)}...</td>
        <td>${new Date(conflict.detected_at).toLocaleString()}</td>
        <td class="actions">
          <button class="primary resolve-btn" data-id="${conflict.id}" data-resolution="keep_local">Keep Local</button>
          <button class="primary resolve-btn" data-id="${conflict.id}" data-resolution="keep_remote">Keep Remote</button>
          <button resolve-btn" data-id="${conflict.id}" data-resolution="keep_both">Keep Both</button>
        </td>
      </tr>
    `).join('');

    return `
      <table>
        <thead>
          <tr><th>File</th><th>Local Hash</th><th>Remote Hash</th><th>Detected</th><th>Actions</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _bindTableEvents() {
    this.shadowRoot.querySelectorAll('.resolve-btn').forEach((button) => {
      button.addEventListener('click', () => {
        this._resolveConflict(button.dataset.id, button.dataset.resolution);
      });
    });
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

  async _resolveConflict(id, resolution) {
    try {
      await fetch(`/api/v1/conflicts/${id}/resolve`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ resolution }),
      });
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to resolve conflict:', error);
    }
  }

  async _resolveAll() {
    if (!confirm('Resolve all conflicts?'))
      return;

    try {
      await fetch('/api/v1/conflicts/resolve-all', { method: 'POST' });
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to resolve all conflicts:', error);
    }
  }
}

customElements.define('aeor-conflicts', AeorConflicts);

export { AeorConflicts };
