'use strict';

class AeorSync extends HTMLElement {
  constructor() {
    super();
    this._relationships = [];
    this._connections   = [];
    this._showAddForm   = false;
  }

  connectedCallback() {
    this.render();
    this._fetchData();
  }

  render() {
    const hasConnections = this._connections.length > 0;
    const canAdd         = hasConnections;

    this.innerHTML = `
      <div class="page-header">
        <h1>Sync Relationships</h1>
        <button id="add-btn" class="${(this._showAddForm) ? 'secondary' : 'primary'}" ${(!canAdd && !this._showAddForm) ? 'disabled' : ''}>${(this._showAddForm) ? 'Cancel' : 'Add Sync'}</button>
      </div>

      ${(this._showAddForm) ? this._renderAddForm() : ''}

      ${(this._relationships.length === 0)
        ? (hasConnections)
          ? '<div class="empty-state">No sync relationships configured.</div>'
          : '<div class="empty-state">You must first add a <a href="#" id="go-connections">Connection</a> before you can set up a sync.</div>'
        : this._renderTable()
      }
    `;

    const addButton = this.querySelector('#add-btn');
    if (addButton && canAdd) {
      addButton.addEventListener('click', () => {
        this._showAddForm = !this._showAddForm;
        this.render();
      });
    }

    const goConnectionsLink = this.querySelector('#go-connections');
    if (goConnectionsLink) {
      goConnectionsLink.addEventListener('click', (event) => {
        event.preventDefault();
        this.dispatchEvent(new CustomEvent('navigate', {
          detail:  { page: 'connections', autoAdd: true },
          bubbles: true,
        }));
      });
    }

    if (this._showAddForm)
      this._bindFormEvents();

    this._bindTableEvents();
  }

  _renderAddForm() {
    const connectionOptions = this._connections.map((connection) =>
      `<option value="${connection.id}">${connection.name} (${connection.url})</option>`
    ).join('');

    return `
      <div class="form-panel">
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
          <button class="primary" id="form-submit">Create</button>
          <button class="secondary" id="form-cancel">Cancel</button>
        </div>
      </div>
    `;
  }

  _renderTable() {
    const rows = this._relationships.map((relationship) => `
      <tr>
        <td class="mono muted">${relationship.id.substring(0, 8)}...</td>
        <td>${relationship.name}</td>
        <td>${relationship.remote_path}</td>
        <td>${relationship.direction}</td>
        <td><span class="badge ${(relationship.enabled) ? 'success' : 'warning'}">${(relationship.enabled) ? 'enabled' : 'disabled'}</span></td>
        <td class="actions">
          <button class="success small trigger-btn" data-id="${relationship.id}">Sync</button>
          <button class="secondary small toggle-btn" data-id="${relationship.id}" data-enabled="${relationship.enabled}">${(relationship.enabled) ? 'Pause' : 'Resume'}</button>
          <button class="danger small delete-btn" data-id="${relationship.id}">Delete</button>
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
    const submitButton = this.querySelector('#form-submit');
    const cancelButton = this.querySelector('#form-cancel');
    if (submitButton) submitButton.addEventListener('click', () => this._submitForm());
    if (cancelButton) cancelButton.addEventListener('click', () => { this._showAddForm = false; this.render(); });
  }

  _bindTableEvents() {
    this.querySelectorAll('.trigger-btn').forEach((button) => {
      button.addEventListener('click', () => this._triggerSync(button.dataset.id));
    });
    this.querySelectorAll('.toggle-btn').forEach((button) => {
      button.addEventListener('click', () => this._toggleSync(button.dataset.id, button.dataset.enabled === 'true'));
    });
    this.querySelectorAll('.delete-btn').forEach((button) => {
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
    const name         = this.querySelector('#form-name').value;
    const connectionId = this.querySelector('#form-connection').value;
    const remotePath   = this.querySelector('#form-remote-path').value;
    const localPath    = this.querySelector('#form-local-path').value;
    const direction    = this.querySelector('#form-direction').value;
    const filter       = this.querySelector('#form-filter').value;

    if (!name || !connectionId || !remotePath || !localPath)
      return;

    try {
      await fetch('/api/v1/sync', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
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
