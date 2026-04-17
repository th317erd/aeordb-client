'use strict';

class AeorDashboard extends HTMLElement {
  connectedCallback() {
    this.render();
    this._fetchData();
  }

  render() {
    this.innerHTML = `
      <h1>Dashboard</h1>

      <div class="cards">
        <div class="card">
          <div class="card-label">Connections</div>
          <div class="card-value" id="connections-count"><span class="loading">...</span></div>
        </div>
        <div class="card">
          <div class="card-label">Sync Relationships</div>
          <div class="card-value" id="sync-count"><span class="loading">...</span></div>
        </div>
        <div class="card">
          <div class="card-label">Conflicts</div>
          <div class="card-value" id="conflicts-count"><span class="loading">...</span></div>
        </div>
        <div class="card">
          <div class="card-label">Status</div>
          <div class="card-value success" id="status-value"><span class="loading">...</span></div>
        </div>
      </div>

      <div class="info-section">
        <h2>System Info</h2>
        <div class="info-row">
          <span class="info-label">Version</span>
          <span class="info-value mono" id="version">-</span>
        </div>
        <div class="info-row">
          <span class="info-label">Uptime</span>
          <span class="info-value mono" id="uptime">-</span>
        </div>
        <div class="info-row">
          <span class="info-label">Client ID</span>
          <span class="info-value mono" id="client-id">-</span>
        </div>
        <div class="info-row">
          <span class="info-label">Client Name</span>
          <span class="info-value mono" id="client-name">-</span>
        </div>
        <div class="info-row">
          <span class="info-label">Config Directory</span>
          <span class="info-value mono" id="config-dir">-</span>
          <button class="btn btn-small" id="open-config-dir" style="margin-left: 8px;">Open</button>
        </div>
        <div class="info-row">
          <span class="info-label">Data Directory</span>
          <span class="info-value mono" id="data-dir">-</span>
          <button class="btn btn-small" id="open-data-dir" style="margin-left: 8px;">Open</button>
        </div>
      </div>
    `;

    this.querySelector('#open-config-dir').addEventListener('click', () => {
      this._openFolder(this.querySelector('#config-dir').textContent);
    });

    this.querySelector('#open-data-dir').addEventListener('click', () => {
      this._openFolder(this.querySelector('#data-dir').textContent);
    });
  }

  async _fetchData() {
    try {
      const [statusResponse, connectionsResponse, syncResponse, conflictsResponse] = await Promise.all([
        fetch('/api/v1/status'),
        fetch('/api/v1/connections'),
        fetch('/api/v1/sync'),
        fetch('/api/v1/conflicts'),
      ]);

      const status     = await statusResponse.json();
      const connections = await connectionsResponse.json();
      const sync       = await syncResponse.json();
      const conflicts  = await conflictsResponse.json();

      this._update('#connections-count', connections.length);
      this._update('#sync-count', sync.length);
      this._update('#conflicts-count', conflicts.length, (conflicts.length > 0) ? 'card-value warning' : 'card-value');
      this._update('#status-value', status.status, 'card-value success');
      this._update('#version', status.version);
      this._update('#uptime', this._formatUptime(status.uptime));
      this._update('#client-id', status.client_id || '-');
      this._update('#client-name', status.client_name || '-');
      this._update('#config-dir', status.config_dir || '-');
      this._update('#data-dir', status.data_dir || '-');
    } catch (error) {
      this._update('#status-value', 'error', 'card-value error');
    }
  }

  _update(selector, value, className) {
    const element = this.querySelector(selector);
    if (!element)
      return;

    element.textContent = value;
    if (className)
      element.className = className;
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

  async _openFolder(path) {
    if (!path || path === '-')
      return;

    try {
      await fetch('/api/v1/open-folder', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path }),
      });
    } catch (error) {
      console.error('failed to open folder:', error);
    }
  }
}

customElements.define('aeor-dashboard', AeorDashboard);

export { AeorDashboard };
