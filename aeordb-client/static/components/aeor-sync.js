'use strict';

import { escapeHtml, formatSize, bindResizeHandle } from './aeor-file-view-shared.js';
import { showRemoteFolderPicker } from './aeor-remote-folder-picker.js';
import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-confirm-button.js';
import '../aeor/components/aeor-checkbox.js';
import '../aeor/components/aeor-info-box.js';
import '../aeor/components/aeor-select.js';

const { div, h1, h2, h3, label, input, option, button, table, thead, tbody, tr, th, td, span, a, strong } = elements;
const aeorCheckbox = elements['aeor-checkbox'];
const aeorSelect   = elements['aeor-select'];
const ACTIVE_PROGRESS_MAX_AGE_MS = 5 * 60 * 1000;
const HEALTH_STALE_MS = 30 * 1000;

function isProgressLikeEvent(type) {
  return type === 'progress' || type === 'scan_heartbeat';
}

function formatLocalIsoTimestamp(timestamp) {
  const date = new Date(Number(timestamp));
  if (Number.isNaN(date.getTime())) return '';

  const pad = (value, width = 2) => String(value).padStart(width, '0');
  const offsetMinutes = -date.getTimezoneOffset();
  const offsetSign = offsetMinutes >= 0 ? '+' : '-';
  const absoluteOffset = Math.abs(offsetMinutes);
  const offsetHours = Math.floor(absoluteOffset / 60);
  const offsetRemainder = absoluteOffset % 60;

  return [
    date.getFullYear(),
    '-',
    pad(date.getMonth() + 1),
    '-',
    pad(date.getDate()),
    'T',
    pad(date.getHours()),
    ':',
    pad(date.getMinutes()),
    ':',
    pad(date.getSeconds()),
    '.',
    pad(date.getMilliseconds(), 3),
    offsetSign,
    pad(offsetHours),
    ':',
    pad(offsetRemainder),
  ].join('');
}

