'use strict';

class AeorPreviewAudio extends HTMLElement {
  connectedCallback() {
    this.innerHTML = '<audio controls class="preview-media"></audio>';
  }

  load() {
    let audio = this.querySelector('audio');
    if (!audio) {
      this.innerHTML = '<audio controls class="preview-media"></audio>';
      audio = this.querySelector('audio');
    }
    audio.src = this.getAttribute('src') || '';
    audio.load();
  }
}

customElements.define('aeor-preview-audio', AeorPreviewAudio);
export { AeorPreviewAudio };
