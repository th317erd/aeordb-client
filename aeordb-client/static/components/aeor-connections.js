'use strict';

class AeorConnections extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this._connections = [];
    this._showAddForm = false;
  }

  connectedCallback() {
    this.render();
    this._fetchConnections();
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host { display: block; }

        h1 {
          font-size: 24px;
          font-weight: 600;
          margin-bottom: 24px;
          color: var(--text-primary, #e6edf3);
          display: flex;
          justify-content: space-between;
          align-items: center;
        }

        button {
          font-size: 14px;
          padding: 6px 16px;
          border-radius: 6px;
          border: 1px solid var(--border, #30363d);
          background-color: var(--accent, #58a6ff);
          color: #000;
          cursor: pointer;
          font-weight: 500;
        }

        button:hover { background-color: var(--accent-hover, #79c0ff); }
        button.secondary { background-color: var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); }
        button.secondary:hover { background-color: var(--border, #30363d); }
        button.danger { background-color: var(--error, #f85149); border-color: var(--error, #f85149); }
        button.danger:hover { opacity: 0.9; }

        table { width: 100%; border-collapse: collapse; }
        th { text-align: left; padding: 8px 12px; border-bottom: 1px solid var(--border, #30363d); color: var(--text-secondary, #8b949e); font-weight: 600; font-size: 12px; text-transform: uppercase; }
        td { padding: 8px 12px; border-bottom: 1px solid var(--bg-tertiary, #21262d); color: var(--text-primary, #e6edf3); }

        .empty { color: var(--text-muted, #484f58); font-style: italic; padding: 40px; text-align: center; }

        .actions { display: flex; gap: 8px; }

        .form-overlay {
          background-color: var(--bg-secondary, #161b22);
          border: 1px solid var(--border, #30363d);
          border-radius: 8px;
          padding: 20px;
          margin-bottom: 20px;
        }

        .form-overlay h2 { font-size: 16px; margin-bottom: 16px; }

        .form-row {
          margin-bottom: 12px;
        }

        .form-row label {
          display: block;
          font-size: 12px;
          color: var(--text-secondary, #8b949e);
          margin-bottom: 4px;
        }

        .form-row input {
          width: 100%;
          padding: 8px 12px;
          border-radius: 6px;
          border: 1px solid var(--border, #30363d);
          background-color: var(--bg-tertiary, #21262d);
          color: var(--text-primary, #e6edf3);
          font-size: 14px;
        }

        .form-row input:focus { border-color: var(--accent, #58a6ff); outline: none; }
        .form-actions { display: flex; gap: 8px; margin-top: 16px; }

        .id-cell {
          font-family: var(--font-mono, monospace);
          font-size: 12px;
          color: var(--text-secondary, #8b949e);
        }
      </style>

      <h1>
        Connections
        <button id="add-btn">${(this._showAddForm) ? 'Cancel' : 'Add Connection'}</button>
      </h1>

      ${(this._showAddForm) ? this._renderAddForm() : ''}

      ${(this._connections.length === 0)
        ? '<div class="empty">No connections configured. Add one to get started.</div>'
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
    return `
      <div class="form-overlay">
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
          <button id="form-submit">Create</button>
          <button class="secondary" id="form-cancel">Cancel</button>
        </div>
      </div>
    `;
  }

  _renderTable() {
    const rows = this._connections.map((connection) => `
      <tr>
        <td class="id-cell">${connection.id.substring(0, 8)}...</td>
        <td>${connection.name}</td>
        <td>${connection.url}</td>
        <td>${connection.auth_type}</td>
        <td class="actions">
          <button class="secondary test-btn" data-id="${connection.id}">Test</button>
          <button class="danger delete-btn" data-id="${connection.id}">Delete</button>
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
    const submitButton = this.shadowRoot.querySelector('#form-submit');
    const cancelButton = this.shadowRoot.querySelector('#form-cancel');

    if (submitButton)
      submitButton.addEventListener('click', () => this._submitForm());

    if (cancelButton)
      cancelButton.addEventListener('click', () => {
        this._showAddForm = false;
        this.render();
      });
  }

  _bindTableEvents() {
    this.shadowRoot.querySelectorAll('.test-btn').forEach((button) => {
      button.addEventListener('click', () => this._testConnection(button.dataset.id));
    });

    this.shadowRoot.querySelectorAll('.delete-btn').forEach((button) => {
      button.addEventListener('click', () => this._deleteConnection(button.dataset.id));
    });
  }

  async _fetchConnections() {
    try {
      const response = await fetch('/api/v1/connections');
      this._connections = await response.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch connections:', error);
    }
  }

  async _submitForm() {
    const name    = this.shadowRoot.querySelector('#form-name').value;
    const url     = this.shadowRoot.querySelector('#form-url').value;
    const apiKey  = this.shadowRoot.querySelector('#form-api-key').value;

    if (!name || !url)
      return;

    const body = {
      name,
      url,
      auth_type: (apiKey) ? 'api_key' : 'none',
      api_key:   (apiKey) ? apiKey : null,
    };

    try {
      await fetch('/api/v1/connections', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify(body),
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
