'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';

// Side-effect imports — register custom elements
import './aeor-nav.js';
import './aeor-dashboard.js';
import './aeor-connections.js';
import './aeor-sync.js';
import './aeor-conflicts.js';
import './aeor-file-browser.js';
import './aeor-settings.js';
import './aeor-toasts.js';

const { div } = elements;

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

    this._state = new ReactiveState({
      currentPage: 'dashboard',
    });

    this._pageOptions = {};
    this._rendered = false;

    // Bind methods used as event handlers or callbacks
    this._handleNavigate = this._handleNavigate.bind(this);
    this._handleFileDragStart = this._handleFileDragStart.bind(this);
  }

  connectedCallback() {
    this._relationshipCache = {};
    this._cacheRelationships();

    if (!this._rendered) {
      this._buildDOM();
      this._rendered = true;
    }

    this._showPage(this._state.currentPage);
  }

  // Build the full DOM once — all pages created, only one visible.
  _buildDOM() {
    this.textContent = '';

    // Build children individually — aeor-app itself is the flex container,
    // so children must be direct descendants (no wrapper div).
    let pageElements = PAGES.map((page) => {
      let tag = PAGE_TAGS[page];
      return elements[tag].dataPage(page)();
    });

    let navEl = elements['aeor-nav'].active(this._state.currentPage)().build(document);
    let contentEl = div.class('app-content')(...pageElements).build(document);
    let toastsEl = elements['aeor-toasts']().build(document);

    this.appendChild(navEl);
    this.appendChild(contentEl);
    this.appendChild(toastsEl);

    // Store page element references for fast toggling
    // Scope to .app-content to avoid matching nav items which also have data-page
    let content = this.querySelector('.app-content');
    this._pageElements = {};
    for (let page of PAGES) {
      this._pageElements[page] = content.querySelector(`[data-page="${page}"]`);
    }

    // Set initial visibility
    this._showPage(this._state.currentPage);

    // Listen on `this` (the root element) — events bubble up from nav and all pages.
    // This survives child re-renders (e.g., aeor-nav rebuilding its DOM).
    this.addEventListener('navigate', this._handleNavigate);
    this.addEventListener('file-drag-start', this._handleFileDragStart);

    // React to page changes
    this._state.on('currentPage', (_changedKeys, state) => {
      this._showPage(state.currentPage);
    });
  }

  _handleNavigate(event) {
    this._navigateTo(event.detail.page, event.detail);
  }

  _navigateTo(page, options = {}) {
    if (!PAGES.includes(page)) return;

    this._state.currentPage = page;
    this._pageOptions = options;

    // Show page synchronously — reactive listener fires on microtask,
    // but autoAdd below needs the page visible immediately.
    this._showPage(page);

    // Handle page-specific options
    if (page === 'connections' && options.autoAdd) {
      let el = this.querySelector('aeor-connections');
      if (el) el.openAddForm();
      this._pageOptions = {};
    }
  }

  _showPage(activePage) {
    for (let page of PAGES) {
      let el = this._pageElements && this._pageElements[page];
      if (!el) continue;

      let isActive = (page === activePage);
      el.style.display = isActive ? '' : 'none';

      // Notify the page it became visible so it can refresh stale data
      if (isActive && typeof el.refresh === 'function') {
        el.refresh();
      }
    }

    // Update nav active state
    let nav = this.querySelector('aeor-nav');
    if (nav) {
      nav.setAttribute('active', activePage);
    }
  }

  _handleFileDragStart(event) {
    let detail = event.detail;
    let { event: dragEvent, adapter, paths } = detail;

    let relId = adapter && adapter.relationshipId;
    let relationship = relId && this._relationshipCache && this._relationshipCache[relId];
    if (!relationship || !relationship.local_path) return;

    let localBase = relationship.local_path.replace(/\/$/, '');
    let localUris = (paths || []).map((p) => {
      let relativePath = p.replace(/^\//, '');
      return `file://${encodeURI(`${localBase}/${relativePath}`)}`;
    });

    if (localUris.length > 0) {
      dragEvent.dataTransfer.setData('text/uri-list', localUris.join('\r\n'));
    }
  }

  async _cacheRelationships() {
    try {
      let response = await fetch('/api/v1/sync');
      if (!response.ok) return;
      let relationships = await response.json();
      this._relationshipCache = {};
      for (let rel of relationships) {
        this._relationshipCache[rel.id] = rel;
      }
    } catch (error) {
      // Non-critical
    }
  }
}

customElements.define('aeor-app', AeorApp);

export { AeorApp };
