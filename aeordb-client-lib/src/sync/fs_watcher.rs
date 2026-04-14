use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// A coalesced filesystem change event.
#[derive(Debug, Clone)]
pub struct FsChange {
  pub path:       PathBuf,
  pub change_type: FsChangeType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsChangeType {
  Created,
  Modified,
  Deleted,
}

/// Configuration for the filesystem watcher.
pub struct FsWatcherConfig {
  /// Debounce quiet window (default: 100ms)
  pub debounce_ms: u64,
  /// Maximum wait before flushing (default: 500ms)
  pub max_wait_ms: u64,
}

impl Default for FsWatcherConfig {
  fn default() -> Self {
    Self {
      debounce_ms: 100,
      max_wait_ms: 500,
    }
  }
}

/// Start watching a directory for filesystem changes.
/// Returns a receiver that emits coalesced FsChange events.
pub fn start_fs_watcher(
  watch_path: &Path,
  config: FsWatcherConfig,
) -> Result<mpsc::Receiver<FsChange>, notify::Error> {
  let (raw_sender, mut raw_receiver) = mpsc::channel::<Event>(1024);
  let (output_sender, output_receiver) = mpsc::channel::<FsChange>(256);

  // Start the native filesystem watcher
  let mut watcher = RecommendedWatcher::new(
    move |result: Result<Event, notify::Error>| {
      if let Ok(event) = result {
        let _ = raw_sender.blocking_send(event);
      }
    },
    notify::Config::default(),
  )?;

  watcher.watch(watch_path, RecursiveMode::Recursive)?;

  // Spawn the coalescing task
  let debounce   = Duration::from_millis(config.debounce_ms);
  let max_wait   = Duration::from_millis(config.max_wait_ms);

  tokio::spawn(async move {
    // Keep the watcher alive — dropping it stops watching
    let _watcher = watcher;

    let mut pending: HashMap<PathBuf, (FsChangeType, Instant)> = HashMap::new();
    let mut _last_flush = Instant::now();

    loop {
      // Wait for events with a timeout
      let timeout = if pending.is_empty() {
        Duration::from_secs(60) // Idle — just keep alive
      } else {
        debounce
      };

      match tokio::time::timeout(timeout, raw_receiver.recv()).await {
        Ok(Some(event)) => {
          // Map notify events to our change types
          for event_path in event.paths {
            let change_type = match event.kind {
              EventKind::Create(_) => FsChangeType::Created,
              EventKind::Modify(_) => FsChangeType::Modified,
              EventKind::Remove(_) => FsChangeType::Deleted,
              _ => continue,
            };

            // Skip directories — we only care about files
            if event_path.is_dir() {
              continue;
            }

            pending.insert(event_path, (change_type, Instant::now()));
          }
        }
        Ok(None) => {
          // Channel closed — watcher dropped
          break;
        }
        Err(_) => {
          // Timeout — check if we should flush
        }
      }

      // Flush logic: flush if debounce window expired or max wait exceeded
      if !pending.is_empty() {
        let now          = Instant::now();
        let oldest_event = pending.values().map(|(_, t)| *t).min().unwrap_or(now);
        let newest_event = pending.values().map(|(_, t)| *t).max().unwrap_or(now);

        let debounce_expired = now.duration_since(newest_event) >= debounce;
        let max_wait_hit     = now.duration_since(oldest_event) >= max_wait;

        if debounce_expired || max_wait_hit {
          for (path, (change_type, _)) in pending.drain() {
            let change = FsChange { path, change_type };
            if output_sender.send(change).await.is_err() {
              return; // Receiver dropped
            }
          }
          _last_flush = now;
        }
      }
    }
  });

  Ok(output_receiver)
}
