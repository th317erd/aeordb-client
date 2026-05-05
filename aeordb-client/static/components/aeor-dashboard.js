'use strict';

import { escapeHtml, openFolder, directionLabel, formatUptime } from './aeor-file-view-shared.js';
import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';

const { div, span, button, h1, h2 } = elements;

class AeorDashboard extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      connectionsCount: '...',
      connectionsClass: 'card-value',
      syncCount: '...',
      syncClass: 'card-value',
      conflictsCount: '...',
      conflictsClass: 'card-value',
      statusValue: '...',
      statusClass: 'card-value success',
      version: '-',
      uptime: '-',
      clientId: '-',
      clientName: '-',
      configDir: '-',
      dataDir: '-',
    });

    this._isConnected = false;
    this._timeoutIds = [];
    this._syncCardsContainer = null;

    this._handleOpenConfigDir = this._handleOpenConfigDir.bind(this);
    this._handleOpenDataDir = this._handleOpenDataDir.bind(this);
    this._triggerSync = this._triggerSync.bind(this);
  }

  connectedCallback() {
    this._isConnected = true;
    this._buildDOM();
    this._fetchData();
  }

  refresh() {
    this._fetchData();
  }

  disconnectedCallback() {
    this._isConnected = false;
    if (this._timeoutIds) {
      this._timeoutIds.forEach(id => clearTimeout(id));
      this._timeoutIds = [];
    }
  }

  _buildDOM() {
    this.textContent = '';

    let root = div.context(this)(
      h1('Dashboard'),

      div.class('cards')(
        div.class('card')(
          div.class('card-label')('Connections'),
          div.class.bindState((s) => s.connectionsClass, ['connectionsClass'])
            .textContent.bindState((s) => String(s.connectionsCount), ['connectionsCount'])(),
        ),
        div.class('card')(
          div.class('card-label')('Sync Relationships'),
          div.class.bindState((s) => s.syncClass, ['syncClass'])
            .textContent.bindState((s) => String(s.syncCount), ['syncCount'])(),
        ),
        div.class('card')(
          div.class('card-label')('Conflicts'),
          div.class.bindState((s) => s.conflictsClass, ['conflictsClass'])
            .textContent.bindState((s) => String(s.conflictsCount), ['conflictsCount'])(),
        ),
        div.class('card')(
          div.class('card-label')('Status'),
          div.class.bindState((s) => s.statusClass, ['statusClass'])
            .textContent.bindState((s) => s.statusValue, ['statusValue'])(),
        ),
      ),

      div.id('sync-cards')(),

      div.class('info-section')(
        h2('System Info'),
        div.class('info-row')(
          span.class('info-label')('Version'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.version, ['version'])(),
        ),
        div.class('info-row')(
          span.class('info-label')('Uptime'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.uptime, ['uptime'])(),
        ),
        div.class('info-row')(
          span.class('info-label')('Client ID'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.clientId, ['clientId'])(),
        ),
        div.class('info-row')(
          span.class('info-label')('Client Name'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.clientName, ['clientName'])(),
        ),
        div.class('info-row')(
          span.class('info-label')('Config Directory'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.configDir, ['configDir'])(),
          button.class('secondary small ml-sm').onClick(this._handleOpenConfigDir)('Open'),
        ),
        div.class('info-row')(
          span.class('info-label')('Data Directory'),
          span.class('info-value mono')
            .textContent.bindState((s) => s.dataDir, ['dataDir'])(),
          button.class('secondary small ml-sm').onClick(this._handleOpenDataDir)('Open'),
        ),
      ),
    ).build(document);

    this.appendChild(root);
    this._syncCardsContainer = this.querySelector('#sync-cards');
  }

  _handleOpenConfigDir() {
    openFolder(this._state.configDir);
  }

  _handleOpenDataDir() {
    openFolder(this._state.dataDir);
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

      if (!statusResponse.ok) throw new Error(`Status request failed: ${statusResponse.status}`);
      if (!connectionsResponse.ok) throw new Error(`Connections request failed: ${connectionsResponse.status}`);
      if (!syncResponse.ok) throw new Error(`Sync request failed: ${syncResponse.status}`);
      if (!conflictsResponse.ok) throw new Error(`Conflicts request failed: ${conflictsResponse.status}`);
      if (!runnerResponse.ok) throw new Error(`Runner status request failed: ${runnerResponse.status}`);

      const status      = await statusResponse.json();
      const connections  = await connectionsResponse.json();
      const sync         = await syncResponse.json();
      const conflicts    = await conflictsResponse.json();
      const runnerStatus = await runnerResponse.json();

      this._state.connectionsCount = connections.length;
      this._state.syncCount = sync.length;
      this._state.conflictsCount = conflicts.length;
      this._state.conflictsClass = (conflicts.length > 0) ? 'card-value warning' : 'card-value';
      this._state.statusValue = status.status;
      this._state.statusClass = 'card-value success';
      this._state.version = status.version;
      this._state.uptime = formatUptime(status.uptime);
      this._state.clientId = status.client_id || '-';
      this._state.clientName = status.client_name || '-';
      this._state.configDir = status.config_dir || '-';
      this._state.dataDir = status.data_dir || '-';

      this._renderSyncCards(sync, runnerStatus);
    } catch (error) {
      this._state.statusValue = 'error';
      this._state.statusClass = 'card-value error';
    }
  }

  _renderSyncCards(relationships, runnerStatus) {
    const container = this._syncCardsContainer;
    if (!container) return;

    if (relationships.length === 0) {
      container.textContent = '';
      return;
    }

    // Direct DOM manipulation for complex list (per implementation guide)
    container.textContent = '';

    const heading = h2('Sync Status').build(document);
    container.appendChild(heading);

    const grid = div.class('sync-status-grid')().build(document);

    for (const rel of relationships) {
      const runner  = runnerStatus.find((r) => r.relationship_id === rel.id);
      const running = runner && runner.running;
      const dotClass = running ? 'synced' : (rel.enabled ? 'pending' : 'not-synced');
      const statusText = running ? 'Running' : (rel.enabled ? 'Stopped' : 'Disabled');

      const card = div.class('sync-status-card')(
        div.class('sync-status-header')(
          div.class('sync-status-name')(
            span.class('sync-badge ' + dotClass)(),
            escapeHtml(rel.name),
          ),
          div.class('sync-status-actions')(
            button.class('secondary small sync-now-btn')
              .onClick((e) => this._triggerSync(e.currentTarget, rel.id))('Sync Now'),
          ),
        ),
        div.class('sync-status-details')(
          span.class('sync-status-detail')(directionLabel(rel.direction)),
          span.class('sync-status-detail')(escapeHtml(rel.remote_path)),
          span.class('sync-status-detail sync-status-state' + (running ? ' success' : ''))(statusText),
        ),
      ).build(document);

      grid.appendChild(card);
    }

    container.appendChild(grid);
  }

  async _triggerSync(btn, id) {
    const originalText = btn.textContent;
    btn.textContent = 'Syncing...';
    btn.disabled = true;

    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      if (!response.ok) throw new Error(`Sync trigger failed: ${response.status}`);
      const result   = await response.json();
      const pull     = result.pull || {};
      const push     = result.push || {};

      const pulled = pull.files_pulled || 0;
      const pushed = push.files_pushed || 0;
      btn.textContent = `\u2713 ${pulled} pulled, ${pushed} pushed`;
      btn.className = 'success small sync-now-btn';

      this._timeoutIds.push(setTimeout(() => {
        if (!this._isConnected) return;
        btn.textContent = originalText;
        btn.className = 'secondary small sync-now-btn';
        btn.disabled = false;
      }, 3000));
    } catch (error) {
      if (!this._isConnected) return;
      btn.textContent = 'Failed';
      btn.className = 'danger small sync-now-btn';

      this._timeoutIds.push(setTimeout(() => {
        if (!this._isConnected) return;
        btn.textContent = originalText;
        btn.className = 'secondary small sync-now-btn';
        btn.disabled = false;
      }, 3000));
    }
  }
}

customElements.define('aeor-dashboard', AeorDashboard);

export { AeorDashboard };
