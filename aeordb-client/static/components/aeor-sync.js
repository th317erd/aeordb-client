'use strict';

import { escapeHtml, formatSize, bindResizeHandle, formatRelativeTime } from './aeor-file-view-shared.js';
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
    });

    this._onAddToggle       = this._onAddToggle.bind(this);
    this._onGoConnections   = this._onGoConnections.bind(this);
    this._onFormSubmit      = this._onFormSubmit.bind(this);
    this._onFormCancel      = this._onFormCancel.bind(this);
    this._onBrowseLocal     = this._onBrowseLocal.bind(this);
    this._onBrowseRemote    = this._onBrowseRemote.bind(this);
    this._onActivityClose   = this._onActivityClose.bind(this);
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchData();
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
      div.class('form-actions')(
        button.class('primary').onClick(this._onFormSubmit)('Create'),
        button.class('secondary').onClick(this._onFormCancel)('Cancel'),
      ),
    ).build(document);
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
      div.class('form-row')(
        label('Delete Propagation'),
        aeorCheckbox.id('form-delete-local-to-remote')(
          'When a file is deleted locally, also delete it on the remote',
        ),
        aeorCheckbox.id('form-delete-remote-to-local')(
          'When a file is deleted on the remote, also delete it locally',
        ),
      ),
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
          button.class('success small')('Sync'),
          button.class('secondary small')('Edit'),
          button.class('secondary small btn-toggle')(rel.enabled ? 'Pause' : 'Resume'),
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
      const buttons = row.querySelectorAll('button');
      buttons[0].addEventListener('click', (e) => { e.stopPropagation(); this._triggerSync(rel.id); });
      buttons[1].addEventListener('click', (e) => { e.stopPropagation(); this._state.editingId = rel.id; this._state.showAddForm = false; });
      buttons[2].addEventListener('click', (e) => { e.stopPropagation(); this._toggleSync(rel.id, rel.enabled); });

      // Confirm-button fires 'confirm' after hold completes — delete directly
      const confirmBtn = row.querySelector('aeor-confirm-button');
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
      const time = formatRelativeTime(event.timestamp);
      const icon = this._eventIcon(event.event_type);
      const hasErrors = event.errors && event.errors.length > 0;

      let detail = escapeHtml(event.summary);
      if (event.files_affected > 0) {
        detail += ` \u00B7 ${event.files_affected} files`;
      }
      if (event.bytes_transferred > 0) {
        detail += ` \u00B7 ${formatSize(event.bytes_transferred)}`;
      }
      if (event.duration_ms > 0) {
        detail += ` \u00B7 ${event.duration_ms}ms`;
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
      case 'error':     return '\u26A0';
      default:          return '\u2022';
    }
  }

  // ---------------------------------------------------------------------------
  // API methods
  // ---------------------------------------------------------------------------

  async _fetchData() {
    try {
      const [syncResponse, connectionsResponse] = await Promise.all([
        fetch('/api/v1/sync'),
        fetch('/api/v1/connections'),
      ]);

      if (!syncResponse.ok) throw new Error(`Sync request failed: ${syncResponse.status}`);
      if (!connectionsResponse.ok) throw new Error(`Connections request failed: ${connectionsResponse.status}`);

      const [relationships, connections] = await Promise.all([
        syncResponse.json(),
        connectionsResponse.json(),
      ]);

      this._state.relationships = relationships;
      this._state.connections   = connections;
    } catch (error) {
      console.error('Failed to fetch data:', error);
    }
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
    try {
      const response = await fetch(`/api/v1/sync/${id}/trigger`, { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      const result   = await response.json();
      const pull     = result.pull || {};
      const push     = result.push || {};
      window.aeorToast(`Sync complete: ${pull.files_pulled || 0} pulled, ${push.files_pushed || 0} pushed`, 'success');
      if (this._state.selectedId === id) {
        this._fetchActivity(id);
      }
    } catch (error) {
      window.aeorToast(`Sync failed: ${error.message}`, 'error', 10000);
    }
  }

  async _toggleSync(id, isEnabled) {
    const action = (isEnabled) ? 'disable' : 'enable';
    try {
      const response = await fetch(`/api/v1/sync/${id}/${action}`, { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to ${action} sync: ${error.message}`, 'error');
    }
  }

  async _deleteSync(id) {
    try {
      const response = await fetch(`/api/v1/sync/${id}`, { method: 'DELETE' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._state.selectedId === id) {
        this._state.selectedId = null;
      }
      await this._fetchData();
    } catch (error) {
      window.aeorToast(`Failed to delete sync: ${error.message}`, 'error');
    }
  }
}

customElements.define('aeor-sync', AeorSync);

export { AeorSync };
