'use strict';

import { AeorNav } from './aeor-nav.js';
import { AeorDashboard } from './aeor-dashboard.js';
import { AeorConnections } from './aeor-connections.js';
import { AeorSync } from './aeor-sync.js';
import { AeorConflicts } from './aeor-conflicts.js';
import { AeorFileBrowser } from './aeor-file-browser.js';
import { AeorSettings } from './aeor-settings.js';
import { AeorToasts } from './aeor-toasts.js';

const PAGES = ['dashboard', 'connections', 'sync', 'files', 'conflicts', 'settings'];

const PAGE_TAGS = {
  dashboard:   'aeor-dashboard',
  connections: 'aeor-connections',
  sync:        'aeor-sync',
  files:       'aeor-file-browser',
  conflicts:   'aeor-conflicts',
  settings:    'aeor-settings',
};

class AeorApp extends HTMLElement {
  constructor() {
    super();
    this._currentPage = 'dashboard';
    this._pageOptions = {};
    this._rendered = false;
  }

  connectedCallback() {
    this._relationshipCache = {};
    this._cacheRelationships();

    if (!this._rendered) {
      this._buildDOM();
      this._rendered = true;
    }

    this._showPage(this._currentPage);
  }

  // Build the full DOM once — all pages created, only one visible.
  _buildDOM() {
    // Create shell
    const pagesHTML = PAGES.map((page) => {
      const tag = PAGE_TAGS[page];
      const hidden = (page !== this._currentPage) ? 'style="display:none"' : '';
      return `<${tag} data-page="${page}" ${hidden}></${tag}>`;
    }).join('\n        ');

    this.innerHTML = `
      <aeor-nav active="${this._currentPage}"></aeor-nav>
      <div class="app-content">
        ${pagesHTML}
      </div>
      <aeor-toasts></aeor-toasts>
    `;

    // Listen on the root element — events bubble up from nav and all pages.
    // This survives child re-renders (e.g., aeor-nav rebuilding its DOM).
    this.addEventListener('navigate', (event) => {
      this._navigateTo(event.detail.page, event.detail);
    });

    this.addEventListener('file-drag-start', (event) => {
      this._handleFileDragStart(event.detail);
    });
  }

  _navigateTo(page, options = {}) {
    if (!PAGES.includes(page)) return;

    this._currentPage = page;
    this._pageOptions = options;
    this._showPage(page);

    // Handle page-specific options
    if (page === 'connections' && options.autoAdd) {
      const el = this.querySelector('aeor-connections');
      if (el) el.openAddForm();
      this._pageOptions = {};
    }
  }

  _showPage(activePage) {
    // Toggle page visibility
    for (const page of PAGES) {
      const el = this.querySelector(`[data-page="${page}"]`);
      if (el) {
        el.style.display = (page === activePage) ? '' : 'none';
      }
    }

    // Update nav active state
    const nav = this.querySelector('aeor-nav');
    if (nav) {
      nav.setAttribute('active', activePage);
    }
  }

  _handleFileDragStart(detail) {
    const { event, adapter, paths } = detail;

    const relId = adapter && adapter.relationshipId;
    const relationship = relId && this._relationshipCache && this._relationshipCache[relId];
    if (!relationship || !relationship.local_path) return;

    const localBase = relationship.local_path.replace(/\/$/, '');
    const localUris = (paths || []).map((p) => {
      const relativePath = p.replace(/^\//, '');
      return `file://${encodeURI(`${localBase}/${relativePath}`)}`;
    });

    if (localUris.length > 0) {
      event.dataTransfer.setData('text/uri-list', localUris.join('\r\n'));
    }
  }

  async _cacheRelationships() {
    try {
      const response = await fetch('/api/v1/sync');
      if (!response.ok) return;
      const relationships = await response.json();
      this._relationshipCache = {};
      for (const rel of relationships) {
        this._relationshipCache[rel.id] = rel;
      }
    } catch (error) {
      // Non-critical
    }
  }
}

customElements.define('aeor-app', AeorApp);

export { AeorApp };
