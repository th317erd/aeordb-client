'use strict';

class AeorPreviewVideo extends HTMLElement {
  static get observedAttributes() {
    return ['src'];
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

    this.innerHTML = `<video controls class="preview-media"><source src="${src}">Your browser does not support video.</video>`;
  }
}

customElements.define('aeor-preview-video', AeorPreviewVideo);
export { AeorPreviewVideo };
