'use strict';

import {
  formatSize, formatDate, fileIcon,
  escapeHtml, escapeAttr, isImageFile, isVideoFile, isAudioFile, isTextFile,
  ENTRY_TYPE_DIR, directionArrow,
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
  } catch (error) {
    console.warn(`Preview component load failed for ${exact}:`, error);
  }

  // Tier 2: group component
  try {
    await import(`./previews/${grouped}.js`);
    if (customElements.get(grouped)) return grouped;
  } catch (error) {
    console.warn(`Preview component load failed for ${grouped}:`, error);
  }

  // Tier 3: default fallback
  try {
    await import('./previews/aeor-preview-default.js');
  } catch (error) {
    console.warn('Default preview component load failed:', error);
  }

  return 'aeor-preview-default';
}

class AeorFileBrowser extends HTMLElement {
  constructor() {
    super();
    this._tabs = [];
    this._activeTabId = null;
    this._relationships = [];
    this._tabCounter = 0;
    this._scrollListener = null;
  }

  _activeTab() {
    return this._tabs.find((t) => t.id === this._activeTabId) || null;
  }

  connectedCallback() {
    this._loadState();
    this.render();
    this._fetchRelationships();

    if (this._activeTabId && this._activeTab()) {
      this._fetchListing();
    }
  }

