'use strict';

// Re-export everything from the shared web components libraries.
// This file exists so that local component imports (`./aeor-file-view-shared.js`)
// continue to work without changing every import path.

// Generic utilities from aeor-web-components
export {
  escapeHtml,
  escapeAttr,
  formatBytes,
  formatBytes as formatSize,
  formatDate,
  formatUptime,
  flashButton,
} from '../aeor/utils.js';

// Confirm dialog from aeor-web-components
export { showConfirm } from '../aeor/confirm.js';

// DB-specific file view helpers from aeordb-web-components
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
