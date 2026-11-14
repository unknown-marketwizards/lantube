mod auth;
mod chapters;
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
use axum::routing::{any, get, post, put};
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
    chapters: Arc<chapters::ChapterStore>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let executable = env::current_exe()?;
    let executable_dir = executable
        .parent()
        .expect("executable must have a parent directory");
    let working_dir = env::current_dir()?;
    let (app, addr) = load_app(&working_dir, executable_dir).await?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("Could not listen on {addr}"))?;
    println!("LanTube listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app.layer(TraceLayer::new_for_http())).await?;
    Ok(())
}

async fn load_app(working_dir: &Path, executable_dir: &Path) -> Result<(Router, SocketAddr)> {
    let config = load_config(working_dir, executable_dir).await?;
    let chapter_directory = working_dir.join("data/chapters");
    let chapters =
        tokio::task::spawn_blocking(move || chapters::ChapterStore::open(&chapter_directory))
            .await?
            .context("Could not resolve the chapter storage directory")?;
    let auth = config
        .auth
        .map(auth::AuthConfig::build)
        .transpose()?
        .unwrap_or_default();
    let state = AppState {
        tools: transcode::MediaTools::from_path(&config.data),
        data: config.data,
        media: Arc::default(),
        chapters: Arc::new(chapters),
    };
    state.media.audio.start_reaper();
    Ok((router(state, auth), config.server.addr))
}

