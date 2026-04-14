use aeordb_client_lib::connections::{
  AuthType, CreateConnectionRequest, RemoteConnection, UpdateConnectionRequest,
  ConnectionTestResult,
};
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
    port:          0,
    database_path,
    auth_token:    None,
  }
}

async fn start_test_server() -> String {
  let config = test_config();
  let (address, _handle) = start_server_with_handle(config)
    .await
    .expect("failed to start server");
  format!("http://{}", address)
}

fn create_request() -> CreateConnectionRequest {
  CreateConnectionRequest {
    name:      "Test Server".to_string(),
    url:       "http://localhost:3000".to_string(),
    auth_type: AuthType::ApiKey,
    api_key:   Some("aeor_test_key_abc123".to_string()),
  }
}

#[tokio::test]
async fn test_create_connection() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&create_request())
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 201);

  let connection: RemoteConnection = response.json().await.expect("parse failed");
  assert_eq!(connection.name, "Test Server");
  assert_eq!(connection.url, "http://localhost:3000");
  assert_eq!(connection.auth_type, AuthType::ApiKey);
  assert!(connection.api_key.is_some());
  uuid::Uuid::parse_str(&connection.id).expect("id should be valid UUID");
}

#[tokio::test]
async fn test_create_connection_strips_trailing_slash() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let mut request = create_request();
  request.url = "http://localhost:3000/".to_string();

  let response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&request)
    .send()
    .await
    .expect("request failed");

  let connection: RemoteConnection = response.json().await.expect("parse failed");
  assert_eq!(connection.url, "http://localhost:3000");
}

#[tokio::test]
async fn test_list_connections_empty() {
  let base_url = start_test_server().await;

  let response = reqwest::get(format!("{}/api/v1/connections", base_url))
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let connections: Vec<RemoteConnection> = response.json().await.expect("parse failed");
  assert!(connections.is_empty());
}

#[tokio::test]
async fn test_list_connections_after_create() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  // Create two connections
  client.post(format!("{}/api/v1/connections", base_url))
    .json(&CreateConnectionRequest {
      name: "Server B".to_string(),
      url: "http://b.example.com".to_string(),
      auth_type: AuthType::None,
      api_key: None,
    })
    .send().await.expect("create failed");

  client.post(format!("{}/api/v1/connections", base_url))
    .json(&CreateConnectionRequest {
      name: "Server A".to_string(),
      url: "http://a.example.com".to_string(),
      auth_type: AuthType::None,
      api_key: None,
    })
    .send().await.expect("create failed");

  let response = reqwest::get(format!("{}/api/v1/connections", base_url))
    .await.expect("list failed");

  let connections: Vec<RemoteConnection> = response.json().await.expect("parse failed");
  assert_eq!(connections.len(), 2);

  // Should be sorted by name
  assert_eq!(connections[0].name, "Server A");
  assert_eq!(connections[1].name, "Server B");
}

#[tokio::test]
async fn test_get_connection() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let create_response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&create_request())
    .send().await.expect("create failed");

  let created: RemoteConnection = create_response.json().await.expect("parse failed");

  let get_response = reqwest::get(format!("{}/api/v1/connections/{}", base_url, created.id))
    .await.expect("get failed");

  assert_eq!(get_response.status(), 200);

  let fetched: RemoteConnection = get_response.json().await.expect("parse failed");
  assert_eq!(fetched.id, created.id);
  assert_eq!(fetched.name, created.name);
}

#[tokio::test]
async fn test_get_connection_not_found() {
  let base_url = start_test_server().await;

  let response = reqwest::get(format!("{}/api/v1/connections/nonexistent-id", base_url))
    .await.expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_update_connection() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let create_response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&create_request())
    .send().await.expect("create failed");

  let created: RemoteConnection = create_response.json().await.expect("parse failed");

  let update = UpdateConnectionRequest {
    name:      Some("Updated Name".to_string()),
    url:       Some("http://new-url.example.com".to_string()),
    auth_type: None,
    api_key:   None,
  };

  let update_response = client
    .patch(format!("{}/api/v1/connections/{}", base_url, created.id))
    .json(&update)
    .send().await.expect("update failed");

  assert_eq!(update_response.status(), 200);

  let updated: RemoteConnection = update_response.json().await.expect("parse failed");
  assert_eq!(updated.name, "Updated Name");
  assert_eq!(updated.url, "http://new-url.example.com");
  // Auth type should be unchanged
  assert_eq!(updated.auth_type, AuthType::ApiKey);
}

#[tokio::test]
async fn test_delete_connection() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let create_response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&create_request())
    .send().await.expect("create failed");

  let created: RemoteConnection = create_response.json().await.expect("parse failed");

  let delete_response = client
    .delete(format!("{}/api/v1/connections/{}", base_url, created.id))
    .send().await.expect("delete failed");

  assert_eq!(delete_response.status(), 204);

  // Verify it's gone
  let get_response = reqwest::get(format!("{}/api/v1/connections/{}", base_url, created.id))
    .await.expect("get failed");

  assert_eq!(get_response.status(), 404);
}

#[tokio::test]
async fn test_delete_connection_not_found() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  let response = client
    .delete(format!("{}/api/v1/connections/nonexistent-id", base_url))
    .send().await.expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_connection_test_unreachable_server() {
  let base_url = start_test_server().await;
  let client   = reqwest::Client::new();

  // Create a connection pointing to a non-existent server
  let create_response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&CreateConnectionRequest {
      name:      "Unreachable".to_string(),
      url:       "http://192.0.2.1:9999".to_string(), // RFC 5737 TEST-NET, guaranteed unreachable
      auth_type: AuthType::None,
      api_key:   None,
    })
    .send().await.expect("create failed");

  let created: RemoteConnection = create_response.json().await.expect("parse failed");

  let test_response = client
    .post(format!("{}/api/v1/connections/{}/test", base_url, created.id))
    .send().await.expect("test failed");

  assert_eq!(test_response.status(), 200);

  let result: ConnectionTestResult = test_response.json().await.expect("parse failed");
  assert!(!result.success);
}
