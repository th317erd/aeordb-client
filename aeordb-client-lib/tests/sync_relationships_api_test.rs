use aeordb_client_lib::connections::{AuthType, CreateConnectionRequest, RemoteConnection};
use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};
use aeordb_client_lib::sync::relationships::{
  CreateSyncRelationshipRequest, SyncDirection, SyncRelationship,
  UpdateSyncRelationshipRequest, DeletePropagation,
};

struct TestContext {
  base_url:    String,
  client:      reqwest::Client,
  temp_dir:    std::path::PathBuf,
}

impl TestContext {
  async fn new() -> Self {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
    let database_path = temp_dir
      .join("test-state.aeordb")
      .to_string_lossy()
      .to_string();

    let config = ServerConfig {
      host:          "127.0.0.1".to_string(),
      port:          0,
      database_path,
    };

    let (address, _handle) = start_server_with_handle(config)
      .await
      .expect("failed to start server");

    Self {
      base_url: format!("http://{}", address),
      client:   reqwest::Client::new(),
      temp_dir,
    }
  }

  async fn create_connection(&self) -> RemoteConnection {
    let response = self.client
      .post(format!("{}/api/v1/connections", self.base_url))
      .json(&CreateConnectionRequest {
        name:      "Test Remote".to_string(),
        url:       "http://localhost:3000".to_string(),
        auth_type: AuthType::None,
        api_key:   None,
      })
      .send().await.expect("create connection failed");

    response.json().await.expect("parse failed")
  }

  fn local_sync_dir(&self) -> String {
    let sync_dir = self.temp_dir.join("sync-target");
    sync_dir.to_string_lossy().to_string()
  }
}

#[tokio::test]
async fn test_create_sync_relationship() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let request = CreateSyncRelationshipRequest {
    name:                 "My Docs".to_string(),
    remote_connection_id: connection.id.clone(),
    remote_path:          "/docs/".to_string(),
    local_path:           ctx.local_sync_dir(),
    direction:            SyncDirection::PullOnly,
    filter:               Some("*.pdf".to_string()),
    delete_propagation:   None,
  };

  let response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&request)
    .send().await.expect("request failed");

  assert_eq!(response.status(), 201);

  let relationship: SyncRelationship = response.json().await.expect("parse failed");
  assert_eq!(relationship.name, "My Docs");
  assert_eq!(relationship.remote_path, "/docs/");
  assert_eq!(relationship.direction, SyncDirection::PullOnly);
  assert_eq!(relationship.filter, Some("*.pdf".to_string()));
  assert!(relationship.enabled);
  assert!(!relationship.delete_propagation.local_to_remote);
  assert!(!relationship.delete_propagation.remote_to_local);
  uuid::Uuid::parse_str(&relationship.id).expect("id should be valid UUID");
}

#[tokio::test]
async fn test_create_relationship_normalizes_remote_path() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let request = CreateSyncRelationshipRequest {
    name:                 "Test".to_string(),
    remote_connection_id: connection.id.clone(),
    remote_path:          "docs".to_string(), // No slashes
    local_path:           ctx.local_sync_dir(),
    direction:            SyncDirection::Bidirectional,
    filter:               None,
    delete_propagation:   None,
  };

  let response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&request)
    .send().await.expect("request failed");

  let relationship: SyncRelationship = response.json().await.expect("parse failed");
  assert_eq!(relationship.remote_path, "/docs/");
}

#[tokio::test]
async fn test_create_relationship_invalid_connection() {
  let ctx = TestContext::new().await;

  let request = CreateSyncRelationshipRequest {
    name:                 "Test".to_string(),
    remote_connection_id: "nonexistent-connection-id".to_string(),
    remote_path:          "/docs/".to_string(),
    local_path:           ctx.local_sync_dir(),
    direction:            SyncDirection::PullOnly,
    filter:               None,
    delete_propagation:   None,
  };

  let response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&request)
    .send().await.expect("request failed");

  assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn test_create_relationship_creates_local_directory() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;
  let local_dir  = ctx.local_sync_dir();

  assert!(!std::path::Path::new(&local_dir).exists());

  let request = CreateSyncRelationshipRequest {
    name:                 "Test".to_string(),
    remote_connection_id: connection.id.clone(),
    remote_path:          "/docs/".to_string(),
    local_path:           local_dir.clone(),
    direction:            SyncDirection::PullOnly,
    filter:               None,
    delete_propagation:   None,
  };

  ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&request)
    .send().await.expect("request failed");

  assert!(std::path::Path::new(&local_dir).exists());
  assert!(std::path::Path::new(&local_dir).is_dir());
}

#[tokio::test]
async fn test_list_relationships_empty() {
  let ctx = TestContext::new().await;

  let response = reqwest::get(format!("{}/api/v1/sync", ctx.base_url))
    .await.expect("request failed");

  assert_eq!(response.status(), 200);

  let relationships: Vec<SyncRelationship> = response.json().await.expect("parse failed");
  assert!(relationships.is_empty());
}

