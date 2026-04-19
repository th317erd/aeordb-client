'use strict';

import { escapeHtml } from './aeor-file-view-shared.js';

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

      <div id="sync-cards"></div>

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
          <button class="secondary small" id="open-config-dir" style="margin-left: 8px;">Open</button>
        </div>
        <div class="info-row">
          <span class="info-label">Data Directory</span>
          <span class="info-value mono" id="data-dir">-</span>
          <button class="secondary small" id="open-data-dir" style="margin-left: 8px;">Open</button>
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
      const [statusResponse, connectionsResponse, syncResponse, conflictsResponse, runnerResponse] = await Promise.all([
        fetch('/api/v1/status'),
        fetch('/api/v1/connections'),
        fetch('/api/v1/sync'),
        fetch('/api/v1/conflicts'),
        fetch('/api/v1/sync/runner/status'),
      ]);

      const status      = await statusResponse.json();
      const connections  = await connectionsResponse.json();
      const sync         = await syncResponse.json();
      const conflicts    = await conflictsResponse.json();
      const runnerStatus = await runnerResponse.json();

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

      this._renderSyncCards(sync, runnerStatus);
    } catch (error) {
      this._update('#status-value', 'error', 'card-value error');
    }
  }

  _renderSyncCards(relationships, runnerStatus) {
    const container = this.querySelector('#sync-cards');
    if (!container) return;

    if (relationships.length === 0) {
      container.innerHTML = '';
      return;
    }

    const directionLabel = (d) => {
      switch (d) {
        case 'pull_only':      return '\u2190 Pull';
        case 'push_only':      return 'Push \u2192';
        case 'bidirectional':  return '\u2194 Bidirectional';
        default:               return d;
      }
    };

    const cards = relationships.map((rel) => {
      const runner  = runnerStatus.find((r) => r.relationship_id === rel.id);
      const running = runner && runner.running;
      const dotClass = running ? 'synced' : (rel.enabled ? 'pending' : 'not-synced');
      const statusText = running ? 'Running' : (rel.enabled ? 'Stopped' : 'Disabled');

      return `
        <div class="sync-status-card">
          <div class="sync-status-header">
            <div class="sync-status-name">
              <span class="sync-badge ${dotClass}"></span>
              ${escapeHtml(rel.name)}
            </div>
            <div class="sync-status-actions">
              <button class="secondary small sync-now-btn" data-id="${rel.id}">Sync Now</button>
            </div>
          </div>
          <div class="sync-status-details">
            <span class="sync-status-detail">${directionLabel(rel.direction)}</span>
            <span class="sync-status-detail">${escapeHtml(rel.remote_path)}</span>
            <span class="sync-status-detail sync-status-state ${running ? 'success' : ''}">${statusText}</span>
          </div>
        </div>
      `;
    }).join('');

    container.innerHTML = `
      <h2>Sync Status</h2>
      <div class="sync-status-grid">${cards}</div>
    `;

    // Bind sync-now buttons
    container.querySelectorAll('.sync-now-btn').forEach((btn) => {
      btn.addEventListener('click', () => this._triggerSync(btn, btn.dataset.id));
    });
  }

  async _triggerSync(btn, id) {
    const originalText = btn.textContent;
    btn.textContent = 'Syncing...';
    btn.disabled = true;

    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      const result   = await response.json();
      const pull     = result.pull || {};
      const push     = result.push || {};

      // Show result briefly in the button
      const pulled = pull.files_pulled || 0;
      const pushed = push.files_pushed || 0;
      btn.textContent = `\u2713 ${pulled} pulled, ${pushed} pushed`;
      btn.className = 'success small sync-now-btn';

      setTimeout(() => {
        btn.textContent = originalText;
        btn.className = 'secondary small sync-now-btn';
        btn.disabled = false;
      }, 3000);
    } catch (error) {
      btn.textContent = 'Failed';
      btn.className = 'danger small sync-now-btn';

      setTimeout(() => {
        btn.textContent = originalText;
        btn.className = 'secondary small sync-now-btn';
        btn.disabled = false;
      }, 3000);
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
