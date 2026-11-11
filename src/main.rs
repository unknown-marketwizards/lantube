mod auth;
mod error;
mod frontend;
mod library;
mod transcode;
mod upload;

use anyhow::{Context, Result, ensure};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
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
    sync::Arc,
};

const MAX_UPLOAD_SIZE: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerConfig {
    addr: SocketAddr,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    data: HashMap<String, PathBuf>,
    server: ServerConfig,
    auth: Option<auth::AuthConfig>,
}

#[derive(Clone, Default)]
struct AppState {
    data: HashMap<String, PathBuf>,
    tools: transcode::MediaTools,
    media: Arc<transcode::MediaAccess>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let executable = env::current_exe()?;
    let executable_dir = executable
        .parent()
        .expect("executable must have a parent directory");
    let config = load_config(&env::current_dir()?, executable_dir).await?;
    let auth = config
        .auth
        .map(auth::AuthConfig::build)
        .transpose()?
        .unwrap_or_default();
    let state = AppState {
        tools: transcode::MediaTools::from_path(&config.data),
        data: config.data,
        media: Arc::default(),
    };
    state.media.audio.start_reaper();
    let app = router(state, auth);

    let listener = tokio::net::TcpListener::bind(config.server.addr)
        .await
        .with_context(|| format!("Could not listen on {}", config.server.addr))?;
    println!("LanTube listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app.layer(TraceLayer::new_for_http())).await?;
    Ok(())
}

fn router(state: AppState, auth: auth::Auth) -> Router {
    let writes = Router::new()
        .route("/api/mkdir", post(library::mkdir))
        .route(
            "/api/upload",
            post(upload::upload).layer(DefaultBodyLimit::max(
                usize::try_from(MAX_UPLOAD_SIZE).unwrap_or(usize::MAX),
            )),
        )
        .route_layer(middleware::from_fn(auth::require_write));

    Router::new()
        .route("/", get(frontend::index))
        .route("/{*path}", get(frontend::asset))
        .route(
            "/api/explorer",
            post(library::explorer).layer(middleware::from_fn(auth::expose_access)),
        )
        .route("/api/transcode/{*path}", get(transcode::playback))
        .route("/api/thumbnail/{*path}", get(transcode::thumbnail))
        .merge(writes)
        .route("/file/", any(|| async { StatusCode::NOT_FOUND }))
        .nest(
            "/file",
            Router::new()
                .route("/{root}/{*path}", get(library::serve_media))
                .route_layer(middleware::from_fn(auth::sandbox_file))
                .fallback(|| async { StatusCode::NOT_FOUND }),
        )
        .layer(middleware::from_fn_with_state(auth, auth::authenticate))
        .with_state(state)
}

async fn load_config(working_dir: &Path, executable_dir: &Path) -> Result<Config> {
    for directory in [working_dir, executable_dir] {
        let path = directory.join("config.toml");
        match fs::symlink_metadata(&path).await {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        }
        let source_path = fs::canonicalize(&path)
            .await
            .with_context(|| format!("Could not resolve {}", path.display()))?;
        let source = fs::read_to_string(&source_path)
            .await
            .with_context(|| format!("Could not read {}", path.display()))?;
        let mut config: Config = toml::from_str(&source).map_err(|error| {
            // 只报告位置，避免解析错误把配置里的值写入日志。
            let location = error.span().map_or_else(String::new, |span| {
                let line = source
                    .bytes()
                    .take(span.start)
                    .filter(|&byte| byte == b'\n')
                    .count()
                    + 1;
                format!(" near line {line}")
            });
            anyhow::anyhow!("Could not parse {}{location}", path.display())
        })?;
        for (name, root) in &mut config.data {
            ensure!(
                library::is_normal_component(name),
                "Invalid media root: {name}"
            );
            if root.is_relative() {
                *root = directory.join(&*root);
            }
        }
        if config.auth.is_some() {
            let working_dir = fs::canonicalize(working_dir)
                .await
                .context("Could not resolve the working directory")?;
            let executable_dir = fs::canonicalize(executable_dir)
                .await
                .context("Could not resolve the executable directory")?;
            validate_private_paths(
                &mut config.data,
                [&working_dir, &executable_dir, &source_path],
            )
            .await?;
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
        auth: None,
    })
}

async fn validate_private_paths(
    data: &mut HashMap<String, PathBuf>,
    protected: [&Path; 3],
) -> Result<()> {
    for (name, root) in data {
        let resolved = fs::canonicalize(&*root)
            .await
            .with_context(|| format!("Could not resolve auth-enabled media root {name:?}"))?;
        ensure!(
            !protected.iter().any(|path| path.starts_with(&resolved)),
            "Auth-enabled media root {name:?} contains the configuration, working directory, or executable directory"
        );
        *root = resolved;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use tower::ServiceExt;

    const CONFIG: &str = "[data]\nmedia = 'clips'\n[server]\naddr = '127.0.0.1:9999'\n";
    const AUTH: &str = "[auth.users.reader]\npassword = 'test-only'\n";

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

    #[tokio::test]
    async fn authenticated_media_roots_cannot_contain_private_paths() {
        let working = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        let config_path = working.path().join("config.toml");
        let clips = working.path().join("clips");
        fs::create_dir(&clips).await.unwrap();
        fs::write(&config_path, format!("{CONFIG}{AUTH}"))
            .await
            .unwrap();
        let config = load_config(working.path(), executable.path())
            .await
            .unwrap();
        assert_eq!(
            config.data["media"],
            fs::canonicalize(&clips).await.unwrap()
        );

        let shared_working = CONFIG.replace("'clips'", "'.'");
        fs::write(&config_path, &shared_working).await.unwrap();
        assert!(load_config(working.path(), executable.path()).await.is_ok());
        fs::write(&config_path, format!("{shared_working}{AUTH}"))
            .await
            .unwrap();
        let error = load_config(working.path(), executable.path())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("contains the configuration"));

        fs::write(&config_path, format!("{CONFIG}{AUTH}"))
            .await
            .unwrap();
        let error = load_config(working.path(), &clips).await.err().unwrap();
        assert!(error.to_string().contains("contains the configuration"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_linked_config_in_a_share_or_with_a_missing_target() {
        let working = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        let clips = working.path().join("clips");
        fs::create_dir(&clips).await.unwrap();
        let target = clips.join("secrets.toml");
        std::os::unix::fs::symlink(&target, working.path().join("config.toml")).unwrap();
        fs::write(executable.path().join("config.toml"), CONFIG)
            .await
            .unwrap();
        let error = load_config(working.path(), executable.path())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Could not resolve"));

        fs::write(target, format!("{CONFIG}{AUTH}")).await.unwrap();
        let error = load_config(working.path(), executable.path())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("contains the configuration"));
    }

    #[tokio::test]
    async fn configuration_parse_errors_do_not_log_values() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.toml"),
            format!("{CONFIG}{AUTH}role = 'do-not-log-this-value'\n"),
        )
        .await
        .unwrap();
        let error = load_config(directory.path(), directory.path())
            .await
            .err()
            .unwrap();
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("Could not parse") && diagnostic.contains("near line"));
        assert!(!diagnostic.contains("do-not-log-this-value"));
    }

