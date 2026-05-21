'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';

const { div, h1, h2, p, span, a } = elements;

/**
 * <aeor-about> — Static "About" page.
 *
 * Mirrors the structure of xenocept-client's About page:
 *   - hero with product name + live version from /api/v1/status
 *   - developer
 *   - primary engineering & product design
 *   - external links (aeordb.com, docs)
 *
 * External anchors route through Tauri's `open_external_url` command
 * so the OS browser handles them — same pattern as the sidebar
 * version link in aeor-nav.js. Falls back to default anchor behavior
 * in plain-browser previews.
 */
class AeorAbout extends HTMLElement {
  constructor() {
    super();
    this._state = new ReactiveState({ version: '0.1.0' });
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchVersion();
  }

  refresh() {
    this._fetchVersion();
  }

  _buildDOM() {
    this.textContent = '';

    const externalLink = (href, text) =>
      a.class('about-link')
        .href(href)
        .target('_blank')
        .rel('noopener noreferrer')
        .onClick((event) => {
          const invoke = window.__TAURI_INTERNALS__?.invoke
                      || window.__TAURI__?.core?.invoke;
          if (invoke) {
            event.preventDefault();
            invoke('open_external_url', { url: href })
              .catch((error) => console.warn('open_external_url failed:', error));
          }
        })(text);

    const root = div.context(this)(
      div.class('page-header')(
        h1('About'),
      ),
      div.class('about-page')(
        div.class('about-section about-section--hero')(
          h2.textContent.bindState(
            (state) => `AeorDB Client v${state.version}`,
            ['version'],
          )(),
          p.class('settings-description')(
            'Desktop client for syncing folders between your local machine and ',
            'one or more AeorDB databases. Browse remote files, manage sync ',
            'relationships, and resolve conflicts when both sides change.',
          ),
        ),
        div.class('about-section')(
          h2('Developer'),
          p('AEOR Development LLC'),
          p(
            'Email: ',
            externalLink('mailto:hello@aeor-development.com', 'hello@aeor-development.com'),
          ),
        ),
        div.class('about-section')(
          h2('Primary engineering & product design'),
          p('Wyatt Greenway'),
        ),
        div.class('about-section')(
          h2('Links'),
          p.class('about-links')(
            externalLink('https://aeordb.com', 'aeordb.com'),
            span.class('about-link-sep')(' · '),
            externalLink('https://aeordb.com/docs/', 'Documentation'),
          ),
        ),
      ),
    ).build(document);

    this.appendChild(root);
  }

  async _fetchVersion() {
    try {
      const response = await fetch('/api/v1/status');
      if (!response.ok) return;
      const data = await response.json();
      if (data && typeof data.version === 'string' && data.version.trim()) {
        this._state.version = data.version.trim();
      }
    } catch (_) {
      // Non-critical — fall back to the constructor default.
    }
  }
}

customElements.define('aeor-about', AeorAbout);
export { AeorAbout };
