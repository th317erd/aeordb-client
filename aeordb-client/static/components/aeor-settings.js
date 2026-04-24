'use strict';

import { escapeHtml, escapeAttr, openFolder } from './aeor-file-view-shared.js';

class AeorSettings extends HTMLElement {
  constructor() {
    super();
    this._settings = null;
    this._saving   = false;
    this._saved    = false;
    this._error    = null;
  }

  connectedCallback() {
    this._isConnected = true;
    this._timeoutIds = [];
    this.render();
    this._fetchSettings();
  }

  disconnectedCallback() {
    this._isConnected = false;
    if (this._timeoutIds) {
      this._timeoutIds.forEach(id => clearTimeout(id));
      this._timeoutIds = [];
    }
  }

  render() {
    this.innerHTML = `
      <div class="page-header">
        <h1>Settings</h1>
      </div>

      ${(this._error) ? `<div class="error-banner">${escapeHtml(this._error)}</div>` : ''}

      ${(this._settings === null)
        ? '<div class="empty-state">Loading settings...</div>'
        : this._renderForm()
      }
    `;

    if (this._settings !== null)
      this._bindEvents();
  }

  _renderForm() {
    const s = this._settings;

    return `
      <div class="form-panel">
        <h2>General</h2>
        <div class="form-row">
          <label for="setting-client-name">Client Name</label>
          <input type="text" id="setting-client-name" value="${escapeAttr(s.client_name || '')}" placeholder="${escapeAttr(this._hostname || 'my-machine')}">
        </div>
        <div class="form-row">
          <label for="setting-sync-interval">Sync Interval (seconds)</label>
          <input type="number" id="setting-sync-interval" value="${s.sync_interval_seconds}" min="10" max="3600">
        </div>
        <div class="form-row">
          <label class="checkbox-row">
            <input type="checkbox" class="checkbox-large" id="setting-auto-start" ${(s.auto_start_sync) ? 'checked' : ''}>
            Auto-start sync on launch
          </label>
        </div>
        <div class="form-row">
          <label class="checkbox-row">
            <input type="checkbox" class="checkbox-large" id="setting-auto-start-system" ${(s.auto_start_system) ? 'checked' : ''}>
            Start when system starts
          </label>
        </div>
      </div>

      <div class="form-panel">
        <h2>Directories</h2>
        <div class="info-section">
          <div class="form-row">
            <label>Config Directory</label>
            <div style="display: flex; gap: 8px; align-items: center;">
              <code style="flex: 1; padding: 8px; background: var(--bg-secondary); border-radius: 4px; font-size: 13px; color: var(--text-secondary);">${escapeHtml(s.config_dir)}</code>
              <button class="secondary small" id="open-config-dir">Open</button>
            </div>
          </div>
          <div class="form-row">
            <label>Data Directory</label>
            <div style="display: flex; gap: 8px; align-items: center;">
              <code style="flex: 1; padding: 8px; background: var(--bg-secondary); border-radius: 4px; font-size: 13px; color: var(--text-secondary);">${escapeHtml(s.data_dir)}</code>
              <button class="secondary small" id="open-data-dir">Open</button>
            </div>
          </div>
        </div>
      </div>

      <div class="form-actions" style="margin-top: 16px;">
        <button class="${(this._saved) ? 'success' : 'primary'}" id="save-settings" ${(this._saving || this._saved) ? 'disabled' : ''}>${(this._saved) ? '\u2713 Saved!' : (this._saving) ? 'Saving...' : 'Save'}</button>
      </div>
    `;
  }

  _bindEvents() {
    const saveButton = this.querySelector('#save-settings');
    if (saveButton) {
      saveButton.addEventListener('click', () => this._saveSettings());
    }

    const openConfigDir = this.querySelector('#open-config-dir');
    if (openConfigDir) {
      openConfigDir.addEventListener('click', () => {
        openFolder(this._settings.config_dir);
      });
    }

    const openDataDir = this.querySelector('#open-data-dir');
    if (openDataDir) {
      openDataDir.addEventListener('click', () => {
        openFolder(this._settings.data_dir);
      });
    }
  }

  async _fetchSettings() {
    try {
      const response = await fetch('/api/v1/settings');
      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        this._error = body.error || `Failed to load settings (${response.status})`;
        this.render();
        return;
      }
      this._settings = await response.json();
      this._error    = null;

      // Try to get hostname for placeholder.
      try {
        const statusResponse = await fetch('/api/v1/status');
        if (statusResponse.ok) {
          const statusData = await statusResponse.json();
          this._hostname   = statusData.identity?.name || null;
        }
      } catch (_) {
        // Non-critical.
      }

      this.render();
    } catch (error) {
      this._error = `Failed to load settings: ${error.message}`;
      this.render();
    }
  }

  async _saveSettings() {
    if (this._saving) return;

    // Read input values BEFORE any re-render destroys the DOM inputs.
    const clientNameInput       = this.querySelector('#setting-client-name');
    const syncIntervalInput     = this.querySelector('#setting-sync-interval');
    const autoStartInput        = this.querySelector('#setting-auto-start');
    const autoStartSystemInput  = this.querySelector('#setting-auto-start-system');

    const clientName      = clientNameInput?.value?.trim() || null;
    const syncInterval    = parseInt(syncIntervalInput?.value, 10);
    const autoStart       = autoStartInput?.checked ?? true;
    const autoStartSystem = autoStartSystemInput?.checked ?? false;

    this._saving = true;
    this._saved  = false;
    this._error  = null;
    this.render();

    if (isNaN(syncInterval) || syncInterval < 10 || syncInterval > 3600) {
      this._saving = false;
      this._error  = 'Sync interval must be between 10 and 3600 seconds.';
      this.render();
      return;
    }

    try {
      const response = await fetch('/api/v1/settings', {
        method:  'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({
          client_name:           clientName,
          sync_interval_seconds: syncInterval,
          auto_start_sync:       autoStart,
          auto_start_system:     autoStartSystem,
        }),
      });

      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        this._error = body.error || `Failed to save settings (${response.status})`;
        this._saving = false;
        this.render();
        return;
      }

      this._settings = await response.json();
      this._saving   = false;
      this._saved    = true;
      this.render();

      // Reset "Saved!" text after 2 seconds.
      this._timeoutIds.push(setTimeout(() => {
        if (!this._isConnected) return;
        this._saved = false;
        const btn = this.querySelector('#save-settings');
        if (btn) btn.textContent = 'Save';
      }, 2000));
    } catch (error) {
      this._saving = false;
      this._error  = `Failed to save settings: ${error.message}`;
      this.render();
    }
  }

}

customElements.define('aeor-settings', AeorSettings);

export { AeorSettings };
