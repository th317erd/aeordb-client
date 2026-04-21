'use strict';

import { AeorNav } from './aeor-nav.js';
import { AeorDashboard } from './aeor-dashboard.js';
import { AeorConnections } from './aeor-connections.js';
import { AeorSync } from './aeor-sync.js';
import { AeorConflicts } from './aeor-conflicts.js';
import { AeorFileBrowser } from './aeor-file-browser.js';
import { AeorSettings } from './aeor-settings.js';
import { AeorToasts } from './aeor-toasts.js';

class AeorApp extends HTMLElement {
  constructor() {
    super();
    this._currentPage = 'dashboard';
    this._pageOptions = {};
  }

  connectedCallback() {
    this._relationshipCache = {};
    this._cacheRelationships();
    this.render();
  }

  render() {
    this.innerHTML = `
      <aeor-nav active="${this._currentPage}"></aeor-nav>
      <div class="app-content">
        ${this._renderPage()}
      </div>
      <aeor-toasts></aeor-toasts>
    `;

    // Listen for navigation from nav bar
    this.querySelector('aeor-nav')
      .addEventListener('navigate', (event) => {
        this._currentPage = event.detail.page;
        this._pageOptions = event.detail;
        this.render();
      });

    // Listen for navigation from anywhere (e.g., sync page → connections)
    this.querySelector('.app-content')
      .addEventListener('navigate', (event) => {
        this._currentPage = event.detail.page;
        this._pageOptions = event.detail;
        this.render();
      });

    // Handle file drag-start from the file browser — resolve local paths
    this.querySelector('.app-content')
      .addEventListener('file-drag-start', (event) => {
        this._handleFileDragStart(event.detail);
      });

    // Pass autoAdd option to connections component if present
    if (this._currentPage === 'connections' && this._pageOptions.autoAdd) {
      const connectionsElement = this.querySelector('aeor-connections');
      if (connectionsElement)
        connectionsElement.openAddForm();

      this._pageOptions = {};
    }
  }

  _handleFileDragStart(detail) {
    const { event, relationshipId, path } = detail;

    // Use cached relationship data to resolve the local path synchronously.
    // dataTransfer is only writable during the synchronous dragstart handler.
    const relationship = this._relationshipCache && this._relationshipCache[relationshipId];
    if (!relationship || !relationship.local_path) return;

    const localBase = relationship.local_path.replace(/\/$/, '');
    const relativePath = path.replace(/^\//, '');
    const localPath = `${localBase}/${relativePath}`;
    const fileUri = `file://${encodeURI(localPath)}`;

    event.dataTransfer.setData('text/uri-list', fileUri);
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

  _renderPage() {
    switch (this._currentPage) {
      case 'dashboard':
        return '<aeor-dashboard></aeor-dashboard>';
      case 'connections':
        return '<aeor-connections></aeor-connections>';
      case 'sync':
        return '<aeor-sync></aeor-sync>';
      case 'files':
        return '<aeor-file-browser></aeor-file-browser>';
      case 'conflicts':
        return '<aeor-conflicts></aeor-conflicts>';
      case 'settings':
        return '<aeor-settings></aeor-settings>';
      default:
        return '<aeor-dashboard></aeor-dashboard>';
    }
  }
}

customElements.define('aeor-app', AeorApp);

export { AeorApp };