#[tokio::test]
async fn test_list_relationships_sorted_by_name() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  for name in ["Zebra", "Alpha", "Mango"] {
    let local_dir = ctx.temp_dir.join(format!("sync-{}", name));

    ctx.client
      .post(format!("{}/api/v1/sync", ctx.base_url))
      .json(&CreateSyncRelationshipRequest {
        name:                 name.to_string(),
        remote_connection_id: connection.id.clone(),
        remote_path:          format!("/{}/", name.to_lowercase()),
        local_path:           local_dir.to_string_lossy().to_string(),
        direction:            SyncDirection::PullOnly,
        filter:               None,
        delete_propagation:   None,
      })
      .send().await.expect("create failed");
  }

  let response = reqwest::get(format!("{}/api/v1/sync", ctx.base_url))
    .await.expect("list failed");

  let relationships: Vec<SyncRelationship> = response.json().await.expect("parse failed");
  assert_eq!(relationships.len(), 3);
  assert_eq!(relationships[0].name, "Alpha");
  assert_eq!(relationships[1].name, "Mango");
  assert_eq!(relationships[2].name, "Zebra");
}

#[tokio::test]
async fn test_get_relationship() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let create_response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&CreateSyncRelationshipRequest {
      name:                 "Test".to_string(),
      remote_connection_id: connection.id.clone(),
      remote_path:          "/docs/".to_string(),
      local_path:           ctx.local_sync_dir(),
      direction:            SyncDirection::PullOnly,
      filter:               None,
      delete_propagation:   None,
    })
    .send().await.expect("create failed");

  let created: SyncRelationship = create_response.json().await.expect("parse failed");

  let get_response = reqwest::get(format!("{}/api/v1/sync/{}", ctx.base_url, created.id))
    .await.expect("get failed");

  assert_eq!(get_response.status(), 200);

  let fetched: SyncRelationship = get_response.json().await.expect("parse failed");
  assert_eq!(fetched.id, created.id);
}

#[tokio::test]
async fn test_get_relationship_not_found() {
  let ctx = TestContext::new().await;

  let response = reqwest::get(format!("{}/api/v1/sync/nonexistent", ctx.base_url))
    .await.expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_update_relationship() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let create_response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&CreateSyncRelationshipRequest {
      name:                 "Test".to_string(),
      remote_connection_id: connection.id.clone(),
      remote_path:          "/docs/".to_string(),
      local_path:           ctx.local_sync_dir(),
      direction:            SyncDirection::PullOnly,
      filter:               None,
      delete_propagation:   None,
    })
    .send().await.expect("create failed");

  let created: SyncRelationship = create_response.json().await.expect("parse failed");

  let update_response = ctx.client
    .patch(format!("{}/api/v1/sync/{}", ctx.base_url, created.id))
    .json(&UpdateSyncRelationshipRequest {
      name:               Some("Updated Name".to_string()),
      direction:          Some(SyncDirection::Bidirectional),
      filter:             Some("*.md".to_string()),
      delete_propagation: Some(DeletePropagation {
        local_to_remote: true,
        remote_to_local: false,
      }),
      enabled:            None,
    })
    .send().await.expect("update failed");

  assert_eq!(update_response.status(), 200);

  let updated: SyncRelationship = update_response.json().await.expect("parse failed");
  assert_eq!(updated.name, "Updated Name");
  assert_eq!(updated.direction, SyncDirection::Bidirectional);
  assert_eq!(updated.filter, Some("*.md".to_string()));
  assert!(updated.delete_propagation.local_to_remote);
  assert!(!updated.delete_propagation.remote_to_local);
}

#[tokio::test]
async fn test_delete_relationship() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let create_response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&CreateSyncRelationshipRequest {
      name:                 "Test".to_string(),
      remote_connection_id: connection.id.clone(),
      remote_path:          "/docs/".to_string(),
      local_path:           ctx.local_sync_dir(),
      direction:            SyncDirection::PullOnly,
      filter:               None,
      delete_propagation:   None,
    })
    .send().await.expect("create failed");

  let created: SyncRelationship = create_response.json().await.expect("parse failed");

  let delete_response = ctx.client
    .delete(format!("{}/api/v1/sync/{}", ctx.base_url, created.id))
    .send().await.expect("delete failed");

  assert_eq!(delete_response.status(), 204);

  let get_response = reqwest::get(format!("{}/api/v1/sync/{}", ctx.base_url, created.id))
    .await.expect("get failed");

  assert_eq!(get_response.status(), 404);
}

#[tokio::test]
async fn test_enable_disable_relationship() {
  let ctx        = TestContext::new().await;
  let connection = ctx.create_connection().await;

  let create_response = ctx.client
    .post(format!("{}/api/v1/sync", ctx.base_url))
    .json(&CreateSyncRelationshipRequest {
      name:                 "Test".to_string(),
      remote_connection_id: connection.id.clone(),
      remote_path:          "/docs/".to_string(),
      local_path:           ctx.local_sync_dir(),
      direction:            SyncDirection::PullOnly,
      filter:               None,
      delete_propagation:   None,
    })
    .send().await.expect("create failed");

  let created: SyncRelationship = create_response.json().await.expect("parse failed");
  assert!(created.enabled);

  // Disable
  let disable_response = ctx.client
    .post(format!("{}/api/v1/sync/{}/disable", ctx.base_url, created.id))
    .send().await.expect("disable failed");

  let disabled: SyncRelationship = disable_response.json().await.expect("parse failed");
  assert!(!disabled.enabled);

  // Enable
  let enable_response = ctx.client
    .post(format!("{}/api/v1/sync/{}/enable", ctx.base_url, created.id))
    .send().await.expect("enable failed");

  let enabled: SyncRelationship = enable_response.json().await.expect("parse failed");
  assert!(enabled.enabled);
}
