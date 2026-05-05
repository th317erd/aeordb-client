'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import { formatSize, bindResizeHandle, showConfirm } from './aeor-file-view-shared.js';

const { div, span, button, table, thead, tbody, tr, th, td, h1, h3 } = elements;

class AeorConflicts extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      conflicts:    [],
      selectedPath: null,
      loading:      false,
    });

    this._handleRowClick = this._handleRowClick.bind(this);
    this._handleDismiss = this._handleDismiss.bind(this);
    this._handleResolve = this._handleResolve.bind(this);
    this._handleDismissAll = this._handleDismissAll.bind(this);
    this._handlePreviewAccept = this._handlePreviewAccept.bind(this);
    this._handlePreviewPickLoser = this._handlePreviewPickLoser.bind(this);
    this._handlePreviewClose = this._handlePreviewClose.bind(this);

    this._state.on('conflicts', () => this._rebuildTable());
    this._state.on('selectedPath', () => this._rebuildPreview());
  }

  connectedCallback() {
    this._isConnected = true;
    this._buildDOM();
    this._fetchConflicts();
  }

  disconnectedCallback() {
    this._isConnected = false;
  }

  refresh() {
    this._fetchConflicts();
  }

  _buildDOM() {
    this.textContent = '';

    let element = div.context(this)(
      div.class('page-header')(
        h1('Conflicts'),
        button.class('success small')
          .id('dismiss-all')
          .hidden.bindState(
            (state) => state.conflicts.length <= 1,
            ['conflicts'],
          )
          .onClick(this._handleDismissAll)(
            'Accept All Winners',
          ),
      ),
      div.class('conflicts-list')(),
      div.class('conflict-preview')
        .hidden.bindState(
          (state) => !state.selectedPath,
          ['selectedPath'],
        )(
          div.class('preview-resize-handle')(),
          div.class('preview-header')(
            h3.class('preview-title')(),
            div.class('preview-actions')(
              button.class('success small')
                .onClick(this._handlePreviewAccept)(
                  'Accept Winner',
                ),
              button.class('primary small')
                .onClick(this._handlePreviewPickLoser)(
                  'Pick Loser',
                ),
              button.class('secondary small conflict-close')
                .onClick(this._handlePreviewClose)(
                  '\u2715',
                ),
            ),
          ),
          div.class('conflict-detail')(),
        ),
    ).build(document);

    this.appendChild(element);

    // Bind resize handle
    let resizeHandle = this.querySelector('.preview-resize-handle');
    let panel = this.querySelector('.conflict-preview');
    if (resizeHandle && panel)
      bindResizeHandle(resizeHandle, panel);
  }

  _rebuildTable() {
    let listContainer = this.querySelector('.conflicts-list');
    if (!listContainer) return;

    listContainer.textContent = '';

    let conflicts = this._state.conflicts;

    if (conflicts.length === 0) {
      let empty = div.class('empty-state')(
        div.class('empty-icon')('\u2713'),
        'No conflicts. Everything is in sync.',
      ).build(document);
      listContainer.appendChild(empty);
      return;
    }

    let selectedPath = this._state.selectedPath;

    let tableEl = table()(
      thead()(
        tr()(
          th('File'),
          th('Winner'),
          th('Loser'),
          th('Detected'),
          th('Actions'),
        ),
      ),
      tbody()(
        ...conflicts.map((conflict) => {
          let winner = conflict.winner || {};
          let loser = conflict.loser || {};
          let isSelected = (conflict.path === selectedPath);

          return tr.class(isSelected ? 'conflict-row selected' : 'conflict-row')
            .data('path', conflict.path)
            .onClick(this._handleRowClick)(
              td()(
                div.class('conflict-path-name')(conflict.path),
                div.class('mono muted conflict-type-label')(
                  conflict.conflict_type || 'modify/modify',
                ),
              ),
              td()(
                span.class('conflict-winner-label')('Winner'),
                span.class('muted conflict-size-label')(formatSize(winner.size)),
              ),
              td()(
                span.class('conflict-loser-label')('Loser'),
                span.class('muted conflict-size-label')(formatSize(loser.size)),
              ),
              td.class('muted')(new Date(conflict.created_at).toLocaleString()),
              td.class('actions')(
                button.class('success small dismiss-btn')
                  .data('path', conflict.path)
                  .onClick(this._handleDismiss)(
                    'Accept',
                  ),
                button.class('primary small resolve-btn')
                  .data('path', conflict.path)
                  .data('pick', 'loser')
                  .onClick(this._handleResolve)(
                    'Pick Loser',
                  ),
              ),
            );
        }),
      ),
    ).build(document);

    listContainer.appendChild(tableEl);
  }

  _rebuildPreview() {
    let selectedPath = this._state.selectedPath;
    let titleEl = this.querySelector('.preview-title');
    let detailEl = this.querySelector('.conflict-detail');

    if (!titleEl || !detailEl) return;

    if (!selectedPath) {
      titleEl.textContent = '';
      detailEl.textContent = '';
      return;
    }

    let conflict = this._state.conflicts.find((c) => c.path === selectedPath);
    if (!conflict) return;

    let winner = conflict.winner || {};
    let loser = conflict.loser || {};

    titleEl.textContent = conflict.path;

    detailEl.textContent = '';

    let detail = div()(
      div.class('conflict-comparison')(
        this._buildVersionPanel('Winner', 'conflict-winner', winner),
        this._buildVersionPanel('Loser', 'conflict-loser', loser),
      ),
      div.class('conflict-info')(
        span.class('muted')(
          'Conflict type: ',
          elements.strong(conflict.conflict_type || 'modify/modify'),
        ),
        span.class('muted')(
          '\u00B7 Detected: ' + new Date(conflict.created_at).toLocaleString(),
        ),
      ),
    ).build(document);

    detailEl.appendChild(detail);
  }

  _buildVersionPanel(label, cssClass, version) {
    return div.class(`conflict-version ${cssClass}`)(
      div.class('conflict-version-label')(label),
      div.class('conflict-version-meta')(
        this._buildInfoRow('Hash', version.hash || '?', true),
        this._buildInfoRow('Size', formatSize(version.size), false),
        this._buildInfoRow('Content Type', version.content_type || 'Unknown', false),
        this._buildInfoRow('Node ID', version.node_id || '?', true),
        this._buildInfoRow('Version Clock', String(version.virtual_time || '?'), true),
      ),
    );
  }

  _buildInfoRow(label, value, isMono) {
    return div.class('info-row')(
      span.class('info-label')(label),
      span.class(isMono ? 'info-value mono' : 'info-value')(value),
    );
  }

  // -- Event handlers --

  _handleRowClick(event) {
    if (event.target.closest('button')) return;

    let row = event.target.closest('.conflict-row');
    if (!row) return;

    let path = row.dataset.path;

    if (this._state.selectedPath === path) {
      this._state.selectedPath = null;
    } else {
      this._state.selectedPath = path;
    }

    // Rebuild table to update selected class
    this._rebuildTable();
  }

  _handleDismiss(event) {
    event.stopPropagation();
    let path = event.target.closest('[data-path]').dataset.path;
    this._dismissConflict(path);
  }

  _handleResolve(event) {
    event.stopPropagation();
    let btn = event.target.closest('[data-path]');
    this._resolveConflict(btn.dataset.path, btn.dataset.pick);
  }

  _handlePreviewAccept() {
    if (this._state.selectedPath)
      this._dismissConflict(this._state.selectedPath);
  }

  _handlePreviewPickLoser() {
    if (this._state.selectedPath)
      this._resolveConflict(this._state.selectedPath, 'loser');
  }

  _handlePreviewClose() {
    this._state.selectedPath = null;
    this._rebuildTable();
  }

  async _handleDismissAll() {
    let conflicts = this._state.conflicts;
    let confirmed = await showConfirm(
      'Accept All Winners',
      `Accept all ${conflicts.length} auto-winner(s)? Losing versions remain in version history.`,
      { confirmText: 'Accept All' },
    );
    if (!confirmed) return;

    try {
      let response = await fetch('/api/v1/conflicts/dismiss-all', { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._state.selectedPath = null;
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to dismiss all conflicts: ${error.message}`, 'error');
    }
  }

  // -- API calls --

  async _fetchConflicts() {
    if (!this._isConnected) return;

    this._state.loading = true;
    try {
      let response = await fetch('/api/v1/conflicts');
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (!this._isConnected) return;
      this._state.conflicts = await response.json();
    } catch (error) {
      console.error('Failed to fetch conflicts:', error);
    } finally {
      this._state.loading = false;
    }
  }

  async _dismissConflict(path) {
    try {
      let response = await fetch('/api/v1/conflicts/dismiss', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._state.selectedPath === path)
        this._state.selectedPath = null;
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to dismiss conflict: ${error.message}`, 'error');
    }
  }

  async _resolveConflict(path, pick) {
    try {
      let response = await fetch('/api/v1/conflicts/resolve', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path, pick }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._state.selectedPath === path)
        this._state.selectedPath = null;
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to resolve conflict: ${error.message}`, 'error');
    }
  }
}

if (!customElements.get('aeor-conflicts'))
  customElements.define('aeor-conflicts', AeorConflicts);

export { AeorConflicts };
