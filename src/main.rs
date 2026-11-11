mod error;
mod frontend;
mod library;
mod upload;

use anyhow::{Context, Result, ensure};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use serde::Deserialize;
use tokio::fs;
use tower_http::trace::TraceLayer;

use std::{
    collections::HashMap,
    env,
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};

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
async fn main() -> Result<()> {
    let executable = env::current_exe()?;
    let executable_dir = executable
        .parent()
        .expect("executable must have a parent directory");
    let config = load_config(&env::current_dir()?, executable_dir).await?;
    let state = AppState { data: config.data };
    let app = Router::new()
        .route("/", get(frontend::index))
        .route("/{*path}", get(frontend::asset))
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
        .with_context(|| format!("Could not listen on {}", config.server.addr))?;
    println!("LanTube listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app.layer(TraceLayer::new_for_http())).await?;
    Ok(())
}

async fn load_config(working_dir: &Path, executable_dir: &Path) -> Result<Config> {
    for directory in [working_dir, executable_dir] {
        let path = directory.join("config.toml");
        let source = match fs::read_to_string(&path).await {
            Ok(source) => source,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        };
        let mut config: Config = toml::from_str(&source)
            .with_context(|| format!("Could not parse {}", path.display()))?;
        for (name, root) in &mut config.data {
            ensure!(
                library::is_normal_component(name),
                "Invalid media root: {name}"
            );
            if root.is_relative() {
                *root = directory.join(&*root);
            }
        }
        return Ok(config);
    }

    let share = executable_dir.join("share");
    let data = match fs::metadata(&share).await {
        Ok(metadata) if metadata.is_dir() => HashMap::from([("share".to_owned(), share)]),
        Ok(_) => HashMap::new(),
        Err(error) if error.kind() == ErrorKind::NotFound => HashMap::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("Could not read {}", share.display()));
        }
    };
    Ok(Config {
        data,
        server: ServerConfig {
            addr: (Ipv4Addr::LOCALHOST, 8888).into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[data]\nmedia = 'clips'\n[server]\naddr = '127.0.0.1:9999'\n";

    #[tokio::test]
    async fn defaults_to_localhost_and_uses_share_beside_the_binary() {
        let working = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        let config = load_config(working.path(), executable.path())
            .await
            .unwrap();
        assert_eq!(config.server.addr, (Ipv4Addr::LOCALHOST, 8888).into());
        assert!(config.data.is_empty());

        let share = executable.path().join("share");
        fs::create_dir(&share).await.unwrap();
        let config = load_config(working.path(), executable.path())
            .await
            .unwrap();
        assert_eq!(config.data["share"], share);
    }

    #[tokio::test]
    async fn prefers_working_directory_config_and_resolves_paths_beside_it() {
        let working = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        fs::write(executable.path().join("config.toml"), CONFIG)
            .await
            .unwrap();
        let config = load_config(working.path(), executable.path())
            .await
            .unwrap();
        assert_eq!(config.data["media"], executable.path().join("clips"));

        fs::write(working.path().join("config.toml"), CONFIG)
            .await
            .unwrap();
        let config = load_config(working.path(), executable.path())
            .await
            .unwrap();
        assert_eq!(config.data["media"], working.path().join("clips"));
    }

    #[tokio::test]
    async fn reports_invalid_config_before_trying_the_next_location() {
        let working = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        fs::write(working.path().join("config.toml"), "invalid")
            .await
            .unwrap();
        fs::write(executable.path().join("config.toml"), CONFIG)
            .await
            .unwrap();
        let error = load_config(working.path(), executable.path())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Could not parse"));
    }
}