class AeorSync extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      relationships: [],
      connections:   [],
      showAddForm:   false,
      editingId:     null,
      selectedId:    null,
      activity:      [],
      syncProgress:  {},
      syncRunning:   {},
      syncExecuting: {},
      manualSyncing: {},
      connectionHealth: {},
    });

    this._eventSource = null;

    this._onAddToggle       = this._onAddToggle.bind(this);
    this._onGoConnections   = this._onGoConnections.bind(this);
    this._onFormSubmit      = this._onFormSubmit.bind(this);
    this._onFormCancel      = this._onFormCancel.bind(this);
    this._onBrowseLocal     = this._onBrowseLocal.bind(this);
    this._onBrowseRemote    = this._onBrowseRemote.bind(this);
    this._onActivityClose   = this._onActivityClose.bind(this);
    this._handleSyncActivity = this._handleSyncActivity.bind(this);
    this._handleConnectionHealth = this._handleConnectionHealth.bind(this);
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchData();
    this._connectEvents();
  }

  disconnectedCallback() {
    if (this._eventSource) {
      this._eventSource.close();
      this._eventSource = null;
    }
  }

  refresh() {
    this._fetchData();
  }

  _buildDOM() {
    this.textContent = '';

    let element = div.context(this)(
      // Page header
      div.class('page-header')(
        h1('Sync Relationships'),
        // Hidden while the add/edit form is open — the form's own Cancel
        // button is the single way out, so users don't see two Cancels
        // (one in the header, one in the form) competing for the same
        // action.
        button.id('add-btn')
          .class('primary')
          .hidden.bindState(
            (s) => s.showAddForm || !!s.editingId,
            ['showAddForm', 'editingId'],
          )
          .disabled.bindState(
            (s) => s.connections.length === 0,
            ['connections'],
          )
          .onClick(this._onAddToggle)(
            'Add Sync',
          ),
      ),

      // Page guide
      elements['aeor-info-box'].compact('').class('page-guide')(
        'A sync relationship pairs a folder on a connected database with a folder on this machine and keeps them in step. Pick a direction — ',
        strong('pull'),
        ' to download from the remote, ',
        strong('push'),
        ' to upload from local, or ',
        strong('bidirectional'),
        ' for both. Add a connection on the Connections page first if you haven’t already.',
      ),

      // Form container — rebuilt dynamically
      div.id('form-container')(),

      // Table / empty state container — rebuilt dynamically
      div.class('sync-list').id('table-container')(),

      // Activity panel
      div.class('sync-activity-panel').id('activity-panel')
        .hidden.bindState(
          (s) => !s.selectedId,
          ['selectedId'],
        )(
        div.class('preview-resize-handle')(),
        div.class('preview-header')(
          h3.class('preview-title').id('activity-title')(),
          div.class('preview-actions')(
            button.class('secondary small').onClick(this._onActivityClose)('\u2715'),
          ),
        ),
        div.class('activity-feed').id('activity-feed')(),
      ),
    ).build(document);

    this.appendChild(element);

    // Bind resize handle
    const resizeHandle = this.querySelector('.preview-resize-handle');
    const panel = this.querySelector('#activity-panel');
    if (resizeHandle && panel) {
      bindResizeHandle(resizeHandle, panel);
    }

    // Listen for state changes that require DOM rebuilds
    this._state.on('showAddForm', () => this._rebuildFormContainer());
    this._state.on('editingId', () => this._rebuildFormContainer());
    this._state.on('relationships', () => this._rebuildTableContainer());
    this._state.on('connections', () => {
      this._rebuildTableContainer();
      this._rebuildFormContainer();
    });
    this._state.on('selectedId', () => this._rebuildTableContainer());
    this._state.on('activity', () => this._rebuildActivityFeed());
    this._state.on('syncProgress', () => this._rebuildTableContainer());
    this._state.on('syncRunning', () => this._rebuildTableContainer());
    this._state.on('syncExecuting', () => this._rebuildTableContainer());
    this._state.on('manualSyncing', () => this._rebuildTableContainer());
    this._state.on('connectionHealth', () => this._rebuildTableContainer());
  }

  // ---------------------------------------------------------------------------
  // Form container — add or edit form, rebuilt on state change
  // ---------------------------------------------------------------------------

  _rebuildFormContainer() {
    const container = this.querySelector('#form-container');
    if (!container) return;
    container.textContent = '';

    const s = this._state;
    if (s.showAddForm) {
      container.appendChild(this._buildAddForm());
    } else if (s.editingId) {
      const form = this._buildEditForm();
      if (form) container.appendChild(form);
    }
  }

  _buildAddForm() {
    const connectionOptions = this._state.connections.map((c) =>
      option.value(c.id)(`${c.name} (${c.url})`)
    );

    return div.class('form-panel').context(this)(
      h2('New Sync Relationship'),
      div.class('form-row')(
        label('Name'),
        input.type('text').id('form-name').placeholder('My Documents')(),
      ),
      div.class('form-row')(
        label('Connection'),
        aeorSelect.id('form-connection').name('form-connection')(...connectionOptions),
      ),
      div.class('form-row')(
        label('Remote Path'),
        div.class('dir-row')(
          input.type('text').id('form-remote-path').class('flex-fill').placeholder('/docs/')(),
          button.class('secondary small').onClick(this._onBrowseRemote)('Browse'),
        ),
      ),
      div.class('form-row')(
        label('Local Path'),
        div.class('dir-row')(
          input.type('text').id('form-local-path').class('flex-fill').placeholder('/home/user/Documents')(),
          button.class('secondary small').onClick(this._onBrowseLocal)('Browse'),
        ),
      ),
      div.class('form-row')(
        label('Direction'),
        aeorSelect.id('form-direction').name('form-direction')(
          option.value('pull_only')('Pull Only'),
          option.value('push_only')('Push Only'),
          option.value('bidirectional')('Bidirectional'),
        ),
      ),
      div.class('form-row')(
        label('Filter (optional, comma-separated globs)'),
        input.type('text').id('form-filter').placeholder('*.pdf, !*.tmp')(),
      ),
      this._buildDeletePropagationFields(),
      div.class('form-actions')(
        button.class('primary').onClick(this._onFormSubmit)('Create'),
        button.class('secondary').onClick(this._onFormCancel)('Cancel'),
      ),
    ).build(document);
  }

  _buildDeletePropagationFields() {
    return div.class('form-row')(
      label('Delete Propagation'),
      aeorCheckbox.id('form-delete-local-to-remote')(
        'When a file is deleted locally, also delete it on the remote',
      ),
      aeorCheckbox.id('form-delete-remote-to-local')(
        'When a file is deleted on the remote, also delete it locally',
      ),
    );
  }

  _buildEditForm() {
    const relationship = this._state.relationships.find((r) => r.id === this._state.editingId);
    if (!relationship) return null;

    const directionValues = ['pull_only', 'push_only', 'bidirectional'];
    const directionLabels = ['Pull Only', 'Push Only', 'Bidirectional'];
    const directionOptions = directionValues.map((val, i) => {
      if (relationship.direction === val)
        return option.value(val).selected('selected')(directionLabels[i]);
      return option.value(val)(directionLabels[i]);
    });

    const el = div.class('form-panel').context(this)(
      h2('Edit Sync Relationship'),
      div.class('form-row')(
        label('Name'),
        input.type('text').id('form-name')(),
      ),
      div.class('form-row')(
        label('Remote Path'),
        div.class('dir-row')(
          input.type('text').id('form-remote-path').class('flex-fill')(),
          button.class('secondary small').onClick(this._onBrowseRemote)('Browse'),
        ),
      ),
      div.class('form-row')(
        label('Local Path'),
        div.class('dir-row')(
          input.type('text').id('form-local-path').class('flex-fill')(),
          button.class('secondary small').onClick(this._onBrowseLocal)('Browse'),
        ),
      ),
      div.class('form-row')(
        label('Direction'),
        aeorSelect.id('form-direction').name('form-direction')(...directionOptions),
      ),
      div.class('form-row')(
        label('Filter (optional, comma-separated globs)'),
        input.type('text').id('form-filter')(),
      ),
      this._buildDeletePropagationFields(),
      div.class('form-actions')(
        button.class('primary').onClick(this._onFormSubmit)('Save Changes'),
        button.class('secondary').onClick(this._onFormCancel)('Cancel'),
      ),
    ).build(document);

    // Populate values after build
    el.querySelector('#form-name').value = relationship.name || '';
    el.querySelector('#form-remote-path').value = relationship.remote_path;
    el.querySelector('#form-local-path').value = relationship.local_path;
    el.querySelector('#form-filter').value = relationship.filter || '';

    const delLocalToRemote = el.querySelector('#form-delete-local-to-remote');
    if (delLocalToRemote) delLocalToRemote.checked = !!(relationship.delete_propagation && relationship.delete_propagation.local_to_remote);

    const delRemoteToLocal = el.querySelector('#form-delete-remote-to-local');
    if (delRemoteToLocal) delRemoteToLocal.checked = !!(relationship.delete_propagation && relationship.delete_propagation.remote_to_local);

    return el;
  }

  // ---------------------------------------------------------------------------
  // Table container — relationship rows or empty state
  // ---------------------------------------------------------------------------

  _rebuildTableContainer() {
    const container = this.querySelector('#table-container');
    if (!container) return;
    container.textContent = '';

    const s = this._state;
    if (s.relationships.length === 0) {
      if (s.connections.length > 0) {
        container.appendChild(
          div.class('empty-state')('No sync relationships configured.').build(document)
        );
      } else {
        const emptyEl = div.class('empty-state').context(this)(
          'You must first add a ',
          a.href('#').onClick(this._onGoConnections)('Connection'),
          ' before you can set up a sync.',
        ).build(document);
        container.appendChild(emptyEl);
      }
      return;
    }

    const rows = s.relationships.map((rel) => {
      const isSelected = (rel.id === s.selectedId);
      const progress = s.syncProgress[rel.id];
      const isSyncExecuting = s.syncExecuting[rel.id] === true;
      const isManualSyncing = s.manualSyncing[rel.id] === true;
      const syncButtonState = this._syncButtonState(rel, isSyncExecuting, isManualSyncing);
      const isSyncing = syncButtonState.active;
      const progressPercent = Math.max(0, Math.min(100, progress?.progress_percent || 0));

      const row = tr.class(isSelected ? 'sync-row selected' : 'sync-row')(
        td.class('mono muted')(`${rel.id.substring(0, 8)}...`),
        td(escapeHtml(rel.name)),
        td(escapeHtml(rel.remote_path)),
        td(escapeHtml(rel.direction)),
        td(
          span.class(rel.enabled ? 'badge badge-fixed success' : 'badge badge-fixed warning')(
            rel.enabled ? 'enabled' : 'disabled',
          ),
        ),
        td.class('actions')(
          elements['aeor-confirm-button']
            .class('confirm-button-new confirm-button-progress sync-progress-button')
            .label(syncButtonState.label)
            .duration('0')
            .disabled(syncButtonState.disabled)
            .progress(isSyncing ? progressPercent : 0)
            .dataId(rel.id)(),
          button.class('secondary small btn-toggle')(rel.enabled ? 'Pause' : 'Resume'),
          button.class('secondary small')('Edit'),
          elements['aeor-confirm-button']
            .class('confirm-button-danger')
            .label('Delete')
            .confirmedText('Deleted!')
            .duration('1000')
            .dataId(rel.id)(),
        ),
      ).build(document);

      // Store relationship id on the row
      row.dataset.id = rel.id;

      // Row click — select to show activity
      row.addEventListener('click', (event) => {
        if (event.target.closest('button') || event.target.closest('aeor-confirm-button')) return;
        if (this._state.selectedId === rel.id) {
          this._state.selectedId = null;
        } else {
          this._state.selectedId = rel.id;
          this._fetchActivity(rel.id);
        }
      });

      // Button events
      const syncButton = row.querySelector('aeor-confirm-button.sync-progress-button');
      if (syncButton)
        syncButton.addEventListener('confirm', (e) => { e.stopPropagation(); this._triggerSync(rel.id); });

      const editButton = row.querySelector('button.secondary:not(.btn-toggle)');
      if (editButton)
        editButton.addEventListener('click', (e) => { e.stopPropagation(); this._state.editingId = rel.id; this._state.showAddForm = false; });

      const toggleButton = row.querySelector('button.btn-toggle');
      if (toggleButton)
        toggleButton.addEventListener('click', (e) => { e.stopPropagation(); this._toggleSync(rel.id, rel.enabled); });

      // Confirm-button fires 'confirm' after hold completes — delete directly
      const confirmBtn = row.querySelector('aeor-confirm-button.confirm-button-danger');
      if (confirmBtn) {
        confirmBtn.addEventListener('confirm', (e) => { e.stopPropagation(); this._deleteSync(rel.id); });
      }

      return row;
    });

    const tbl = table(
      thead(
        tr(
          th('ID'), th('Name'), th('Remote'), th('Direction'), th('Status'), th('Actions'),
        ),
      ),
      tbody(),
    ).build(document);

    const tbodyEl = tbl.querySelector('tbody');
    for (const row of rows) {
      tbodyEl.appendChild(row);
    }

    container.appendChild(tbl);
  }

  _syncButtonState(relationship, isSyncExecuting, isManualSyncing) {
    if (relationship?.enabled !== true) {
      return {
        label: 'Sync',
        disabled: true,
        active: false,
      };
    }

    const health = this._connectionHealthStatus(relationship);
    const executing = isSyncExecuting || isManualSyncing;

    if (health === 'down') {
      return {
        label: executing ? 'Waiting...' : 'Offline',
        disabled: true,
        active: false,
      };
    }

    if (health !== 'up') {
      return {
        label: 'Checking...',
        disabled: true,
        active: false,
      };
    }

    return {
      label: executing ? 'Syncing...' : 'Sync',
      disabled: executing,
      active: executing,
    };
  }

  _connectionHealthStatus(relationship) {
    if (!relationship?.remote_connection_id) return 'unknown';
    const snapshot = this._state.connectionHealth?.[relationship.remote_connection_id];
    if (!snapshot?.status) return 'unknown';
    if (snapshot.checked_at && Date.now() - Number(snapshot.checked_at) > HEALTH_STALE_MS) {
      return 'unknown';
    }
    return snapshot.status;
  }

  _relationshipConnectionHealthy(relationship) {
    return this._connectionHealthStatus(relationship) === 'up';
  }

  // ---------------------------------------------------------------------------
  // Activity feed — rebuilt when activity state changes
  // ---------------------------------------------------------------------------

  _rebuildActivityFeed() {
    const feed = this.querySelector('#activity-feed');
    if (!feed) return;
    feed.textContent = '';

    const activity = this._state.activity;
    if (activity.length === 0) {
      feed.appendChild(
        div.class('empty-state')('No sync activity recorded yet.').build(document)
      );
      return;
    }

    for (const event of activity) {
      const time = formatLocalIsoTimestamp(event.timestamp);
      const icon = this._eventIcon(event.event_type);
      const hasErrors = event.errors && event.errors.length > 0;

      const summary = this._activitySummary(event);
      let detail = escapeHtml(summary);
      if (!summary.includes(' · ')) {
        if (event.files_affected > 0) {
          detail += ` \u00B7 ${event.files_affected} files`;
        }
        if (event.bytes_transferred > 0) {
          detail += ` \u00B7 ${formatSize(event.bytes_transferred)}`;
        }
        if (event.duration_ms > 0) {
          detail += ` \u00B7 ${this._formatDuration(event.duration_ms)}`;
        }
      }

      const bodyChildren = [
        div.class('activity-summary')(),
      ];

      if (hasErrors) {
        const errorDivs = event.errors.map((e) =>
          div.class('activity-error')(escapeHtml(e))
        );
        bodyChildren.push(div.class('activity-errors')(...errorDivs));
      }

      const item = div.class(hasErrors ? 'activity-item activity-item-error' : 'activity-item')(
        div.class('activity-icon')(icon),
        div.class('activity-body')(...bodyChildren),
        div.class('activity-time')(time),
      ).build(document);

      // Set innerHTML for summary since detail may contain escaped HTML entities
      item.querySelector('.activity-summary').innerHTML = detail;

      feed.appendChild(item);
    }
  }

  // ---------------------------------------------------------------------------
  // Event handlers
  // ---------------------------------------------------------------------------

  _onAddToggle() {
    const s = this._state;
    s.showAddForm = !s.showAddForm && !s.editingId;
    s.editingId   = null;
  }

  _onGoConnections(event) {
    event.preventDefault();
    this.dispatchEvent(new CustomEvent('navigate', {
      detail:  { page: 'connections', autoAdd: true },
      bubbles: true,
    }));
  }

  _onFormSubmit() {
    if (this._state.editingId)
      this._submitEdit();
    else
      this._submitForm();
  }

  _onFormCancel() {
    this._state.showAddForm = false;
    this._state.editingId   = null;
  }

  async _onBrowseLocal() {
    try {
      const response = await fetch('/api/v1/pick-directory', { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      const result = await response.json();
      if (result.path) {
        const input = this.querySelector('#form-local-path');
        if (input) input.value = result.path;
      }
    } catch (error) {
      console.error('Directory picker failed:', error);
    }
  }

  async _onBrowseRemote() {
    const connectionSelect = this.querySelector('#form-connection');
    const connectionId = connectionSelect
      ? connectionSelect.value
      : (this._state.editingId
        ? this._state.relationships.find((r) => r.id === this._state.editingId)?.remote_connection_id
        : null);

    if (!connectionId) {
      window.aeorToast('Please select a connection first', 'warning');
      return;
    }

    const connection = this._state.connections.find((c) => c.id === connectionId);
    if (!connection) {
      window.aeorToast('Connection not found', 'error');
      return;
    }

    const path = await showRemoteFolderPicker(connection.id);
    if (path) {
      const input = this.querySelector('#form-remote-path');
      if (input) input.value = path;
    }
  }

  _onActivityClose() {
    this._state.selectedId = null;
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  _eventIcon(type) {
    switch (type) {
      case 'pull':      return '\u2B07';
      case 'push':      return '\u2B06';
      case 'full_sync': return '\u21C4';
      case 'progress':  return '\u21BB';
      case 'scan_heartbeat': return '\u21BB';
      case 'error':     return '\u26A0';
      default:          return '\u2022';
    }
  }

  _activitySummary(event) {
    const summary = event.summary || '';

    const progressMatch = summary.match(/^push progress: processed=(\d+), pushed=(\d+), skipped=(\d+), failed=(\d+), deleted=(\d+)$/);
    if (progressMatch) {
      const [, processed, pushed, skipped, failed, deleted] = progressMatch.map(Number);
      const parts = [`Uploading ${this._formatCount(processed)} entries`];
      if (Number.isFinite(event.progress_percent)) {
        parts[0] += ` (${Math.round(event.progress_percent)}%)`;
      }
      if (pushed > 0) {
        parts.push(`${this._formatCount(pushed)} committed`);
        if (event.bytes_transferred > 0) {
          parts.push(`totaling ${formatSize(event.bytes_transferred)}`);
        }
      }
      if (skipped > 0) parts.push(`${this._formatCount(skipped)} unchanged`);
      if (deleted > 0) parts.push(`${this._formatCount(deleted)} deleted`);
      if (failed > 0) parts.push(`${this._formatCount(failed)} failed`);
      return parts.join(' · ');
    }

    const startedMatch = summary.match(/^push started: scanning (\d+) local entries under (.+)$/);
    if (startedMatch) {
      return `Scanning ${this._formatCount(Number(startedMatch[1]))} local entries in ${startedMatch[2]}`;
    }

    const uploadingMatch = summary.match(/^uploading (.+): (\d+)\/(\d+) chunks$/);
    if (uploadingMatch) {
      return `Uploading ${uploadingMatch[1]}: ${this._formatCount(Number(uploadingMatch[2]))} of ${this._formatCount(Number(uploadingMatch[3]))} chunks`;
    }

    const zeroByteUploadMatch = summary.match(/^Uploaded (.+) \(0 B\)(.*)$/);
    if (zeroByteUploadMatch) {
      return `Committed ${zeroByteUploadMatch[1].replace(/file(s)?$/, 'remote update$1')} (0 B sent)${zeroByteUploadMatch[2]}`;
    }

    return summary;
  }

  _formatCount(value) {
    return new Intl.NumberFormat().format(value);
  }

  _formatDuration(durationMs) {
    if (durationMs < 1000) return `${durationMs}ms`;
    if (durationMs < 60000) return `${(durationMs / 1000).toFixed(1)}s`;
    const totalSeconds = Math.floor(durationMs / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    if (minutes < 60) return `${minutes}m ${String(seconds).padStart(2, '0')}s`;
    const hours = Math.floor(minutes / 60);
    return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
  }

  // ---------------------------------------------------------------------------
  // API methods
  // ---------------------------------------------------------------------------

  async _fetchData() {
    try {
      const [syncResponse, connectionsResponse, runnerStatusResponse, healthResponse] = await Promise.all([
        fetch('/api/v1/sync'),
        fetch('/api/v1/connections'),
        fetch('/api/v1/sync/runner/status'),
        fetch('/api/v1/health/connections'),
      ]);

      if (!syncResponse.ok) throw new Error(`Sync request failed: ${syncResponse.status}`);
      if (!connectionsResponse.ok) throw new Error(`Connections request failed: ${connectionsResponse.status}`);
      if (!runnerStatusResponse.ok) throw new Error(`Runner status request failed: ${runnerStatusResponse.status}`);
      if (!healthResponse.ok) throw new Error(`Health request failed: ${healthResponse.status}`);

      const [relationships, connections, runnerStatus, healthSnapshots] = await Promise.all([
        syncResponse.json(),
        connectionsResponse.json(),
        runnerStatusResponse.json(),
        healthResponse.json(),
      ]);

      const syncRunning = {};
      const syncExecuting = {};
      const connectionHealth = this._healthMapFromSnapshots(healthSnapshots);
      if (Array.isArray(runnerStatus)) {
        for (const status of runnerStatus) {
          if (status?.relationship_id) {
            syncRunning[status.relationship_id] = status.running === true;
            syncExecuting[status.relationship_id] = status.executing === true;
          }
          if (status?.remote_connection_id && status.connection_health) {
            connectionHealth[status.remote_connection_id] = {
              connection_id: status.remote_connection_id,
              status: status.connection_health,
              checked_at: status.connection_checked_at,
              message: status.connection_message,
            };
          }
        }
      }

      this._state.relationships = relationships;
      this._state.connections   = connections;
      this._state.syncRunning   = syncRunning;
      this._state.syncExecuting = syncExecuting;
      this._state.connectionHealth = connectionHealth;
      await this._hydrateSyncProgress(relationships, syncExecuting);
    } catch (error) {
      console.error('Failed to fetch data:', error);
    }
  }

  _healthMapFromSnapshots(snapshots) {
    const health = {};
    if (!Array.isArray(snapshots)) return health;
    for (const snapshot of snapshots) {
      if (!snapshot?.connection_id) continue;
      health[snapshot.connection_id] = snapshot;
    }
    return health;
  }

  async _hydrateSyncProgress(relationships, syncExecuting = this._state.syncExecuting) {
    if (!Array.isArray(relationships) || relationships.length === 0) {
      this._state.syncProgress = {};
      return;
    }

    const activeRelationships = relationships.filter((relationship) =>
      relationship?.enabled === true &&
      syncExecuting?.[relationship.id] === true &&
      this._relationshipConnectionHealthy(relationship)
    );

    if (activeRelationships.length === 0) {
      this._state.syncProgress = {};
      return;
    }

    const responses = await Promise.all(activeRelationships.map(async (relationship) => {
      try {
        const response = await fetch(`/api/v1/sync/${relationship.id}/activity`);
        if (!response.ok) return null;
        const events = await response.json();
        return {
          id: relationship.id,
          progress: this._progressFromActivityEvents(events),
        };
      } catch {
        return null;
      }
    }));

    if (!responses.some((item) => item !== null)) {
      return;
    }

    const nextProgress = {};
    for (const item of responses) {
      if (item && item.progress) {
        nextProgress[item.id] = item.progress;
      }
    }

    this._state.syncProgress = nextProgress;
  }

  _progressFromActivityEvents(events) {
    if (!Array.isArray(events) || events.length === 0) return null;

    const latest = [...events]
      .filter((event) => isProgressLikeEvent(event?.event_type))
      .sort((a, b) => (b?.timestamp || 0) - (a?.timestamp || 0))[0];
    if (!latest) return null;
    if (Date.now() - latest.timestamp > ACTIVE_PROGRESS_MAX_AGE_MS) return null;

    return {
      progress_percent: latest.progress_percent || 0,
      summary:          latest.summary || 'Syncing...',
    };
  }

  _connectEvents() {
    if (this._eventSource) return;

    this._eventSource = new EventSource('/api/v1/events');
    this._eventSource.addEventListener('sync_activity', this._handleSyncActivity);
    this._eventSource.addEventListener('connection_health', this._handleConnectionHealth);
  }

  _handleConnectionHealth(event) {
    let data;
    try {
      data = JSON.parse(event.data);
    } catch {
      return;
    }

    if (!data?.connection_id) return;
    this._state.connectionHealth = {
      ...(this._state.connectionHealth || {}),
      [data.connection_id]: data,
    };
  }

  _handleSyncActivity(event) {
    let data;
    try {
      data = JSON.parse(event.data);
    } catch {
      return;
    }

    const relationshipId = data.relationship_id;
    if (!relationshipId) return;

    const progress = { ...this._state.syncProgress };
    if (isProgressLikeEvent(data.event_type)) {
      const relationship = this._state.relationships.find((item) => item.id === relationshipId);
      const isKnownManualRun = this._state.manualSyncing[relationshipId] === true;
      const isContinuousRun = relationship?.enabled === true && this._state.syncRunning[relationshipId] === true;
      const isConnectionHealthy = this._relationshipConnectionHealthy(relationship);
      if ((!isKnownManualRun && !isContinuousRun) || !isConnectionHealthy) {
        delete progress[relationshipId];
        this._state.syncProgress = progress;
        return;
      }

      const age = Date.now() - data.timestamp;
      if (age <= ACTIVE_PROGRESS_MAX_AGE_MS) {
        progress[relationshipId] = {
          progress_percent: data.progress_percent || 0,
          summary:          data.summary || 'Syncing...',
        };
      } else {
        delete progress[relationshipId];
      }
      this._setRelationshipSyncExecuting(relationshipId, true);
    } else {
      delete progress[relationshipId];
      this._setRelationshipSyncExecuting(relationshipId, false);
      this._setManualSyncing(relationshipId, false);
    }
    this._state.syncProgress = progress;

    if (this._state.selectedId === relationshipId) {
      const current = Array.isArray(this._state.activity) ? this._state.activity : [];
      this._state.activity = this._mergeActivityEvent(data, current);
    }
  }

  _mergeActivityEvent(event, current) {
    const existing = Array.isArray(current) ? current : [];
    if (event.event_type === 'progress') {
      return [event, ...existing.filter((item) => item.event_type !== 'progress')].slice(0, 50);
    }
    if (event.event_type === 'scan_heartbeat') {
      return [event, ...existing].slice(0, 50);
    }

    return [event, ...existing.filter((item) => item.event_type !== 'progress')].slice(0, 50);
  }

  _setRelationshipSyncExecuting(relationshipId, value) {
    if (!relationshipId) return;
    const current = this._state.syncExecuting || {};
    if (current[relationshipId] === value) return;
    this._state.syncExecuting = {
      ...current,
      [relationshipId]: value,
    };
  }

  _setManualSyncing(relationshipId, value) {
    if (!relationshipId) return;
    const current = this._state.manualSyncing || {};
    if (current[relationshipId] === value) return;
    this._state.manualSyncing = {
      ...current,
      [relationshipId]: value,
    };
  }

  async _fetchActivity(id) {
    const relationship = this._state.relationships.find((r) => r.id === id);
    if (!relationship) return;

    const titleEl = this.querySelector('#activity-title');
    if (titleEl) titleEl.textContent = `${relationship.name} \u2014 Activity`;

    const feed = this.querySelector('#activity-feed');
    if (feed) feed.innerHTML = '<div class="loading">Loading activity...</div>';

    try {
      const response = await fetch(`/api/v1/sync/${id}/activity`);
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.activity = await response.json();
    } catch (error) {
      if (feed) feed.innerHTML = '<div class="empty-state">Failed to load activity.</div>';
    }
  }

  async _submitForm() {
    const name         = this.querySelector('#form-name').value;
    const connectionId = this.querySelector('#form-connection').value;
    const remotePath   = this.querySelector('#form-remote-path').value;
    const localPath    = this.querySelector('#form-local-path').value;
    const direction    = this.querySelector('#form-direction').value;
    const filter       = this.querySelector('#form-filter').value;
    const localToRemote = this.querySelector('#form-delete-local-to-remote')?.checked || false;
    const remoteToLocal = this.querySelector('#form-delete-remote-to-local')?.checked || false;

    if (!name || !connectionId || !remotePath || !localPath)
      return;

    try {
      const response = await fetch('/api/v1/sync', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          name,
          remote_connection_id: connectionId,
          remote_path:          remotePath,
          local_path:           localPath,
          direction,
          filter:               (filter) ? filter : null,
          delete_propagation: {
            local_to_remote: localToRemote,
            remote_to_local: remoteToLocal,
          },
        }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.showAddForm = false;
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to create sync: ${error.message}`, 'error');
    }
  }

  async _submitEdit() {
    const name       = this.querySelector('#form-name').value;
    const remotePath = this.querySelector('#form-remote-path').value;
    const localPath  = this.querySelector('#form-local-path').value;
    const direction  = this.querySelector('#form-direction').value;
    const filter     = this.querySelector('#form-filter').value;

    const localToRemote = this.querySelector('#form-delete-local-to-remote')?.checked || false;
    const remoteToLocal = this.querySelector('#form-delete-remote-to-local')?.checked || false;

    try {
      const response = await fetch(`/api/v1/sync/${this._state.editingId}`, {
        method:  'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          name:               (name) ? name : null,
          remote_path:        (remotePath) ? remotePath : null,
          local_path:         (localPath) ? localPath : null,
          direction,
          filter:             (filter) ? filter : null,
          delete_propagation: {
            local_to_remote: localToRemote,
            remote_to_local: remoteToLocal,
          },
        }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.editingId = null;
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to update sync: ${error.message}`, 'error');
    }
  }

  async _triggerSync(id) {
    const progress = { ...this._state.syncProgress };
    progress[id] = { progress_percent: 0, summary: 'Sync requested...' };
    this._state.syncProgress = progress;
    this._state.manualSyncing = {
      ...this._state.manualSyncing,
      [id]: true,
    };

    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      if (!response.ok) throw new Error(await this._responseErrorMessage(response));
      const result   = await response.json();
      if (result.already_running) {
        const progress = { ...this._state.syncProgress };
        delete progress[id];
        this._state.syncProgress = progress;
        const manualSyncing = { ...this._state.manualSyncing };
        delete manualSyncing[id];
        this._state.manualSyncing = manualSyncing;
        window.aeorToast(result.message || 'Sync already in progress', 'info');
        await this._fetchData();
        return;
      }
      const pull     = result.pull || {};
      const push     = result.push || {};
      const nextProgress = { ...this._state.syncProgress };
      delete nextProgress[id];
      this._state.syncProgress = nextProgress;
      const manualSyncing = { ...this._state.manualSyncing };
      delete manualSyncing[id];
      this._state.manualSyncing = manualSyncing;
      window.aeorToast(`Sync complete: ${pull.files_pulled || 0} pulled, ${push.files_pushed || 0} pushed`, 'success');
      if (this._state.selectedId === id) {
        this._fetchActivity(id);
      }
    } catch (error) {
      const nextProgress = { ...this._state.syncProgress };
      delete nextProgress[id];
      this._state.syncProgress = nextProgress;
      const manualSyncing = { ...this._state.manualSyncing };
      delete manualSyncing[id];
      this._state.manualSyncing = manualSyncing;
      window.aeorToast(`Sync failed: ${error.message}`, 'error', 10000);
    }
  }

  async _toggleSync(id, isEnabled) {
    const action = (isEnabled) ? 'disable' : 'enable';
    try {
      if (isEnabled) {
        const progress = { ...this._state.syncProgress };
        delete progress[id];
        this._state.syncProgress = progress;
        const manualSyncing = { ...this._state.manualSyncing };
        delete manualSyncing[id];
        this._state.manualSyncing = manualSyncing;

        this._state.syncRunning = {
          ...this._state.syncRunning,
          [id]: false,
        };
        this._state.syncExecuting = {
          ...this._state.syncExecuting,
          [id]: false,
        };
      }

      const response = await fetch(`/api/v1/sync/${id}/${action}`, { method: 'POST' });
      if (!response.ok) throw new Error(await this._responseErrorMessage(response));
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to ${action} sync: ${error.message}`, 'error');
    }
  }

  async _deleteSync(id) {
    const relationship = this._state.relationships.find((item) => item.id === id);
    const name = relationship?.name || id;

    try {
      const response = await fetch(`/api/v1/sync/${id}`, { method: 'DELETE' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._state.selectedId === id) {
        this._state.selectedId = null;
      }
      await this._fetchData();
      window.aeorToast?.(`Successfully deleted sync "${name}".`, 'success');
    } catch (error) {
      window.aeorToast(`Failed to delete sync: ${error.message}`, 'error');
    }
  }

  async _responseErrorMessage(response) {
    try {
      const body = await response.json();
      if (body?.error) return body.error;
      if (body?.message) return body.message;
    } catch {
      // Fall through to the generic response message.
    }
    return `Request failed: ${response.status}`;
  }
}

customElements.define('aeor-sync', AeorSync);

export { AeorSync };
