'use strict';

import { escapeHtml, escapeAttr } from './aeor-file-view-shared.js';

class AeorConflicts extends HTMLElement {
  constructor() {
    super();
    this._conflicts = [];
    this._selectedPath = null;
  }

  connectedCallback() {
    this.render();
    this._fetchConflicts();
  }

  render() {
    this.innerHTML = `
      <div class="page-header">
        <h1>Conflicts</h1>
        ${(this._conflicts.length > 1)
          ? '<button class="success small" id="dismiss-all">Accept All Winners</button>'
          : ''
        }
      </div>

      <div class="conflicts-list">
        ${(this._conflicts.length === 0)
          ? '<div class="empty-state"><div class="empty-icon">&#10003;</div>No conflicts. Everything is in sync.</div>'
          : this._renderTable()
        }
      </div>

      <div class="conflict-preview" style="display:none">
        <div class="preview-resize-handle"></div>
        <div class="preview-header">
          <h3 class="preview-title"></h3>
          <div class="preview-actions">
            <button class="success small" data-action="accept">Accept Winner</button>
            <button class="primary small" data-action="pick-loser">Pick Loser</button>
            <button class="secondary small conflict-close">\u2715</button>
          </div>
        </div>
        <div class="conflict-detail"></div>
      </div>
    `;

    this._bindEvents();

    if (this._selectedPath) {
      this._showConflictPreview(this._selectedPath);
    }
  }

