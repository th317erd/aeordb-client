use aeordb_client_lib::models::status::StatusResponse;
use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};

fn test_config() -> ServerConfig {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  let database_path = temp_dir
    .keep()
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  ServerConfig {
    host:          "127.0.0.1".to_string(),
    port:          0, // OS assigns a free port
    database_path,
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
