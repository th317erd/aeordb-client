use aeordb_client_lib::models::status::StatusResponse;
use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};

fn test_config() -> ServerConfig {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let data_path = temp_dir.join("test-state.aeordb");
  let config_path = temp_dir.join("config.yaml");

  ServerConfig {
    host:        "127.0.0.1".to_string(),
    port:        0, // OS assigns a free port
    config_path,
    data_path,
  }
}

#[tokio::test]
async fn test_status_endpoint_returns_running() {
  let config = test_config();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/status", address);
  let response = reqwest::get(&url)
    .await
    .expect("failed to send request");

  assert_eq!(response.status(), 200);

  let body: StatusResponse = response
    .json()
    .await
    .expect("failed to parse response body");

  assert_eq!(body.status, "running");
  assert!(!body.version.is_empty());
}

#[tokio::test]
async fn test_status_endpoint_includes_client_identity() {
  let config = test_config();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/status", address);
  let response = reqwest::get(&url)
    .await
    .expect("failed to send request");

  let body: StatusResponse = response
    .json()
    .await
    .expect("failed to parse response body");

  assert!(body.client_id.is_some(), "client_id should be present");
  assert!(body.client_name.is_some(), "client_name should be present");

  let client_id = body.client_id.unwrap();
  assert!(!client_id.is_empty(), "client_id should not be empty");

  // Verify it's a valid UUID
  uuid::Uuid::parse_str(&client_id).expect("client_id should be a valid UUID");
}

#[tokio::test]
async fn test_status_endpoint_uptime_is_non_negative() {
  let config = test_config();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/status", address);
  let response = reqwest::get(&url)
    .await
    .expect("failed to send request");

  let body: StatusResponse = response
    .json()
    .await
    .expect("failed to parse response body");

  // Uptime should be 0 or very small since we just started
  assert!(body.uptime < 5);
}

#[tokio::test]
async fn test_status_endpoint_includes_directory_paths() {
  let config = test_config();
  let expected_config_dir = config.config_path.parent().unwrap().to_string_lossy().to_string();
  let expected_data_dir   = config.data_path.parent().unwrap().to_string_lossy().to_string();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/status", address);
  let response = reqwest::get(&url)
    .await
    .expect("failed to send request");

  let body: StatusResponse = response
    .json()
    .await
    .expect("failed to parse response body");

  assert!(body.config_dir.is_some(), "config_dir should be present");
  assert!(body.data_dir.is_some(), "data_dir should be present");
  assert_eq!(body.config_dir.unwrap(), expected_config_dir);
  assert_eq!(body.data_dir.unwrap(), expected_data_dir);
}

#[tokio::test]
async fn test_unknown_route_returns_404() {
  let config = test_config();

  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");

  let url      = format!("http://{}/api/v1/nonexistent", address);
  let response = reqwest::get(&url)
    .await
    .expect("failed to send request");

  assert_eq!(response.status(), 404);
}
