'use strict';

import { escapeHtml } from './aeor-file-view-shared.js';
import '../shared/components/aeor-modal.js';

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

    const modal = document.createElement('aeor-modal');
    modal.title = 'Select Remote Folder';

    const finish = (result) => {
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
        const cleanPath = path.replace(/\/+$/, '') || '';
        const url = `${connectionUrl}/files${cleanPath}/?limit=500`;

        const headers = {};
        const jwt = await getJwt();
        if (jwt) headers['Authorization'] = `Bearer ${jwt}`;
        else if (apiKey) headers['Authorization'] = `Bearer ${apiKey}`;

        const response = await fetch(url, { headers });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);

        const data = await response.json();
        entries = (data.items || []).filter((e) => e.entry_type === 3); // directories only
      } catch (error) {
        entries = [];
        console.error('Failed to list remote directory:', error);
      }

      loading = false;
      render();
    }

    function renderBreadcrumbs() {
      const segments = currentPath.split('/').filter((s) => s.length > 0);
      let html = '<span class="folder-picker-crumb" data-path="/">/</span>';
      let accumulated = '/';
      for (const segment of segments) {
        accumulated += segment + '/';
        html += ` <span style="color:var(--text-muted)">/</span> <span class="folder-picker-crumb" data-path="${escapeHtml(accumulated)}">${escapeHtml(segment)}</span>`;
      }
      return html;
    }

    function render() {
      let content = `
        <div style="margin-bottom: 12px; font-size: 0.9rem; color: var(--text-secondary);">
          ${renderBreadcrumbs()}
        </div>
      `;

      if (loading) {
        content += '<div style="color: var(--text-muted); padding: 20px 0;">Loading...</div>';
      } else if (entries.length === 0) {
        content += '<div style="color: var(--text-muted); padding: 20px 0;">No subfolders</div>';
      } else {
        content += '<div class="folder-picker-list">';
        for (const entry of entries) {
          content += `
            <div class="folder-picker-item" data-name="${escapeHtml(entry.name)}">
              <span style="margin-right: 8px;">\uD83D\uDCC1</span>${escapeHtml(entry.name)}
            </div>
          `;
        }
        content += '</div>';
      }

      content += `
        <div style="display: flex; gap: 8px; justify-content: space-between; align-items: center; margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--border, #30363d);">
          <div style="font-family: var(--font-mono, monospace); font-size: 0.85rem; color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
            ${escapeHtml(currentPath)}
          </div>
          <div style="display: flex; gap: 8px; flex-shrink: 0;">
            <button class="secondary folder-picker-cancel">Cancel</button>
            <button class="primary folder-picker-select">Select This Folder</button>
          </div>
        </div>
      `;

      modal.innerHTML = content;
      bindEvents();
    }

    function bindEvents() {
      // Breadcrumb clicks
      modal.querySelectorAll('.folder-picker-crumb').forEach((el) => {
        el.style.cursor = 'pointer';
        el.style.color = 'var(--accent, #f97316)';
        el.addEventListener('click', () => {
          currentPath = el.dataset.path;
          fetchListing(currentPath);
        });
      });

      // Folder clicks
      modal.querySelectorAll('.folder-picker-item').forEach((el) => {
        el.addEventListener('click', () => {
          currentPath = currentPath.replace(/\/+$/, '') + '/' + el.dataset.name + '/';
          fetchListing(currentPath);
        });
      });

      // Select button
      const selectBtn = modal.querySelector('.folder-picker-select');
      if (selectBtn) {
        selectBtn.addEventListener('click', () => finish(currentPath));
      }

      // Cancel button
      const cancelBtn = modal.querySelector('.folder-picker-cancel');
      if (cancelBtn) {
        cancelBtn.addEventListener('click', () => finish(null));
      }
    }

    document.body.appendChild(modal);
    fetchListing(currentPath);
  });
}
