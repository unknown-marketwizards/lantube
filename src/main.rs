mod error;

use crate::error::AppError;

use axum::extract::{DefaultBodyLimit, State};
use axum::{extract::Multipart, routing::post, Json, Router};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser, Debug)]
#[command(version, long_about = None)]
struct Args {
    data_dir: String,
    /// listening address
    #[arg(long)]
    addr: Option<String>,
}

#[derive(Default, Clone)]
struct AppState {
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let addr = if let Some(addr) = args.addr {
        SocketAddr::from_str(&addr).unwrap()
    } else {
        SocketAddr::from(([127, 0, 0, 1], 8888))
    };

    let state = AppState {
        data_dir: PathBuf::from(&args.data_dir),
    };

    let app = Router::new()
        .nest_service("/", ServeFile::new("./frontend/dist/index.html"))
        .nest_service("/assets", ServeDir::new("./frontend/dist/assets"))
        .nest_service("/file", ServeDir::new(args.data_dir))
        .route("/api/explorer", post(explorer))
        .route("/api/mkdir", post(mkdir))
        .route("/api/upload", post(upload))
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

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

fn get_path(data_dir: &PathBuf, path: String) -> PathBuf {
    data_dir.join(if path.starts_with("/") {
        &path[1..]
    } else {
        path.as_str()
    })
}

async fn explorer(
    State(state): State<AppState>,
    Json(PathParams { path }): Json<PathParams>,
) -> Result<Json<Vec<ExplorerItem>>, AppError> {
    let dir = get_path(&state.data_dir, path);

    let mut result = Vec::new();

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

    Ok(Json(result))
}

async fn mkdir(
    State(state): State<AppState>,
    Json(PathParams { path }): Json<PathParams>,
) -> Result<(), AppError> {
    let dir = get_path(&state.data_dir, path);

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

                    target_path = Some(get_path(&state.data_dir, path_str.clone()));
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
