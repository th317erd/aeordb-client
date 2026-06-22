use std::path::Path;

/// Detect MIME content type from a file extension.
pub fn mime_from_extension(path: &Path) -> Option<String> {
  let extension = path.extension()?.to_str()?;

  let mime = match extension.to_lowercase().as_str() {
    "json" => "application/json",
    "txt" => "text/plain",
    "md" | "markdown" => "text/markdown",
    "html" | "htm" => "text/html",
    "css" => "text/css",
    "js" | "mjs" => "application/javascript",
    "xml" => "application/xml",
    "csv" => "text/csv",
    "pdf" => "application/pdf",
    "png" => "image/png",
    "jpg" | "jpeg" => "image/jpeg",
    "gif" => "image/gif",
    "svg" => "image/svg+xml",
    "webp" => "image/webp",
    "mp4" | "m4v" => "video/mp4",
    "webm" => "video/webm",
    "ogv" | "ogg" => "video/ogg",
    "mov" => "video/quicktime",
    "avi" => "video/x-msvideo",
    "mkv" => "video/x-matroska",
    "mp3" => "audio/mpeg",
    "wav" => "audio/wav",
    "flac" => "audio/flac",
    "aac" => "audio/aac",
    "m4a" => "audio/mp4",
    "zip" => "application/zip",
    "tar" => "application/x-tar",
    "gz" => "application/gzip",
    "yaml" | "yml" => "application/yaml",
    "toml" => "application/toml",
    "rs" => "text/x-rust",
    "py" => "text/x-python",
    _ => "application/octet-stream",
  };

  Some(mime.to_string())
}

#[cfg(test)]
mod tests {
  use super::mime_from_extension;
  use std::path::Path;

  #[test]
  fn detects_streamable_media_types_from_extension() {
    assert_eq!(
      mime_from_extension(Path::new("movie.mp4")).as_deref(),
      Some("video/mp4"),
    );
    assert_eq!(
      mime_from_extension(Path::new("clip.webm")).as_deref(),
      Some("video/webm"),
    );
    assert_eq!(
      mime_from_extension(Path::new("song.mp3")).as_deref(),
      Some("audio/mpeg"),
    );
  }
}