  _saveState() {
    try {
      const serializableTabs = this._tabs.map((tab) => ({
        id:                tab.id,
        relationshipId:   tab.relationshipId,
        relationshipName: tab.relationshipName,
        path:              tab.path,
        viewMode:         tab.viewMode,
        pageSize:         tab.pageSize,
        previewHeight:    tab.previewHeight,
      }));
      localStorage.setItem('aeordb-file-browser', JSON.stringify({
        tabs:          serializableTabs,
        active_tab_id: this._activeTabId,
        tab_counter:   this._tabCounter,
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
      this._activeTabId = state.active_tab_id || null;
      this._tabCounter   = state.tab_counter || 0;

      this._tabs = (state.tabs || []).map((tab) => ({
        id:                tab.id,
        relationshipId:   tab.relationshipId || tab.relationship_id,
        relationshipName: tab.relationshipName || tab.relationship_name,
        path:              tab.path,
        viewMode:         tab.viewMode || tab.view_mode || 'list',
        entries:           [],
        total:             null,
        loading:           false,
        loadingMore:      false,
        pageSize:         tab.pageSize || tab.page_size || 100,
        previewEntry:     null,
        previewComponent: null,
        previewHeight:    tab.previewHeight || null,
      }));
    } catch (error) {
      // start fresh
    }
  }

  // ---------------------------------------------------------------------------
  // Full render — rebuilds the entire DOM. Used for structural changes
  // (open/close tab, new tab button, relationship selector).
  // ---------------------------------------------------------------------------
  render() {
    let html = '<div class="page-header"><h1>Files</h1></div>';

    if (this._tabs.length > 0) {
      html += this._renderTabBar();
    }

    if (!this._activeTabId) {
      html += this._renderRelationshipSelector();
      this.innerHTML = html;
      this._bindShellEvents();
      return;
    }

    // Render all tab content containers — only the active one is visible
    for (const tab of this._tabs) {
      const isActive = (tab.id === this._activeTabId);
      html += `<div class="tab-content" id="tab-content-${tab.id}" style="${isActive ? '' : 'display:none'}">`;
      html += this._renderDirectoryViewFor(tab);
      html += '</div>';
    }

    this.innerHTML = html;
    this._bindShellEvents();
    this._bindTabContentEvents(this._activeTabId);
    this._hydratePreview();
  }

  // ---------------------------------------------------------------------------
  // Update only a single tab's content container — no structural DOM change.
  // ---------------------------------------------------------------------------
  _updateTabContent(tabId) {
    const container = this.querySelector(`#tab-content-${tabId}`);
    const tab = this._tabs.find((t) => t.id === tabId);
    if (!container || !tab) return;

    container.innerHTML = this._renderDirectoryViewFor(tab);
    this._bindTabContentEvents(tabId);

    if (tabId === this._activeTabId) {
      this._hydratePreview();
    }
  }

  _renderRelationshipSelector() {
    if (this._relationships.length === 0) {
      return '<div class="empty-state">No sync relationships configured. Set up a sync first.</div>';
    }

    const cards = this._relationships.map((rel) => {
      const remoteName = rel.remote_path.replace(/\/$/, '').split('/').pop() || rel.remote_path;
      const localName  = rel.local_path.split('/').pop() || rel.local_path;
      const arrow      = directionArrow(rel.direction);
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
      const isActive = (tab.id === this._activeTabId);
      const label    = this._truncate(`${tab.relationshipName} ${tab.path}`, 30);

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

  _renderDirectoryViewFor(tab) {
    const viewMode    = tab.viewMode || 'list';
    const breadcrumbs = this._renderBreadcrumbs(tab);
    const header = `
      <div class="page-header">
        ${breadcrumbs}
        <div style="display: flex; gap: 8px; align-items: center;">
          <div class="view-toggle">
            <button class="small ${(viewMode === 'list') ? 'primary' : 'secondary'}" data-view="list" title="List view">&#9776;</button>
            <button class="small ${(viewMode === 'grid') ? 'primary' : 'secondary'}" data-view="grid" title="Grid view">&#9638;</button>
          </div>
          <button class="primary small upload-button">Upload</button>
          <input type="file" class="upload-input" style="display:none" multiple>
        </div>
      </div>
    `;

    if (tab.loading) {
      return `${header}<div class="tab-listing"><div class="loading">Loading...</div></div>`;
    }

    if (tab.entries.length === 0) {
      return `${header}<div class="tab-listing"><div class="empty-state">This directory is empty.</div></div>`;
    }

    const countText = (tab.total != null)
      ? `Showing ${tab.entries.length} of ${tab.total}`
      : `${tab.entries.length} items`;
    const loadingMore = (tab.loadingMore)
      ? '<div class="scroll-loading">Loading more...</div>'
      : '';

    const listing = (viewMode === 'grid')
      ? this._renderGridViewFor(tab)
      : this._renderListViewFor(tab);

    return `${header}<div class="tab-listing">${listing}<div class="entry-count">${countText}</div>${loadingMore}</div>
      <div class="preview-panel" style="display:none; ${tab.previewHeight ? 'height:' + tab.previewHeight + 'px' : ''}">
        <div class="preview-resize-handle"></div>
        <div class="preview-header">
          <input type="text" class="preview-title" spellcheck="false">
          <div class="preview-actions"></div>
        </div>
        <div class="preview-content"></div>
        <div class="preview-meta"></div>
      </div>`;
  }

  _renderListViewFor(tab) {
    const rows = tab.entries.map((entry) => {
      const isDir     = (entry.entry_type === ENTRY_TYPE_DIR);
      const icon      = fileIcon(entry.entry_type);
      const size      = (isDir) ? '\u2014' : formatSize(entry.size);
      const created   = formatDate(entry.created_at);
      const modified  = formatDate(entry.updated_at);
      const [syncClass, syncTitle] = this._syncBadge(entry);

      return `
        <tr class="file-entry" data-name="${escapeAttr(entry.name)}" data-type="${entry.entry_type}" ${isDir ? '' : 'draggable="true"'}>
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

  _renderGridViewFor(tab) {
    const cards = tab.entries.map((entry) => {
      const isDir     = (entry.entry_type === ENTRY_TYPE_DIR);
      const icon      = fileIcon(entry.entry_type);
      const [syncClass, syncTitle] = this._syncBadge(entry);
      const size      = (isDir) ? 'Folder' : formatSize(entry.size);

      let thumbnail = `<div class="grid-card-icon">${icon}</div>`;

      if (!isDir && isImageFile(entry.name) && entry.has_local) {
        const encodedPath = encodeURIComponent(tab.path.replace(/\/$/, '') + '/' + entry.name);
        thumbnail = `<div class="grid-card-thumbnail"><img src="/api/v1/files/${tab.relationshipId}/${encodedPath}" alt="${escapeAttr(entry.name)}" loading="lazy"></div>`;
      }

      return `
        <div class="grid-card file-entry" data-name="${escapeAttr(entry.name)}" data-type="${entry.entry_type}" ${isDir ? '' : 'draggable="true"'}
          <span class="sync-badge ${syncClass}" title="${syncTitle}"></span>
          ${thumbnail}
          <div class="grid-card-name" title="${escapeAttr(entry.name)}">${escapeHtml(this._truncate(entry.name, 20))}</div>
          <div class="grid-card-meta">${size}</div>
        </div>
      `;
    }).join('');

    return `<div class="file-grid">${cards}</div>`;
  }

  _renderBreadcrumbs(tab) {
    const path = tab.path;
    const rootLabel = tab.relationshipName || 'Root';
    const segments = path.split('/').filter((s) => s.length > 0);
    let html = `<div class="breadcrumbs"><span class="breadcrumb-segment" data-path="/">${escapeHtml(rootLabel)}</span>`;

    let accumulated = '/';
    for (const segment of segments) {
      accumulated += segment + '/';
      html += `<span class="breadcrumb-separator">/</span><span class="breadcrumb-segment" data-path="${escapeAttr(accumulated)}">${escapeHtml(segment)}</span>`;
    }

    html += '</div>';
    return html;
  }

  // Update the persistent preview panel's contents in place — no DOM destruction.
  _showPreview(tab) {
    const container = this.querySelector(`#tab-content-${tab.id}`);
    if (!container) return;

    const panel = container.querySelector('.preview-panel');
    if (!panel) return;

    const entry = tab.previewEntry;
    const componentName = tab.previewComponent;

    if (!entry || !componentName) {
      panel.style.display = 'none';
      return;
    }

    // Update header — editable filename input
    const titleInput = panel.querySelector('.preview-title');
    titleInput.value = entry.name;
    titleInput.dataset.original = entry.name;

    // Update action buttons
    panel.querySelector('.preview-actions').innerHTML = `
      ${(entry.has_local)
        ? '<button class="primary small" data-action="open-local">Open Locally</button>'
        : ''
      }
      <button class="danger small" data-action="delete">Delete</button>
      <button class="secondary small" data-action="close-preview">\u2715</button>
    `;

    // Update preview component — only swap if the component type changed
    const contentEl = panel.querySelector('.preview-content');
    const existingPreview = contentEl.firstElementChild;
    if (!existingPreview || existingPreview.tagName.toLowerCase() !== componentName) {
      contentEl.innerHTML = `<${componentName}></${componentName}>`;
    }

    // Set attributes on the preview element
    const previewEl = contentEl.querySelector(componentName);
    if (previewEl) {
      const contentType = entry.content_type || 'application/octet-stream';
      const filePath = tab.path.replace(/\/$/, '') + '/' + entry.name;
      const fileUrl = `/api/v1/files/${tab.relationshipId}/${encodeURIComponent(filePath)}`;
      previewEl.setAttribute('src', fileUrl);
      previewEl.setAttribute('filename', entry.name);
      previewEl.setAttribute('size', entry.size || 0);
      previewEl.setAttribute('content-type', contentType);
      if (previewEl.load) previewEl.load();
    }

    // Update meta
    panel.querySelector('.preview-meta').textContent =
      `${formatSize(entry.size)} \u00B7 ${entry.content_type || 'Unknown type'} \u00B7 ${formatDate(entry.created_at)}`;

    // Bind action buttons
    panel.querySelectorAll('[data-action]').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._handlePreviewAction(button.dataset.action);
      });
    });

    // Bind rename on Enter or blur
    const self = this;
    titleInput.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        titleInput.blur();
      } else if (event.key === 'Escape') {
        titleInput.value = titleInput.dataset.original;
        titleInput.blur();
      }
    });
    titleInput.addEventListener('blur', () => {
      const newName = titleInput.value.trim();
      const oldName = titleInput.dataset.original;
      if (newName && newName !== oldName) {
        self._renamePreviewFile(newName);
      }
    });

    // Show it
    panel.style.display = '';
  }

  // ---------------------------------------------------------------------------
  // Event binding — split into shell (tab bar) and tab content (per-tab).
  // ---------------------------------------------------------------------------
  _bindShellEvents() {
    // Relationship cards
    this.querySelectorAll('.relationship-card').forEach((card) => {
      card.addEventListener('click', () => {
        this._openTab(card.dataset.id, card.dataset.name);
      });
    });

    // Tab clicks
    this.querySelectorAll('.tab-label').forEach((label) => {
      const tabEl = label.closest('.tab');
      label.addEventListener('click', () => {
        this._switchTab(tabEl.dataset.tabId);
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
        this._activeTabId = null;
        this.render();
      });
    }
  }

  _bindTabContentEvents(tabId) {
    const container = this.querySelector(`#tab-content-${tabId}`);
    if (!container) return;

    const tab = this._tabs.find((t) => t.id === tabId);
    if (!tab) return;

    // View toggle
    container.querySelectorAll('[data-view]').forEach((btn) => {
      btn.addEventListener('click', () => {
        tab.viewMode = btn.dataset.view;
        this._saveState();
        this._updateTabContent(tabId);
      });
    });

    // Breadcrumbs
    container.querySelectorAll('.breadcrumb-segment').forEach((segment) => {
      segment.addEventListener('click', () => {
        this._navigateTo(segment.dataset.path);
      });
    });

    // File entries (both list rows and grid cards)
    container.querySelectorAll('.file-entry').forEach((el) => {
      el.addEventListener('click', () => {
        const entryType = parseInt(el.dataset.type, 10);
        if (entryType === ENTRY_TYPE_DIR) {
          const newPath = tab.path.replace(/\/$/, '') + '/' + el.dataset.name + '/';
          this._navigateTo(newPath);
        } else {
          tab.previewEntry = tab.entries.find((e) => e.name === el.dataset.name) || null;
          tab.previewComponent = null;
          this._loadPreview();
        }
      });

      // Context menu
      el.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        const entryType = parseInt(el.dataset.type, 10);
        if (entryType === ENTRY_TYPE_DIR) return;

        const entry = tab.entries.find((e) => e.name === el.dataset.name);
        if (!entry) return;

        this._showContextMenu(event.clientX, event.clientY, entry);
      });
    });

    // Upload (button)
    const uploadButton = container.querySelector('.upload-button');
    const uploadInput = container.querySelector('.upload-input');
    if (uploadButton && uploadInput) {
      uploadButton.addEventListener('click', () => uploadInput.click());
      uploadInput.addEventListener('change', (event) => this._handleUpload(event));
    }

    // Drag-and-drop: drop files onto the listing to upload
    const listing = container.querySelector('.tab-listing');
    if (listing) {
      let dragCounter = 0; // track nested enter/leave events

      listing.addEventListener('dragover', (event) => {
        // Only accept external file drops, not internal reorders
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault();
          event.dataTransfer.dropEffect = 'copy';
        }
      });

      listing.addEventListener('dragenter', (event) => {
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault();
          dragCounter++;
          listing.classList.add('drop-active');
        }
      });

