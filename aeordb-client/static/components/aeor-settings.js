'use strict';

import { openFolder } from './aeor-file-view-shared.js';
import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-checkbox.js';

const { div, h1, h2, label, input, button, code } = elements;
const aeorCheckbox = elements['aeor-checkbox'];

// Debounce window for the auto-save on settings inputs. 1s lets the
// user pause mid-thought without firing a PATCH on every keystroke.
const SAVE_DEBOUNCE_MS = 1000;

class AeorSettings extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      // Settings values
      client_name: '',
      sync_interval_seconds: 60,
      auto_start_sync: true,
      auto_start_system: false,
      config_dir: '',
      data_dir: '',

      // UI state
      loaded: false,
      error: null,
      hostname: '',
    });

    this._scheduleSave = this._scheduleSave.bind(this);
    this._openConfigDir = this._openConfigDir.bind(this);
    this._openDataDir = this._openDataDir.bind(this);
    this._saveTimer = null;
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchSettings();
  }

  disconnectedCallback() {
    if (this._saveTimer) {
      clearTimeout(this._saveTimer);
      this._saveTimer = null;
    }
  }

  refresh() {
    this._fetchSettings();
  }

  _buildDOM() {
    this.textContent = '';

    let element = div.context(this)(
      div.class('page-header')(
        h1('Settings'),
      ),

      // Error banner
      div.class.bindState(
        (state) => state.error ? 'error-banner' : 'error-banner hidden',
        ['error'],
      ).textContent.bindState(
        (state) => state.error || '',
        ['error'],
      )(),

      // Loading state
      div.class('empty-state')
        .hidden.bindState((state) => state.loaded, ['loaded'])(
          'Loading settings...',
        ),

      // Form — hidden until loaded
      div.class('settings-form')
        .hidden.bindState((state) => !state.loaded, ['loaded'])(
        // General panel
        div.class('form-panel')(
          h2('General'),
          div.class('form-row')(
            label.for('setting-client-name')('Client Name'),
            input.type('text').id('setting-client-name')
              .placeholder.bindState(
                (state) => state.hostname || 'my-machine',
                ['hostname'],
              )(),
          ),
          div.class('form-row')(
            label.for('setting-sync-interval')('Sync Interval (seconds)'),
            input.type('number').id('setting-sync-interval')
              .min('10').max('3600')(),
          ),
          div.class('form-row')(
            aeorCheckbox.id('setting-auto-start')('Auto-start sync on launch'),
          ),
          div.class('form-row')(
            aeorCheckbox.id('setting-auto-start-system')('Start when system starts'),
          ),
        ),

        // Directories panel
        div.class('form-panel')(
          h2('Directories'),
          div.class('info-section')(
            div.class('form-row')(
              label('Config Directory'),
              div.class('dir-row')(
                code.class('dir-path')
                  .textContent.bindState(
                    (state) => state.config_dir,
                    ['config_dir'],
                  )(),
                button.class('secondary small').onClick(this._openConfigDir)('Open'),
              ),
            ),
            div.class('form-row')(
              label('Data Directory'),
              div.class('dir-row')(
                code.class('dir-path')
                  .textContent.bindState(
                    (state) => state.data_dir,
                    ['data_dir'],
                  )(),
                button.class('secondary small').onClick(this._openDataDir)('Open'),
              ),
            ),
          ),
        ),

      ),
    ).build(document);

    this.appendChild(element);

    // Auto-save on debounce: any input change schedules a PATCH after
    // SAVE_DEBOUNCE_MS of inactivity. No explicit Save button — the
    // standard for our clients now.
    const watchedIds = [
      'setting-client-name',
      'setting-sync-interval',
      'setting-auto-start',
      'setting-auto-start-system',
    ];
    for (const id of watchedIds) {
      const el = this.querySelector('#' + id);
      if (!el) continue;
      el.addEventListener('input', this._scheduleSave);
      el.addEventListener('change', this._scheduleSave);
    }
  }

  _populateInputs() {
    const s = this._state;

    const clientNameInput = this.querySelector('#setting-client-name');
    if (clientNameInput) clientNameInput.value = s.client_name || '';

    const syncIntervalInput = this.querySelector('#setting-sync-interval');
    if (syncIntervalInput) syncIntervalInput.value = s.sync_interval_seconds;

    const autoStartInput = this.querySelector('#setting-auto-start');
    if (autoStartInput) autoStartInput.checked = s.auto_start_sync;

    const autoStartSystemInput = this.querySelector('#setting-auto-start-system');
    if (autoStartSystemInput) autoStartSystemInput.checked = s.auto_start_system;
  }

  _openConfigDir() {
    openFolder(this._state.config_dir);
  }

  _openDataDir() {
    openFolder(this._state.data_dir);
  }

  async _fetchSettings() {
    try {
      const response = await fetch('/api/v1/settings');
      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        this._state.error = body.error || `Failed to load settings (${response.status})`;
        return;
      }

      const settings = await response.json();
      this._state.client_name = settings.client_name || '';
      this._state.sync_interval_seconds = settings.sync_interval_seconds;
      this._state.auto_start_sync = settings.auto_start_sync;
      this._state.auto_start_system = settings.auto_start_system;
      this._state.config_dir = settings.config_dir;
      this._state.data_dir = settings.data_dir;
      this._state.error = null;
      this._state.loaded = true;

      // Populate input values after state is set
      this._populateInputs();

      // Try to get hostname for placeholder (non-critical).
      try {
        const statusResponse = await fetch('/api/v1/status');
        if (statusResponse.ok) {
          const statusData = await statusResponse.json();
          this._state.hostname = statusData.identity?.name || '';
        }
      } catch (_) {
        // Non-critical.
      }
    } catch (error) {
      this._state.error = `Failed to load settings: ${error.message}`;
    }
  }

  _scheduleSave() {
    if (this._saveTimer) clearTimeout(this._saveTimer);
    this._saveTimer = setTimeout(() => {
      this._saveTimer = null;
      this._save();
    }, SAVE_DEBOUNCE_MS);
  }

  async _save() {
    // Read input values from DOM
    const clientNameInput = this.querySelector('#setting-client-name');
    const syncIntervalInput = this.querySelector('#setting-sync-interval');
    const autoStartInput = this.querySelector('#setting-auto-start');
    const autoStartSystemInput = this.querySelector('#setting-auto-start-system');

    const clientName = clientNameInput?.value?.trim() || null;
    const syncInterval = parseInt(syncIntervalInput?.value, 10);
    const autoStart = autoStartInput?.checked ?? true;
    const autoStartSystem = autoStartSystemInput?.checked ?? false;

    if (isNaN(syncInterval) || syncInterval < 10 || syncInterval > 3600) {
      // Mark the field as invalid; toast on transition so we don't
      // re-spam while the user is still typing. Don't PATCH.
      if (syncIntervalInput && !syncIntervalInput.classList.contains('invalid')) {
        syncIntervalInput.classList.add('invalid');
        window.aeorToast?.('Sync interval must be between 10 and 3600 seconds.', 'error');
        // Clear the invalid mark as soon as the user types again.
        syncIntervalInput.addEventListener('input', function clearOnce() {
          syncIntervalInput.classList.remove('invalid');
          syncIntervalInput.removeEventListener('input', clearOnce);
        });
      }
      return;
    }

    try {
      const response = await fetch('/api/v1/settings', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          client_name: clientName,
          sync_interval_seconds: syncInterval,
          auto_start_sync: autoStart,
          auto_start_system: autoStartSystem,
        }),
      });

      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        window.aeorToast?.(body.error || `Failed to save settings (${response.status})`, 'error');
        return;
      }

      const settings = await response.json();
      this._state.client_name = settings.client_name || '';
      this._state.sync_interval_seconds = settings.sync_interval_seconds;
      this._state.auto_start_sync = settings.auto_start_sync;
      this._state.auto_start_system = settings.auto_start_system;
      this._state.config_dir = settings.config_dir;
      this._state.data_dir = settings.data_dir;

      // Don't re-populate inputs on success — the user may have continued
      // typing during the PATCH and we'd clobber their in-flight edits.
      // The reactive state is the source of truth for code that reads it;
      // the DOM mirrors whatever the user has typed.
    } catch (error) {
      window.aeorToast?.(`Failed to save settings: ${error.message}`, 'error');
    }
  }
}

customElements.define('aeor-settings', AeorSettings);

export { AeorSettings };
