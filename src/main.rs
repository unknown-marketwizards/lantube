mod error;

use crate::error::AppError;

use anyhow::{anyhow, Result};
use axum::extract::{DefaultBodyLimit, State};
use axum::{extract::Multipart, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Deserialize)]
struct ServerConfig {
    addr: SocketAddr,
}

#[derive(Deserialize)]
struct Config {
    data: HashMap<String, PathBuf>,

    server: ServerConfig,
}

#[derive(Default, Clone)]
struct AppState {
    data: HashMap<String, PathBuf>,
}

#[tokio::main]
async fn main() {
    let config_str = fs::read_to_string("config.toml").await.unwrap();

    let config: Config = toml::from_str(&config_str).unwrap();

    let addr = config.server.addr;

    let state = AppState {
        data: config.data.clone(),
    };

    let mut router = Router::new()
        .nest_service("/", ServeFile::new("./frontend/dist/index.html"))
        .nest_service("/assets", ServeDir::new("./frontend/dist/assets"))
        .route("/api/explorer", post(explorer))
        .route("/api/mkdir", post(mkdir))
        .route("/api/upload", post(upload));

    for (name, path) in config.data {
        router = router.nest_service(format!("/file/{}", name).as_str(), ServeDir::new(path));
    }

    let app = router.layer(DefaultBodyLimit::disable()).with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    axum::serve(listener, app.layer(TraceLayer::new_for_http()))
        .await
        .unwrap();
}

#[derive(Serialize)]
struct ExplorerItem {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Deserialize)]
struct PathParams {
    path: String,
}

fn get_path(data: &HashMap<String, PathBuf>, path_str: String) -> Result<PathBuf> {
    let path = if path_str.starts_with("/") {
        &path_str[1..]
    } else {
        path_str.as_str()
    };

    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if parts.len() == 0 {
        return Err(anyhow!("Invalid path: {}", path_str));
    }

    let data_dir = data
        .get(parts[0])
        .ok_or(anyhow!("name {} not exist", parts[0]))?;

    if parts.len() > 1 {
        Ok(data_dir.join(parts[1]))
    } else {
        Ok(data_dir.clone())
    }
}

async fn explorer(
    State(state): State<AppState>,
    Json(PathParams { path }): Json<PathParams>,
) -> Result<Json<Vec<ExplorerItem>>, AppError> {
    let mut result = Vec::new();

    if path == "/" {
        for name in state.data.keys() {
            result.push(ExplorerItem {
                name: name.clone(),
                type_: "dir".to_string(),
            });
        }
    } else {
        let dir = get_path(&state.data, path)?;

        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_name = if let Some(f) = path.file_name() {
                f.to_string_lossy().into_owned()
            } else {
                continue;
            };

            if path.is_dir() {
                result.push(ExplorerItem {
                    name: file_name,
                    type_: "dir".to_string(),
                });
            } else {
                if let Some(ext) = path.extension() {
                    if is_supported_file_type(ext) {
                        result.push(ExplorerItem {
                            name: file_name,
                            type_: "file".to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(Json(result))
}

async fn mkdir(
    State(state): State<AppState>,
    Json(PathParams { path }): Json<PathParams>,
) -> Result<(), AppError> {
    let dir = get_path(&state.data, path)?;

    fs::create_dir(dir).await?;

    Ok(())
}

async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Result<(), AppError> {
    let mut target_path: Option<PathBuf> = None;

    while let Some(field) = multipart.next_field().await? {
        if let Some(name) = field.name() {
            match name {
                "file" => {
                    if let Some(dir) = &target_path {
                        if let Some(f) = field.file_name() {
                            let filename = dir.join(f);

                            if let Ok(bytes) = field.bytes().await {
                                fs::write(&filename, &bytes).await?;
                            }
                        }
                    }
                }
                "path" => {
                    let path_str = field.text().await?;
                    target_path = Some(get_path(&state.data, path_str.clone())?);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn is_supported_file_type(ext: &std::ffi::OsStr) -> bool {
    let supported_extensions = [
        "mp4", "mp3", "m4a", "webm", "wmv", "ts", "rmvb", "rm", "avi", "flv", "ogg", "aac", "wav",
        "flac", "mkv",
    ];
    ext.to_str().map_or(false, |s| {
        supported_extensions.contains(&&*s.to_lowercase())
    })
}
