'use strict';

class AeorPreviewAudio extends HTMLElement {
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

    this.innerHTML = `<audio controls class="preview-media"><source src="${src}">Your browser does not support audio.</audio>`;
  }
}

customElements.define('aeor-preview-audio', AeorPreviewAudio);
export { AeorPreviewAudio };
