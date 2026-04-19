'use strict';

class AeorPreviewText extends HTMLElement {
  static get observedAttributes() {
    return ['src', 'filename', 'content-type'];
  }

  connectedCallback() {
    this.render();
  }

  attributeChangedCallback() {
    // Don't re-fetch on every attribute change — wait for load()
  }

  async load() {
    const src = this.getAttribute('src');
    if (!src) {
      this.innerHTML = '<div class="preview-binary">No source URL</div>';
      return;
    }

    this.innerHTML = '<div class="loading">Loading preview...</div>';

    try {
      const response = await fetch(src);
      const text = await response.text();
      const content = text.substring(0, 50000); // cap at 50K chars
      const filename = this.getAttribute('filename') || '';
      const contentType = this.getAttribute('content-type') || 'text/plain';

      if (this._isMarkdown(filename, contentType)) {
        this.innerHTML = `<div class="preview-markdown">${this._renderMarkdown(content)}</div>`;
      } else {
        this.innerHTML = `<pre class="preview-text"><code>${this._escapeHtml(content)}</code></pre>`;
      }
    } catch (error) {
      this.innerHTML = `<div class="preview-binary">Failed to load preview: ${error.message}</div>`;
    }
  }

  render() {
    this.innerHTML = '<div class="loading">Loading preview...</div>';
  }

  _isMarkdown(filename, contentType) {
    if (contentType === 'text/markdown') return true;
    const ext = filename.split('.').pop().toLowerCase();
    return ext === 'md' || ext === 'markdown';
  }

  _renderMarkdown(text) {
    // Simple markdown to HTML conversion
    // Handles: headers, bold, italic, code blocks, inline code, links, lists, paragraphs
    let html = this._escapeHtml(text);

    // Code blocks (``` ... ```)
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, '<pre class="preview-code-block"><code>$2</code></pre>');

    // Headers
    html = html.replace(/^######\s+(.+)$/gm, '<h6>$1</h6>');
    html = html.replace(/^#####\s+(.+)$/gm, '<h5>$1</h5>');
    html = html.replace(/^####\s+(.+)$/gm, '<h4>$1</h4>');
    html = html.replace(/^###\s+(.+)$/gm, '<h3>$1</h3>');
    html = html.replace(/^##\s+(.+)$/gm, '<h2>$1</h2>');
    html = html.replace(/^#\s+(.+)$/gm, '<h1>$1</h1>');

    // Bold and italic
    html = html.replace(/\*\*\*(.+?)\*\*\*/g, '<strong><em>$1</em></strong>');
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    html = html.replace(/\*(.+?)\*/g, '<em>$1</em>');

    // Inline code
    html = html.replace(/`([^`]+)`/g, '<code class="preview-inline-code">$1</code>');

    // Unordered lists
    html = html.replace(/^\s*[-*]\s+(.+)$/gm, '<li>$1</li>');
    html = html.replace(/(<li>.*<\/li>\n?)+/g, '<ul>$&</ul>');

    // Links [text](url) — but don't create actual clickable links for security
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<span class="preview-link">$1</span>');

    // Paragraphs (double newline)
    html = html.replace(/\n\n/g, '</p><p>');
    html = '<p>' + html + '</p>';

    // Clean up empty paragraphs
    html = html.replace(/<p>\s*<\/p>/g, '');

    return html;
  }

  _escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }
}

customElements.define('aeor-preview-text', AeorPreviewText);
export { AeorPreviewText };
