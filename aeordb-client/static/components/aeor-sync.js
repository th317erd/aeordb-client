'use strict';

class AeorSync extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this._relationships = [];
    this._connections    = [];
    this._showAddForm    = false;
  }

  connectedCallback() {
    this.render();
    this._fetchData();
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host { display: block; }
        h1 { font-size: 24px; font-weight: 600; margin-bottom: 24px; color: var(--text-primary, #e6edf3); display: flex; justify-content: space-between; align-items: center; }
        button { font-size: 14px; padding: 6px 16px; border-radius: 6px; border: 1px solid var(--border, #30363d); background-color: var(--accent, #58a6ff); color: #000; cursor: pointer; font-weight: 500; }
        button:hover { background-color: var(--accent-hover, #79c0ff); }
        button.secondary { background-color: var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); }
        button.secondary:hover { background-color: var(--border, #30363d); }
        button.danger { background-color: var(--error, #f85149); border-color: var(--error, #f85149); }
        button.success { background-color: var(--success, #3fb950); border-color: var(--success, #3fb950); }
        table { width: 100%; border-collapse: collapse; }
        th { text-align: left; padding: 8px 12px; border-bottom: 1px solid var(--border, #30363d); color: var(--text-secondary, #8b949e); font-weight: 600; font-size: 12px; text-transform: uppercase; }
        td { padding: 8px 12px; border-bottom: 1px solid var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); }
        .empty { color: var(--text-muted, #484f58); font-style: italic; padding: 40px; text-align: center; }
        .actions { display: flex; gap: 8px; flex-wrap: wrap; }
        .badge { display: inline-block; padding: 2px 8px; border-radius: 12px; font-size: 12px; font-weight: 500; }
        .badge.success { background-color: rgba(63, 185, 80, 0.15); color: var(--success, #3fb950); }
        .badge.warning { background-color: rgba(210, 153, 34, 0.15); color: var(--warning, #d29922); }
        .form-overlay { background-color: var(--bg-secondary, #161b22); border: 1px solid var(--border, #30363d); border-radius: 8px; padding: 20px; margin-bottom: 20px; }
        .form-overlay h2 { font-size: 16px; margin-bottom: 16px; }
        .form-row { margin-bottom: 12px; }
        .form-row label { display: block; font-size: 12px; color: var(--text-secondary, #8b949e); margin-bottom: 4px; }
        .form-row input, .form-row select { width: 100%; padding: 8px 12px; border-radius: 6px; border: 1px solid var(--border, #30363d); background-color: var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); font-size: 14px; }
        .form-row input:focus, .form-row select:focus { border-color: var(--accent, #58a6ff); outline: none; }
        .form-actions { display: flex; gap: 8px; margin-top: 16px; }
        .id-cell { font-family: var(--font-mono, monospace); font-size: 12px; color: var(--text-secondary, #8b949e); }
      </style>

      <h1>
        Sync Relationships
        <button id="add-btn">${(this._showAddForm) ? 'Cancel' : 'Add Sync'}</button>
      </h1>

      ${(this._showAddForm) ? this._renderAddForm() : ''}

      ${(this._relationships.length === 0)
        ? '<div class="empty">No sync relationships configured.</div>'
        : this._renderTable()
      }
    `;

    this.shadowRoot.querySelector('#add-btn')
      .addEventListener('click', () => {
        this._showAddForm = !this._showAddForm;
        this.render();
      });

    if (this._showAddForm)
      this._bindFormEvents();

    this._bindTableEvents();
  }

  _renderAddForm() {
    const connectionOptions = this._connections.map((connection) =>
      `<option value="${connection.id}">${connection.name} (${connection.url})</option>`
    ).join('');

    return `
      <div class="form-overlay">
        <h2>New Sync Relationship</h2>
        <div class="form-row">
          <label>Name</label>
          <input type="text" id="form-name" placeholder="My Documents">
        </div>
        <div class="form-row">
          <label>Connection</label>
          <select id="form-connection">${connectionOptions}</select>
        </div>
        <div class="form-row">
          <label>Remote Path</label>
          <input type="text" id="form-remote-path" placeholder="/docs/">
        </div>
        <div class="form-row">
          <label>Local Path</label>
          <input type="text" id="form-local-path" placeholder="/home/user/Documents">
        </div>
        <div class="form-row">
          <label>Direction</label>
          <select id="form-direction">
            <option value="pull_only">Pull Only</option>
            <option value="push_only">Push Only</option>
            <option value="bidirectional">Bidirectional</option>
          </select>
        </div>
        <div class="form-row">
          <label>Filter (optional, comma-separated globs)</label>
          <input type="text" id="form-filter" placeholder="*.pdf, !*.tmp">
        </div>
        <div class="form-actions">
          <button id="form-submit">Create</button>
          <button class="secondary" id="form-cancel">Cancel</button>
        </div>
      </div>
    `;
  }

  _renderTable() {
    const rows = this._relationships.map((relationship) => `
      <tr>
        <td class="id-cell">${relationship.id.substring(0, 8)}...</td>
        <td>${relationship.name}</td>
        <td>${relationship.remote_path}</td>
        <td>${relationship.direction}</td>
        <td><span class="badge ${(relationship.enabled) ? 'success' : 'warning'}">${(relationship.enabled) ? 'enabled' : 'disabled'}</span></td>
        <td class="actions">
          <button class="success trigger-btn" data-id="${relationship.id}">Sync</button>
          <button class="secondary toggle-btn" data-id="${relationship.id}" data-enabled="${relationship.enabled}">${(relationship.enabled) ? 'Pause' : 'Resume'}</button>
          <button class="danger delete-btn" data-id="${relationship.id}">Delete</button>
        </td>
      </tr>
    `).join('');

    return `
      <table>
        <thead>
          <tr><th>ID</th><th>Name</th><th>Remote</th><th>Direction</th><th>Status</th><th>Actions</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _bindFormEvents() {
    const submitButton = this.shadowRoot.querySelector('#form-submit');
    const cancelButton = this.shadowRoot.querySelector('#form-cancel');
    if (submitButton) submitButton.addEventListener('click', () => this._submitForm());
    if (cancelButton) cancelButton.addEventListener('click', () => { this._showAddForm = false; this.render(); });
  }

  _bindTableEvents() {
    this.shadowRoot.querySelectorAll('.trigger-btn').forEach((button) => {
      button.addEventListener('click', () => this._triggerSync(button.dataset.id));
    });
    this.shadowRoot.querySelectorAll('.toggle-btn').forEach((button) => {
      button.addEventListener('click', () => this._toggleSync(button.dataset.id, button.dataset.enabled === 'true'));
    });
    this.shadowRoot.querySelectorAll('.delete-btn').forEach((button) => {
      button.addEventListener('click', () => this._deleteSync(button.dataset.id));
    });
  }

  async _fetchData() {
    try {
      const [syncResponse, connectionsResponse] = await Promise.all([
        fetch('/api/v1/sync'),
        fetch('/api/v1/connections'),
      ]);
      this._relationships = await syncResponse.json();
      this._connections   = await connectionsResponse.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch data:', error);
    }
  }

  async _submitForm() {
    const name          = this.shadowRoot.querySelector('#form-name').value;
    const connectionId  = this.shadowRoot.querySelector('#form-connection').value;
    const remotePath    = this.shadowRoot.querySelector('#form-remote-path').value;
    const localPath     = this.shadowRoot.querySelector('#form-local-path').value;
    const direction     = this.shadowRoot.querySelector('#form-direction').value;
    const filter        = this.shadowRoot.querySelector('#form-filter').value;

    if (!name || !connectionId || !remotePath || !localPath)
      return;

    try {
      await fetch('/api/v1/sync', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name,
          remote_connection_id: connectionId,
          remote_path:          remotePath,
          local_path:           localPath,
          direction,
          filter:               (filter) ? filter : null,
        }),
      });
      this._showAddForm = false;
      await this._fetchData();
    } catch (error) {
      console.error('Failed to create sync:', error);
    }
  }

  async _triggerSync(id) {
    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      const result   = await response.json();
      const pull     = result.pull || {};
      const push     = result.push || {};
      alert(`Sync complete!\nPulled: ${pull.files_downloaded || 0} files\nPushed: ${push.files_uploaded || 0} files`);
    } catch (error) {
      alert(`Sync failed: ${error.message}`);
    }
  }

  async _toggleSync(id, isEnabled) {
    const action = (isEnabled) ? 'disable' : 'enable';
    try {
      await fetch(`/api/v1/sync/${id}/${action}`, { method: 'POST' });
      await this._fetchData();
    } catch (error) {
      console.error('Failed to toggle sync:', error);
    }
  }

  async _deleteSync(id) {
    if (!confirm('Delete this sync relationship?'))
      return;

    try {
      await fetch(`/api/v1/sync/${id}`, { method: 'DELETE' });
      await this._fetchData();
    } catch (error) {
      console.error('Failed to delete sync:', error);
    }
  }
}

customElements.define('aeor-sync', AeorSync);

export { AeorSync };
