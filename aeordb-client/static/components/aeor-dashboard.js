'use strict';

import { escapeHtml, openFolder, directionLabel, formatUptime } from './aeor-file-view-shared.js';
import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-confirm-button.js';

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
    // In-flight force-resync IDs. The dashboard re-renders the whole
    // sync-card grid on every refresh() (nav-away → nav-back), which
    // would orphan the "Syncing..." button and give the user a fresh
    // enabled "Force Sync" they could spam mid-sync. Tracking the IDs
    // here lets _renderSyncCards rehydrate the disabled state.
    this._inFlightSyncs = new Set();

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

      // Force-Sync hold-to-confirm. Matches Xenocept's confirm-button
      // pattern (.onConfirm builder, event.currentTarget for the btn).
      // Deliberately NO confirmedText: the post-confirm 5s auto-reset
      // is hardcoded inside aeor-confirm-button and a force-resync can
      // run for minutes — we manage label/disabled imperatively for
      // the lifetime of the fetch instead.
      const isInFlight = this._inFlightSyncs.has(rel.id);

      const card = div.class('sync-status-card')(
        div.class('sync-status-header')(
          div.class('sync-status-name')(
            span.class('sync-badge ' + dotClass)(),
            escapeHtml(rel.name),
          ),
          div.class('sync-status-actions')(
            elements['aeor-confirm-button']
              .class('confirm-button-new force-sync-btn')
              .label(isInFlight ? 'Syncing...' : 'Force Sync')
              .duration('1000')
              .disabled(isInFlight)
              .dataId(rel.id)
              .onConfirm((event) => this._triggerSync(event.currentTarget, rel.id))(),
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

  async _triggerSync(confirmBtn, id) {
    // Guard against double-fire \u2014 element-builder's onConfirm wires the
    // 'confirm' event, but a re-rendered card could end up with a fresh
    // button while the previous fetch is still in flight. The in-flight
    // set is the source of truth.
    if (this._inFlightSyncs.has(id)) return;
    this._inFlightSyncs.add(id);

    // The hold animation has already played. Use the host element's
    // exposed `label` / `disabled` setters (defined in
    // aeor-confirm-button.js) rather than poking attributes directly \u2014
    // setters keep _state and the attribute in lockstep, which the
    // reactive label binding depends on. Safe even after the post-
    // confirm internal _reset fires 300ms later (it reads this.label
    // and we'll have already set it to "Syncing...").
    if (confirmBtn && confirmBtn.isConnected) {
      confirmBtn.label    = 'Syncing...';
      confirmBtn.disabled = true;
    }

    try {
      const response = await fetch(`/api/v1/sync/${id}/force-resync`, { method: 'POST' });
      if (!response.ok) throw new Error(`Sync trigger failed: ${response.status}`);
      const result = await response.json();
      const pulled = (result.pull || {}).files_pulled || 0;
      const pushed = (result.push || {}).files_pushed || 0;

      if (window.aeorToast)
        window.aeorToast(`\u2713 Force sync complete \u2014 ${pulled} pulled, ${pushed} pushed`, 'success');
    } catch (error) {
      if (window.aeorToast)
        window.aeorToast(`Force sync failed: ${error.message || error}`, 'error', 10000);
    } finally {
      this._inFlightSyncs.delete(id);
      if (!this._isConnected) return;

      // The card may have been replaced during refresh() while the
      // fetch was running. Re-query by data-id rather than relying on
      // the closed-over button reference, which could be detached.
      const liveBtn = this.querySelector(`aeor-confirm-button.force-sync-btn[data-id="${id}"]`);
      if (liveBtn) {
        liveBtn.label    = 'Force Sync';
        liveBtn.disabled = false;
      }
    }
  }
}

customElements.define('aeor-dashboard', AeorDashboard);

export { AeorDashboard };
