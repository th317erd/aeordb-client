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
