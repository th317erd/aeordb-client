'use strict';

class AeorPreviewImage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = '<img class="preview-image" loading="lazy">';
  }

  load() {
    let img = this.querySelector('img');
    if (!img) {
      this.innerHTML = '<img class="preview-image" loading="lazy">';
      img = this.querySelector('img');
    }
    img.src = this.getAttribute('src') || '';
    img.alt = this.getAttribute('filename') || '';
  }
}

customElements.define('aeor-preview-image', AeorPreviewImage);
export { AeorPreviewImage };
