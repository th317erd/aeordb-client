'use strict';

import {
  formatSize, formatDate, fileIcon, syncBadgeClass,
  escapeHtml, escapeAttr, isImageFile,
} from './aeor-file-view-shared.js';

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
    this._loadState();
    this.render();
    this._fetchRelationships();

    if (this._active_tab_id) {
      const tab = this._tabs.find((t) => t.id === this._active_tab_id);
      if (tab)
        this._fetchListing(tab.relationship_id, tab.path);
    }
  }

  _saveState() {
    try {
      localStorage.setItem('aeordb-file-browser', JSON.stringify({
        tabs:          this._tabs,
        active_tab_id: this._active_tab_id,
        tab_counter:   this._tab_counter,
      }));
    } catch (error) {
      // localStorage unavailable
    }
  }

  _loadState() {
    try {
      const raw = localStorage.getItem('aeordb-file-browser');
      if (!raw) return;

      const state         = JSON.parse(raw);
      this._tabs          = state.tabs || [];
      this._active_tab_id = state.active_tab_id || null;
      this._tab_counter   = state.tab_counter || 0;
    } catch (error) {
      // start fresh
    }
  }

  render() {
    if (this._active_tab_id) {
      this.innerHTML = `
        <div class="page-header">
          <h1>Files</h1>
        </div>
        ${this._renderTabBar()}
        ${this._renderDirectoryView()}
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
      const localName  = rel.local_path.split('/').pop() || rel.local_path;
      const arrow      = (rel.direction === 'pull_only') ? '\u2190' : (rel.direction === 'push_only') ? '\u2192' : '\u2194';
      const displayName = rel.name || `${remoteName} ${arrow} ${localName}`;

      return `
        <div class="relationship-card" data-id="${rel.id}" data-name="${escapeAttr(displayName)}">
          <div class="relationship-card-name">${escapeHtml(displayName)}</div>
          <div class="relationship-card-paths">${escapeHtml(rel.remote_path)} ${arrow} ${escapeHtml(rel.local_path)}</div>
        </div>
      `;
    }).join('');

    return `<div class="file-browser-relationships">${cards}</div>`;
  }

  _renderTabBar() {
    const tabs = this._tabs.map((tab) => {
      const isActive = (tab.id === this._active_tab_id);
      const label    = this._truncate(`${tab.relationship_name} ${tab.path}`, 30);

      return `
        <div class="tab ${(isActive) ? 'active' : ''}" data-tab-id="${tab.id}">
          <span class="tab-label">${escapeHtml(label)}</span>
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

  _renderDirectoryView() {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return '';

    const viewMode    = tab.view_mode || 'list';
    const breadcrumbs = this._renderBreadcrumbs(tab.path);
    const header = `
      <div class="page-header">
        ${breadcrumbs}
        <div style="display: flex; gap: 8px; align-items: center;">
          <div class="view-toggle">
            <button class="small ${(viewMode === 'list') ? 'primary' : 'secondary'}" data-view="list" title="List view">&#9776;</button>
            <button class="small ${(viewMode === 'grid') ? 'primary' : 'secondary'}" data-view="grid" title="Grid view">&#9638;</button>
          </div>
          <button class="secondary small" disabled>Upload</button>
        </div>
      </div>
    `;

    if (this._loading) {
      return `${header}<div class="loading">Loading...</div>`;
    }

    if (this._current_entries.length === 0) {
      return `${header}<div class="empty-state">This directory is empty.</div>`;
    }

    if (viewMode === 'grid') {
      return `${header}${this._renderGridView()}`;
    }

    return `${header}${this._renderListView()}`;
  }

  _renderListView() {
    const rows = this._current_entries.map((entry) => {
      const isDir     = (entry.entry_type === 3);
      const icon      = fileIcon(entry.entry_type);
      const size      = (isDir) ? '\u2014' : formatSize(entry.size);
      const created   = formatDate(entry.created_at);
      const modified  = formatDate(entry.updated_at);
      const syncClass = syncBadgeClass(entry.sync_status);
      const syncTitle = entry.sync_status || 'unknown';

      return `
        <tr class="file-entry" data-name="${escapeAttr(entry.name)}" data-type="${entry.entry_type}">
          <td><span class="sync-badge ${syncClass}" title="${syncTitle}"></span><span class="file-icon">${icon}</span>${escapeHtml(entry.name)}</td>
          <td>${size}</td>
          <td>${created}</td>
          <td>${modified}</td>
        </tr>
      `;
    }).join('');

    return `
      <table>
        <thead>
          <tr><th>Name</th><th>Size</th><th>Created</th><th>Modified</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _renderGridView() {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);

    const cards = this._current_entries.map((entry) => {
      const isDir     = (entry.entry_type === 3);
      const icon      = fileIcon(entry.entry_type);
      const syncClass = syncBadgeClass(entry.sync_status);
      const size      = (isDir) ? 'Folder' : formatSize(entry.size);

      let thumbnail = `<div class="grid-card-icon">${icon}</div>`;

      // Show image thumbnail if it's an image and synced locally
      if (!isDir && isImageFile(entry.name) && entry.has_local && tab) {
        const encodedPath = encodeURIComponent(tab.path.replace(/\/$/, '') + '/' + entry.name);
        thumbnail = `<div class="grid-card-thumbnail"><img src="/api/v1/files/${tab.relationship_id}/${encodedPath}" alt="${escapeAttr(entry.name)}" loading="lazy"></div>`;
      }

      return `
        <div class="grid-card file-entry" data-name="${escapeAttr(entry.name)}" data-type="${entry.entry_type}">
          <span class="sync-badge ${syncClass}" title="${entry.sync_status || 'unknown'}"></span>
          ${thumbnail}
          <div class="grid-card-name" title="${escapeAttr(entry.name)}">${escapeHtml(this._truncate(entry.name, 20))}</div>
          <div class="grid-card-meta">${size}</div>
        </div>
      `;
    }).join('');

    return `<div class="file-grid">${cards}</div>`;
  }

  _renderBreadcrumbs(path) {
    const segments = path.split('/').filter((s) => s.length > 0);
    let html = '<div class="breadcrumbs"><span class="breadcrumb-segment" data-path="/">Root</span>';

    let accumulated = '/';
    for (const segment of segments) {
      accumulated += segment + '/';
      html += `<span class="breadcrumb-separator">/</span><span class="breadcrumb-segment" data-path="${escapeAttr(accumulated)}">${escapeHtml(segment)}</span>`;
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

    // View toggle (per-tab)
    this.querySelectorAll('[data-view]').forEach((btn) => {
      btn.addEventListener('click', () => {
        const tab = this._tabs.find((t) => t.id === this._active_tab_id);
        if (tab) {
          tab.view_mode = btn.dataset.view;
          this._saveState();
          this.render();
        }
      });
    });

    // Breadcrumbs
    this.querySelectorAll('.breadcrumb-segment').forEach((segment) => {
      segment.addEventListener('click', () => {
        this._navigateTo(segment.dataset.path);
      });
    });

    // File entries (both list rows and grid cards)
    this.querySelectorAll('.file-entry').forEach((el) => {
      el.addEventListener('click', () => {
        const entryType = parseInt(el.dataset.type, 10);
        if (entryType === 3) {
          const tab = this._tabs.find((t) => t.id === this._active_tab_id);
          if (tab) {
            const newPath = tab.path.replace(/\/$/, '') + '/' + el.dataset.name + '/';
            this._navigateTo(newPath);
          }
        }
      });
    });
  }

  _openTab(relationshipId, relationshipName) {
    this._tab_counter++;
    const tabId = 'tab-' + this._tab_counter;
    this._tabs.push({
      relationship_id:   relationshipId,
      relationship_name: relationshipName,
      path:              '/',
      id:                tabId,
      view_mode:         'list',
    });
    this._active_tab_id = tabId;
    this._saveState();
    this._fetchListing(relationshipId, '/');
  }

  _switchTab(tabId) {
    if (this._active_tab_id === tabId) return;
    this._active_tab_id = tabId;
    this._saveState();
    const tab = this._tabs.find((t) => t.id === tabId);
    if (tab)
      this._fetchListing(tab.relationship_id, tab.path);
  }

  _closeTab(tabId) {
    this._tabs = this._tabs.filter((t) => t.id !== tabId);
    if (this._active_tab_id === tabId) {
      if (this._tabs.length > 0) {
        this._active_tab_id = this._tabs[this._tabs.length - 1].id;
        const tab = this._tabs.find((t) => t.id === this._active_tab_id);
        if (tab) {
          this._saveState();
          this._fetchListing(tab.relationship_id, tab.path);
          return;
        }
      } else {
        this._active_tab_id = null;
        this._current_entries = [];
      }
    }
    this._saveState();
    this.render();
  }

  _navigateTo(path) {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return;
    tab.path = path;
    this._saveState();
    this._fetchListing(tab.relationship_id, path);
  }

  async _fetchRelationships() {
    try {
      const response      = await fetch('/api/v1/sync');
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
      const url         = (encodedPath)
        ? `/api/v1/browse/${relationshipId}/${encodedPath}`
        : `/api/v1/browse/${relationshipId}`;
      const response = await fetch(url);
      const data     = await response.json();
      this._current_entries = data.entries || [];
    } catch (error) {
      console.error('Failed to fetch listing:', error);
      this._current_entries = [];
    }

    this._loading = false;
    this.render();
  }

  _truncate(str, max) {
    if (str.length <= max) return str;
    return str.substring(0, max - 1) + '\u2026';
  }
}

customElements.define('aeor-file-browser', AeorFileBrowser);

export { AeorFileBrowser };
