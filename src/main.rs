use axum::http::StatusCode;
use axum::{extract::Multipart, routing::post, Json, Router};
use clap::Parser;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use axum::extract::DefaultBodyLimit;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::RwLock;

#[derive(Parser, Debug)]
#[command(version, long_about = None)]
struct Args {
    data_dir: String,
    /// listening address
    #[arg(long)]
    addr: Option<String>,
}

lazy_static! {
    static ref DATA_DIR: RwLock<PathBuf> = RwLock::new(PathBuf::new());
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let addr = if let Some(addr) = args.addr {
        SocketAddr::from_str(&addr).unwrap()
    } else {
        SocketAddr::from(([127, 0, 0, 1], 8888))
    };

    *DATA_DIR.write().unwrap() = PathBuf::from(args.data_dir.clone());

    let app = Router::new()
        .nest_service("/", ServeFile::new("./frontend/dist/index.html"))
        .nest_service("/assets", ServeDir::new("./frontend/dist/assets"))
        .nest_service("/file", ServeDir::new(args.data_dir))
        .route("/api/explorer", post(explorer))
        .route("/api/mkdir", post(mkdir))
        .route("/api/upload", post(upload))
        .layer(DefaultBodyLimit::disable());

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

fn get_path(path: String) -> PathBuf {
    DATA_DIR.read().unwrap().join(if path.starts_with("/") {
        &path[1..]
    } else {
        path.as_str()
    })
}

async fn explorer(
    Json(PathParams { path }): Json<PathParams>,
) -> (StatusCode, Json<Vec<ExplorerItem>>) {
    let dir = get_path(path);

    let mut result = Vec::new();

    let mut entries = fs::read_dir(dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let path = entry.path();
        if path.is_dir() {
            result.push(ExplorerItem {
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                type_: "dir".to_string(),
            });
        } else {
            if let Some(ext) = path.extension() {
                if is_supported_file_type(ext) {
                    result.push(ExplorerItem {
                        name: path.file_name().unwrap().to_string_lossy().into_owned(),
                        type_: "file".to_string(),
                    });
                }
            }
        }
    }

    (StatusCode::OK, Json(result))
}

async fn mkdir(Json(PathParams { path }): Json<PathParams>) -> StatusCode {
    let dir = get_path(path);

    fs::create_dir(dir).await.unwrap();

    StatusCode::OK
}

async fn upload(mut multipart: Multipart) {
    let mut target_path: Option<PathBuf> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(name) = field.name() {
            match name {
                "file" => {
                    if let Some(dir) = &target_path {
                        let filename = dir.join(field.file_name().unwrap());

                        if let Ok(bytes) = field.bytes().await {
                            fs::write(&filename, &bytes).await.unwrap();
                        }
                    }
                }
                "path" => {
                    let path_str = field.text().await.unwrap();

                    target_path = Some(get_path(path_str.clone()));
                }
                _ => {}
            }
        }
    }
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
