'use strict';

class AeorNav extends HTMLElement {
  static get observedAttributes() {
    return ['active'];
  }

  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
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

    this.shadowRoot.innerHTML = `
      <style>
        :host {
          display: flex;
          flex-direction: column;
          background-color: var(--bg-secondary, #161b22);
          border-right: 1px solid var(--border, #30363d);
          height: 100%;
        }

        .logo {
          padding: 20px 16px;
          font-size: 16px;
          font-weight: 700;
          color: var(--text-primary, #e6edf3);
          border-bottom: 1px solid var(--border, #30363d);
          letter-spacing: -0.02em;
        }

        .logo span {
          color: var(--accent, #58a6ff);
        }

        nav {
          flex: 1;
          padding: 8px;
        }

        .nav-item {
          display: flex;
          align-items: center;
          gap: 10px;
          padding: 8px 12px;
          border-radius: 6px;
          color: var(--text-secondary, #8b949e);
          cursor: pointer;
          font-size: 14px;
          transition: background-color 0.15s, color 0.15s;
          margin-bottom: 2px;
        }

        .nav-item:hover {
          background-color: var(--bg-tertiary, #21262d);
          color: var(--text-primary, #e6edf3);
        }

        .nav-item.active {
          background-color: var(--bg-tertiary, #21262d);
          color: var(--text-primary, #e6edf3);
        }

        .nav-icon {
          width: 16px;
          text-align: center;
          font-size: 14px;
        }

        .version {
          padding: 12px 16px;
          font-size: 11px;
          color: var(--text-muted, #484f58);
          border-top: 1px solid var(--border, #30363d);
        }
      </style>

      <div class="logo">Aeor<span>DB</span> Client</div>

      <nav>
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

      <div class="version">v${this._version || '0.1.0'}</div>
    `;

    // Add click handlers
    this.shadowRoot.querySelectorAll('.nav-item').forEach((item) => {
      item.addEventListener('click', () => {
        const page = item.dataset.page;
        this.dispatchEvent(new CustomEvent('navigate', {
          detail:  { page },
          bubbles: true,
        }));
      });
    });

    // Fetch version
    this._fetchVersion();
  }

  async _fetchVersion() {
    try {
      const response = await fetch('/api/v1/status');
      const data     = await response.json();
      this._version  = data.version;

      const versionElement = this.shadowRoot.querySelector('.version');
      if (versionElement)
        versionElement.textContent = `v${data.version}`;
    } catch (error) {
      // Ignore — version display is non-critical
    }
  }
}

customElements.define('aeor-nav', AeorNav);

export { AeorNav };
