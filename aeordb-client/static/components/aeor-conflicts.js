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
        ${(this._conflicts.length > 0)
          ? '<button class="danger" id="resolve-all-btn">Resolve All</button>'
          : ''
        }
      </div>

      ${(this._conflicts.length === 0)
        ? '<div class="empty-state"><div class="empty-icon">&#10003;</div>No conflicts. Everything is in sync.</div>'
        : this._renderTable()
      }
    `;

    const resolveAllButton = this.querySelector('#resolve-all-btn');
    if (resolveAllButton)
      resolveAllButton.addEventListener('click', () => this._resolveAll());

    this._bindTableEvents();
  }

  _renderTable() {
    const rows = this._conflicts.map((conflict) => `
      <tr>
        <td>${conflict.file_path}</td>
        <td class="mono muted">${conflict.local_hash.substring(0, 12)}...</td>
        <td class="mono muted">${conflict.remote_hash.substring(0, 12)}...</td>
        <td>${new Date(conflict.detected_at).toLocaleString()}</td>
        <td class="actions">
          <button class="primary small resolve-btn" data-id="${conflict.id}" data-resolution="keep_local">Keep Local</button>
          <button class="primary small resolve-btn" data-id="${conflict.id}" data-resolution="keep_remote">Keep Remote</button>
          <button class="secondary small resolve-btn" data-id="${conflict.id}" data-resolution="keep_both">Keep Both</button>
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
    this.querySelectorAll('.resolve-btn').forEach((button) => {
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
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ resolution }),
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
