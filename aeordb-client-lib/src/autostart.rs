use std::path::PathBuf;

use crate::error::{ClientError, Result};

/// Install or remove system autostart for the aeordb-client.
/// Supports Linux (XDG autostart), macOS (LaunchAgents), and Windows (Startup folder).
pub fn set_autostart(enabled: bool) -> Result<()> {
  if enabled {
    install_autostart()
  } else {
    remove_autostart()
  }
}

/// Check if autostart is currently installed.
pub fn is_autostart_installed() -> bool {
  autostart_path().map(|p| p.exists()).unwrap_or(false)
}

fn autostart_path() -> Option<PathBuf> {
  #[cfg(target_os = "linux")]
  {
    dirs::config_dir().map(|d| d.join("autostart").join("aeordb-client.desktop"))
  }

  #[cfg(target_os = "macos")]
  {
    dirs::home_dir().map(|d| d.join("Library/LaunchAgents/com.aeor.aeordb-client.plist"))
  }

  #[cfg(target_os = "windows")]
  {
    dirs::data_dir().map(|d| {
      d.join("Microsoft\\Windows\\Start Menu\\Programs\\Startup\\aeordb-client.bat")
    })
  }

  #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
  {
    None
  }
}

fn current_exe_path() -> Result<String> {
  std::env::current_exe()
    .map(|p| p.to_string_lossy().to_string())
    .map_err(|e| ClientError::Configuration(format!("failed to get current exe path: {}", e)))
}

fn install_autostart() -> Result<()> {
  let path = autostart_path().ok_or_else(|| {
    ClientError::Configuration("autostart not supported on this platform".to_string())
  })?;

  let exe = current_exe_path()?;

  // Ensure parent directory exists
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| {
      ClientError::Configuration(format!("failed to create autostart directory: {}", e))
    })?;
  }

  let content = autostart_content(&exe);

  std::fs::write(&path, content).map_err(|e| {
    ClientError::Configuration(format!("failed to write autostart file {:?}: {}", path, e))
  })?;

  tracing::info!("autostart installed at {:?}", path);
  Ok(())
}

fn remove_autostart() -> Result<()> {
  let path = autostart_path().ok_or_else(|| {
    ClientError::Configuration("autostart not supported on this platform".to_string())
  })?;

  if path.exists() {
    std::fs::remove_file(&path).map_err(|e| {
      ClientError::Configuration(format!("failed to remove autostart file {:?}: {}", path, e))
    })?;
    tracing::info!("autostart removed from {:?}", path);
  }

  Ok(())
}

#[cfg(target_os = "linux")]
fn autostart_content(exe: &str) -> String {
  format!(
    "[Desktop Entry]\n\
     Type=Application\n\
     Name=AeorDB Client\n\
     Comment=Sync-first desktop client for AeorDB\n\
     Exec={} start --headless\n\
     Terminal=false\n\
     X-GNOME-Autostart-enabled=true\n",
    exe,
  )
}

#[cfg(target_os = "macos")]
fn autostart_content(exe: &str) -> String {
  format!(
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.aeor.aeordb-client</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>start</string>
    <string>--headless</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
</dict>
</plist>
"#,
    exe,
  )
}

#[cfg(target_os = "windows")]
fn autostart_content(exe: &str) -> String {
  format!("@echo off\r\nstart \"\" \"{}\" start --headless\r\n", exe)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn autostart_content(_exe: &str) -> String {
  String::new()
}
