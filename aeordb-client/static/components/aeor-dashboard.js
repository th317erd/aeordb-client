'use strict';

class AeorDashboard extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
  }

  connectedCallback() {
    this.render();
    this._fetchData();
  }

  render() {
    this.shadowRoot.innerHTML = `
      <style>
        :host {
          display: block;
        }

        h1 {
          font-size: 24px;
          font-weight: 600;
          margin-bottom: 24px;
          color: var(--text-primary, #e6edf3);
        }

        .cards {
          display: grid;
          grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
          gap: 16px;
          margin-bottom: 32px;
        }

        .card {
          background-color: var(--bg-secondary, #161b22);
          border: 1px solid var(--border, #30363d);
          border-radius: 8px;
          padding: 20px;
        }

        .card-label {
          font-size: 12px;
          color: var(--text-secondary, #8b949e);
          text-transform: uppercase;
          letter-spacing: 0.05em;
          margin-bottom: 8px;
        }

        .card-value {
          font-size: 28px;
          font-weight: 700;
          color: var(--text-primary, #e6edf3);
        }

        .card-value.success { color: var(--success, #3fb950); }
        .card-value.warning { color: var(--warning, #d29922); }
        .card-value.error   { color: var(--error, #f85149); }

        .status-section {
          background-color: var(--bg-secondary, #161b22);
          border: 1px solid var(--border, #30363d);
          border-radius: 8px;
          padding: 20px;
        }

        .status-section h2 {
          font-size: 16px;
          font-weight: 600;
          margin-bottom: 12px;
          color: var(--text-primary, #e6edf3);
        }

        .status-row {
          display: flex;
          justify-content: space-between;
          padding: 6px 0;
          border-bottom: 1px solid var(--bg-tertiary, #21262d);
        }

        .status-row:last-child {
          border-bottom: none;
        }

        .status-label {
          color: var(--text-secondary, #8b949e);
        }

        .status-value {
          color: var(--text-primary, #e6edf3);
          font-family: var(--font-mono, monospace);
        }

        .loading {
          color: var(--text-muted, #484f58);
          font-style: italic;
        }
      </style>

      <h1>Dashboard</h1>

      <div class="cards">
        <div class="card">
          <div class="card-label">Connections</div>
          <div class="card-value" id="connections-count">
            <span class="loading">...</span>
          </div>
        </div>
        <div class="card">
          <div class="card-label">Sync Relationships</div>
          <div class="card-value" id="sync-count">
            <span class="loading">...</span>
          </div>
        </div>
        <div class="card">
          <div class="card-label">Conflicts</div>
          <div class="card-value" id="conflicts-count">
            <span class="loading">...</span>
          </div>
        </div>
        <div class="card">
          <div class="card-label">Status</div>
          <div class="card-value success" id="status-value">
            <span class="loading">...</span>
          </div>
        </div>
      </div>

      <div class="status-section">
        <h2>System Info</h2>
        <div class="status-row">
          <span class="status-label">Version</span>
          <span class="status-value" id="version">-</span>
        </div>
        <div class="status-row">
          <span class="status-label">Uptime</span>
          <span class="status-value" id="uptime">-</span>
        </div>
        <div class="status-row">
          <span class="status-label">Client ID</span>
          <span class="status-value" id="client-id">-</span>
        </div>
        <div class="status-row">
          <span class="status-label">Client Name</span>
          <span class="status-value" id="client-name">-</span>
        </div>
      </div>
    `;
  }

  async _fetchData() {
    try {
      const [statusResponse, connectionsResponse, syncResponse, conflictsResponse] = await Promise.all([
        fetch('/api/v1/status'),
        fetch('/api/v1/connections'),
        fetch('/api/v1/sync'),
        fetch('/api/v1/conflicts'),
      ]);

      const status      = await statusResponse.json();
      const connections  = await connectionsResponse.json();
      const sync         = await syncResponse.json();
      const conflicts    = await conflictsResponse.json();

      this._update('#connections-count', connections.length);
      this._update('#sync-count', sync.length);
      this._update('#conflicts-count', conflicts.length, (conflicts.length > 0) ? 'warning' : '');
      this._update('#status-value', status.status, 'success');
      this._update('#version', status.version);
      this._update('#uptime', this._formatUptime(status.uptime));
      this._update('#client-id', status.client_id || '-');
      this._update('#client-name', status.client_name || '-');
    } catch (error) {
      this._update('#status-value', 'error', 'error');
    }
  }

  _update(selector, value, extraClass) {
    const element = this.shadowRoot.querySelector(selector);
    if (!element)
      return;

    element.textContent = value;
    if (extraClass)
      element.className = `card-value ${extraClass}`;
  }

  _formatUptime(seconds) {
    if (seconds < 60)
      return `${seconds}s`;

    if (seconds < 3600)
      return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;

    const hours   = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${minutes}m`;
  }
}

customElements.define('aeor-dashboard', AeorDashboard);

export { AeorDashboard };