      listing.addEventListener('dragleave', () => {
        dragCounter--;
        if (dragCounter <= 0) {
          dragCounter = 0;
          listing.classList.remove('drop-active');
        }
      });

      listing.addEventListener('drop', (event) => {
        event.preventDefault();
        dragCounter = 0;
        listing.classList.remove('drop-active');

        if (event.dataTransfer.files.length > 0) {
          this._uploadFiles(event.dataTransfer.files);
        }
      });
    }

    // Drag-out: dragging file entries out for download
    container.querySelectorAll('.file-entry[draggable="true"]').forEach((el) => {
      el.addEventListener('dragstart', (event) => {
        const entryName = el.dataset.name;
        const filePath = tab.path.replace(/\/$/, '') + '/' + entryName;
        const fileUrl = `${window.location.origin}/api/v1/files/${tab.relationshipId}/${encodeURIComponent(filePath)}`;
        const entry = tab.entries.find((e) => e.name === entryName);
        const mime = (entry && entry.content_type) || 'application/octet-stream';

        // DownloadURL format: mime:filename:url (Chrome/WebKitGTK)
        event.dataTransfer.setData('DownloadURL', `${mime}:${entryName}:${fileUrl}`);
        event.dataTransfer.setData('text/uri-list', fileUrl);
        event.dataTransfer.setData('text/plain', fileUrl);
        event.dataTransfer.effectAllowed = 'copy';
      });
    });

    // Preview panel resize handle (persistent — bound once per tab)
    const resizeHandle = container.querySelector('.preview-resize-handle');
    const previewPanel = container.querySelector('.preview-panel');
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
          tab.previewHeight = newHeight;
        };

        const onMouseUp = () => {
          document.removeEventListener('mousemove', onMouseMove);
          document.removeEventListener('mouseup', onMouseUp);
          self._saveState();
        };

        document.addEventListener('mousemove', onMouseMove);
        document.addEventListener('mouseup', onMouseUp);
      });
    }
  }

  // ---------------------------------------------------------------------------
  // Tab lifecycle
  // ---------------------------------------------------------------------------
  _openTab(relationshipId, relationshipName) {
    this._tabCounter++;
    const tabId = 'tab-' + this._tabCounter;
    this._tabs.push({
      relationshipId:   relationshipId,
      relationshipName: relationshipName,
      path:              '/',
      id:                tabId,
      viewMode:         'list',
      entries:           [],
      total:             null,
      loading:           false,
      loadingMore:      false,
      pageSize:         100,
      previewEntry:     null,
      previewComponent: null,
      previewHeight:    null,
    });
    this._activeTabId = tabId;
    this._saveState();
    this.render();
    this._fetchListing();
  }

  _switchTab(tabId) {
    if (this._activeTabId === tabId) return;

    // Hide current tab content
    const currentContainer = this.querySelector(`#tab-content-${this._activeTabId}`);
    if (currentContainer) currentContainer.style.display = 'none';

    const currentTabEl = this.querySelector(`.tab[data-tab-id="${this._activeTabId}"]`);
    if (currentTabEl) currentTabEl.classList.remove('active');

    // Show new tab content
    this._activeTabId = tabId;

    const newContainer = this.querySelector(`#tab-content-${tabId}`);
    if (newContainer) newContainer.style.display = '';

    const newTabEl = this.querySelector(`.tab[data-tab-id="${tabId}"]`);
    if (newTabEl) newTabEl.classList.add('active');

    this._saveState();

    // Load data if this tab hasn't been fetched yet
    const tab = this._activeTab();
    if (tab && tab.entries.length === 0 && !tab.loading) {
      this._fetchListing();
    } else {
      this._hydratePreview();
      this._attachScrollListener();
    }
  }

  _closeTab(tabId) {
    // Remove the tab's DOM container
    const container = this.querySelector(`#tab-content-${tabId}`);
    if (container) container.remove();

    this._tabs = this._tabs.filter((t) => t.id !== tabId);

    if (this._activeTabId === tabId) {
      if (this._tabs.length > 0) {
        this._activeTabId = this._tabs[this._tabs.length - 1].id;
      } else {
        this._activeTabId = null;
      }
    }

    this._saveState();
    this.render();
  }

  _navigateTo(path) {
    const tab = this._activeTab();
    if (!tab) return;
    tab.path = path;
    tab.previewEntry = null;
    this._saveState();
    // Update tab bar label (breadcrumb changed)
    this._updateTabBarLabel(tab);
    this._fetchListing();
  }

  _updateTabBarLabel(tab) {
    const tabEl = this.querySelector(`.tab[data-tab-id="${tab.id}"] .tab-label`);
    if (tabEl) {
      tabEl.textContent = this._truncate(`${tab.relationshipName} ${tab.path}`, 30);
    }
  }

  // ---------------------------------------------------------------------------
  // Data fetching
  // ---------------------------------------------------------------------------
  async _fetchRelationships() {
    try {
      const response = await fetch('/api/v1/sync');
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._relationships = await response.json();
      // Only full-render if we're on the selector screen
      if (!this._activeTabId) this.render();
    } catch (error) {
      console.error('Failed to fetch relationships:', error);
    }
  }

  async _fetchListing() {
    const tab = this._activeTab();
    if (!tab) return;

    tab.entries = [];
    tab.total = null;
    tab.loadingMore = false;
    tab.loading = true;
    this._updateTabContent(tab.id);

    try {
      const encodedPath = (tab.path === '/') ? '' : encodeURIComponent(tab.path);
      const baseUrl = (encodedPath)
        ? `/api/v1/browse/${tab.relationshipId}/${encodedPath}`
        : `/api/v1/browse/${tab.relationshipId}`;
      const url = `${baseUrl}?limit=${tab.pageSize || 100}&offset=0`;
      const response = await fetch(url);
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      const data = await response.json();
      tab.entries = data.entries || [];
      tab.total = (data.total != null) ? data.total : tab.entries.length;
    } catch (error) {
      console.error('Failed to fetch listing:', error);
      tab.entries = [];
    }

    tab.loading = false;
    this._updateTabContent(tab.id);
    this._attachScrollListener();
  }

  async _fetchNextPage() {
    const tab = this._activeTab();
    if (!tab || tab.loadingMore) return;
    if (tab.entries.length >= (tab.total || 0)) return;

    tab.loadingMore = true;
    this._updateTabContent(tab.id);

    try {
      const encodedPath = (tab.path === '/') ? '' : encodeURIComponent(tab.path);
      const baseUrl = (encodedPath)
        ? `/api/v1/browse/${tab.relationshipId}/${encodedPath}`
        : `/api/v1/browse/${tab.relationshipId}`;
      const url = `${baseUrl}?limit=${tab.pageSize || 100}&offset=${tab.entries.length}`;
      const response = await fetch(url);
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      const data = await response.json();
      const newEntries = data.entries || [];
      for (const entry of newEntries) {
        tab.entries.push(entry);
      }
      tab.total = (data.total != null) ? data.total : tab.entries.length;
    } catch (error) {
      console.error('Failed to fetch next page:', error);
    }

    tab.loadingMore = false;
    this._updateTabContent(tab.id);
    this._attachScrollListener();
  }

  _attachScrollListener() {
    const activeContainer = this.querySelector(`#tab-content-${this._activeTabId}`);
    const listing = activeContainer && activeContainer.querySelector('.tab-listing');
    if (!listing) return;

    if (this._scrollListener && this._scrollListenerTarget) {
      this._scrollListenerTarget.removeEventListener('scroll', this._scrollListener);
    }

    this._scrollListenerTarget = listing;
    this._scrollListener = () => {
      const tab = this._activeTab();
      if (!tab || tab.loadingMore) return;
      if (tab.total == null) return;
      if (tab.entries.length >= tab.total) return;

      const scrollBottom = listing.scrollHeight - listing.scrollTop - listing.clientHeight;
      if (scrollBottom < 200) {
        this._fetchNextPage();
      }
    };

    listing.addEventListener('scroll', this._scrollListener);
  }

  // ---------------------------------------------------------------------------
  // Preview
  // ---------------------------------------------------------------------------
  async _loadPreview() {
    const tab = this._activeTab();
    if (!tab || !tab.previewEntry) return;

    const contentType = tab.previewEntry.content_type || 'application/octet-stream';
    tab.previewComponent = await loadPreviewComponent(contentType);
    this._showPreview(tab);
  }

  _hydratePreview() {
    const tab = this._activeTab();
    if (!tab) return;
    this._showPreview(tab);
  }

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------
  async _renamePreviewFile(newName) {
    const tab = this._activeTab();
    if (!tab || !tab.previewEntry) return;

    const oldName = tab.previewEntry.name;
    const fromPath = tab.path.replace(/\/$/, '') + '/' + oldName;
    const toPath = tab.path.replace(/\/$/, '') + '/' + newName;

    try {
      const response = await fetch(`/api/v1/files/${tab.relationshipId}/rename`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ from: fromPath, to: toPath }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      tab.previewEntry.name = newName;
      // Update the input's original value to the new name
      const container = this.querySelector(`#tab-content-${tab.id}`);
      const titleInput = container && container.querySelector('.preview-title');
      if (titleInput) titleInput.dataset.original = newName;
      this._fetchListing();
    } catch (error) {
      window.aeorToast('Rename failed: ' + error.message, 'error');
      // Revert the input
      const container = this.querySelector(`#tab-content-${tab.id}`);
      const titleInput = container && container.querySelector('.preview-title');
      if (titleInput) titleInput.value = oldName;
    }
  }

  async _handlePreviewAction(action) {
    const tab = this._activeTab();
    if (!tab || !tab.previewEntry) return;

    const entry = tab.previewEntry;
    const filePath = tab.path.replace(/\/$/, '') + '/' + entry.name;

    switch (action) {
      case 'open-local': {
        const openResponse = await fetch(`/api/v1/files/${tab.relationshipId}/open`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: filePath.replace(/^\//, '') }),
        });
        if (!openResponse.ok)
          window.aeorToast(`Failed to open file: ${openResponse.status}`, 'error');
        break;
      }

      case 'delete':
        if (!confirm(`Delete "${entry.name}"? This cannot be undone.`)) break;
        try {
          const encodedPath = encodeURIComponent(filePath);
          const deleteResponse = await fetch(`/api/v1/files/${tab.relationshipId}/${encodedPath}`, {
            method: 'DELETE',
          });
          if (!deleteResponse.ok) throw new Error(`Request failed: ${deleteResponse.status}`);
          tab.previewEntry = null;
          this._fetchListing();
        } catch (error) {
          window.aeorToast('Delete failed: ' + error.message, 'error');
        }
        break;

      case 'close-preview':
        tab.previewEntry = null;
        tab.previewComponent = null;
        this._showPreview(tab);
        break;
    }
  }

  async _handleUpload(event) {
    await this._uploadFiles(event.target.files);
    event.target.value = '';
  }

  async _uploadFiles(files) {
    const tab = this._activeTab();
    if (!tab || files.length === 0) return;

    let uploaded = 0;
    for (const file of files) {
      const filePath = tab.path.replace(/\/$/, '') + '/' + file.name;
      const encodedPath = encodeURIComponent(filePath);

      try {
        const arrayBuffer = await file.arrayBuffer();
        const response = await fetch(`/api/v1/files/${tab.relationshipId}/${encodedPath}`, {
          method: 'PUT',
          headers: { 'Content-Type': file.type || 'application/octet-stream' },
          body: arrayBuffer,
        });
        if (!response.ok) throw new Error(`Request failed: ${response.status}`);
        uploaded++;
      } catch (error) {
        window.aeorToast(`Upload failed for ${file.name}: ${error.message}`, 'error');
      }
    }

    if (uploaded > 0) {
      window.aeorToast(`Uploaded ${uploaded} file${uploaded > 1 ? 's' : ''}`, 'success');
    }

    this._fetchListing();
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
        : ''
      }
      <div class="context-menu-item" data-context="preview">Preview</div>
      <div class="context-menu-item context-menu-danger" data-context="delete">Delete</div>
    `;

    this.appendChild(menu);

    // Adjust position if menu overflows viewport
    const rect = menu.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      menu.style.left = (x - rect.width) + 'px';
    }
    if (rect.bottom > window.innerHeight) {
      menu.style.top = (y - rect.height) + 'px';
    }

    menu.querySelectorAll('.context-menu-item').forEach((item) => {
      item.addEventListener('click', () => {
        menu.remove();
        const activeTab = this._activeTab();
        if (item.dataset.context === 'preview') {
          if (activeTab) {
            activeTab.previewEntry = entry;
            activeTab.previewComponent = null;
          }
          this._loadPreview();
        } else {
          if (activeTab) activeTab.previewEntry = entry;
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

  // ---------------------------------------------------------------------------
  // Utilities
  // ---------------------------------------------------------------------------
  _syncBadge(entry) {
    switch (entry.sync_status) {
      case 'synced':       return ['synced', 'Synced'];
      case 'pending_pull': return ['pending', 'Pending pull'];
      case 'pending_push': return ['pending', 'Pending push'];
      case 'error':        return ['error', 'Sync error'];
      default:             return ['not-synced', 'Not synced'];
    }
  }

  _truncate(str, max) {
    if (str.length <= max) return str;
    return str.substring(0, max - 1) + '\u2026';
  }
}

customElements.define('aeor-file-browser', AeorFileBrowser);

export { AeorFileBrowser };
