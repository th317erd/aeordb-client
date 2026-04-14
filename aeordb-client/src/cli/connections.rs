use anyhow::Result;

use super::{api_delete, api_get, api_post, print_output};

pub async fn list(host: &str, json_mode: bool) -> Result<()> {
  let value = api_get(host, "/api/v1/connections").await?;

  print_output(json_mode, &value, |v| {
    let empty = vec![];
    let connections = v.as_array().unwrap_or(&empty);

    if connections.is_empty() {
      println!("No connections configured.");
      return;
    }

    println!("{:<38} {:<20} {:<40} {}", "ID", "NAME", "URL", "AUTH");
    println!("{}", "-".repeat(105));

    for connection in connections {
      println!(
        "{:<38} {:<20} {:<40} {}",
        connection["id"].as_str().unwrap_or(""),
        connection["name"].as_str().unwrap_or(""),
        connection["url"].as_str().unwrap_or(""),
        connection["auth_type"].as_str().unwrap_or("none"),
      );
    }
  });

  Ok(())
}

pub async fn add(host: &str, json_mode: bool, name: &str, url: &str, api_key: Option<&str>) -> Result<()> {
  let auth_type = if api_key.is_some() { "api_key" } else { "none" };

  let body = serde_json::json!({
    "name":      name,
    "url":       url,
    "auth_type": auth_type,
    "api_key":   api_key,
  });

  let value = api_post(host, "/api/v1/connections", &body).await?;

  print_output(json_mode, &value, |v| {
    println!("Created connection:");
    println!("  ID:   {}", v["id"].as_str().unwrap_or(""));
    println!("  Name: {}", v["name"].as_str().unwrap_or(""));
    println!("  URL:  {}", v["url"].as_str().unwrap_or(""));
  });

  Ok(())
}

pub async fn remove(host: &str, id: &str) -> Result<()> {
  api_delete(host, &format!("/api/v1/connections/{}", id)).await?;
  println!("Connection {} deleted.", id);
  Ok(())
}

pub async fn test(host: &str, json_mode: bool, id: &str) -> Result<()> {
  let value = api_post(host, &format!("/api/v1/connections/{}/test", id), &serde_json::json!({})).await?;

  print_output(json_mode, &value, |v| {
    let success = v["success"].as_bool().unwrap_or(false);
    let message = v["message"].as_str().unwrap_or("unknown");

    if success {
      print!("OK — {}", message);
      if let Some(latency) = v["latency_ms"].as_u64() {
        print!(" ({}ms)", latency);
      }
      println!();
    } else {
      println!("FAILED — {}", message);
    }
  });

  Ok(())
}
