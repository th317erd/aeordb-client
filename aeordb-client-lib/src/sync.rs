use std::path::Path;

use crate::error::{ClientError, Result};

pub mod activity;
pub mod content_type;
pub mod filter;
pub mod fs_watcher;
pub mod hierarchy;
pub mod metadata;
pub mod pull;
pub mod push;
pub mod relationships;
pub mod replication;
pub mod runner;
pub mod sse_listener;

/// Get the file modification time as milliseconds since the Unix epoch.
pub(crate) fn file_mtime(path: &Path) -> Result<i64> {
  let metadata = path.metadata()?;
  let modified = metadata.modified()?;
  let duration = modified
    .duration_since(std::time::UNIX_EPOCH)
    .map_err(|error| ClientError::Io(
      std::io::Error::new(std::io::ErrorKind::Other, format!("system time error: {}", error)),
    ))?;

  Ok(duration.as_millis() as i64)
}
