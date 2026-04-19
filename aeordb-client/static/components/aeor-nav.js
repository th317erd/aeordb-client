'use strict';

class AeorNav extends HTMLElement {
  static get observedAttributes() {
    return ['active'];
  }

  constructor() {
    super();
    this._cachedVersion = null;
  }

  connectedCallback() {
    this.render();
  }

  attributeChangedCallback() {
    this.render();
  }

  get active() {
    return this.getAttribute('active') || 'dashboard';
  }

  render() {
    const active = this.active;

    this.innerHTML = `
      <div class="nav-logo">Aeor<span>DB</span> Client</div>

      <nav class="nav-items">
        <div class="nav-item ${(active === 'dashboard') ? 'active' : ''}" data-page="dashboard">
          <span class="nav-icon" style="color: var(--accent)">&#9632;</span>
          Dashboard
        </div>
        <div class="nav-item ${(active === 'connections') ? 'active' : ''}" data-page="connections">
          <span class="nav-icon" style="color: #58a6ff">&#8644;</span>
          Connections
        </div>
        <div class="nav-item ${(active === 'sync') ? 'active' : ''}" data-page="sync">
          <span class="nav-icon" style="color: var(--success)">&#8635;</span>
          Sync
        </div>
        <div class="nav-item ${(active === 'files') ? 'active' : ''}" data-page="files">
          <span class="nav-icon" style="color: #a78bfa">&#128193;</span>
          Files
        </div>
        <div class="nav-item ${(active === 'conflicts') ? 'active' : ''}" data-page="conflicts">
          <span class="nav-icon" style="color: var(--warning)">&#9888;</span>
          Conflicts
        </div>
        <div class="nav-item ${(active === 'settings') ? 'active' : ''}" data-page="settings">
          <span class="nav-icon" style="color: var(--text-secondary)">&#9881;</span>
          Settings
        </div>
      </nav>

      <div class="nav-version">v${this._version || '0.1.0'}</div>
    `;

    this.querySelectorAll('.nav-item').forEach((item) => {
      item.addEventListener('click', () => {
        this.dispatchEvent(new CustomEvent('navigate', {
          detail:  { page: item.dataset.page },
          bubbles: true,
        }));
      });
    });

    this._fetchVersion();
  }

  async _fetchVersion() {
    if (this._cachedVersion) return;

    try {
      const response = await fetch('/api/v1/status');
      if (!response.ok) return;

      const data           = await response.json();
      this._version        = data.version;
      this._cachedVersion  = data.version;
      const versionElement = this.querySelector('.nav-version');
      if (versionElement)
        versionElement.textContent = `v${data.version}`;
    } catch (error) {
      // Non-critical
    }
  }
}

customElements.define('aeor-nav', AeorNav);

export { AeorNav };
