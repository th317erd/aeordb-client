'use strict';

class AeorFileBrowser extends HTMLElement {
  constructor() {
    super();
    this._tabs = [];
    this._active_tab_id = null;
    this._relationships = [];
    this._current_entries = [];
    this._tab_counter = 0;
    this._loading = false;
  }

  connectedCallback() {
    this.render();
    this._fetchRelationships();
  }

  render() {
    if (this._active_tab_id) {
      this.innerHTML = `
        <div class="page-header">
          <h1>Files</h1>
        </div>
        ${this._renderTabBar()}
        ${this._renderDirectoryListing()}
      `;
    } else {
      this.innerHTML = `
        <div class="page-header">
          <h1>Files</h1>
        </div>
        ${(this._tabs.length > 0) ? this._renderTabBar() : ''}
        ${this._renderRelationshipSelector()}
      `;
    }

    this._bindEvents();
  }

  _renderRelationshipSelector() {
    if (this._relationships.length === 0) {
      return '<div class="empty-state">No sync relationships configured. Set up a sync first.</div>';
    }

    const cards = this._relationships.map((rel) => {
      const remoteName = rel.remote_path.replace(/\/$/, '').split('/').pop() || rel.remote_path;
      const localName = rel.local_path.split('/').pop() || rel.local_path;
      const arrow = (rel.direction === 'pull_only') ? '\u2190' : (rel.direction === 'push_only') ? '\u2192' : '\u2194';
      const displayName = rel.name || `${remoteName} ${arrow} ${localName}`;

      return `
        <div class="relationship-card" data-id="${rel.id}" data-name="${this._escapeAttr(displayName)}">
          <div class="relationship-card-name">${this._escapeHtml(displayName)}</div>
          <div class="relationship-card-paths">${this._escapeHtml(rel.remote_path)} ${arrow} ${this._escapeHtml(rel.local_path)}</div>
        </div>
      `;
    }).join('');

    return `
      <div class="file-browser-relationships">
        ${cards}
      </div>
    `;
  }

  _renderTabBar() {
    const tabs = this._tabs.map((tab) => {
      const isActive = (tab.id === this._active_tab_id);
      const label = this._truncate(`${tab.relationship_name} ${tab.path}`, 30);

      return `
        <div class="tab ${(isActive) ? 'active' : ''}" data-tab-id="${tab.id}">
          <span class="tab-label">${this._escapeHtml(label)}</span>
          <span class="tab-close" data-tab-close="${tab.id}">&times;</span>
        </div>
      `;
    }).join('');

    return `
      <div class="tab-bar">
        ${tabs}
        <div class="tab-new" title="Open another relationship">+</div>
      </div>
    `;
  }

