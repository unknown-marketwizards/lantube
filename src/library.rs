use crate::{AppState, error::AppError};
use axum::{
    Json,
    body::Body,
    extract::{Path as RoutePath, Request, State},
    http::{HeaderValue, header},
    response::Response,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use tokio::fs;
use tower_http::services::ServeFile;

#[derive(Serialize)]
pub struct ExplorerItem {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<u64>,
}

impl ExplorerItem {
    fn directory(name: String) -> Self {
        Self {
            name,
            kind: "dir",
            size: None,
            modified: None,
        }
    }
}

#[derive(Deserialize)]
pub struct PathParams {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerParams {
    path: String,
    #[serde(default)]
    include_other: bool,
}

pub fn is_normal_component(value: &str) -> bool {
    if value.is_empty() || value.contains(['/', '\\', '\0']) {
        return false;
    }

    #[cfg(windows)]
    {
        if value.ends_with(['.', ' '])
            || value.contains([':', '<', '>', '"', '|', '?', '*'])
            || value.bytes().any(|byte| byte < 32)
        {
            return false;
        }
        let device = value
            .split('.')
            .next()
            .unwrap()
            .trim_end()
            .to_ascii_uppercase();
        if matches!(
            device.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        {
            return false;
        }
    }

    let mut parts = Path::new(value).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    )
}

fn split_path<'a>(
    data: &'a HashMap<String, PathBuf>,
    path: &str,
) -> Result<(&'a Path, PathBuf), AppError> {
    let mut parts = path.strip_prefix('/').unwrap_or(path).split('/');
    let name = parts
        .next()
        .filter(|name| is_normal_component(name))
        .ok_or_else(|| AppError::bad_request("Invalid media path"))?;
    let root = data
        .get(name)
        .ok_or_else(|| AppError::bad_request("Unknown media root"))?;
    let mut relative = PathBuf::new();
    for part in parts {
        if !is_normal_component(part) {
            return Err(AppError::bad_request("Invalid media path component"));
        }
        relative.push(part);
    }
    Ok((root, relative))
}

async fn resolve_media(state: &AppState, path: &str) -> Result<(PathBuf, PathBuf), AppError> {
    let (root, relative) = split_path(&state.data, path)?;
    let root = fs::canonicalize(root).await?;
    if state.chapters.overlaps(&root) {
        return Err(AppError::forbidden(
            "Media root overlaps the chapter storage directory",
        ));
    }
    let target = if relative.as_os_str().is_empty() {
        root.clone()
    } else {
        fs::canonicalize(root.join(relative)).await?
    };
    if !target.starts_with(&root) {
        return Err(AppError::forbidden("Path leaves the media root"));
    }
    Ok((root, target))
}

pub async fn resolve_directory(state: &AppState, path: &str) -> Result<PathBuf, AppError> {
    let (_, directory) = resolve_media(state, path).await?;
    if !fs::metadata(&directory).await?.is_dir() {
        return Err(AppError::bad_request("Expected a directory"));
    }
    Ok(directory)
}

pub async fn explorer(
    State(state): State<AppState>,
    Json(ExplorerParams {
        path,
        include_other,
    }): Json<ExplorerParams>,
) -> Result<Json<Vec<ExplorerItem>>, AppError> {
    if path == "/" {
        return Ok(Json(
            state
                .data
                .keys()
                .cloned()
                .map(ExplorerItem::directory)
                .collect(),
        ));
    }

    let (root, directory) = resolve_media(&state, &path).await?;
    let mut entries = fs::read_dir(directory).await?;
    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !is_normal_component(&name) {
            continue;
        }
        let entry_type = entry.file_type().await?;
        let target = if entry_type.is_symlink() {
            match fs::canonicalize(entry.path()).await {
                Ok(target) if target.starts_with(&root) => target,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
        } else {
            entry.path()
        };
        if entry_type.is_dir() {
            result.push(ExplorerItem::directory(name));
            continue;
        }
        let known_media = is_media_extension(Path::new(&name));
        if !entry_type.is_symlink() && !known_media && !include_other {
            continue;
        }
        let metadata = match fs::metadata(target).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.is_dir() {
            result.push(ExplorerItem::directory(name));
        } else if metadata.is_file() && (known_media || include_other) {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok());
            result.push(ExplorerItem {
                name,
                kind: if known_media { "file" } else { "other" },
                size: Some(metadata.len()),
                modified,
            });
        }
    }
    Ok(Json(result))
}

