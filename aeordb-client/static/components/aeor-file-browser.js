'use strict';

import {
  formatSize, formatDate, fileIcon,
  escapeHtml, escapeAttr, isImageFile, isVideoFile, isAudioFile, isTextFile,
} from './aeor-file-view-shared.js';

async function loadPreviewComponent(contentType) {
  if (!contentType) return 'aeor-preview-default';

  const [group, subtype] = contentType.split('/');
  const sanitizedSubtype = (subtype || '').replace(/[^a-z0-9]/g, '-');
  const exact = `aeor-preview-${group}-${sanitizedSubtype}`;
  const grouped = `aeor-preview-${group}`;

  // Tier 1: exact mime type component
  try {
    await import(`./previews/${exact}.js`);
    if (customElements.get(exact)) return exact;
  } catch {}

  // Tier 2: group component
  try {
    await import(`./previews/${grouped}.js`);
    if (customElements.get(grouped)) return grouped;
  } catch {}

  // Tier 3: default fallback
  try {
    await import('./previews/aeor-preview-default.js');
  } catch {}

  return 'aeor-preview-default';
}

class AeorFileBrowser extends HTMLElement {
  constructor() {
    super();
    this._tabs = [];
    this._active_tab_id = null;
    this._relationships = [];
    this._tab_counter = 0;
    this._loading = false;
    this._preview_entry = null;
    this._preview_component = null;
    this._scroll_listener = null;
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
      // Only persist tab metadata — entries are fetched fresh each time
      const serializable_tabs = this._tabs.map((tab) => ({
        id:                tab.id,
        relationship_id:   tab.relationship_id,
        relationship_name: tab.relationship_name,
        path:              tab.path,
        view_mode:         tab.view_mode,
        page_size:         tab.page_size,
      }));
      localStorage.setItem('aeordb-file-browser', JSON.stringify({
        tabs:          serializable_tabs,
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
      this._active_tab_id = state.active_tab_id || null;
      this._tab_counter   = state.tab_counter || 0;

      // Restore tabs with runtime fields initialized
      this._tabs = (state.tabs || []).map((tab) => ({
        ...tab,
        entries:      [],
        total:        null,
        loading_more: false,
        page_size:    tab.page_size || 100,
      }));
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
          <button class="primary small" id="upload-button">Upload</button>
          <input type="file" id="upload-input" style="display:none" multiple>
        </div>
      </div>
    `;

    if (this._loading) {
      return `${header}<div class="loading">Loading...</div>`;
    }

    if (tab.entries.length === 0) {
      return `${header}<div class="empty-state">This directory is empty.</div>`;
    }

    const countText = (tab.total != null)
      ? `Showing ${tab.entries.length} of ${tab.total}`
      : `${tab.entries.length} items`;
    const loadingMore = (tab.loading_more)
      ? '<div class="scroll-loading">Loading more...</div>'
      : '';

    const listing = (viewMode === 'grid')
      ? this._renderGridView()
      : this._renderListView();

    return `${header}${listing}<div class="entry-count">${countText}</div>${loadingMore}${this._renderPreviewPanel()}`;
  }

  _renderListView() {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return '';

    const rows = tab.entries.map((entry) => {
      const isDir     = (entry.entry_type === 3);
      const icon      = fileIcon(entry.entry_type);
      const size      = (isDir) ? '\u2014' : formatSize(entry.size);
      const created   = formatDate(entry.created_at);
      const modified  = formatDate(entry.updated_at);
      const syncClass = (entry.has_local) ? 'synced' : 'not-synced';
      const syncTitle = (entry.has_local) ? 'Available locally' : 'Remote only';

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
    if (!tab) return '';

    const cards = tab.entries.map((entry) => {
      const isDir     = (entry.entry_type === 3);
      const icon      = fileIcon(entry.entry_type);
      const syncClass = (entry.has_local) ? 'synced' : 'not-synced';
      const syncTitle = (entry.has_local) ? 'Available locally' : 'Remote only';
      const size      = (isDir) ? 'Folder' : formatSize(entry.size);

      let thumbnail = `<div class="grid-card-icon">${icon}</div>`;

      // Show image thumbnail if it's an image and synced locally
      if (!isDir && isImageFile(entry.name) && entry.has_local && tab) {
        const encodedPath = encodeURIComponent(tab.path.replace(/\/$/, '') + '/' + entry.name);
        thumbnail = `<div class="grid-card-thumbnail"><img src="/api/v1/files/${tab.relationship_id}/${encodedPath}" alt="${escapeAttr(entry.name)}" loading="lazy"></div>`;
      }

      return `
        <div class="grid-card file-entry" data-name="${escapeAttr(entry.name)}" data-type="${entry.entry_type}">
          <span class="sync-badge ${syncClass}" title="${syncTitle}"></span>
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
        } else {
          const tab = this._tabs.find((t) => t.id === this._active_tab_id);
          const entries = (tab && tab.entries) || [];
          this._preview_entry = entries.find((e) => e.name === el.dataset.name) || null;
          this._preview_component = null;
          this._loadPreview();
        }
      });

      // Context menu (right-click) for file entries
      el.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        const entryType = parseInt(el.dataset.type, 10);
        if (entryType === 3) return;

        const tab = this._tabs.find((t) => t.id === this._active_tab_id);
        const entries = (tab && tab.entries) || [];
        const entry = entries.find((e) => e.name === el.dataset.name);
        if (!entry) return;

        this._showContextMenu(event.clientX, event.clientY, entry);
      });
    });

    // Preview action buttons
    this.querySelectorAll('[data-action]').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._handlePreviewAction(button.dataset.action);
      });
    });

    // Upload
    const uploadButton = this.querySelector('#upload-button');
    const uploadInput = this.querySelector('#upload-input');
    if (uploadButton && uploadInput) {
      uploadButton.addEventListener('click', () => uploadInput.click());
      uploadInput.addEventListener('change', (event) => this._handleUpload(event));
    }

    // Preview panel resize handle
    const resizeHandle = this.querySelector('#preview-resize-handle');
    const previewPanel = this.querySelector('#preview-panel');
    if (resizeHandle && previewPanel) {
      resizeHandle.addEventListener('mousedown', (event) => {
        event.preventDefault();
        const startY      = event.clientY;
        const startHeight = previewPanel.offsetHeight;

        const self = this;
        const onMouseMove = (moveEvent) => {
          const delta     = startY - moveEvent.clientY;
          const newHeight = Math.max(150, Math.min(window.innerHeight * 0.8, startHeight + delta));
          previewPanel.style.height = newHeight + 'px';
          self._updateContentPadding();
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

  _openTab(relationshipId, relationshipName) {
    this._tab_counter++;
    const tabId = 'tab-' + this._tab_counter;
    this._tabs.push({
      relationship_id:   relationshipId,
      relationship_name: relationshipName,
      path:              '/',
      id:                tabId,
      view_mode:         'list',
      entries:           [],
      total:             null,
      loading_more:      false,
      page_size:         100,
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
      }
    }
    this._saveState();
    this.render();
  }

  _navigateTo(path) {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return;
    tab.path = path;
    this._preview_entry = null; // Close preview when navigating
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
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return;

    tab.entries = [];
    tab.total = null;
    tab.loading_more = false;
    this._loading = true;
    this.render();

    try {
      const encodedPath = (path === '/') ? '' : encodeURIComponent(path);
      const baseUrl = (encodedPath)
        ? `/api/v1/browse/${relationshipId}/${encodedPath}`
        : `/api/v1/browse/${relationshipId}`;
      const url = `${baseUrl}?limit=${tab.page_size || 100}&offset=0`;
      const response = await fetch(url);
      const data = await response.json();
      tab.entries = data.entries || [];
      tab.total = (data.total != null) ? data.total : tab.entries.length;
    } catch (error) {
      console.error('Failed to fetch listing:', error);
      tab.entries = [];
    }

    this._loading = false;
    this.render();
    this._attachScrollListener();
  }

  async _fetchNextPage() {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab || tab.loading_more) return;
    if (tab.entries.length >= (tab.total || 0)) return;

    tab.loading_more = true;
    this.render();

    try {
      const encodedPath = (tab.path === '/') ? '' : encodeURIComponent(tab.path);
      const baseUrl = (encodedPath)
        ? `/api/v1/browse/${tab.relationship_id}/${encodedPath}`
        : `/api/v1/browse/${tab.relationship_id}`;
      const url = `${baseUrl}?limit=${tab.page_size || 100}&offset=${tab.entries.length}`;
      const response = await fetch(url);
      const data = await response.json();
      const newEntries = data.entries || [];
      for (const entry of newEntries) {
        tab.entries.push(entry);
      }
      tab.total = (data.total != null) ? data.total : tab.entries.length;
    } catch (error) {
      console.error('Failed to fetch next page:', error);
    }

    tab.loading_more = false;
    this.render();
    this._attachScrollListener();
  }

  _attachScrollListener() {
    const content = this.closest('.app-content');
    if (!content) return;

    // Remove previous listener to avoid duplicates
    if (this._scroll_listener) {
      content.removeEventListener('scroll', this._scroll_listener);
    }

    this._scroll_listener = () => {
      const tab = this._tabs.find((t) => t.id === this._active_tab_id);
      if (!tab || tab.loading_more) return;
      if (tab.total == null) return;
      if (tab.entries.length >= tab.total) return;

      const scrollBottom = content.scrollHeight - content.scrollTop - content.clientHeight;
      if (scrollBottom < 200) {
        this._fetchNextPage();
      }
    };

    content.addEventListener('scroll', this._scroll_listener);
  }

  _renderPreviewPanel() {
    if (!this._preview_entry || !this._preview_component) return '';

    const entry = this._preview_entry;
    const componentName = this._preview_component;

    return `
      <div class="preview-panel" id="preview-panel">
        <div class="preview-resize-handle" id="preview-resize-handle"></div>
        <div class="preview-header">
          <h3>${escapeHtml(entry.name)}</h3>
          <div class="preview-actions">
            ${(entry.has_local)
              ? '<button class="primary small" data-action="open-local">Open Locally</button>'
              : '<button class="secondary small" data-action="download">Download</button>'
            }
            <button class="secondary small" data-action="rename">Rename</button>
            <button class="danger small" data-action="delete">Delete</button>
            <button class="secondary small" data-action="close-preview">\u2715</button>
          </div>
        </div>
        <div class="preview-content">
          <${componentName}></${componentName}>
        </div>
        <div class="preview-meta">
          ${formatSize(entry.size)} \u00B7 ${entry.content_type || 'Unknown type'} \u00B7 ${formatDate(entry.created_at)}
        </div>
      </div>
    `;
  }

  async _loadPreview() {
    if (!this._preview_entry) return;

    const contentType = this._preview_entry.content_type || 'application/octet-stream';
    this._preview_component = await loadPreviewComponent(contentType);
    this.render();

    // After render, the custom element is in the DOM — set its attributes
    const previewEl = this.querySelector(this._preview_component);
    if (previewEl) {
      const tab = this._tabs.find((t) => t.id === this._active_tab_id);
      if (tab) {
        const filePath = tab.path.replace(/\/$/, '') + '/' + this._preview_entry.name;
        const fileUrl = `/api/v1/files/${tab.relationship_id}/${encodeURIComponent(filePath)}`;
        previewEl.setAttribute('src', fileUrl);
        previewEl.setAttribute('filename', this._preview_entry.name);
        previewEl.setAttribute('size', this._preview_entry.size || 0);
        previewEl.setAttribute('content-type', contentType);
        if (previewEl.load) previewEl.load();
      }
    }

    this._updateContentPadding();
  }

  _updateContentPadding() {
    const content = this.closest('.app-content');
    if (!content) return;

    const panel = this.querySelector('#preview-panel');
    if (panel) {
      content.style.paddingBottom = panel.offsetHeight + 'px';
    } else {
      content.style.paddingBottom = '';
    }
  }

  async _handlePreviewAction(action) {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab || !this._preview_entry) return;

    const entry = this._preview_entry;
    const filePath = tab.path.replace(/\/$/, '') + '/' + entry.name;

    switch (action) {
      case 'open-local':
        await fetch(`/api/v1/files/${tab.relationship_id}/open`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: filePath.replace(/^\//, '') }),
        });
        break;

      case 'download': {
        const url = `/api/v1/files/${tab.relationship_id}/${encodeURIComponent(filePath)}`;
        const anchor = document.createElement('a');
        anchor.href = url;
        anchor.download = entry.name;
        anchor.click();
        break;
      }

      case 'rename': {
        const newName = prompt('New name:', entry.name);
        if (!newName || newName === entry.name) break;
        const fromPath = tab.path.replace(/\/$/, '') + '/' + entry.name;
        const toPath = tab.path.replace(/\/$/, '') + '/' + newName;
        try {
          await fetch(`/api/v1/files/${tab.relationship_id}/rename`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ from: fromPath, to: toPath }),
          });
          this._preview_entry = null;
          this._fetchListing(tab.relationship_id, tab.path);
        } catch (error) {
          alert('Rename failed: ' + error.message);
        }
        break;
      }

      case 'delete':
        if (!confirm(`Delete "${entry.name}"? This cannot be undone.`)) break;
        try {
          const encodedPath = encodeURIComponent(filePath);
          await fetch(`/api/v1/files/${tab.relationship_id}/${encodedPath}`, {
            method: 'DELETE',
          });
          this._preview_entry = null;
          this._fetchListing(tab.relationship_id, tab.path);
        } catch (error) {
          alert('Delete failed: ' + error.message);
        }
        break;

      case 'close-preview':
        this._preview_entry = null;
        this._preview_component = null;
        this.render();
        this._updateContentPadding();
        break;
    }
  }

  async _handleUpload(event) {
    const tab = this._tabs.find((t) => t.id === this._active_tab_id);
    if (!tab) return;

    const files = event.target.files;
    for (const file of files) {
      const filePath = tab.path.replace(/\/$/, '') + '/' + file.name;
      const encodedPath = encodeURIComponent(filePath);

      try {
        const arrayBuffer = await file.arrayBuffer();
        await fetch(`/api/v1/files/${tab.relationship_id}/${encodedPath}`, {
          method: 'PUT',
          headers: { 'Content-Type': file.type || 'application/octet-stream' },
          body: arrayBuffer,
        });
      } catch (error) {
        alert(`Upload failed for ${file.name}: ${error.message}`);
      }
    }

    event.target.value = '';
    this._fetchListing(tab.relationship_id, tab.path);
  }

  _showContextMenu(x, y, entry) {
    const existing = this.querySelector('.context-menu');
    if (existing) existing.remove();

    const menu = document.createElement('div');
    menu.className = 'context-menu';
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';
    menu.innerHTML = `
      ${(entry.has_local)
        ? '<div class="context-menu-item" data-context="open-local">Open Locally</div>'
        : '<div class="context-menu-item" data-context="download">Download</div>'
      }
      <div class="context-menu-item" data-context="preview">Preview</div>
      <div class="context-menu-item" data-context="rename">Rename</div>
      <div class="context-menu-item context-menu-danger" data-context="delete">Delete</div>
    `;

    this.appendChild(menu);

    menu.querySelectorAll('.context-menu-item').forEach((item) => {
      item.addEventListener('click', () => {
        menu.remove();
        if (item.dataset.context === 'preview') {
          this._preview_entry = entry;
          this._preview_component = null;
          this._loadPreview();
        } else {
          this._preview_entry = entry;
          this._handlePreviewAction(item.dataset.context);
        }
      });
    });

    const closeMenu = (event) => {
      if (!menu.contains(event.target)) {
        menu.remove();
        document.removeEventListener('click', closeMenu);
      }
    };
    setTimeout(() => document.addEventListener('click', closeMenu), 0);
  }

  _truncate(str, max) {
    if (str.length <= max) return str;
    return str.substring(0, max - 1) + '\u2026';
  }
}

customElements.define('aeor-file-browser', AeorFileBrowser);

export { AeorFileBrowser };
