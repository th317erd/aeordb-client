use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Response, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
struct StaticAssets;

pub fn static_routes() -> Router {
  Router::new()
    .route("/", get(serve_index))
    .route("/static/{*path}", get(serve_static))
}

async fn serve_index() -> impl IntoResponse {
  match StaticAssets::get("index.html") {
    Some(content) => Response::builder()
      .status(StatusCode::OK)
      .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
      .body(Body::from(content.data.to_vec()))
      .unwrap(),
    None => Response::builder()
      .status(StatusCode::NOT_FOUND)
      .body(Body::from("index.html not found"))
      .unwrap(),
  }
}

async fn serve_static(Path(path): Path<String>) -> impl IntoResponse {
  match StaticAssets::get(&path) {
    Some(content) => {
      let mime = mime_from_path(&path);

      Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(content.data.to_vec()))
        .unwrap()
    }
    None => Response::builder()
      .status(StatusCode::NOT_FOUND)
      .body(Body::from("not found"))
      .unwrap(),
  }
}

fn mime_from_path(path: &str) -> &'static str {
  if path.ends_with(".html")      { return "text/html; charset=utf-8"; }
  if path.ends_with(".css")       { return "text/css; charset=utf-8"; }
  if path.ends_with(".js")        { return "application/javascript; charset=utf-8"; }
  if path.ends_with(".mjs")       { return "application/javascript; charset=utf-8"; }
  if path.ends_with(".json")      { return "application/json"; }
  if path.ends_with(".svg")       { return "image/svg+xml"; }
  if path.ends_with(".png")       { return "image/png"; }
  if path.ends_with(".ico")       { return "image/x-icon"; }
  if path.ends_with(".woff2")     { return "font/woff2"; }
  if path.ends_with(".woff")      { return "font/woff"; }
  "application/octet-stream"
}
