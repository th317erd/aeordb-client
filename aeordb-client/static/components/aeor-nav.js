'use strict';

class AeorNav extends HTMLElement {
  static get observedAttributes() {
    return ['active'];
  }

  constructor() {
    super();
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
          <span class="nav-icon">&#9632;</span>
          Dashboard
        </div>
        <div class="nav-item ${(active === 'connections') ? 'active' : ''}" data-page="connections">
          <span class="nav-icon">&#8644;</span>
          Connections
        </div>
        <div class="nav-item ${(active === 'sync') ? 'active' : ''}" data-page="sync">
          <span class="nav-icon">&#8635;</span>
          Sync
        </div>
        <div class="nav-item ${(active === 'conflicts') ? 'active' : ''}" data-page="conflicts">
          <span class="nav-icon">&#9888;</span>
          Conflicts
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
    try {
      const response      = await fetch('/api/v1/status');
      const data          = await response.json();
      this._version       = data.version;
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
