'use strict';

import { escapeHtml, escapeAttr, formatSize, bindResizeHandle, formatRelativeTime, showConfirm } from './aeor-file-view-shared.js';

class AeorSync extends HTMLElement {
  constructor() {
    super();
    this._relationships = [];
    this._connections   = [];
    this._showAddForm   = false;
    this._editingId     = null;
    this._selectedId    = null;
    this._activity      = [];
  }

  connectedCallback() {
    this.render();
    this._fetchData();
  }

  refresh() {
    this._fetchData();
  }

  render() {
    const hasConnections = this._connections.length > 0;
    const canAdd         = hasConnections;

    this.innerHTML = `
      <div class="page-header">
        <h1>Sync Relationships</h1>
        <button id="add-btn" class="${(this._showAddForm || this._editingId) ? 'secondary' : 'primary'}" ${(!canAdd && !this._showAddForm && !this._editingId) ? 'disabled' : ''}>${(this._showAddForm || this._editingId) ? 'Cancel' : 'Add Sync'}</button>
      </div>

      ${(this._showAddForm) ? this._renderAddForm() : ''}
      ${(this._editingId) ? this._renderEditForm() : ''}

      <div class="sync-list">
        ${(this._relationships.length === 0)
          ? (hasConnections)
            ? '<div class="empty-state">No sync relationships configured.</div>'
            : '<div class="empty-state">You must first add a <a href="#" id="go-connections">Connection</a> before you can set up a sync.</div>'
          : this._renderTable()
        }
      </div>

      <div class="sync-activity-panel" style="display:none">
        <div class="preview-resize-handle"></div>
        <div class="preview-header">
          <h3 class="preview-title"></h3>
          <div class="preview-actions">
            <button class="secondary small activity-close">\u2715</button>
          </div>
        </div>
        <div class="activity-feed"></div>
      </div>
    `;

    const addButton = this.querySelector('#add-btn');
    if (addButton && (canAdd || this._showAddForm || this._editingId)) {
      addButton.addEventListener('click', () => {
        this._showAddForm = !this._showAddForm && !this._editingId;
        this._editingId   = null;
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

    if (this._showAddForm || this._editingId)
      this._bindFormEvents();

    this._bindTableEvents();
    this._bindActivityEvents();

    // Restore selection
    if (this._selectedId) {
      this._fetchActivity(this._selectedId);
    }
  }

  _renderAddForm() {
    const connectionOptions = this._connections.map((connection) =>
      `<option value="${escapeAttr(connection.id)}">${escapeHtml(connection.name)} (${escapeHtml(connection.url)})</option>`
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
          <div style="display: flex; gap: 8px;">
            <input type="text" id="form-local-path" placeholder="/home/user/Documents" style="flex: 1;">
            <button class="secondary small" type="button" id="browse-local-path">Browse</button>
          </div>
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

  _renderEditForm() {
    const relationship = this._relationships.find((r) => r.id === this._editingId);
    if (!relationship) return '';

    return `
      <div class="form-panel">
        <h2>Edit Sync Relationship</h2>
        <div class="form-row">
          <label>Name</label>
          <input type="text" id="form-name" value="${escapeAttr(relationship.name || '')}">
        </div>
        <div class="form-row">
          <label>Remote Path</label>
          <input type="text" id="form-remote-path" value="${escapeAttr(relationship.remote_path)}">
        </div>
        <div class="form-row">
          <label>Local Path</label>
          <div style="display: flex; gap: 8px;">
            <input type="text" id="form-local-path" value="${escapeAttr(relationship.local_path)}" style="flex: 1;">
            <button class="secondary small" type="button" id="browse-local-path">Browse</button>
          </div>
        </div>
        <div class="form-row">
          <label>Direction</label>
          <select id="form-direction">
            <option value="pull_only" ${(relationship.direction === 'pull_only') ? 'selected' : ''}>Pull Only</option>
            <option value="push_only" ${(relationship.direction === 'push_only') ? 'selected' : ''}>Push Only</option>
            <option value="bidirectional" ${(relationship.direction === 'bidirectional') ? 'selected' : ''}>Bidirectional</option>
          </select>
        </div>
        <div class="form-row">
          <label>Filter (optional, comma-separated globs)</label>
          <input type="text" id="form-filter" value="${escapeAttr(relationship.filter || '')}">
        </div>
        <div class="form-row">
          <label>Delete Propagation</label>
          <label class="checkbox-row">
            <input type="checkbox" class="checkbox-large" id="form-delete-local-to-remote" ${(relationship.delete_propagation && relationship.delete_propagation.local_to_remote) ? 'checked' : ''}>
            When a file is deleted locally, also delete it on the remote
          </label>
          <label class="checkbox-row">
            <input type="checkbox" class="checkbox-large" id="form-delete-remote-to-local" ${(relationship.delete_propagation && relationship.delete_propagation.remote_to_local) ? 'checked' : ''}>
            When a file is deleted on the remote, also delete it locally
          </label>
        </div>
        <div class="form-actions">
          <button class="primary" id="form-submit">Save Changes</button>
          <button class="secondary" id="form-cancel">Cancel</button>
        </div>
      </div>
    `;
  }

  _renderTable() {
    const rows = this._relationships.map((relationship) => {
      const isSelected = (relationship.id === this._selectedId);
      return `
        <tr class="sync-row ${isSelected ? 'selected' : ''}" data-id="${relationship.id}">
          <td class="mono muted">${escapeHtml(relationship.id.substring(0, 8))}...</td>
          <td>${escapeHtml(relationship.name)}</td>
          <td>${escapeHtml(relationship.remote_path)}</td>
          <td>${escapeHtml(relationship.direction)}</td>
          <td><span class="badge ${(relationship.enabled) ? 'success' : 'warning'}" style="min-width: 72px; text-align: center; display: inline-block;">${(relationship.enabled) ? 'enabled' : 'disabled'}</span></td>
          <td class="actions">
            <button class="success small trigger-btn" data-id="${relationship.id}">Sync</button>
            <button class="secondary small edit-btn" data-id="${relationship.id}">Edit</button>
            <button class="secondary small toggle-btn" data-id="${relationship.id}" data-enabled="${relationship.enabled}" style="min-width: 70px;">${(relationship.enabled) ? 'Pause' : 'Resume'}</button>
            <button class="danger small delete-btn" data-id="${relationship.id}">Delete</button>
          </td>
        </tr>
      `;
    }).join('');

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

    if (submitButton) {
      submitButton.addEventListener('click', () => {
        if (this._editingId)
          this._submitEdit();
        else
          this._submitForm();
      });
    }

    if (cancelButton) {
      cancelButton.addEventListener('click', () => {
        this._showAddForm = false;
        this._editingId   = null;
        this.render();
      });
    }

    const browseButton = this.querySelector('#browse-local-path');
    if (browseButton) {
      browseButton.addEventListener('click', async () => {
        try {
          const response = await fetch('/api/v1/pick-directory', { method: 'POST' });
          if (!response.ok) throw new Error(`Request failed: ${response.status}`);
          const result   = await response.json();
          if (result.path) {
            const input = this.querySelector('#form-local-path');
            if (input) input.value = result.path;
          }
        } catch (error) {
          console.error('Directory picker failed:', error);
        }
      });
    }
  }

  _bindTableEvents() {
    // Row click — select to show activity
    this.querySelectorAll('.sync-row').forEach((row) => {
      row.addEventListener('click', (event) => {
        if (event.target.closest('button')) return;
        const id = row.dataset.id;
        if (this._selectedId === id) {
          this._selectedId = null;
          this._hideActivity();
          row.classList.remove('selected');
        } else {
          const prev = this.querySelector('.sync-row.selected');
          if (prev) prev.classList.remove('selected');
          this._selectedId = id;
          row.classList.add('selected');
          this._fetchActivity(id);
        }
      });
    });

    this.querySelectorAll('.trigger-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._triggerSync(button.dataset.id);
      });
    });
    this.querySelectorAll('.edit-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._editingId   = button.dataset.id;
        this._showAddForm = false;
        this.render();
      });
    });
    this.querySelectorAll('.toggle-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._toggleSync(button.dataset.id, button.dataset.enabled === 'true');
      });
    });
    this.querySelectorAll('.delete-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._deleteSync(button.dataset.id);
      });
    });
  }

  _bindActivityEvents() {
    const closeBtn = this.querySelector('.activity-close');
    if (closeBtn) {
      closeBtn.addEventListener('click', () => {
        this._selectedId = null;
        this._hideActivity();
        const prev = this.querySelector('.sync-row.selected');
        if (prev) prev.classList.remove('selected');
      });
    }

    const resizeHandle = this.querySelector('.sync-activity-panel .preview-resize-handle');
    const panel = this.querySelector('.sync-activity-panel');
    if (resizeHandle && panel) {
      bindResizeHandle(resizeHandle, panel);
    }
  }

  async _fetchActivity(id) {
    const relationship = this._relationships.find((r) => r.id === id);
    if (!relationship) return;

    const panel = this.querySelector('.sync-activity-panel');
    if (!panel) return;

    panel.querySelector('.preview-title').textContent = `${relationship.name} — Activity`;
    panel.style.display = '';

    const feed = panel.querySelector('.activity-feed');
    feed.innerHTML = '<div class="loading">Loading activity...</div>';

    try {
      const response = await fetch(`/api/v1/sync/${id}/activity`);
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._activity = await response.json();
      this._renderActivityFeed(feed);
    } catch (error) {
      feed.innerHTML = '<div class="empty-state">Failed to load activity.</div>';
    }
  }

  _renderActivityFeed(container) {
    if (this._activity.length === 0) {
      container.innerHTML = '<div class="empty-state">No sync activity recorded yet.</div>';
      return;
    }

    const items = this._activity.map((event) => {
      const time = formatRelativeTime(event.timestamp);
      const icon = this._eventIcon(event.event_type);
      const hasErrors = event.errors && event.errors.length > 0;
      const errorClass = hasErrors ? ' activity-item-error' : '';

      let detail = escapeHtml(event.summary);
      if (event.files_affected > 0) {
        detail += ` \u00B7 ${event.files_affected} files`;
      }
      if (event.bytes_transferred > 0) {
        detail += ` \u00B7 ${formatSize(event.bytes_transferred)}`;
      }
      if (event.duration_ms > 0) {
        detail += ` \u00B7 ${event.duration_ms}ms`;
      }

      let errorHtml = '';
      if (hasErrors) {
        errorHtml = `<div class="activity-errors">${event.errors.map((e) => `<div class="activity-error">${escapeHtml(e)}</div>`).join('')}</div>`;
      }

      return `
        <div class="activity-item${errorClass}">
          <div class="activity-icon">${icon}</div>
          <div class="activity-body">
            <div class="activity-summary">${detail}</div>
            ${errorHtml}
          </div>
          <div class="activity-time">${time}</div>
        </div>
      `;
    }).join('');

    container.innerHTML = items;
  }

  _hideActivity() {
    const panel = this.querySelector('.sync-activity-panel');
    if (panel) panel.style.display = 'none';
  }

  _eventIcon(type) {
    switch (type) {
      case 'pull':      return '\u2B07';  // down arrow
      case 'push':      return '\u2B06';  // up arrow
      case 'full_sync': return '\u21C4';  // bidirectional arrow
      case 'error':     return '\u26A0';  // warning
      default:          return '\u2022';  // bullet
    }
  }

  async _fetchData() {
    try {
      const [syncResponse, connectionsResponse] = await Promise.all([
        fetch('/api/v1/sync'),
        fetch('/api/v1/connections'),
      ]);

      if (!syncResponse.ok) throw new Error(`Sync request failed: ${syncResponse.status}`);
      if (!connectionsResponse.ok) throw new Error(`Connections request failed: ${connectionsResponse.status}`);

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
      const response = await fetch('/api/v1/sync', {
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
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._showAddForm = false;
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to create sync: ${error.message}`, 'error');
    }
  }

  async _submitEdit() {
    const name       = this.querySelector('#form-name').value;
    const remotePath = this.querySelector('#form-remote-path').value;
    const localPath  = this.querySelector('#form-local-path').value;
    const direction  = this.querySelector('#form-direction').value;
    const filter     = this.querySelector('#form-filter').value;

    const localToRemote = this.querySelector('#form-delete-local-to-remote')?.checked || false;
    const remoteToLocal = this.querySelector('#form-delete-remote-to-local')?.checked || false;

    try {
      const response = await fetch(`/api/v1/sync/${this._editingId}`, {
        method:  'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          name:               (name) ? name : null,
          remote_path:        (remotePath) ? remotePath : null,
          local_path:         (localPath) ? localPath : null,
          direction,
          filter:             (filter) ? filter : null,
          delete_propagation: {
            local_to_remote: localToRemote,
            remote_to_local: remoteToLocal,
          },
        }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._editingId = null;
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to update sync: ${error.message}`, 'error');
    }
  }

  async _triggerSync(id) {
    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      const result   = await response.json();
      const pull     = result.pull || {};
      const push     = result.push || {};
      window.aeorToast(`Sync complete: ${pull.files_pulled || 0} pulled, ${push.files_pushed || 0} pushed`, 'success');
      // Refresh activity if this relationship is selected
      if (this._selectedId === id) {
        this._fetchActivity(id);
      }
    } catch (error) {
      window.aeorToast(`Sync failed: ${error.message}`, 'error', 10000);
    }
  }

  async _toggleSync(id, isEnabled) {
    const action = (isEnabled) ? 'disable' : 'enable';
    try {
      const response = await fetch(`/api/v1/sync/${id}/${action}`, { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to ${action} sync: ${error.message}`, 'error');
    }
  }

  async _deleteSync(id) {
    const confirmed = await showConfirm('Delete Sync Relationship', 'Are you sure you want to delete this sync relationship?', { confirmText: 'Delete', danger: true });
    if (!confirmed) return;

    try {
      const response = await fetch(`/api/v1/sync/${id}`, { method: 'DELETE' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._selectedId === id) {
        this._selectedId = null;
        this._hideActivity();
      }
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to delete sync: ${error.message}`, 'error');
    }
  }
}

customElements.define('aeor-sync', AeorSync);

export { AeorSync };
