'use strict';

// Re-export everything from the shared web components library.
// This file exists so that local component imports (`./aeor-file-view-shared.js`)
// continue to work without changing every import path.

export {
  escapeHtml,
  escapeAttr,
  formatBytes,
  formatBytes as formatSize,
  formatDate,
  formatUptime,
} from '../shared/utils.js';

export {
  fileIcon,
  ENTRY_TYPE_FILE,
  ENTRY_TYPE_DIR,
  ENTRY_TYPE_SYMLINK,
  formatRelativeTime,
  directionLabel,
  directionArrow,
  bindResizeHandle,
  openFolder,
  fileExtension,
  isImageFile,
  isVideoFile,
  isAudioFile,
  isTextFile,
} from '../shared/components/aeor-file-view-shared.js';

// Ensure the modal component is registered
import '../shared/components/aeor-modal.js';

/**
 * Show a styled confirmation dialog using <aeor-modal>.
 * Returns a Promise that resolves to true (confirmed) or false (cancelled).
 */
export function showConfirm(title, message, { confirmText = 'Confirm', cancelText = 'Cancel', danger = false } = {}) {
  // Import escapeHtml at call time to avoid circular dependency
  const _escape = (str) => {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  };

  return new Promise((resolve) => {
    const modal = document.createElement('aeor-modal');
    modal.title = title;

    const confirmClass = danger ? 'danger' : 'primary';

    modal.innerHTML = `
      <p style="color: var(--text-primary, #e6edf3); line-height: 1.6; margin: 0 0 20px 0; font-size: 0.95rem;">${_escape(message)}</p>
      <div style="display: flex; gap: 10px; justify-content: flex-end;">
        <button class="secondary" id="modal-cancel">${_escape(cancelText)}</button>
        <button class="${confirmClass}" id="modal-confirm">${_escape(confirmText)}</button>
      </div>
    `;

    document.body.appendChild(modal);

    let resolved = false;
    const finish = (result) => {
      if (resolved) return;
      resolved = true;
      modal.remove();
      resolve(result);
    };

    modal.querySelector('#modal-confirm').addEventListener('click', () => finish(true));
    modal.querySelector('#modal-cancel').addEventListener('click', () => finish(false));
    modal.addEventListener('close', () => finish(false));
  });
}
