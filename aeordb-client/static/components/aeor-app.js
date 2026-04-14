'use strict';

import { AeorNav } from './aeor-nav.js';
import { AeorDashboard } from './aeor-dashboard.js';
import { AeorConnections } from './aeor-connections.js';
import { AeorSync } from './aeor-sync.js';
import { AeorConflicts } from './aeor-conflicts.js';

class AeorApp extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this._currentPage = 'dashboard';
  }

  connectedCallback() {
    this.render();
    this.shadowRoot.querySelector('aeor-nav')
      .addEventListener('navigate', (event) => {
        this._currentPage = event.detail.page;
        this.render();
      });
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host {
          display: flex;
          height: 100vh;
          width: 100%;
        }

        aeor-nav {
          width: 220px;
          flex-shrink: 0;
        }

        .content {
          flex: 1;
          overflow-y: auto;
          padding: 24px;
        }
      </style>

      <aeor-nav active="${this._currentPage}"></aeor-nav>
      <div class="content">
        ${this._renderPage()}
      </div>
    `;

    // Re-attach navigation listener after render
    this.shadowRoot.querySelector('aeor-nav')
      .addEventListener('navigate', (event) => {
        this._currentPage = event.detail.page;
        this.render();
      });
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
