'use strict';

class AeorPreviewDefault extends HTMLElement {
  static get observedAttributes() {
    return ['src', 'filename', 'size', 'content-type'];
  }

  connectedCallback() {
    this.render();
  }

  attributeChangedCallback() {
    this.render();
  }

  load() {
    this.render();
  }

  render() {
    const filename = this.getAttribute('filename') || 'Unknown';
    const size = parseInt(this.getAttribute('size') || '0', 10);
    const contentType = this.getAttribute('content-type') || 'application/octet-stream';

    this.innerHTML = `
      <div class="preview-binary-info">
        <div class="preview-binary-icon">\uD83D\uDCC4</div>
        <div class="preview-binary-details">
          <div class="preview-binary-name">${filename}</div>
          <div class="preview-binary-meta">${contentType}</div>
          <div class="preview-binary-meta">${this._formatSize(size)}</div>
        </div>
      </div>
    `;
  }

  _formatSize(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB';
  }
}

customElements.define('aeor-preview-default', AeorPreviewDefault);
export { AeorPreviewDefault };
