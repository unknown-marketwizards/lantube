use crate::{
    AppState,
    error::AppError,
    library::{is_normal_component, resolve_directory},
};
use anyhow::Result;
use axum::{
    body::Bytes,
    extract::{Multipart, State, multipart::Field},
};
use futures_util::{Stream, StreamExt};
use std::path::{Path, PathBuf};
use tempfile::TempPath;
use tokio::{fs, io::AsyncWriteExt};

const MAX_PATH_BYTES: usize = 16 * 1024;

struct StagedUpload {
    target: PathBuf,
    temporary: TempPath,
}

async fn read_path(mut field: Field<'_>) -> Result<String, AppError> {
    let mut path = Vec::new();
    while let Some(chunk) = field.chunk().await? {
        if chunk.len() > MAX_PATH_BYTES - path.len() {
            return Err(AppError::bad_request("Upload path is too long"));
        }
        path.extend_from_slice(&chunk);
    }
    String::from_utf8(path).map_err(|_| AppError::bad_request("Upload path must be UTF-8"))
}

async fn stage_file<S, E>(target: PathBuf, chunks: S) -> Result<StagedUpload>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let parent = target
        .parent()
        .expect("upload target must have a parent")
        .to_owned();
    let temporary =
        tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(parent)).await??;
    let (file, temporary) = temporary.into_parts();
    let mut output = fs::File::from_std(file);
    futures_util::pin_mut!(chunks);
    while let Some(chunk) = chunks.next().await {
        output.write_all(&chunk?).await?;
    }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    Ok(StagedUpload { target, temporary })
}

pub async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<(), AppError> {
    let mut directory = None;
    let mut staged = Vec::new();

    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("path") => {
                if directory.is_some() {
                    return Err(AppError::bad_request(
                        "Expected one path field before the file fields",
                    ));
                }
                let path = read_path(field).await?;
                directory = Some(resolve_directory(&state.data, &path).await?);
            }
            Some("file") => {
                let parent: &Path = directory.as_deref().ok_or_else(|| {
                    AppError::bad_request("Path field must precede the file fields")
                })?;
                let name = field
                    .file_name()
                    .filter(|name| is_normal_component(name))
                    .ok_or_else(|| AppError::bad_request("Invalid file name"))?;
                let target = parent.join(name);
                match fs::metadata(&target).await {
                    Ok(metadata) if metadata.is_dir() => {
                        return Err(AppError::bad_request(
                            "File name matches an existing directory",
                        ));
                    }
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error.into());
                    }
                    _ => {}
                }
                staged.push(stage_file(target, field).await?);
            }
            _ => {}
        }
    }

    if directory.is_none() {
        return Err(AppError::bad_request("Missing path field"));
    }
    if staged.is_empty() {
        return Err(AppError::bad_request("Missing file field"));
    }

    // 表单校验结束后由独立任务提交整批文件，客户端断开也不丢弃余下文件。
    tokio::spawn(async move {
        for StagedUpload { target, temporary } in staged {
            state
                .media
                .replace(&target, move |destination| {
                    temporary
                        .persist(destination)
                        .map_err(|error| error.error)?;
                    Ok(())
                })
                .await?;
        }
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::FromRequest,
        http::{Request, StatusCode, header},
        response::IntoResponse,
    };
    use std::collections::HashMap;

    async fn multipart(body: String) -> Multipart {
        let request = Request::post("/api/upload")
            .header(
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=boundary",
            )
            .body(Body::from(body))
            .unwrap();
        Multipart::from_request(request, &()).await.unwrap()
    }

    fn state(root: &Path) -> State<AppState> {
        State(AppState {
            data: HashMap::from([("media".to_string(), root.to_owned())]),
            ..Default::default()
        })
    }

    const PATH: &str =
        "--boundary\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nmedia\r\n";
    const FILE: &str = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.mp4\"\r\n\r\nnew content\r\n";
    const END: &str = "--boundary--\r\n";

    #[tokio::test]
    async fn trailing_invalid_field_keeps_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("clip.mp4");
        fs::write(&target, b"old content").await.unwrap();
        let form = multipart(format!("{PATH}{FILE}{PATH}{END}")).await;
        let error = upload(state(directory.path()), form).await.unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(fs::read(&target).await.unwrap(), b"old content");
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
    }

    #[tokio::test]
    async fn complete_form_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("clip.mp4");
        fs::write(&target, b"old content").await.unwrap();
        let form = multipart(format!("{PATH}{FILE}{END}")).await;
        upload(state(directory.path()), form).await.unwrap();
        assert_eq!(fs::read(&target).await.unwrap(), b"new content");
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
    }

    #[tokio::test]
    async fn interrupted_stream_discards_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("clip.mp4");
        fs::write(&target, b"old content").await.unwrap();
        let chunks = futures_util::stream::iter([
            Ok(Bytes::from_static(b"new content")),
            Err(std::io::Error::other("upload interrupted")),
        ]);
        assert!(stage_file(target.clone(), chunks).await.is_err());
        assert_eq!(fs::read(&target).await.unwrap(), b"old content");
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
    }

    #[tokio::test]
    async fn rejects_an_oversized_path_field() {
        let directory = tempfile::tempdir().unwrap();
        let path = "x".repeat(MAX_PATH_BYTES + 1);
        let form = multipart(format!(
            "--boundary\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\n{path}\r\n{END}"
        ))
        .await;
        let error = upload(state(directory.path()), form).await.unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 100)
            .await
            .unwrap();
        assert_eq!(&body[..], b"Upload path is too long");
    }
}
