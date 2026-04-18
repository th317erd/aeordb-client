'use strict';

/// Shared utilities for file view components (list and grid).
/// Not a component itself — just exported helper functions.

export function formatSize(bytes) {
  if (bytes == null) return '\u2014';
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB';
}

export function formatDate(timestamp) {
  if (!timestamp) return '\u2014';
  const date = new Date(timestamp);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  const seconds = String(date.getSeconds()).padStart(2, '0');
  return `${year}/${month}/${day} ${hours}:${minutes}:${seconds}`;
}

export function fileIcon(entryType) {
  if (entryType === 3) return '\uD83D\uDCC1'; // folder
  if (entryType === 8) return '\uD83D\uDD17'; // symlink
  return '\uD83D\uDCC4'; // file
}

export function syncBadgeClass(syncStatus) {
  if (syncStatus === 'synced') return 'synced';
  if (syncStatus === 'pending') return 'pending';
  return 'not-synced';
}

export function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

export function escapeAttr(str) {
  return str.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

export function fileExtension(name) {
  const dot = name.lastIndexOf('.');
  if (dot < 0) return '';
  return name.substring(dot + 1).toLowerCase();
}

export function isImageFile(name) {
  const ext = fileExtension(name);
  return ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico'].includes(ext);
}

export function isVideoFile(name) {
  const ext = fileExtension(name);
  return ['mp4', 'webm', 'ogg', 'mov', 'avi', 'mkv'].includes(ext);
}

export function isAudioFile(name) {
  const ext = fileExtension(name);
  return ['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'].includes(ext);
}