fn router(state: AppState, auth: auth::Auth) -> Router {
    let writes = Router::new()
        .route("/api/mkdir", post(library::mkdir))
        .route("/api/chapters", put(chapters::save))
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
        .route("/api/chapters", get(chapters::load))
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

    fn explorer_request(path: &str) -> Request<Body> {
        Request::post("/api/explorer")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::json!({"path": path}).to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn offline_media_root_does_not_block_startup_or_other_roots() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        let online = root.join("clips");
        fs::create_dir(&online).await.unwrap();
        fs::write(online.join("clip.mp4"), b"video").await.unwrap();

        #[cfg(windows)]
        let offline = {
            let drive = ('D'..='Z')
                .map(|letter| PathBuf::from(format!("{letter}:\\")))
                .find(|drive| {
                    std::fs::symlink_metadata(drive)
                        .is_err_and(|err| err.kind() == ErrorKind::NotFound)
                })
                .expect("need an unmounted drive letter for this test");
            drive.join("clips")
        };
        #[cfg(unix)]
        let offline = {
            let link = root.join("offline");
            std::os::unix::fs::symlink(root.join("unavailable"), &link).unwrap();
            link
        };
        fs::write(
            root.join("config.toml"),
            CONFIG.replace(
                "media = 'clips'",
                &format!("media = 'clips'\noffline = '{}'", offline.display()),
            ),
        )
        .await
        .unwrap();

        let (app, addr) = load_app(root, root).await.unwrap();
        assert_eq!(addr, (Ipv4Addr::LOCALHOST, 9999).into());
        let response = app.clone().oneshot(explorer_request("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let roots: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            roots
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == "offline")
        );

        let response = app
            .clone()
            .oneshot(explorer_request("/offline"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = app
            .oneshot(
                Request::get("/file/media/clip.mp4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(&body[..], b"video");
    }

    #[tokio::test]
    async fn rejects_media_roots_that_overlap_chapter_storage() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        fs::create_dir_all(root.join("data/chapters/nested"))
            .await
            .unwrap();
        fs::create_dir(root.join("data/media")).await.unwrap();
        for path in ["data", "data/chapters", "data/chapters/nested"] {
            fs::write(
                root.join("config.toml"),
                CONFIG.replace("'clips'", &format!("'{path}'")),
            )
            .await
            .unwrap();
            let (app, _) = load_app(root, root).await.unwrap();
            let response = app.oneshot(explorer_request("/media")).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }
        fs::write(
            root.join("config.toml"),
            CONFIG.replace("'clips'", "'data/media'"),
        )
        .await
        .unwrap();
        let (app, _) = load_app(root, root).await.unwrap();
        let response = app.oneshot(explorer_request("/media")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_chapter_storage_symlink_into_a_media_root() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        fs::create_dir_all(root.join("share/metadata"))
            .await
            .unwrap();
        fs::create_dir(root.join("data")).await.unwrap();
        std::os::unix::fs::symlink(root.join("share/metadata"), root.join("data/chapters"))
            .unwrap();
        fs::write(
            root.join("config.toml"),
            CONFIG.replace("'clips'", "'share'"),
        )
        .await
        .unwrap();
        let (app, _) = load_app(root, root).await.unwrap();
        let response = app.oneshot(explorer_request("/media")).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checks_for_overlap_when_an_offline_media_root_becomes_accessible() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        let storage = root.join("data/chapters");
        fs::create_dir_all(&storage).await.unwrap();
        fs::write(storage.join("record.json"), b"private")
            .await
            .unwrap();
        fs::write(
            root.join("config.toml"),
            CONFIG.replace("'clips'", "'late-root'"),
        )
        .await
        .unwrap();
        let (app, _) = load_app(root, root).await.unwrap();
        let response = app
            .clone()
            .oneshot(explorer_request("/media"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        std::os::unix::fs::symlink(&storage, root.join("late-root")).unwrap();
        let response = app
            .oneshot(
                Request::get("/file/media/record.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chapter_requests_refuse_storage_redirected_after_startup() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        let share = root.join("clips");
        fs::create_dir(&share).await.unwrap();
        let video = share.join("clip.mp4");
        fs::write(&video, b"video").await.unwrap();
        fs::write(root.join("config.toml"), CONFIG).await.unwrap();
        let (app, _) = load_app(root, root).await.unwrap();
        let revision = library::file_revision(&video).await.unwrap().to_string();
        let load = || {
            Request::get(format!(
                "/api/chapters?path=%2Fmedia%2Fclip.mp4&revision={revision}"
            ))
            .body(Body::empty())
            .unwrap()
        };
        let response = app.clone().oneshot(load()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let initial: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let update = serde_json::json!({
            "path": "/media/clip.mp4",
            "revision": revision,
            "version": initial["version"],
            "chapters": [{"startMs": 0, "title": "Intro"}],
        });
        let save = || {
            Request::put("/api/chapters")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(update.to_string()))
                .unwrap()
        };

        let data = root.join("data");
        assert!(!data.exists());
        std::os::unix::fs::symlink(&share, &data).unwrap();
        let response = app.clone().oneshot(save()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = app.clone().oneshot(load()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!share.join("chapters").exists());

        fs::remove_file(&data).await.unwrap();
        let response = app.clone().oneshot(save()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.oneshot(load()).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(saved["chapters"][0]["title"], "Intro");
        assert!(data.join("chapters").exists());
        assert!(!share.join("chapters").exists());
    }

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
    async fn stale_playback_revision_is_rejected_by_media_and_chapter_routes() {
        let directory = tempfile::tempdir().unwrap();
        let share = directory.path().join("share");
        fs::create_dir(&share).await.unwrap();
        let file = share.join("clip.mp4");
        fs::write(&file, b"original").await.unwrap();
        let app = router(
            AppState {
                data: HashMap::from([("share".into(), share)]),
                chapters: Arc::new(
                    chapters::ChapterStore::open(&directory.path().join("chapters")).unwrap(),
                ),
                ..Default::default()
            },
            auth::Auth::default(),
        );
        let request = |method, path: String| {
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap()
        };
        let request_revision = || async {
            let response = app
                .clone()
                .oneshot(request("HEAD", "/file/share/clip.mp4".into()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response.headers()["x-media-revision"]
                .to_str()
                .unwrap()
                .to_owned()
        };
        let old = request_revision().await;
        let chapter_url = "/api/chapters?path=%2Fshare%2Fclip.mp4&revision=";
        let raw_url = format!("/file/share/clip.mp4?revision={old}");
        let media = app
            .clone()
            .oneshot(request("GET", raw_url.clone()))
            .await
            .unwrap();
        assert_eq!(media.status(), StatusCode::OK);
        drop(media);
        fs::write(&file, b"replacement").await.unwrap();

        for (method, path) in [
            ("GET", raw_url),
            ("GET", format!("{chapter_url}{old}")),
            (
                "HEAD",
                format!("/api/transcode/share/clip.mp4?revision={old}"),
            ),
        ] {
            let response = app.clone().oneshot(request(method, path)).await.unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
        let fresh = request_revision().await;
        assert_ne!(fresh, old);
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/file/share/clip.mp4?revision={fresh}"))
                    .header(header::RANGE, "bytes=0-3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(&body[..], b"repl");
        let response = app
            .clone()
            .oneshot(request("GET", format!("{chapter_url}{fresh}")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let update = serde_json::json!({
            "path": "/share/clip.mp4",
            "revision": old,
            "version": saved["version"],
            "chapters": [{"startMs": 0, "title": "Old picture"}],
        });
        let response = app
            .oneshot(
                Request::put("/api/chapters")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!directory.path().join("chapters").exists());
    }

    #[tokio::test]
    #[ignore = "requires ffprobe on PATH"]
    async fn chapter_api_rejects_out_of_range_cuts_for_real_media() {
        let directory = tempfile::tempdir().unwrap();
        let share = directory.path().join("share");
        fs::create_dir(&share).await.unwrap();
        let mut wav = Vec::new();
        let data_len = 80_000_u32;
        // 单声道 8-bit PCM，8000 Hz，正好 10 秒。
        wav.extend_from_slice(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(8_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend(data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 128);
        let file = share.join("clip.wav");
        fs::write(&file, wav).await.unwrap();
        let data = HashMap::from([("share".into(), share)]);
        let app = router(
            AppState {
                tools: transcode::MediaTools::from_path(&data),
                data,
                chapters: Arc::new(
                    chapters::ChapterStore::open(&directory.path().join("chapters")).unwrap(),
                ),
                ..Default::default()
            },
            auth::Auth::default(),
        );
        let revision = library::file_revision(&file).await.unwrap().to_string();
        let read = || {
            Request::get(format!(
                "/api/chapters?path=%2Fshare%2Fclip.wav&revision={revision}"
            ))
            .body(Body::empty())
            .unwrap()
        };
        let response = app.clone().oneshot(read()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let initial: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for (time, expected) in [
            (60_000, StatusCode::BAD_REQUEST),
            (10_000, StatusCode::BAD_REQUEST),
            (9_999, StatusCode::OK),
        ] {
            let update = serde_json::json!({
                "path": "/share/clip.wav",
                "revision": revision,
                "version": initial["version"],
                "chapters": [{"startMs": 0, "title": ""}, {"startMs": time, "title": "Cut"}],
            });
            let response = app
                .clone()
                .oneshot(
                    Request::put("/api/chapters")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(update.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{time}");
        }
        let response = app.oneshot(read()).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(saved["chapters"][1]["startMs"], 9_999);
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
            ("GET", "/api/chapters?path=%2Fshare%2Fvideo.mp4"),
            ("PUT", "/api/chapters"),
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
    async fn viewers_can_read_chapters_and_only_editors_can_save_them() {
        let directory = tempfile::tempdir().unwrap();
        let share = directory.path().join("share");
        fs::create_dir(&share).await.unwrap();
        fs::write(share.join("test.mp4"), b"sample").await.unwrap();
        let app = router(
            AppState {
                data: HashMap::from([("share".into(), share)]),
                chapters: Arc::new(
                    chapters::ChapterStore::open(&directory.path().join("chapters")).unwrap(),
                ),
                ..Default::default()
            },
            configured_auth(),
        );
        let revision = library::file_revision(&directory.path().join("share/test.mp4"))
            .await
            .unwrap()
            .to_string();
        let read = || {
            Request::get(format!(
                "/api/chapters?path=%2Fshare%2Ftest.mp4&revision={revision}"
            ))
            .header(
                header::AUTHORIZATION,
                format!("Basic {}", STANDARD.encode("reader:read:only")),
            )
            .body(Body::empty())
            .unwrap()
        };
        let response = app.clone().oneshot(read()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let version: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for (credentials, write_header, status) in [
            ("reader:read:only", true, StatusCode::FORBIDDEN),
            ("operator:write", false, StatusCode::FORBIDDEN),
            ("operator:write", true, StatusCode::OK),
        ] {
            let mut request = Request::put("/api/chapters")
                .header(
                    header::AUTHORIZATION,
                    format!("Basic {}", STANDARD.encode(credentials)),
                )
                .header(header::CONTENT_TYPE, "application/json");
            if write_header {
                request = request.header("x-lantube-write", "1");
            }
            let body = serde_json::json!({
                "path": "/share/test.mp4",
                "version": version["version"],
                "revision": revision,
                "chapters": [{"startMs": 0, "title": "Intro"}],
            });
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
        let response = app.oneshot(read()).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(saved["chapters"][0]["title"], "Intro");
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
