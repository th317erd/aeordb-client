use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Response, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
#[include = "index.html"]
#[include = "**/*.js"]
#[include = "**/*.mjs"]
#[include = "**/*.css"]
#[include = "**/*.svg"]
#[include = "**/*.png"]
#[include = "**/*.ico"]
#[include = "**/*.woff"]
#[include = "**/*.woff2"]
#[include = "**/*.ttf"]
struct StaticAssets;

pub fn static_routes() -> Router {
  Router::new()
    .route("/", get(serve_index))
    .route("/static/{*path}", get(serve_static))
}

async fn serve_index() -> impl IntoResponse {
  if let Some(bytes) = read_asset("index.html").await {
    return Response::builder()
      .status(StatusCode::OK)
      .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
      .body(Body::from(bytes))
      .unwrap();
  }
  Response::builder()
    .status(StatusCode::NOT_FOUND)
    .body(Body::from("index.html not found"))
    .unwrap()
}

async fn serve_static(Path(path): Path<String>) -> impl IntoResponse {
  if let Some(bytes) = read_asset(&path).await {
    return Response::builder()
      .status(StatusCode::OK)
      .header(header::CONTENT_TYPE, mime_from_path(&path))
      .header(header::CACHE_CONTROL, "no-cache")
      .body(Body::from(bytes))
      .unwrap();
  }
  Response::builder()
    .status(StatusCode::NOT_FOUND)
    .body(Body::from("not found"))
    .unwrap()
}

// Debug builds read from disk so edits to symlinked source repos (static/aeor,
// static/shared) go live without rebuild. rust-embed's debug-mode runtime
// rejects files inside symlinked subdirectories because the canonical path
// escapes its canonical folder root. Release uses the compiled-in embed.
#[cfg(debug_assertions)]
async fn read_asset(path: &str) -> Option<Vec<u8>> {
  const DEV_STATIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/static");
  let full = std::path::Path::new(DEV_STATIC_DIR).join(path);
  tokio::fs::read(&full).await.ok()
}

#[cfg(not(debug_assertions))]
async fn read_asset(path: &str) -> Option<Vec<u8>> {
  StaticAssets::get(path).map(|f| f.data.to_vec())
}

fn mime_from_path(path: &str) -> &'static str {
  if path.ends_with(".html") {
    return "text/html; charset=utf-8";
  }
  if path.ends_with(".css") {
    return "text/css; charset=utf-8";
  }
  if path.ends_with(".js") {
    return "application/javascript; charset=utf-8";
  }
  if path.ends_with(".mjs") {
    return "application/javascript; charset=utf-8";
  }
  if path.ends_with(".json") {
    return "application/json";
  }
  if path.ends_with(".svg") {
    return "image/svg+xml";
  }
  if path.ends_with(".png") {
    return "image/png";
  }
  if path.ends_with(".ico") {
    return "image/x-icon";
  }
  if path.ends_with(".woff2") {
    return "font/woff2";
  }
  if path.ends_with(".woff") {
    return "font/woff";
  }
  "application/octet-stream"
}
