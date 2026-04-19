'use strict';

class AeorPreviewVideo extends HTMLElement {
  connectedCallback() {
    this.innerHTML = '<video controls class="preview-media"></video>';
  }

  load() {
    let video = this.querySelector('video');
    if (!video) {
      this.innerHTML = '<video controls class="preview-media"></video>';
      video = this.querySelector('video');
    }
    video.src = this.getAttribute('src') || '';
    video.load();
  }
}

customElements.define('aeor-preview-video', AeorPreviewVideo);
export { AeorPreviewVideo };
