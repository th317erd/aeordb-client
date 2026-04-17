use std::path::Path;

/// Detect MIME content type from a file extension.
pub fn mime_from_extension(path: &Path) -> Option<String> {
  let extension = path.extension()?.to_str()?;

  let mime = match extension.to_lowercase().as_str() {
    "json"             => "application/json",
    "txt"              => "text/plain",
    "md" | "markdown"  => "text/markdown",
    "html" | "htm"     => "text/html",
    "css"              => "text/css",
    "js" | "mjs"       => "application/javascript",
    "xml"              => "application/xml",
    "csv"              => "text/csv",
    "pdf"              => "application/pdf",
    "png"              => "image/png",
    "jpg" | "jpeg"     => "image/jpeg",
    "gif"              => "image/gif",
    "svg"              => "image/svg+xml",
    "webp"             => "image/webp",
    "zip"              => "application/zip",
    "tar"              => "application/x-tar",
    "gz"               => "application/gzip",
    "yaml" | "yml"     => "application/yaml",
    "toml"             => "application/toml",
    "rs"               => "text/x-rust",
    "py"               => "text/x-python",
    _                  => "application/octet-stream",
  };

  Some(mime.to_string())
}