    fn configured_auth() -> auth::Auth {
        let config: auth::AuthConfig = toml::from_str(
            "[users.reader]\npassword = 'read:only'\n[users.operator]\npassword = 'write'\nrole = 'editor'\n",
        )
        .unwrap();
        config.build().unwrap()
    }

    #[tokio::test]
    async fn authentication_covers_html_assets_and_all_media_routes() {
        let app = router(AppState::default(), configured_auth());
        for (method, path) in [
            ("GET", "/"),
            ("GET", "/lantube.svg"),
            ("POST", "/api/explorer"),
            ("POST", "/api/upload"),
            ("POST", "/api/mkdir"),
            ("GET", "/file/share/video.mp4"),
            ("HEAD", "/file/share/video.mp4"),
            ("GET", "/api/thumbnail/share/video.mp4"),
            ("GET", "/api/transcode/share/video.mp4"),
            ("GET", "/api/transcode/share/video.mp4?segment=0"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
            assert!(
                response.headers()[header::WWW_AUTHENTICATE]
                    .to_str()
                    .unwrap()
                    .starts_with("Basic ")
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[tokio::test]
    async fn readers_cannot_write_and_editors_must_send_the_write_header() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState {
            data: HashMap::from([("share".into(), directory.path().to_owned())]),
            ..Default::default()
        };
        let app = router(state.clone(), configured_auth());
        for (credentials, writable) in [("reader:read:only", "false"), ("operator:write", "true")] {
            let request = Request::post("/api/explorer")
                .header(
                    header::AUTHORIZATION,
                    format!("Basic {}", STANDARD.encode(credentials)),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"path":"/"}"#))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["x-lantube-can-write"], writable);
        }
        for (credentials, write_header, status) in [
            ("reader:wrong", true, StatusCode::UNAUTHORIZED),
            ("reader:read:only", true, StatusCode::FORBIDDEN),
            ("operator:write", false, StatusCode::FORBIDDEN),
            ("operator:write", true, StatusCode::OK),
        ] {
            let mut request = Request::post("/api/mkdir")
                .header(
                    header::AUTHORIZATION,
                    format!("Basic {}", STANDARD.encode(credentials)),
                )
                .header(header::CONTENT_TYPE, "application/json");
            if write_header {
                request = request.header("x-lantube-write", "1");
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(Body::from(r#"{"path":"/share/new"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
        let request = Request::post("/api/mkdir")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"path":"/share/anonymous"}"#))
            .unwrap();
        let response = router(state, auth::Auth::default())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
