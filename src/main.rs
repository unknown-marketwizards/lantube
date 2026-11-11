mod error;
mod library;
mod upload;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use serde::Deserialize;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

const MAX_UPLOAD_SIZE: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
struct ServerConfig {
    addr: SocketAddr,
}

#[derive(Deserialize)]
struct Config {
    data: HashMap<String, PathBuf>,
    server: ServerConfig,
}

#[derive(Clone)]
struct AppState {
    data: HashMap<String, PathBuf>,
}

#[tokio::main]
async fn main() {
    let config_str = tokio::fs::read_to_string("config.toml").await.unwrap();
    let config: Config = toml::from_str(&config_str).unwrap();
    for name in config.data.keys() {
        assert!(
            library::is_normal_component(name),
            "Invalid media root: {name}"
        );
    }

    let state = AppState { data: config.data };
    let app = Router::new()
        .fallback_service(ServeFile::new("./frontend/dist/index.html"))
        .nest_service("/assets", ServeDir::new("./frontend/dist/assets"))
        .route_service(
            "/favicon.ico",
            ServeFile::new("./frontend/dist/favicon.ico"),
        )
        .route("/api/explorer", post(library::explorer))
        .route("/api/mkdir", post(library::mkdir))
        .route(
            "/api/upload",
            post(upload::upload).layer(DefaultBodyLimit::max(
                usize::try_from(MAX_UPLOAD_SIZE).unwrap_or(usize::MAX),
            )),
        )
        .route("/file/", any(|| async { StatusCode::NOT_FOUND }))
        .nest(
            "/file",
            Router::new()
                .route("/{root}/{*path}", get(library::serve_media))
                .fallback(|| async { StatusCode::NOT_FOUND }),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.server.addr)
        .await
        .unwrap();
    axum::serve(listener, app.layer(TraceLayer::new_for_http()))
        .await
        .unwrap();
}
