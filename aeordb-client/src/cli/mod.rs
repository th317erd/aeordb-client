pub mod connections;
pub mod status;
pub mod sync;

use anyhow::Result;

/// Helper: make a GET request and return the JSON body.
pub async fn api_get(host: &str, path: &str) -> Result<serde_json::Value> {
  let url      = format!("{}{}", host, path);
  let response = reqwest::get(&url).await?;

  if !response.status().is_success() {
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let error_message = body["error"].as_str().unwrap_or("unknown error");
    anyhow::bail!("HTTP {} — {}", status, error_message);
  }

  Ok(response.json().await?)
}

/// Helper: make a POST request with JSON body.
pub async fn api_post(host: &str, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
  let url      = format!("{}{}", host, path);
  let client   = reqwest::Client::new();
  let response = client.post(&url).json(body).send().await?;

  if !response.status().is_success() {
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let error_message = body["error"].as_str().unwrap_or("unknown error");
    anyhow::bail!("HTTP {} — {}", status, error_message);
  }

  Ok(response.json().await?)
}

/// Helper: make a DELETE request.
pub async fn api_delete(host: &str, path: &str) -> Result<()> {
  let url      = format!("{}{}", host, path);
  let client   = reqwest::Client::new();
  let response = client.delete(&url).send().await?;

  if !response.status().is_success() {
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let error_message = body["error"].as_str().unwrap_or("unknown error");
    anyhow::bail!("HTTP {} — {}", status, error_message);
  }

  Ok(())
}

/// Helper: print JSON or formatted output.
pub fn print_output(json_mode: bool, value: &serde_json::Value, formatter: impl FnOnce(&serde_json::Value)) {
  if json_mode {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
  } else {
    formatter(value);
  }
}
