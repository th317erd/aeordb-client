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

    this._bindTableEvents();
    this._bindResolveAllEvents();
  }

  _renderConflictsView() {
    return `
      <div class="info-section" style="margin-bottom: 20px;">
        <p style="color: var(--text-secondary); line-height: 1.7;">
          These files were changed on <strong style="color: var(--text-primary);">both</strong> your local machine and the remote server since the last sync.
          No version is ever lost &mdash; AeorDB keeps full history. Choose how to resolve each file:
        </p>
        <ul style="color: var(--text-secondary); margin: 12px 0 12px 20px; line-height: 1.9;">
          <li><strong style="color: var(--accent);">Keep Local</strong> &mdash; Push your local version to the server, overwriting the remote changes.</li>
          <li><strong style="color: var(--accent);">Keep Remote</strong> &mdash; Pull the server version to your machine, overwriting your local changes.</li>
          <li><strong style="color: var(--text-primary);">Keep Both</strong> &mdash; Rename your local file to <code style="color: var(--accent); background: var(--bg-tertiary); padding: 1px 5px; border-radius: 3px;">.local-conflict</code> and pull the server version. You can then manually merge them.</li>
        </ul>
      </div>

      ${this._renderTable()}

      <div style="margin-top: 20px; display: flex; align-items: center; gap: 12px;">
        <span style="color: var(--text-secondary); font-size: 13px;">
          Resolve all ${this._conflicts.length} conflict(s) at once:
        </span>
        <button class="primary small" id="resolve-all-local">Keep All Local</button>
        <button class="primary small" id="resolve-all-remote">Keep All Remote</button>
        <button class="secondary small" id="resolve-all-both">Keep All Both</button>
      </div>
    `;
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

  _bindResolveAllEvents() {
    const localButton  = this.querySelector('#resolve-all-local');
    const remoteButton = this.querySelector('#resolve-all-remote');
    const bothButton   = this.querySelector('#resolve-all-both');

    if (localButton)
      localButton.addEventListener('click', () => this._resolveAllWith('keep_local'));

    if (remoteButton)
      remoteButton.addEventListener('click', () => this._resolveAllWith('keep_remote'));

    if (bothButton)
      bothButton.addEventListener('click', () => this._resolveAllWith('keep_both'));
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

  async _resolveAllWith(resolution) {
    const count = this._conflicts.length;
    const labels = {
      keep_local:  'Keep All Local (push your versions to server)',
      keep_remote: 'Keep All Remote (pull server versions to your machine)',
      keep_both:   'Keep All Both (rename local files, pull server versions)',
    };

    if (!confirm(`${labels[resolution]}\n\nThis will resolve all ${count} conflict(s). Continue?`))
      return;

    try {
      for (const conflict of this._conflicts) {
        await fetch(`/api/v1/conflicts/${conflict.id}/resolve`, {
          method:  'POST',
          headers: { 'Content-Type': 'application/json' },
          body:    JSON.stringify({ resolution }),
        });
      }
      await this._fetchConflicts();
    } catch (error) {
      console.error('Failed to resolve conflicts:', error);
    }
  }
}

customElements.define('aeor-conflicts', AeorConflicts);

export { AeorConflicts };