  _renderTable() {
    const rows = this._conflicts.map((conflict) => {
      const winner = conflict.winner || {};
      const loser  = conflict.loser || {};
      const isSelected = (conflict.path === this._selectedPath);
      const sizeDiff = (winner.size != null && loser.size != null)
        ? this._formatSize(Math.abs(winner.size - loser.size))
        : '';

      return `
        <tr class="conflict-row ${isSelected ? 'selected' : ''}" data-path="${escapeAttr(conflict.path)}">
          <td>
            <div style="font-weight: 500;">${escapeHtml(conflict.path)}</div>
            <div class="mono muted" style="margin-top: 4px; font-size: 11px;">
              ${escapeHtml(conflict.conflict_type || 'modify/modify')}
            </div>
          </td>
          <td>
            <span style="color: var(--success); font-weight: 500;">Winner</span>
            <span class="muted" style="font-size: 12px;">${this._formatSize(winner.size)}</span>
          </td>
          <td>
            <span style="color: var(--text-secondary);">Loser</span>
            <span class="muted" style="font-size: 12px;">${this._formatSize(loser.size)}</span>
          </td>
          <td class="muted">${new Date(conflict.created_at).toLocaleString()}</td>
          <td class="actions">
            <button class="success small dismiss-btn" data-path="${escapeAttr(conflict.path)}">Accept</button>
            <button class="primary small resolve-btn" data-path="${escapeAttr(conflict.path)}" data-pick="loser">Pick Loser</button>
          </td>
        </tr>
      `;
    }).join('');

    return `
      <table>
        <thead>
          <tr><th>File</th><th>Winner</th><th>Loser</th><th>Detected</th><th>Actions</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;
  }

  _bindEvents() {
    // Row click — show detail preview
    this.querySelectorAll('.conflict-row').forEach((row) => {
      row.addEventListener('click', (event) => {
        if (event.target.closest('button')) return;
        const path = row.dataset.path;
        if (this._selectedPath === path) {
          this._selectedPath = null;
          this._hidePreview();
          row.classList.remove('selected');
        } else {
          const prev = this.querySelector('.conflict-row.selected');
          if (prev) prev.classList.remove('selected');
          this._selectedPath = path;
          row.classList.add('selected');
          this._showConflictPreview(path);
        }
      });
    });

    this.querySelectorAll('.dismiss-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._dismissConflict(button.dataset.path);
      });
    });

    this.querySelectorAll('.resolve-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        this._resolveConflict(button.dataset.path, button.dataset.pick);
      });
    });

    const dismissAllButton = this.querySelector('#dismiss-all');
    if (dismissAllButton)
      dismissAllButton.addEventListener('click', () => this._dismissAll());

    // Preview panel events
    const closeBtn = this.querySelector('.conflict-close');
    if (closeBtn) {
      closeBtn.addEventListener('click', () => {
        this._selectedPath = null;
        this._hidePreview();
        const prev = this.querySelector('.conflict-row.selected');
        if (prev) prev.classList.remove('selected');
      });
    }

    const acceptBtn = this.querySelector('[data-action="accept"]');
    if (acceptBtn) {
      acceptBtn.addEventListener('click', () => {
        if (this._selectedPath) this._dismissConflict(this._selectedPath);
      });
    }

    const pickLoserBtn = this.querySelector('[data-action="pick-loser"]');
    if (pickLoserBtn) {
      pickLoserBtn.addEventListener('click', () => {
        if (this._selectedPath) this._resolveConflict(this._selectedPath, 'loser');
      });
    }

    // Resize handle
    const resizeHandle = this.querySelector('.conflict-preview .preview-resize-handle');
    const panel = this.querySelector('.conflict-preview');
    if (resizeHandle && panel) {
      resizeHandle.addEventListener('mousedown', (event) => {
        event.preventDefault();
        const startY = event.clientY;
        const startHeight = panel.offsetHeight;

        const onMouseMove = (moveEvent) => {
          const delta = startY - moveEvent.clientY;
          const newHeight = Math.max(150, Math.min(window.innerHeight * 0.8, startHeight + delta));
          panel.style.height = newHeight + 'px';
        };

        const onMouseUp = () => {
          document.removeEventListener('mousemove', onMouseMove);
          document.removeEventListener('mouseup', onMouseUp);
        };

        document.addEventListener('mousemove', onMouseMove);
        document.addEventListener('mouseup', onMouseUp);
      });
    }
  }

  _showConflictPreview(path) {
    const conflict = this._conflicts.find((c) => c.path === path);
    if (!conflict) return;

    const panel = this.querySelector('.conflict-preview');
    if (!panel) return;

    const winner = conflict.winner || {};
    const loser = conflict.loser || {};

    panel.querySelector('.preview-title').textContent = conflict.path;

    const detail = panel.querySelector('.conflict-detail');
    detail.innerHTML = `
      <div class="conflict-comparison">
        <div class="conflict-version conflict-winner">
          <div class="conflict-version-label">Winner</div>
          <div class="conflict-version-meta">
            <div class="info-row">
              <span class="info-label">Hash</span>
              <span class="info-value mono">${escapeHtml(winner.hash || '?')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Size</span>
              <span class="info-value">${this._formatSize(winner.size)}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Content Type</span>
              <span class="info-value">${escapeHtml(winner.content_type || 'Unknown')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Node ID</span>
              <span class="info-value mono">${escapeHtml(winner.node_id || '?')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Version Clock</span>
              <span class="info-value mono">${escapeHtml(String(winner.virtual_time || '?'))}</span>
            </div>
          </div>
        </div>
        <div class="conflict-version conflict-loser">
          <div class="conflict-version-label">Loser</div>
          <div class="conflict-version-meta">
            <div class="info-row">
              <span class="info-label">Hash</span>
              <span class="info-value mono">${escapeHtml(loser.hash || '?')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Size</span>
              <span class="info-value">${this._formatSize(loser.size)}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Content Type</span>
              <span class="info-value">${escapeHtml(loser.content_type || 'Unknown')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Node ID</span>
              <span class="info-value mono">${escapeHtml(loser.node_id || '?')}</span>
            </div>
            <div class="info-row">
              <span class="info-label">Version Clock</span>
              <span class="info-value mono">${escapeHtml(String(loser.virtual_time || '?'))}</span>
            </div>
          </div>
        </div>
      </div>
      <div class="conflict-info">
        <span class="muted">Conflict type: <strong>${escapeHtml(conflict.conflict_type || 'modify/modify')}</strong></span>
        <span class="muted">\u00B7 Detected: ${new Date(conflict.created_at).toLocaleString()}</span>
      </div>
    `;

    panel.style.display = '';
  }

  _hidePreview() {
    const panel = this.querySelector('.conflict-preview');
    if (panel) panel.style.display = 'none';
  }

  async _fetchConflicts() {
    try {
      const response = await fetch('/api/v1/conflicts');
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._conflicts = await response.json();
      this.render();
    } catch (error) {
      console.error('Failed to fetch conflicts:', error);
    }
  }

  async _dismissConflict(path) {
    try {
      const response = await fetch('/api/v1/conflicts/dismiss', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._selectedPath === path) {
        this._selectedPath = null;
      }
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to dismiss conflict: ${error.message}`, 'error');
    }
  }

  async _resolveConflict(path, pick) {
    try {
      const response = await fetch('/api/v1/conflicts/resolve', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ path, pick }),
      });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      if (this._selectedPath === path) {
        this._selectedPath = null;
      }
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to resolve conflict: ${error.message}`, 'error');
    }
  }

  async _dismissAll() {
    if (!confirm(`Accept all ${this._conflicts.length} auto-winners?\n\nLosing versions remain in version history.`))
      return;

    try {
      const response = await fetch('/api/v1/conflicts/dismiss-all', { method: 'POST' });
      if (!response.ok) throw new Error(`Request failed: ${response.status}`);
      this._selectedPath = null;
      await this._fetchConflicts();
    } catch (error) {
      window.aeorToast(`Failed to dismiss all conflicts: ${error.message}`, 'error');
    }
  }

  _formatSize(bytes) {
    if (bytes == null) return '?';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
}

customElements.define('aeor-conflicts', AeorConflicts);

export { AeorConflicts };
