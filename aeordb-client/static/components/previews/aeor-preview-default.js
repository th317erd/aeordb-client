'use strict';

import { formatSize } from '../aeor-file-view-shared.js';

class AeorPreviewDefault extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="preview-binary-info">
        <div class="preview-binary-icon">\uD83D\uDCC4</div>
        <div class="preview-binary-details">
          <div class="preview-binary-name"></div>
          <div class="preview-binary-meta preview-binary-type"></div>
          <div class="preview-binary-meta preview-binary-size"></div>
        </div>
      </div>
    `;
  }

  load() {
    const filename = this.getAttribute('filename') || 'Unknown';
    const size = parseInt(this.getAttribute('size') || '0', 10);
    const contentType = this.getAttribute('content-type') || 'application/octet-stream';

    const nameEl = this.querySelector('.preview-binary-name');
    const typeEl = this.querySelector('.preview-binary-type');
    const sizeEl = this.querySelector('.preview-binary-size');

    if (nameEl) nameEl.textContent = filename;
    if (typeEl) typeEl.textContent = contentType;
    if (sizeEl) sizeEl.textContent = formatSize(size);
  }

}

customElements.define('aeor-preview-default', AeorPreviewDefault);
export { AeorPreviewDefault };
