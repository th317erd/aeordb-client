'use strict';

import { AeorNav } from './aeor-nav.js';
import { AeorDashboard } from './aeor-dashboard.js';
import { AeorConnections } from './aeor-connections.js';
import { AeorSync } from './aeor-sync.js';
import { AeorConflicts } from './aeor-conflicts.js';

class AeorApp extends HTMLElement {
  constructor() {
    super();
    this._currentPage = 'dashboard';
    this._pageOptions = {};
  }

  connectedCallback() {
    this.render();
  }

  render() {
    this.innerHTML = `
      <aeor-nav active="${this._currentPage}"></aeor-nav>
      <div class="app-content">
        ${this._renderPage()}
      </div>
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

    // Pass autoAdd option to connections component if present
    if (this._currentPage === 'connections' && this._pageOptions.autoAdd) {
      const connectionsElement = this.querySelector('aeor-connections');
      if (connectionsElement)
        connectionsElement.openAddForm();

      this._pageOptions = {}; // Clear after use
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
      case 'conflicts':
        return '<aeor-conflicts></aeor-conflicts>';
      default:
        return '<aeor-dashboard></aeor-dashboard>';
    }
  }
}

customElements.define('aeor-app', AeorApp);

export { AeorApp };
