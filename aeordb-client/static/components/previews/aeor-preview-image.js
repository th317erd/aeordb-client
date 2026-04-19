'use strict';

class AeorPreviewImage extends HTMLElement {
  static get observedAttributes() {
    return ['src', 'filename'];
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
    const src = this.getAttribute('src') || '';
    const filename = this.getAttribute('filename') || '';

    this.innerHTML = `<img src="${src}" alt="${filename}" class="preview-image" loading="lazy">`;
  }
}

customElements.define('aeor-preview-image', AeorPreviewImage);
export { AeorPreviewImage };