pub async fn mkdir(
    State(state): State<AppState>,
    Json(PathParams { path }): Json<PathParams>,
) -> Result<(), AppError> {
    let (parent, name) = path
        .rsplit_once('/')
        .filter(|(_, name)| is_normal_component(name))
        .ok_or_else(|| AppError::bad_request("Invalid new directory path"))?;
    let parent = resolve_directory(&state, parent).await?;
    fs::create_dir(parent.join(name)).await?;
    Ok(())
}

pub async fn serve_media(
    State(state): State<AppState>,
    RoutePath((root, path)): RoutePath<(String, String)>,
    request: Request,
) -> Result<Response, AppError> {
    if !is_normal_component(&root) {
        return Err(AppError::bad_request("Invalid media root"));
    }
    let file = resolve_file(&state, &format!("{root}/{path}")).await?;
    let axum::extract::Query(MediaQuery { revision }) =
        axum::extract::Query::try_from_uri(request.uri())
            .map_err(|_| AppError::bad_request("Invalid media revision"))?;
    let versioned = revision.is_some() || request.method() == axum::http::Method::HEAD;
    let _read = versioned.then(|| state.media.read(&file)).transpose()?;
    let revision = if versioned {
        let actual = file_revision(&file).await?;
        if let Some(expected) = revision {
            require_revision(actual, expected)?;
        }
        Some(actual)
    } else {
        None
    };
    let mut response = ServeFile::new(&file).try_call(request).await?;
    if let Some(revision) = revision {
        check_revision(&file, revision).await?;
        response
            .headers_mut()
            .insert("x-media-revision", revision.into());
    }
    // 原片每次复用前都需重新校验，重启后启用认证时同样适用。
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    Ok(response.map(Body::new))
}

#[derive(Deserialize)]
struct MediaQuery {
    revision: Option<u64>,
}

pub async fn file_revision(path: &Path) -> Result<u64, AppError> {
    let path = path.to_owned();
    Ok(tokio::task::spawn_blocking(move || read_revision(&path)).await??)
}

pub fn read_revision(path: &Path) -> io::Result<u64> {
    let file = std::fs::File::open(path)?;
    opened_file_revision(&file)
}

pub fn opened_file_revision(file: &std::fs::File) -> io::Result<u64> {
    let metadata = file.metadata()?;
    let mut digest = Sha256::new();
    digest.update(metadata.len().to_le_bytes());
    for time in [Some(metadata.modified()?), metadata.created().ok()] {
        digest.update([u8::from(time.is_some())]);
        if let Some(time) = time {
            let ns = match time.duration_since(UNIX_EPOCH) {
                Ok(duration) => duration.as_nanos() as i128,
                Err(error) => -(error.duration().as_nanos() as i128),
            };
            digest.update(ns.to_le_bytes());
        }
    }
    #[cfg(unix)]
    let changed_at = {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
        i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec())
    };
    #[cfg(windows)]
    let changed_at = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // 从已打开的文件取标识，避免将替换前后的路径属性混用。
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        digest.update(info.dwVolumeSerialNumber.to_le_bytes());
        digest.update(info.nFileIndexHigh.to_le_bytes());
        digest.update(info.nFileIndexLow.to_le_bytes());
        let mut basic = FILE_BASIC_INFO::default();
        // 覆盖原内容后即使恢复修改时间，文件系统的变更时间也会更新。
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw mut basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        i128::from(basic.ChangeTime)
    };
    #[cfg(not(any(unix, windows)))]
    let changed_at = 0_i128;
    digest.update(changed_at.to_le_bytes());
    let prefix = digest.finalize()[..8]
        .try_into()
        .expect("SHA-256 has 32 bytes");
    Ok(u64::from_le_bytes(prefix))
}

pub fn require_revision(actual: u64, expected: u64) -> Result<(), AppError> {
    if actual != expected {
        return Err(AppError::new(
            axum::http::StatusCode::CONFLICT,
            "Media file changed",
        ));
    }
    Ok(())
}