  _renderDirectoryListing() {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return '';

    const breadcrumbs = this._renderBreadcrumbs(tab.path);
    const header = `
      <div class="page-header">
        ${breadcrumbs}
        <button class="secondary small" disabled>Upload</button>
      </div>
    `;

    if (this._loading) {
      return `${header}<div class="loading">Loading...</div>`;
    }

    if (this._current_entries.length === 0) {
      return `${header}<div class="empty-state">This directory is empty.</div>`;
    }

    const rows = this._current_entries.map((entry) => {
      const isDir = (entry.entry_type === 3);
      const isSymlink = (entry.entry_type === 8);
      const icon = (isDir) ? '\uD83D\uDCC1' : (isSymlink) ? '\uD83D\uDD17' : '\uD83D\uDCC4';
      const size = (isDir) ? '\u2014' : this._formatSize(entry.size);
      const created = this._formatDate(entry.created_at);
      const modified = this._formatDate(entry.updated_at);
      const syncClass = (entry.sync_status === 'synced') ? 'synced' : (entry.sync_status === 'pending') ? 'pending' : 'not-synced';
      const syncTitle = entry.sync_status || 'unknown';

      return `
        <tr class="file-entry" data-name="${this._escapeAttr(entry.name)}" data-type="${entry.entry_type}">
          <td><span class="sync-badge ${syncClass}" title="${syncTitle}"></span><span class="file-icon">${icon}</span>${this._escapeHtml(entry.name)}</td>
          <td>${size}</td>
          <td>${created}</td>
          <td>${modified}</td>
        </tr>
      `;
    }).join('');

    return `
      ${header}
      <table>
        <thead>
          <tr><th>Name</th><th>Size</th><th>Created</th><th>Modified</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _renderBreadcrumbs(path) {
    const segments = path.split('/').filter((s) => s.length > 0);
    let html = '<div class="breadcrumbs"><span class="breadcrumb-segment" data-path="/">Root</span>';

    let accumulated = '/';
    for (const segment of segments) {
      accumulated += segment + '/';
      html += `<span class="breadcrumb-separator">/</span><span class="breadcrumb-segment" data-path="${this._escapeAttr(accumulated)}">${this._escapeHtml(segment)}</span>`;
    }

    html += '</div>';
    return html;
  }

  _bindEvents() {
    // Relationship cards
    this.querySelectorAll('.relationship-card').forEach((card) => {
      card.addEventListener('click', () => {
        this._openTab(card.dataset.id, card.dataset.name);
      });
    });

    // Tab clicks
    this.querySelectorAll('.tab-label').forEach((label) => {
      const tab = label.closest('.tab');
      label.addEventListener('click', () => {
        this._switchTab(tab.dataset.tabId);
      });
    });

    // Tab close
    this.querySelectorAll('.tab-close').forEach((btn) => {
      btn.addEventListener('click', (event) => {
        event.stopPropagation();
        this._closeTab(btn.dataset.tabClose);
      });
    });

    // New tab
    const newTabBtn = this.querySelector('.tab-new');
    if (newTabBtn) {
      newTabBtn.addEventListener('click', () => {
        this._active_tab_id = null;
        this.render();
      });
    }

    // Breadcrumbs
    this.querySelectorAll('.breadcrumb-segment').forEach((segment) => {
      segment.addEventListener('click', () => {
        this._navigateTo(segment.dataset.path);
      });
    });

    // File entries
    this.querySelectorAll('.file-entry').forEach((row) => {
      row.addEventListener('click', () => {
        const entryType = parseInt(row.dataset.type, 10);
        if (entryType === 3) {
          const tab = this._tabs.find((t) => t.id === this._active_tab_id);
          if (tab) {
            const newPath = tab.path.replace(/\/$/, '') + '/' + row.dataset.name + '/';
            this._navigateTo(newPath);
          }
        }
        // Files: no action for now (Phase 3)
      });
    });
  }

  _openTab(relationshipId, relationshipName) {
    this._tab_counter++;
    const tabId = 'tab-' + this._tab_counter;
    this._tabs.push({
      relationship_id: relationshipId,
      relationship_name: relationshipName,
      path: '/',
      id: tabId,
    });
    this._active_tab_id = tabId;
    this._fetchListing(relationshipId, '/');
  }

  _switchTab(tabId) {
    if (this._active_tab_id === tabId) return;
    this._active_tab_id = tabId;
    const tab = this._tabs.find((t) => t.id === tabId);
    if (tab) {
      this._fetchListing(tab.relationship_id, tab.path);
    }
  }

  _closeTab(tabId) {
    this._tabs = this._tabs.filter((t) => t.id !== tabId);
    if (this._active_tab_id === tabId) {
      if (this._tabs.length > 0) {
        this._active_tab_id = this._tabs[this._tabs.length - 1].id;
        const tab = this._tabs.find((t) => t.id === this._active_tab_id);
        if (tab) {
          this._fetchListing(tab.relationship_id, tab.path);
          return;
        }
      } else {
        this._active_tab_id = null;
        this._current_entries = [];
      }
    }
    this.render();
  }

  _navigateTo(path) {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return;
    tab.path = path;
    this._fetchListing(tab.relationship_id, path);
  }

  async _fetchRelationships() {
    try {
      const response = await fetch('/api/v1/sync');
      this._relationships = await response.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch relationships:', error);
    }
  }

  async _fetchListing(relationshipId, path) {
    this._loading = true;
    this.render();

    try {
      const encodedPath = (path === '/') ? '' : encodeURIComponent(path);
      const url = (encodedPath)
        ? `/api/v1/browse/${relationshipId}/${encodedPath}`
        : `/api/v1/browse/${relationshipId}`;
      const response = await fetch(url);
      const data = await response.json();
      this._current_entries = data.entries || [];
    } catch (error) {
      console.error('Failed to fetch listing:', error);
      this._current_entries = [];
    }

    this._loading = false;
    this.render();
  }

  _formatSize(bytes) {
    if (bytes == null) return '\u2014';
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB';
  }

  _formatDate(timestamp) {
    if (!timestamp) return '\u2014';
    const date = new Date(timestamp);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');
    const hours = String(date.getHours()).padStart(2, '0');
    const minutes = String(date.getMinutes()).padStart(2, '0');
    const seconds = String(date.getSeconds()).padStart(2, '0');
    return `${year}/${month}/${day} ${hours}:${minutes}:${seconds}`;
  }

  _truncate(str, max) {
    if (str.length <= max) return str;
    return str.substring(0, max - 1) + '\u2026';
  }

  _escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  _escapeAttr(str) {
    return str.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

customElements.define('aeor-file-browser', AeorFileBrowser);

export { AeorFileBrowser };
