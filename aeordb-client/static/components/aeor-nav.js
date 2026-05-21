'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';

const { div, nav, span, a, svg, path, circle, line } = elements;

const LOGO_PATH = 'M 22.999629,0 0,34.99993 H 7.1680371 L 22.999629,10.921296 h 5.16e-4 L 26.235608,6.00015 h 2.764173 v 28.99978 h 3.235976 2.764174 V 0 h -6.00015 z m 0,21.843111 -8.650634,13.156819 h 8.650634 z';

// (i)-in-a-circle \u2014 same Feather-style geometry as aeor-info-box's
// icon. `currentColor` so it tracks the nav-item's text color (active
// vs idle) instead of hard-coding a tint. Built as a factory so each
// NAV_ITEMS map iteration gets a fresh builder.
const aboutIcon = () =>
  svg
    .width('1em').height('1em')
    .viewBox('0 0 24 24')
    .fill('none')
    .stroke('currentColor')
    .strokeWidth('2')
    .strokeLinecap('round')
    .strokeLinejoin('round')(
      circle.cx('12').cy('12').r('10')(),
      line.x1('12').y1('16').x2('12').y2('12')(),
      line.x1('12').y1('8').x2('12.01').y2('8')(),
    );

const NAV_ITEMS = [
  { page: 'dashboard',   icon: '\u25A0', iconClass: 'nav-icon-accent',     label: 'Dashboard' },
  { page: 'connections', icon: '\u21C4', iconClass: 'nav-icon-connection', label: 'Connections' },
  { page: 'sync',        icon: '\u21BB', iconClass: 'nav-icon-success',    label: 'Sync' },
  { page: 'files',       icon: '\uD83D\uDCC1', iconClass: 'nav-icon-files', label: 'Files' },
  { page: 'conflicts',   icon: '\u26A0', iconClass: 'nav-icon-warning',    label: 'Conflicts' },
  { page: 'settings',    icon: '\u2699', iconClass: 'nav-icon-muted',      label: 'Settings' },
  { page: 'about',       icon: aboutIcon, iconClass: 'nav-icon-muted',     label: 'About' },
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
        svg.class('nav-logo-icon')
          .width('20')
          .height('20')
          .viewBox('0 0 35 35')(
            path.fill('var(--accent)').d(LOGO_PATH)(),
          ),
        div.class('nav-logo-text')(
          'Aeor',
          span('DB'),
          ' Client',
        ),
      ),
      nav.class('nav-items')(
        ...NAV_ITEMS.map((item) =>
          div.class.bindState(
            (state) => (state.active === item.page) ? 'nav-item active' : 'nav-item',
            ['active'],
          ).dataPage(item.page)
            .onClick(this._handleNavClick)(
              span.class(`nav-icon ${item.iconClass}`)(
                (typeof item.icon === 'function') ? item.icon() : item.icon,
              ),
              item.label,
            ),
        ),
      ),
      div.class('nav-version')(
        span.textContent.bindState(
          (state) => `v${state.version}`,
          ['version'],
        )(),
        ' · ',
        a.class('nav-version-link')
          .href('https://aeordb.com')
          .target('_blank')
          .rel('noopener noreferrer')
          .onClick((event) => {
            // Tauri's webview can't navigate to external URLs (no
            // browser-tab context to spawn into), so target=_blank
            // no-ops. Route through the open_external_url Tauri
            // command which shells out to xdg-open / open / cmd-start
            // depending on platform. Plain-browser previews fall
            // through to the anchor's default navigation, preserving
            // middle-click / cmd-click.
            const invoke = window.__TAURI_INTERNALS__?.invoke
                        || window.__TAURI__?.core?.invoke;
            if (invoke) {
              event.preventDefault();
              invoke('open_external_url', { url: 'https://aeordb.com' })
                .catch((error) => console.warn('open_external_url failed:', error));
            }
          })('aeordb.com'),
      ),
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
