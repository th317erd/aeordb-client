'use strict';

import { bindResizeHandle } from './aeor-file-view-shared.js';
import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-confirm-button.js';
import '../aeor/components/aeor-info-box.js';
import { AeorDashboard } from '../shared/components/aeor-dashboard.js';

// Register the shared dashboard under a distinct tag name so it does not
// conflict with the client's own <aeor-dashboard> component.
//
// The remote variant disables SSE: it talks to the engine through the
// local /proxy route, which buffers the entire upstream body before
// responding (axum's reqwest::Response::bytes()). SSE streams never end,
// so EventSource would hang. The base class already falls back to 15s
// polling when SSE fails, so we just trigger that path directly.
if (!customElements.get('aeor-remote-dashboard'))
  customElements.define('aeor-remote-dashboard', class extends AeorDashboard {
    connectSSE() {
      if (!this._interval) {
        this._interval = setInterval(() => this.fetchStats(), 15000);
      }
    }
  });

const { div, h1, h2, h3, label, input, button, table, thead, tbody, tr, th, td } = elements;

class AeorConnections extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      connections: [],
      showAddForm: false,
      selectedId: null,
    });

    this._toggleAddForm = this._toggleAddForm.bind(this);
    this._submitForm = this._submitForm.bind(this);
    this._cancelForm = this._cancelForm.bind(this);
    this._closePreview = this._closePreview.bind(this);
    this._onRowClick = this._onRowClick.bind(this);
    this._onTestClick = this._onTestClick.bind(this);
    this._onConfirmDelete = this._onConfirmDelete.bind(this);
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchConnections();
  }

  refresh() {
    this._fetchConnections();
  }

  openAddForm() {
    this._state.showAddForm = true;
  }

  _buildDOM() {
    this.textContent = '';

    let element = div.context(this)(
      // Page header
      div.class('page-header')(
        h1('Connections'),
        // Hidden while the form is open — the form's own Cancel button
        // is the single way out, so users don't see two Cancels (one in
        // the header, one in the form) competing for the same action.
        button.id('add-btn')
          .class('primary')
          .hidden.bindState(
            (state) => state.showAddForm,
            ['showAddForm'],
          )
          .onClick(this._toggleAddForm)(
            'Add Connection',
          ),
      ),

      // Page guide
      elements['aeor-info-box'].compact('').class('page-guide')(
        'Connections are pointers to remote AeorDB instances. Add one here, then head to the ',
        elements.strong('Sync'),
        ' page to choose which folders to mirror between that database and your local machine.',
      ),

      // Add form panel — hidden until toggled
      div.class('form-panel')
        .hidden.bindState(
          (state) => !state.showAddForm,
          ['showAddForm'],
        )(
          h2('New Connection'),
          div.class('form-row')(
            label('Name'),
            input.type('text').id('form-name').placeholder('My Server')(),
          ),
          div.class('form-row')(
            label('URL'),
            input.type('text').id('form-url').placeholder('http://localhost:6830')(),
          ),
          div.class('form-row')(
            label('API Key (optional)'),
            input.type('text').id('form-api-key').placeholder('aeor_...')(),
          ),
          div.class('form-row')(
            label('Share Domain (optional)'),
            input.type('text').id('form-share-url').placeholder('Defaults to connection URL')(),
          ),
          div.class('form-actions')(
            button.class('primary').id('form-submit').onClick(this._submitForm)('Create'),
            button.class('secondary').id('form-cancel').onClick(this._cancelForm)('Cancel'),
          ),
        ),

      // Connection list — table rebuilt reactively
      div.class('connections-list')(
        div.class('empty-state')
          .hidden.bindState(
            (state) => state.connections.length > 0,
            ['connections'],
          )('No connections configured. Add one to get started.'),
        table
          .hidden.bindState(
            (state) => state.connections.length === 0,
            ['connections'],
          )(
            thead(
              tr(th('ID'), th('Name'), th('URL'), th('Auth'), th('Actions')),
            ),
            tbody.class('connections-tbody')(),
          ),
      ),

      // Preview panel — hidden until a row is selected
      div.class('connection-preview')
        .hidden.bindState(
          (state) => !state.selectedId,
          ['selectedId'],
        )(
          div.class('preview-resize-handle')(),
          div.class('preview-header')(
            h3.class('preview-title')
              .textContent.bindState(
                (state) => {
                  if (!state.selectedId) return '';
                  let conn = state.connections.find((c) => c.id === state.selectedId);
                  return conn ? `${conn.name} \u2014 Dashboard` : '';
                },
                ['selectedId', 'connections'],
              )(),
            div.class('preview-actions')(
              button.class('secondary small preview-close')
                .onClick(this._closePreview)('\u2715'),
            ),
          ),
          div.class('preview-dashboard-container')(),
        ),
    ).build(document);

    this.appendChild(element);

    // Clear the `.invalid` marker as soon as the user starts correcting
    // a field, so the red ring disappears the instant the input registers.
    for (const id of ['form-name', 'form-url', 'form-api-key', 'form-share-url']) {
      const field = this.querySelector('#' + id);
      if (field) field.addEventListener('input', () => field.classList.remove('invalid'));
    }

    // Bind resize handle
    let resizeHandle = this.querySelector('.preview-resize-handle');
    let panel = this.querySelector('.connection-preview');
    if (resizeHandle && panel)
      bindResizeHandle(resizeHandle, panel, { minHeight: 200, maxRatio: 0.85 });

    // Listen for connection list changes — rebuild table rows
    this._state.on('connections', () => this._rebuildTableRows());

    // Listen for selectedId changes — manage dashboard element and row highlighting
    this._state.on('selectedId', (changedKeys, state) => {
      this._updateRowSelection(state.selectedId);
      this._updateDashboard(state.selectedId);
    });
  }

  _rebuildTableRows() {
    let tbodyEl = this.querySelector('.connections-tbody');
    if (!tbodyEl) return;

    tbodyEl.textContent = '';

    for (let connection of this._state.connections) {
      let isSelected = (connection.id === this._state.selectedId);

      let row = tr.class(isSelected ? 'connection-row selected' : 'connection-row')
        .dataId(connection.id)
        .onClick(this._onRowClick)(
          td.class('mono muted')(connection.id.substring(0, 8) + '...'),
          td(connection.name),
          td(connection.url),
          td(connection.auth_type),
          td.class('actions')(
            button.class('secondary small test-btn')
              .dataId(connection.id)
              .onClick(this._onTestClick)('Test'),
            elements['aeor-confirm-button']
              .class('confirm-button-danger')
              .label('Delete')
              .confirmedText('Deleted!')
              .duration('1000')
              .dataId(connection.id)(),
          ),
        ).build(document);

      let confirmBtn = row.querySelector('aeor-confirm-button');
      if (confirmBtn)
        confirmBtn.addEventListener('confirm', this._onConfirmDelete);

      tbodyEl.appendChild(row);
    }
  }

  _updateRowSelection(selectedId) {
    let prev = this.querySelector('.connection-row.selected');
    if (prev) prev.classList.remove('selected');

    if (selectedId) {
      let row = this.querySelector(`.connection-row[data-id="${selectedId}"]`);
      if (row) row.classList.add('selected');
    }
  }

  _updateDashboard(selectedId) {
    let container = this.querySelector('.preview-dashboard-container');
    if (!container) return;

    if (!selectedId) {
      // Remove dashboard so it stops polling/SSE
      let dashboard = container.querySelector('.connection-dashboard');
      if (dashboard) dashboard.remove();
      return;
    }

    let connection = this._state.connections.find((c) => c.id === selectedId);
    if (!connection) return;

    let dashboard = container.querySelector('.connection-dashboard');
    if (!dashboard) {
      dashboard = document.createElement('aeor-remote-dashboard');
      dashboard.className = 'connection-dashboard';
      container.appendChild(dashboard);
    }
    // Point the dashboard at the local proxy instead of the remote URL —
    // direct cross-origin fetches to the engine fail CORS preflight from
    // the Tauri webview. The proxy attaches the connection's JWT and
    // forwards to ${connection.url}/${path}.
    dashboard.setAttribute('base-url', `/api/v1/connections/${connection.id}/proxy`);
  }

  _toggleAddForm() {
    this._state.showAddForm = !this._state.showAddForm;
    if (this._state.showAddForm)
      this._state.selectedId = null;
  }

  _cancelForm() {
    this._state.showAddForm = false;
  }

  _closePreview() {
    this._state.selectedId = null;
  }

  _onRowClick(event) {
    if (event.target.closest('button')) return;
    let row = event.target.closest('.connection-row');
    if (!row) return;

    let id = row.dataset.id;
    this._state.selectedId = (this._state.selectedId === id) ? null : id;
  }

  _onTestClick(event) {
    event.stopPropagation();
    let id = event.target.closest('[data-id]').dataset.id;
    this._testConnection(id);
  }

  _onConfirmDelete(event) {
    event.stopPropagation();
    let id = event.target.closest('[data-id]').dataset.id;
    this._deleteConnection(id);
  }

  async _fetchConnections() {
    try {
      let response = await fetch('/api/v1/connections');
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.connections = await response.json();
    } catch (error) {
      console.error('Failed to fetch connections:', error);
    }
  }

  async _submitForm() {
    const nameEl     = this.querySelector('#form-name');
    const urlEl      = this.querySelector('#form-url');
    const apiKeyEl   = this.querySelector('#form-api-key');
    const shareUrlEl = this.querySelector('#form-share-url');

    if (!nameEl || !urlEl) return;

    const name     = nameEl.value.trim();
    const url      = urlEl.value.trim();
    const apiKey   = apiKeyEl ? apiKeyEl.value : '';
    const shareUrl = shareUrlEl ? shareUrlEl.value : '';

    // Validation: visibly mark empty required fields + tell the user
    // what's missing. The .invalid class is cleared on `input` (wired
    // when the form is built) so the marker disappears as soon as the
    // user starts typing.
    const missing = [];
    if (!name) { nameEl.classList.add('invalid'); missing.push('Name'); }
    if (!url)  { urlEl.classList.add('invalid'); missing.push('URL'); }
    if (missing.length) {
      const which = missing.length === 1 ? missing[0] : missing.join(' and ');
      window.aeorToast?.(`${which} ${missing.length === 1 ? 'is' : 'are'} required.`, 'error');
      (missing[0] === 'Name' ? nameEl : urlEl).focus();
      return;
    }

    try {
      const response = await fetch('/api/v1/connections', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          name,
          url,
          auth_type:      apiKey ? 'api_key' : 'none',
          api_key:        apiKey || null,
          share_base_url: shareUrl || null,
        }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.showAddForm = false;
      await this._fetchConnections();
    } catch (error) {
      // Optional chain: if aeor-toasts hasn't connected yet, don't throw a
      // secondary error that swallows the original one silently.
      window.aeorToast?.(`Failed to create connection: ${error.message}`, 'error');
    }
  }

  async _testConnection(id) {
    try {
      let response = await fetch(`/api/v1/connections/${id}/test`, { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      let result = await response.json();
      window.aeorToast(
        result.success ? `Connected! (${result.latency_ms}ms)` : `Failed: ${result.message}`,
        result.success ? 'success' : 'error',
      );
    } catch (error) {
      window.aeorToast(`Test failed: ${error.message}`, 'error');
    }
  }

  async _deleteConnection(id) {
    try {
      let response = await fetch(`/api/v1/connections/${id}`, { method: 'DELETE' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._state.selectedId === id)
        this._state.selectedId = null;
      await this._fetchConnections();
    } catch (error) {
      window.aeorToast(`Failed to delete connection: ${error.message}`, 'error');
    }
  }
}

customElements.define('aeor-connections', AeorConnections);

export { AeorConnections };
