use anyhow::Result;

use super::{api_get, print_output};

pub async fn run(host: &str, json_mode: bool) -> Result<()> {
  let value = api_get(host, "/api/v1/status").await?;

  print_output(json_mode, &value, |v| {
    println!("Status:      {}", v["status"].as_str().unwrap_or("unknown"));
    println!("Version:     {}", v["version"].as_str().unwrap_or("unknown"));
    println!("Uptime:      {}s", v["uptime"].as_u64().unwrap_or(0));
    println!("Client ID:   {}", v["client_id"].as_str().unwrap_or("unknown"));
    println!("Client Name: {}", v["client_name"].as_str().unwrap_or("unknown"));
  });

  Ok(())
}
