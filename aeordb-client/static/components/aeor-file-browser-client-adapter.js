'use strict';

import { FileBrowserAdapter } from '../shared/components/aeor-file-browser-adapter.js';

/**
 * Client-specific adapter for the file browser.
 * Talks to the aeordb-client API with sync relationship awareness.
 */
export class ClientFileBrowserAdapter extends FileBrowserAdapter {
  constructor(relationshipId) {
    super();
    this._relationshipId = relationshipId;
  }

  get relationshipId() { return this._relationshipId; }
  get supportsTabs() { return true; }
  get supportsSync() { return true; }
  get supportsOpenLocally() { return true; }

  async browse(path, limit, offset) {
    const encodedPath = (path === '/') ? '' : encodeURIComponent(path);
    const baseUrl = encodedPath
      ? `/api/v1/browse/${this._relationshipId}/${encodedPath}`
      : `/api/v1/browse/${this._relationshipId}`;
    const url = `${baseUrl}?limit=${limit}&offset=${offset}`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`Request failed: ${response.status}`);
    return response.json();
  }

  fileUrl(path) {
    return `/api/v1/files/${this._relationshipId}/${encodeURIComponent(path)}`;
  }

  fullFileUrl(path) {
    return `${window.location.origin}${this.fileUrl(path)}`;
  }

  async upload(path, body, contentType) {
    const response = await fetch(this.fileUrl(path), {
      method: 'PUT',
      headers: { 'Content-Type': contentType },
      body: body,
    });
    if (!response.ok) throw new Error(`Request failed: ${response.status}`);
  }

  async delete(path) {
    const response = await fetch(this.fileUrl(path), { method: 'DELETE' });
    if (!response.ok) throw new Error(`Request failed: ${response.status}`);
  }

  async rename(fromPath, toPath) {
    const response = await fetch(`/api/v1/files/${this._relationshipId}/rename`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ from: fromPath, to: toPath }),
    });
    if (!response.ok) throw new Error(`Request failed: ${response.status}`);
  }

  async openLocally(path) {
    const response = await fetch(`/api/v1/files/${this._relationshipId}/open`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: path.replace(/^\//, '') }),
    });
    if (!response.ok) throw new Error(`Open failed: ${response.status}`);
  }

  /** Fetch sync relationships for the tab selector. */
  async listRelationships() {
    const response = await fetch('/api/v1/sync');
    if (!response.ok) throw new Error(`Request failed: ${response.status}`);
    return response.json();
  }
}
