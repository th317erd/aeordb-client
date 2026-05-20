'use strict';

import { elements } from '../aeor/elements.js';
import '../aeor/components/aeor-modal.js';

const { div, span, button } = elements;

/**
 * Remote folder picker dialog.
 *
 * Browses a remote aeordb server's directory tree via the local client's
 * proxy endpoint (`/api/v1/connections/{id}/browse?path=...`). Proxying
 * server-side avoids the engine's CORS preflight failure and keeps the
 * api-key/JWT handling in Rust where it belongs.
 *
 * Usage:
 *   const path = await showRemoteFolderPicker(connectionId);
 *   // path is e.g. "/Pictures/Family/Harlo/" or null if cancelled
 */
export async function showRemoteFolderPicker(connectionId) {
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

    async function fetchListing(path) {
      loading = true;
      render();

      try {
        const url = `/api/v1/connections/${encodeURIComponent(connectionId)}/browse?path=${encodeURIComponent(path)}`;
        const response = await fetch(url);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const data = await response.json();
        entries = data.entries || [];
      } catch (error) {
        entries = [];
        console.error('Failed to list remote directory:', error);
        window.aeorToast?.(`Failed to load remote folder: ${error.message}`, 'error');
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
            currentPath = entry.full_path.replace(/\/+$/, '') + '/';
            fetchListing(currentPath);
          })(
            span.class('folder-picker-icon')('📁'),
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