pub async fn check_revision(path: &Path, expected: u64) -> Result<(), AppError> {
    require_revision(file_revision(path).await?, expected)
}

pub async fn resolve_file(state: &AppState, path: &str) -> Result<PathBuf, AppError> {
    let (_, file) = resolve_media(state, path).await?;
    if !fs::metadata(&file).await?.is_file() {
        return Err(io::Error::from(io::ErrorKind::NotFound).into());
    }
    Ok(file)
}

fn is_media_extension(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "mp4", "m4v", "mov", "mp3", "m4a", "webm", "wmv", "ts", "rmvb", "rm", "avi", "flv", "ogg",
        "ogv", "opus", "aac", "wav", "flac", "mkv",
    ];
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Method, StatusCode, header},
        response::IntoResponse,
    };

    #[test]
    fn validates_each_path_segment() {
        let data = HashMap::from([("media".to_string(), PathBuf::from("data-root"))]);
        let (root, relative) = split_path(&data, "/media/series/中文 视频.mkv").unwrap();
        assert_eq!(root, Path::new("data-root"));
        assert_eq!(relative, Path::new("series").join("中文 视频.mkv"));
        for path in [
            "",
            "/",
            "media/.",
            "media/../outside",
            "media//video.mp4",
            "media/sub\\..\\outside",
            "unknown/video.mp4",
        ] {
            assert!(split_path(&data, path).is_err(), "{path}");
        }
        assert!(!is_normal_component("file\0.mp4"));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_win32_aliases_and_streams() {
        for name in [
            ".. ",
            "directory.",
            "NUL.mp4",
            "COM¹",
            "PRN .mov",
            "video.mp4:stream",
            "C:video.mp4",
        ] {
            assert!(!is_normal_component(name), "{name}");
        }
    }

    #[tokio::test]
    async fn lists_unrecognized_files_only_on_request() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["clip.MKV", "renamed.data", "extensionless"] {
            fs::write(directory.path().join(name), []).await.unwrap();
        }
        let state = AppState {
            data: HashMap::from([("media".to_string(), directory.path().to_owned())]),
            ..Default::default()
        };
        let default = serde_json::from_str(r#"{"path":"/media"}"#).unwrap();
        let Json(items) = explorer(State(state.clone()), Json(default)).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].name.as_str(), items[0].kind),
            ("clip.MKV", "file")
        );

        let all = serde_json::from_str(r#"{"path":"/media","includeOther":true}"#).unwrap();
        let Json(items) = explorer(State(state), Json(all)).await.unwrap();
        let kinds: HashMap<_, _> = items
            .into_iter()
            .map(|item| (item.name, item.kind))
            .collect();
        assert_eq!(kinds.len(), 3);
        assert_eq!(kinds["clip.MKV"], "file");
        assert_eq!(kinds["renamed.data"], "other");
        assert_eq!(kinds["extensionless"], "other");
    }

    #[tokio::test]
    async fn media_serves_ranges_and_head() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("clip.mp4"), b"0123456789")
            .await
            .unwrap();
        let state = AppState {
            data: HashMap::from([("media".to_string(), directory.path().to_owned())]),
            ..Default::default()
        };
        let route = || RoutePath(("media".to_string(), "clip.mp4".to_string()));
        let request = Request::builder()
            .header(header::RANGE, "bytes=2-4")
            .body(Body::empty())
            .unwrap();
        let response = serve_media(State(state.clone()), route(), request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        assert_eq!(
            &to_bytes(response.into_body(), 10).await.unwrap()[..],
            b"234"
        );

        let request = Request::builder()
            .method(Method::HEAD)
            .body(Body::empty())
            .unwrap();
        let response = serve_media(State(state), route(), request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert!(to_bytes(response.into_body(), 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_media_returns_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState {
            data: HashMap::from([("media".to_string(), directory.path().to_owned())]),
            ..AppState::default()
        };
        let error = resolve_media(&state, "media/missing.mp4")
            .await
            .unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symbolic_links_outside_media_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("media");
        let outside = directory.path().join("media-outside");
        fs::create_dir(&root).await.unwrap();
        fs::create_dir(&outside).await.unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let state = AppState {
            data: HashMap::from([("media".to_string(), root)]),
            ..AppState::default()
        };
        let error = resolve_media(&state, "media/link").await.unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
    }
}
