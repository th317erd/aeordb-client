use anyhow::Result;

use super::{api_delete, api_get, api_post, print_output};

pub async fn list(host: &str, json_mode: bool) -> Result<()> {
  let value = api_get(host, "/api/v1/sync").await?;

  print_output(json_mode, &value, |v| {
    let empty = vec![];
    let relationships = v.as_array().unwrap_or(&empty);

    if relationships.is_empty() {
      println!("No sync relationships configured.");
      return;
    }

    println!("{:<38} {:<15} {:<20} {:<12} {}", "ID", "NAME", "REMOTE PATH", "DIRECTION", "ENABLED");
    println!("{}", "-".repeat(100));

    for relationship in relationships {
      println!(
        "{:<38} {:<15} {:<20} {:<12} {}",
        relationship["id"].as_str().unwrap_or(""),
        relationship["name"].as_str().unwrap_or(""),
        relationship["remote_path"].as_str().unwrap_or(""),
        relationship["direction"].as_str().unwrap_or(""),
        if relationship["enabled"].as_bool().unwrap_or(false) { "yes" } else { "no" },
      );
    }
  });

  Ok(())
}

pub async fn add(
  host: &str,
  json_mode: bool,
  name: &str,
  connection_id: &str,
  remote_path: &str,
  local_path: &str,
  direction: &str,
  filter: Option<&str>,
) -> Result<()> {
  let direction_value = match direction {
    "bidirectional" | "bi" => "bidirectional",
    "pull-only" | "pull"   => "pull_only",
    "push-only" | "push"   => "push_only",
    other => anyhow::bail!("invalid direction '{}' — use bidirectional, pull-only, or push-only", other),
  };

  let body = serde_json::json!({
    "name":                 name,
    "remote_connection_id": connection_id,
    "remote_path":          remote_path,
    "local_path":           local_path,
    "direction":            direction_value,
    "filter":               filter,
  });

  let value = api_post(host, "/api/v1/sync", &body).await?;

  print_output(json_mode, &value, |v| {
    println!("Created sync relationship:");
    println!("  ID:          {}", v["id"].as_str().unwrap_or(""));
    println!("  Name:        {}", v["name"].as_str().unwrap_or(""));
    println!("  Remote:      {}", v["remote_path"].as_str().unwrap_or(""));
    println!("  Local:       {}", v["local_path"].as_str().unwrap_or(""));
    println!("  Direction:   {}", v["direction"].as_str().unwrap_or(""));
  });

  Ok(())
}

pub async fn remove(host: &str, id: &str) -> Result<()> {
  api_delete(host, &format!("/api/v1/sync/{}", id)).await?;
  println!("Sync relationship {} deleted.", id);
  Ok(())
}

pub async fn status(host: &str, json_mode: bool, id: Option<&str>) -> Result<()> {
  match id {
    Some(id) => {
      let value = api_get(host, &format!("/api/v1/sync/{}", id)).await?;

      print_output(json_mode, &value, |v| {
        println!("Sync Relationship: {}", v["name"].as_str().unwrap_or("unknown"));
        println!("  ID:        {}", v["id"].as_str().unwrap_or(""));
        println!("  Remote:    {}", v["remote_path"].as_str().unwrap_or(""));
        println!("  Local:     {}", v["local_path"].as_str().unwrap_or(""));
        println!("  Direction: {}", v["direction"].as_str().unwrap_or(""));
        println!("  Enabled:   {}", v["enabled"].as_bool().unwrap_or(false));
        if let Some(filter) = v["filter"].as_str() {
          println!("  Filter:    {}", filter);
        }
      });
    }
    None => {
      // List all with status
      list(host, json_mode).await?;
    }
  }

  Ok(())
}

pub async fn trigger(host: &str, json_mode: bool, id: &str) -> Result<()> {
  println!("Triggering sync for {}...", id);

  let value = api_post(host, &format!("/api/v1/sync/{}/trigger", id), &serde_json::json!({})).await?;

  print_output(json_mode, &value, |v| {
    let downloaded = v["files_downloaded"].as_u64().unwrap_or(0);
    let skipped    = v["files_skipped"].as_u64().unwrap_or(0);
    let failed     = v["files_failed"].as_u64().unwrap_or(0);
    let bytes      = v["total_bytes"].as_u64().unwrap_or(0);
    let duration   = v["duration_ms"].as_u64().unwrap_or(0);

    println!("Sync complete:");
    println!("  Downloaded: {} files ({} bytes)", downloaded, bytes);
    println!("  Skipped:    {} files (unchanged)", skipped);
    println!("  Failed:     {} files", failed);
    println!("  Duration:   {}ms", duration);

    if let Some(errors) = v["errors"].as_array() {
      if !errors.is_empty() {
        println!("  Errors:");
        for error in errors {
          println!("    - {}", error.as_str().unwrap_or("unknown"));
        }
      }
    }
  });

  Ok(())
}

pub async fn pause(host: &str, id: Option<&str>) -> Result<()> {
  match id {
    Some(id) => {
      api_post(host, &format!("/api/v1/sync/{}/disable", id), &serde_json::json!({})).await?;
      println!("Sync {} paused.", id);
    }
    None => {
      // Pause all
      let relationships = api_get(host, "/api/v1/sync").await?;
      let empty_items = vec![];
      let items = relationships.as_array().unwrap_or(&empty_items);

      for relationship in items {
        if let Some(relationship_id) = relationship["id"].as_str() {
          if relationship["enabled"].as_bool().unwrap_or(false) {
            api_post(host, &format!("/api/v1/sync/{}/disable", relationship_id), &serde_json::json!({})).await?;
            println!("Paused: {}", relationship["name"].as_str().unwrap_or(relationship_id));
          }
        }
      }
    }
  }

  Ok(())
}

pub async fn resume(host: &str, id: Option<&str>) -> Result<()> {
  match id {
    Some(id) => {
      api_post(host, &format!("/api/v1/sync/{}/enable", id), &serde_json::json!({})).await?;
      println!("Sync {} resumed.", id);
    }
    None => {
      // Resume all
      let relationships = api_get(host, "/api/v1/sync").await?;
      let empty_items = vec![];
      let items = relationships.as_array().unwrap_or(&empty_items);

      for relationship in items {
        if let Some(relationship_id) = relationship["id"].as_str() {
          if !relationship["enabled"].as_bool().unwrap_or(true) {
            api_post(host, &format!("/api/v1/sync/{}/enable", relationship_id), &serde_json::json!({})).await?;
            println!("Resumed: {}", relationship["name"].as_str().unwrap_or(relationship_id));
          }
        }
      }
    }
  }

  Ok(())
}
