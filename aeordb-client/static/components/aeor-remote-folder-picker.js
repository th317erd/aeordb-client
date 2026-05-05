'use strict';

import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-modal.js';

const { div, span, button } = elements;

/**
 * Remote folder picker dialog.
 *
 * Opens a modal that browses a remote aeordb server's directory tree,
 * showing only folders. The user navigates by clicking folders and
 * selects the current path.
 *
 * Usage:
 *   const path = await showRemoteFolderPicker(connectionUrl, apiKey);
 *   // path is e.g. "/docs/archive/" or null if cancelled
 */
export async function showRemoteFolderPicker(connectionUrl, apiKey) {
  return new Promise((resolve) => {
    let currentPath = '/';
    let entries = [];
    let loading = false;
    let resolved = false;

    let modal = document.createElement('aeor-modal');
    modal.title = 'Select Remote Folder';

    let finish = (result) => {
      if (resolved) return;
      resolved = true;
      modal.remove();
      resolve(result);
    };

    modal.addEventListener('close', () => finish(null));

    async function getJwt() {
      if (!apiKey) return null;
      try {
        const response = await fetch(`${connectionUrl}/auth/token`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ api_key: apiKey }),
        });
        if (!response.ok) return null;
        const data = await response.json();
        return data.token || null;
      } catch (e) {
        return null;
      }
    }

    async function fetchListing(path) {
      loading = true;
      render();

      try {
        let cleanPath = path.replace(/\/+$/, '') || '';
        let url = `${connectionUrl}/files${cleanPath}/?limit=500`;

        let headers = {};
        let jwt = await getJwt();
        if (jwt) headers['Authorization'] = `Bearer ${jwt}`;
        else if (apiKey) headers['Authorization'] = `Bearer ${apiKey}`;

        let response = await fetch(url, { headers });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);

        let data = await response.json();
        entries = (data.items || []).filter((e) => e.entry_type === 3); // directories only
      } catch (error) {
        entries = [];
        console.error('Failed to list remote directory:', error);
      }

      loading = false;
      render();
    }

    function buildBreadcrumbs() {
      let segments = currentPath.split('/').filter((s) => s.length > 0);
      let crumbs = [
        span.class('folder-picker-crumb')
          .onClick(() => { currentPath = '/'; fetchListing(currentPath); })('/'),
      ];

      let accumulated = '/';
      for (let segment of segments) {
        accumulated += segment + '/';
        let crumbPath = accumulated;
        crumbs.push(
          span.class('folder-picker-separator')(' / '),
          span.class('folder-picker-crumb')
            .onClick(() => { currentPath = crumbPath; fetchListing(currentPath); })(segment),
        );
      }

      return div.class('folder-picker-breadcrumbs')(...crumbs).build(document);
    }

    function buildFolderList() {
      if (loading) {
        return div.class('folder-picker-status')('Loading...').build(document);
      }

      if (entries.length === 0) {
        return div.class('folder-picker-status')('No subfolders').build(document);
      }

      let items = entries.map((entry) => {
        return div.class('folder-picker-item')
          .onClick(() => {
            currentPath = currentPath.replace(/\/+$/, '') + '/' + entry.name + '/';
            fetchListing(currentPath);
          })(
            span.class('folder-picker-icon')('\uD83D\uDCC1'),
            entry.name,
          );
      });

      return div.class('folder-picker-list')(...items).build(document);
    }

    function buildFooter() {
      return div.class('folder-picker-footer')(
        div.class('folder-picker-current-path')(currentPath),
        div.class('folder-picker-actions')(
          button.class('secondary').onClick(() => finish(null))('Cancel'),
          button.class('primary').onClick(() => finish(currentPath))('Select This Folder'),
        ),
      ).build(document);
    }

    function render() {
      let body = modal.querySelector('.aeor-modal__body');
      if (!body) return;

      body.textContent = '';
      body.appendChild(buildBreadcrumbs());
      body.appendChild(buildFolderList());
      body.appendChild(buildFooter());
    }

    document.body.appendChild(modal);
    fetchListing(currentPath);
  });
}
