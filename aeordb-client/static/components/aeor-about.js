'use strict';

import { ReactiveState } from '../aeor/reactive-state.js';
import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-cycling-text.js';
import '../aeor/components/aeor-confirm-button.js';

const { div, h1, h2, p, span, a } = elements;
const aeorConfirmButton = elements['aeor-confirm-button'];

// Easter-egg variants for the "Wyatt Greenway" line. Pool inherited
// from Xenocept's About-page version (commit fa03641); we extend it
// with AeorDB-flavored titles so the cycle has fresh entries the
// Xenocept user hasn't already discovered. First entry is the real
// name so the cycle can land back on truth.
const WYATT_TITLES = [
  'Wyatt Greenway',
  // ── inherited from Xenocept ──
  'Engineer Extraordinaire',
  'The Fabulous and Fantastic',
  'The Wizard',
  'Architect of Pixels',
  'Slayer of Bugs, Tamer of Race Conditions',
  'Senior Vice President of Things That Actually Ship',
  'Patron Saint of Just-One-More-Commit',
  'Knight Commander of the Order of Cargo Build',
  'Maestro of Mouse Wheels and Pen Sizes',
  'Conjurer of Aeordb Indexes',
  'Wizard of the Trigram Tower',
  'Champion of the Hold-To-Confirm',
  'The Honorable Webview Whisperer',
  'High Druid of Tauri Single-Instance',
  'Chief Bottle-Washer of the Plugin Manifest',
  'Last Defender of Server-Side Truth',
  'The Refactor Whisperer',
  'Steward of Six Claude Destinations',
  'Lord Protector of `.xenocept-sig`',
  'Custodian of Capture Channels',
  'Heir to the dev-watch Throne',
  'Vice Admiral of the SSE Fleet',
  'Bard of the Build-Mirror',
  // ── new for AeorDB Client ──
  'Captain of the Conflict-Resolution Brigade',
  'Synchronizer of Folders Both Near and Far',
  'Guardian of the Force-Resync',
  'Whisperer of Remote Connections',
  'Diviner of File Diffs',
  'Marshal of the Pull-And-Push',
  'Reverend of Relationship IDs',
  'Headmaster of the Element Builder',
  'Curator of the Reactive State',
  'Ambassador to the Engine Team',
  'Foreman of the Build Mirror',
  'Plenipotentiary of Symlink Routing',
];

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
    // updateInfo mirrors `GET /api/v1/update/status`. Starts empty;
    // _fetchUpdateInfo() fills it on mount and `refresh()` re-polls
    // when the user navigates back to this page (in case a background
    // /api/version check happened while they were elsewhere).
    this._state = new ReactiveState({
      version:    '0.1.0',
      updateInfo: null,
    });
  }

  connectedCallback() {
    this._buildDOM();
    this._fetchVersion();
    this._fetchUpdateInfo();
  }

  refresh() {
    this._fetchVersion();
    this._fetchUpdateInfo();
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
        // Updates section. The hold-to-confirm button is always
        // visible (so the user knows the feature exists); when there's
        // nothing to install, it sits disabled and the status line
        // reads "You're up to date." When an update is available, the
        // label shows the target version and the status line
        // disappears as soon as the apply begins.
        //
        // Both the label, the disabled state, and the status line are
        // bound to state.updateInfo so a `refresh()` (or the startup
        // poll landing) re-renders them in place without rebuilding
        // the whole page.
        div.class('about-section about-section--update')(
          h2('Updates'),
          div.class('about-update-row')(
            aeorConfirmButton
              .class('about-update-btn confirm-button-progress')
              .confirmedText('Updating…')
              .duration('1000')
              .label.bindState(
                (state) => {
                  const ui = state.updateInfo;
                  return (ui && ui.available && ui.latest_version)
                    ? `Update to v${ui.latest_version}`
                    : 'Update';
                },
                ['updateInfo'],
              )
              .disabled.bindState(
                (state) => !(state.updateInfo && state.updateInfo.available),
                ['updateInfo'],
              )
              .ariaLabel.bindState(
                (state) => {
                  const ui = state.updateInfo;
                  return (ui && ui.available && ui.latest_version)
                    ? `Hold to update AeorDB Client to v${ui.latest_version}`
                    : 'AeorDB Client is up to date';
                },
                ['updateInfo'],
              )
              .onConfirm((event) => this._handleUpdateConfirm(event))(),
            span.class('about-update-status').textContent.bindState(
              (state) => {
                const ui = state.updateInfo;
                if (ui && ui.available && ui.latest_version) {
                  return `Version ${ui.latest_version} is available.`;
                }
                return "You're up to date.";
              },
              ['updateInfo'],
            )(),
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
          // Click-to-cycle easter egg — see aeor-cycling-text.js. The
          // attribute carries JSON-encoded variants; the element parses
          // on connect. The `title` tooltip is the only outward affordance
          // (cursor + hover color come from the shared CSS).
          p(
            elements['aeor-cycling-text']
              .title('Click me')
              .variants(JSON.stringify(WYATT_TITLES))(
                'Wyatt Greenway',
              ),
          ),
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

  async _fetchUpdateInfo() {
    try {
      const response = await fetch('/api/v1/update/status');
      if (!response.ok) return;
      const info = await response.json();
      this._state.updateInfo = info;
    } catch (error) {
      console.warn('[update] status fetch failed:', error);
    }
  }

  /**
   * onConfirm handler for the Update button. Streams NDJSON progress
   * events from `POST /api/v1/update/apply`, drives the progress fill
   * via CSS variable, and on success polls `/api/v1/status` until the
   * version changes (a sign that the relauncher swapped the binary)
   * before reloading the page.
   *
   * Takes direct DOM control of the button while the apply runs — the
   * bindStates above would fight us otherwise. After a successful
   * update we reload, so the temporary DOM divergence doesn't matter.
   * On error we keep the error styling visible.
   */
  async _handleUpdateConfirm(event) {
    const target = event.currentTarget;
    // Hide the "Version X.Y.Z is available." status line — it goes
    // stale the moment we commit to the apply.
    const statusEl = target.parentElement?.querySelector('.about-update-status');
    if (statusEl) statusEl.style.display = 'none';

    const ui = this._state.updateInfo;
    const fromVersion = ui && ui.current_version;

    target.classList.add('applying');
    target.setAttribute('label', 'Updating…');
    target.disabled = true;
    target.progress = 0;

    const setProgress = (pct) => {
      target.progress = Math.max(0, Math.min(100, pct));
    };
    const fmtMB = (b) => (b / (1024 * 1024)).toFixed(1);

    try {
      const r = await fetch('/api/v1/update/apply', { method: 'POST' });
      if (!r.ok) {
        const detail = await r.text().catch(() => '');
        throw new Error(`HTTP ${r.status}${detail ? `: ${detail}` : ''}`);
      }

      // Stream-read NDJSON progress events. Each line is one JSON
      // object: {"phase":"downloading","bytes":N,"total":M},
      // {"phase":"verifying"}, {"phase":"staging"}, {"phase":"complete"},
      // or {"phase":"error","message":"…"}.
      const reader  = r.body.getReader();
      const decoder = new TextDecoder();
      let buffer  = '';
      let sawError = null;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let nl;
        while ((nl = buffer.indexOf('\n')) >= 0) {
          const line = buffer.slice(0, nl); buffer = buffer.slice(nl + 1);
          if (!line.trim()) continue;
          let evt;
          try { evt = JSON.parse(line); } catch { continue; }
          if (evt.phase === 'downloading') {
            const pct = evt.total ? (evt.bytes / evt.total) * 100 : 0;
            setProgress(pct);
            target.setAttribute('label', evt.total
              ? `Updating… ${fmtMB(evt.bytes)} / ${fmtMB(evt.total)} MB`
              : `Updating… ${fmtMB(evt.bytes)} MB`);
          } else if (evt.phase === 'verifying') {
            setProgress(100);
            target.setAttribute('label', 'Verifying signature…');
          } else if (evt.phase === 'staging') {
            target.setAttribute('label', 'Staging update…');
          } else if (evt.phase === 'complete') {
            target.setAttribute('label', 'Restarting…');
          } else if (evt.phase === 'error') {
            sawError = evt.message || 'unknown error';
          }
        }
      }
      if (sawError) throw new Error(sawError);

      // Stream closed cleanly — the server has exited. Poll
      // /api/v1/status until the new binary is up AND reports a
      // different version, then reload so the UI re-fetches code from
      // the new server.
      target.setAttribute('label', 'Restarting…');
      await this._pollForRelaunchAndReload(fromVersion, target);
    } catch (error) {
      console.error('[update] apply failed:', error);
      target.classList.add('about-update-btn-error');
      target.setAttribute('label', `Update failed: ${String(error && error.message ? error.message : error)}`);
      target.disabled = false;
      target.classList.remove('applying');
    }
  }

  /**
   * Poll /api/v1/status every 500ms until the new binary is up AND
   * self-reports a different version than `fromVersion`. Then reload
   * so the UI re-fetches static assets + JS from the new server.
   * Times out after 60s with a clear failure message in the button.
   */
  async _pollForRelaunchAndReload(fromVersion, btnHost) {
    const start = Date.now();
    while (Date.now() - start < 60000) {
      try {
        const r = await fetch('/api/v1/status', { cache: 'no-store' });
        if (r.ok) {
          const info = await r.json();
          if (info && info.version && info.version !== fromVersion) {
            location.reload();
            return;
          }
        }
      } catch { /* server still down between PID-exit and relauncher mv+spawn */ }
      await new Promise(r => setTimeout(r, 500));
    }
    if (btnHost) {
      btnHost.setAttribute('label', 'Restart took too long. Please relaunch aeordb-client manually.');
    }
  }
}

customElements.define('aeor-about', AeorAbout);
export { AeorAbout };
