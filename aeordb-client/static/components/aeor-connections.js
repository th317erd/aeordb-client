'use strict';

class AeorConnections extends HTMLElement {
  constructor() {
    super();
    this._connections = [];
    this._showAddForm = false;
  }

  connectedCallback() {
    this.render();
    this._fetchConnections();
  }

  openAddForm() {
    this._showAddForm = true;
    this.render();
  }

  render() {
    this.innerHTML = `
      <div class="page-header">
        <h1>Connections</h1>
        <button id="add-btn" class="${(this._showAddForm) ? 'secondary' : 'primary'}">${(this._showAddForm) ? 'Cancel' : 'Add Connection'}</button>
      </div>

      ${(this._showAddForm) ? this._renderAddForm() : ''}

      ${(this._connections.length === 0)
        ? '<div class="empty-state">No connections configured. Add one to get started.</div>'
        : this._renderTable()
      }
    `;

    this.querySelector('#add-btn')
      .addEventListener('click', () => {
        this._showAddForm = !this._showAddForm;
        this.render();
      });

    if (this._showAddForm)
      this._bindFormEvents();

    this._bindTableEvents();
  }

  _renderAddForm() {
    return `
      <div class="form-panel">
        <h2>New Connection</h2>
        <div class="form-row">
          <label>Name</label>
          <input type="text" id="form-name" placeholder="My Server">
        </div>
        <div class="form-row">
          <label>URL</label>
          <input type="text" id="form-url" placeholder="http://localhost:3000">
        </div>
        <div class="form-row">
          <label>API Key (optional)</label>
          <input type="text" id="form-api-key" placeholder="aeor_...">
        </div>
        <div class="form-actions">
          <button class="primary" id="form-submit">Create</button>
          <button class="secondary" id="form-cancel">Cancel</button>
        </div>
      </div>
    `;
  }

  _renderTable() {
    const rows = this._connections.map((connection) => `
      <tr>
        <td class="mono muted">${connection.id.substring(0, 8)}...</td>
        <td>${connection.name}</td>
        <td>${connection.url}</td>
        <td>${connection.auth_type}</td>
        <td class="actions">
          <button class="secondary small test-btn" data-id="${connection.id}">Test</button>
          <button class="danger small delete-btn" data-id="${connection.id}">Delete</button>
        </td>
      </tr>
    `).join('');

    return `
      <table>
        <thead>
          <tr><th>ID</th><th>Name</th><th>URL</th><th>Auth</th><th>Actions</th></tr>
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
    this.querySelectorAll('.test-btn').forEach((button) => {
      button.addEventListener('click', () => this._testConnection(button.dataset.id));
    });
    this.querySelectorAll('.delete-btn').forEach((button) => {
      button.addEventListener('click', () => this._deleteConnection(button.dataset.id));
    });
  }

  async _fetchConnections() {
    try {
      const response    = await fetch('/api/v1/connections');
      this._connections = await response.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch connections:', error);
    }
  }

  async _submitForm() {
    const name   = this.querySelector('#form-name').value;
    const url    = this.querySelector('#form-url').value;
    const apiKey = this.querySelector('#form-api-key').value;

    if (!name || !url)
      return;

    try {
      await fetch('/api/v1/connections', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          name,
          url,
          auth_type: (apiKey) ? 'api_key' : 'none',
          api_key:   (apiKey) ? apiKey : null,
        }),
      });
      this._showAddForm = false;
      await this._fetchConnections();
    } catch (error) {
      console.error('Failed to create connection:', error);
    }
  }

  async _testConnection(id) {
    try {
      const response = await fetch(`/api/v1/connections/${id}/test`, { method: 'POST' });
      const result   = await response.json();
      alert((result.success) ? `Connected! (${result.latency_ms}ms)` : `Failed: ${result.message}`);
    } catch (error) {
      alert(`Test failed: ${error.message}`);
    }
  }

  async _deleteConnection(id) {
    if (!confirm('Delete this connection?'))
      return;

    try {
      await fetch(`/api/v1/connections/${id}`, { method: 'DELETE' });
      await this._fetchConnections();
    } catch (error) {
      console.error('Failed to delete connection:', error);
    }
  }
}

customElements.define('aeor-connections', AeorConnections);

export { AeorConnections };
