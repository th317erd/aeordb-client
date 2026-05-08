'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';

const { div, nav, span } = elements;

const NAV_ITEMS = [
  { page: 'dashboard',   icon: '\u25A0', iconClass: 'nav-icon-accent',     label: 'Dashboard' },
  { page: 'connections', icon: '\u21C4', iconClass: 'nav-icon-connection', label: 'Connections' },
  { page: 'sync',        icon: '\u21BB', iconClass: 'nav-icon-success',    label: 'Sync' },
  { page: 'files',       icon: '\uD83D\uDCC1', iconClass: 'nav-icon-files', label: 'Files' },
  { page: 'conflicts',   icon: '\u26A0', iconClass: 'nav-icon-warning',    label: 'Conflicts' },
  { page: 'settings',    icon: '\u2699', iconClass: 'nav-icon-muted',      label: 'Settings' },
];

class AeorNav extends HTMLElement {
  constructor() {
    super();

    this._state = new ReactiveState({
      active:  'dashboard',
      version: '0.1.0',
    });

    this._cachedVersion = null;
    this._handleNavClick = this._handleNavClick.bind(this);
  }

  static get observedAttributes() {
    return ['active'];
  }

  get active() {
    return this._state.active;
  }

  set active(value) {
    this._state.active = value || 'dashboard';
  }

  attributeChangedCallback(name, _oldValue, newValue) {
    if (name === 'active')
      this._state.active = newValue || 'dashboard';
  }

  connectedCallback() {
    this._isConnected = true;
    this._state.active = this.getAttribute('active') || 'dashboard';
    this._buildDOM();
    this._fetchVersion();
  }

  disconnectedCallback() {
    this._isConnected = false;
  }

  _buildDOM() {
    this.textContent = '';

    let element = div.context(this)(
      div.class('nav-logo')(
        'Aeor',
        span('DB'),
        ' Client',
      ),
      nav.class('nav-items')(
        ...NAV_ITEMS.map((item) =>
          div.class.bindState(
            (state) => (state.active === item.page) ? 'nav-item active' : 'nav-item',
            ['active'],
          ).dataPage(item.page)
            .onClick(this._handleNavClick)(
              span.class(`nav-icon ${item.iconClass}`)(item.icon),
              item.label,
            ),
        ),
      ),
      div.class('nav-version')
        .textContent.bindState(
          (state) => `v${state.version}`,
          ['version'],
        )(),
    ).build(document);

    this.appendChild(element);
  }

  _handleNavClick(event) {
    let navItem = event.target.closest('.nav-item');
    if (!navItem)
      return;

    this.dispatchEvent(new CustomEvent('navigate', {
      detail:  { page: navItem.dataset.page },
      bubbles: true,
    }));
  }

  async _fetchVersion() {
    if (this._cachedVersion)
      return;

    try {
      let response = await fetch('/api/v1/status');
      if (!response.ok)
        return;

      let data = await response.json();
      if (!this._isConnected)
        return;

      this._cachedVersion = data.version;
      this._state.version = data.version;
    } catch (error) {
      // Non-critical — version display is best-effort
    }
  }
}

if (!customElements.get('aeor-nav'))
  customElements.define('aeor-nav', AeorNav);

export { AeorNav };
