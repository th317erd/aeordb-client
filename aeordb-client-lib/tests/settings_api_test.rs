use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};
use serde::{Deserialize, Serialize};

fn test_config() -> ServerConfig {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let data_path = temp_dir.join("test-state.aeordb");
  let config_path = temp_dir.join("config.yaml");

  ServerConfig {
    host:        "127.0.0.1".to_string(),
    port:        0,
    config_path,
    data_path,
  }
}

#[derive(Debug, Deserialize)]
struct SettingsResponse {
  sync_interval_seconds: u64,
  auto_start_sync:       bool,
  client_name:           Option<String>,
  config_dir:            String,
  data_dir:              String,
}

#[derive(Debug, Serialize)]
struct UpdateSettings {
  #[serde(skip_serializing_if = "Option::is_none")]
  sync_interval_seconds: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  auto_start_sync:       Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  client_name:           Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
  error: String,
}

// ── GET /api/v1/settings ──

#[tokio::test]
async fn test_get_settings_returns_defaults() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/settings", address);
  let response = reqwest::get(&url).await.expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 60);
  assert!(body.auto_start_sync);
  // client_name defaults to hostname when not explicitly set
  assert!(body.client_name.is_some());
  assert!(!body.config_dir.is_empty());
  assert!(!body.data_dir.is_empty());
}

#[tokio::test]
async fn test_get_settings_includes_directory_paths() {
  let config = test_config();
  let expected_config_dir = config.config_path.parent().unwrap().to_string_lossy().to_string();
  let expected_data_dir   = config.data_path.parent().unwrap().to_string_lossy().to_string();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/settings", address);
  let response = reqwest::get(&url).await.expect("request failed");
  let body: SettingsResponse = response.json().await.expect("failed to parse");

  assert_eq!(body.config_dir, expected_config_dir);
  assert_eq!(body.data_dir, expected_data_dir);
}

// ── PATCH /api/v1/settings — happy paths ──

#[tokio::test]
async fn test_patch_settings_update_sync_interval() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(120),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 120);
  // Other fields unchanged.
  assert!(body.auto_start_sync);
  // client_name defaults to hostname when not explicitly set
  assert!(body.client_name.is_some());
}

#[tokio::test]
async fn test_patch_settings_update_auto_start() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: None,
      auto_start_sync:       Some(false),
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert!(!body.auto_start_sync);
  assert_eq!(body.sync_interval_seconds, 60);
}

#[tokio::test]
async fn test_patch_settings_update_client_name() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: None,
      auto_start_sync:       None,
      client_name:           Some("my-workstation".to_string()),
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.client_name, Some("my-workstation".to_string()));
}

#[tokio::test]
async fn test_patch_settings_clear_client_name() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  // First set a name.
  client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: None,
      auto_start_sync:       None,
      client_name:           Some("my-workstation".to_string()),
    })
    .send()
    .await
    .expect("request failed");

  // Now clear it.
  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: None,
      auto_start_sync:       None,
      client_name:           Some(String::new()),
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  // client_name defaults to hostname when not explicitly set
  assert!(body.client_name.is_some());
}

#[tokio::test]
async fn test_patch_settings_update_all_fields() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(300),
      auto_start_sync:       Some(false),
      client_name:           Some("test-box".to_string()),
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 300);
  assert!(!body.auto_start_sync);
  assert_eq!(body.client_name, Some("test-box".to_string()));
}

#[tokio::test]
async fn test_patch_settings_persists_across_get() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  // Update settings.
  client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(45),
      auto_start_sync:       Some(false),
      client_name:           Some("persistent-test".to_string()),
    })
    .send()
    .await
    .expect("request failed");

  // GET should reflect the updated values.
  let response = reqwest::get(&url).await.expect("request failed");
  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 45);
  assert!(!body.auto_start_sync);
  assert_eq!(body.client_name, Some("persistent-test".to_string()));
}

#[tokio::test]
async fn test_patch_settings_empty_body_is_noop() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&serde_json::json!({}))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  // Defaults unchanged.
  assert_eq!(body.sync_interval_seconds, 60);
  assert!(body.auto_start_sync);
  // client_name defaults to hostname when not explicitly set
  assert!(body.client_name.is_some());
}

// ── PATCH /api/v1/settings — validation / error paths ──

#[tokio::test]
async fn test_patch_settings_rejects_sync_interval_too_low() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(5),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 400);

  let body: ErrorResponse = response.json().await.expect("failed to parse");
  assert!(body.error.contains("between 10 and 3600"));
}

#[tokio::test]
async fn test_patch_settings_rejects_sync_interval_too_high() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(7200),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 400);

  let body: ErrorResponse = response.json().await.expect("failed to parse");
  assert!(body.error.contains("between 10 and 3600"));
}

#[tokio::test]
async fn test_patch_settings_boundary_min_interval() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(10),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 10);
}

#[tokio::test]
async fn test_patch_settings_boundary_max_interval() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(3600),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 3600);
}

#[tokio::test]
async fn test_patch_settings_invalid_json_returns_error() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .header("Content-Type", "application/json")
    .body("not valid json")
    .send()
    .await
    .expect("request failed");

  // Axum returns 400 for malformed JSON body.
  assert!(response.status().is_client_error());
}

#[tokio::test]
async fn test_patch_settings_wrong_content_type_returns_error() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.patch(&url)
    .header("Content-Type", "text/plain")
    .body("{}")
    .send()
    .await
    .expect("request failed");

  // Axum rejects non-JSON content types.
  assert!(response.status().is_client_error());
}

#[tokio::test]
async fn test_patch_settings_rejected_value_does_not_persist() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  // Try to set an invalid interval.
  let response = client.patch(&url)
    .json(&UpdateSettings {
      sync_interval_seconds: Some(1),
      auto_start_sync:       None,
      client_name:           None,
    })
    .send()
    .await
    .expect("request failed");
  assert_eq!(response.status(), 400);

  // GET should still have the default.
  let response = reqwest::get(&url).await.expect("request failed");
  let body: SettingsResponse = response.json().await.expect("failed to parse");
  assert_eq!(body.sync_interval_seconds, 60);
}

// ── Method not allowed ──

#[tokio::test]
async fn test_settings_post_returns_method_not_allowed() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.post(&url)
    .json(&serde_json::json!({}))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 405);
}

#[tokio::test]
async fn test_settings_delete_returns_method_not_allowed() {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let client = reqwest::Client::new();
  let url    = format!("http://{}/api/v1/settings", address);

  let response = client.delete(&url)
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 405);
}
