'use strict';

class AeorConnections extends HTMLElement {
  constructor() {
    super();
    this._connections = [];
    this._showAddForm = false;
    this._selectedId = null;
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

      <div class="connections-list">
        ${(this._connections.length === 0)
          ? '<div class="empty-state">No connections configured. Add one to get started.</div>'
          : this._renderTable()
        }
      </div>

      <div class="connection-preview" style="display:none">
        <div class="preview-resize-handle"></div>
        <div class="preview-header">
          <h3 class="preview-title"></h3>
          <div class="preview-actions">
            <button class="secondary small preview-close">\u2715</button>
          </div>
        </div>
        <div class="preview-iframe-container">
          <iframe class="connection-iframe" sandbox="allow-same-origin allow-scripts allow-forms allow-popups" referrerpolicy="no-referrer"></iframe>
        </div>
      </div>
    `;

    this.querySelector('#add-btn')
      .addEventListener('click', () => {
        this._showAddForm = !this._showAddForm;
        this._selectedId = null;
        this.render();
      });

    if (this._showAddForm)
      this._bindFormEvents();

    this._bindTableEvents();
    this._bindPreviewEvents();

    // Restore selection if we had one
    if (this._selectedId) {
      this._showConnectionPreview(this._selectedId);
    }
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
          <input type="text" id="form-url" placeholder="http://localhost:6830">
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
    const rows = this._connections.map((connection) => {
      const isSelected = (connection.id === this._selectedId);
      return `
        <tr class="connection-row ${isSelected ? 'selected' : ''}" data-id="${connection.id}">
          <td class="mono muted">${connection.id.substring(0, 8)}...</td>
          <td>${connection.name}</td>
          <td>${connection.url}</td>
          <td>${connection.auth_type}</td>
          <td class="actions">
            <button class="secondary small test-btn" data-id="${connection.id}">Test</button>
            <button class="danger small delete-btn" data-id="${connection.id}">Delete</button>
          </td>
        </tr>
      `;
    }).join('');

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
    // Row click — select connection (but not when clicking buttons)
    this.querySelectorAll('.connection-row').forEach((row) => {
      row.addEventListener('click', (event) => {
        if (event.target.closest('button')) return;
        const id = row.dataset.id;
        if (this._selectedId === id) {
          // Clicking the same row deselects
          this._selectedId = null;
          this._hideConnectionPreview();
          row.classList.remove('selected');
        } else {
          // Deselect previous
          const prev = this.querySelector('.connection-row.selected');
          if (prev) prev.classList.remove('selected');
          // Select new
          this._selectedId = id;
          row.classList.add('selected');
          this._showConnectionPreview(id);
        }
      });
    });

    this.querySelectorAll('.test-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._testConnection(button.dataset.id);
      });
    });
    this.querySelectorAll('.delete-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._deleteConnection(button.dataset.id);
      });
    });
  }

  _bindPreviewEvents() {
    const closeBtn = this.querySelector('.preview-close');
    if (closeBtn) {
      closeBtn.addEventListener('click', () => {
        this._selectedId = null;
        this._hideConnectionPreview();
        const prev = this.querySelector('.connection-row.selected');
        if (prev) prev.classList.remove('selected');
      });
    }

    // Resize handle
    const resizeHandle = this.querySelector('.connection-preview .preview-resize-handle');
    const panel = this.querySelector('.connection-preview');
    if (resizeHandle && panel) {
      resizeHandle.addEventListener('mousedown', (event) => {
        event.preventDefault();
        const startY = event.clientY;
        const startHeight = panel.offsetHeight;

        const onMouseMove = (moveEvent) => {
          const delta = startY - moveEvent.clientY;
          const newHeight = Math.max(200, Math.min(window.innerHeight * 0.85, startHeight + delta));
          panel.style.height = newHeight + 'px';
        };

        const onMouseUp = () => {
          document.removeEventListener('mousemove', onMouseMove);
          document.removeEventListener('mouseup', onMouseUp);
        };

        document.addEventListener('mousemove', onMouseMove);
        document.addEventListener('mouseup', onMouseUp);
      });
    }
  }

  _showConnectionPreview(id) {
    const connection = this._connections.find((c) => c.id === id);
    if (!connection) return;

    const panel = this.querySelector('.connection-preview');
    if (!panel) return;

    // Update header
    panel.querySelector('.preview-title').textContent = `${connection.name} — Dashboard`;

    // Set iframe src — use the portal URL with frame=true for future support
    const iframe = panel.querySelector('.connection-iframe');
    const portalUrl = `${connection.url}/system/portal?page=dashboard&frame=true`;
    if (iframe.src !== portalUrl) {
      iframe.src = portalUrl;
    }

    panel.style.display = '';
  }

  _hideConnectionPreview() {
    const panel = this.querySelector('.connection-preview');
    if (!panel) return;

    panel.style.display = 'none';

    // Clear the iframe to stop any ongoing loading
    const iframe = panel.querySelector('.connection-iframe');
    if (iframe) iframe.src = 'about:blank';
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
      window.aeorToast(
        (result.success) ? `Connected! (${result.latency_ms}ms)` : `Failed: ${result.message}`,
        (result.success) ? 'success' : 'error',
      );
    } catch (error) {
      window.aeorToast(`Test failed: ${error.message}`, 'error');
    }
  }

  async _deleteConnection(id) {
    if (!confirm('Delete this connection?'))
      return;

    try {
      await fetch(`/api/v1/connections/${id}`, { method: 'DELETE' });
      if (this._selectedId === id) {
        this._selectedId = null;
        this._hideConnectionPreview();
      }
      await this._fetchConnections();
    } catch (error) {
      console.error('Failed to delete connection:', error);
    }
  }
}

customElements.define('aeor-connections', AeorConnections);

export { AeorConnections };
