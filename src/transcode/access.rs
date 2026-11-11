use super::audio::AudioScratch;
use crate::error::AppError;
use axum::http::StatusCode;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

pub type ReadLease = Arc<OwnedRwLockReadGuard<()>>;

#[derive(Default)]
pub struct MediaAccess {
    files: Mutex<HashMap<PathBuf, Weak<RwLock<()>>>>,
    pub audio: Arc<AudioScratch>,
}

impl MediaAccess {
    fn gate(&self, path: &Path) -> Arc<RwLock<()>> {
        #[cfg(windows)]
        let path = PathBuf::from(path.to_string_lossy().to_lowercase());
        #[cfg(windows)]
        let path = path.as_path();
        let mut files = self.files.lock().expect("media index is not poisoned");
        files.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = files.get(path).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(RwLock::new(()));
        files.insert(path.to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub fn read(&self, path: &Path) -> Result<ReadLease, AppError> {
        self.gate(path)
            .try_read_owned()
            .map(Arc::new)
            .map_err(|_| AppError::new(StatusCode::CONFLICT, "Media replacement in progress"))
    }

    pub async fn replace(
        &self,
        target: &Path,
        commit: impl FnOnce(PathBuf) -> Result<(), AppError> + Send + 'static,
    ) -> Result<(), AppError> {
        let destination = upload_entry(target).await?;
        // Windows 不允许在读取进程还持有目标时原子覆盖。
        let write = self.gate(&destination).write_owned().await;
        self.audio.close(&destination).await?;
        tokio::task::spawn_blocking(move || {
            let _write = write;
            commit(destination)
        })
        .await?
    }
}

#[cfg(not(windows))]
async fn upload_entry(path: &Path) -> Result<PathBuf, AppError> {
    Ok(path.to_owned())
}

#[cfg(windows)]
async fn upload_entry(path: &Path) -> Result<PathBuf, AppError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::{
            ffi::OsString,
            io,
            os::windows::ffi::{OsStrExt, OsStringExt},
        };
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW},
        };

        let query: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut data = WIN32_FIND_DATAW::default();
        // 查询真实目录项名称以展开 NTFS 短名，但不跟随该目录项上的符号链接。
        // query 以 NUL 结尾，data 由成功的 FindFirstFileW 初始化。
        let handle = unsafe { FindFirstFileW(query.as_ptr(), &mut data) };
        if handle == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(path)
            } else {
                Err(error.into())
            };
        }
        // 句柄是本次查询成功返回的有效句柄。
        if unsafe { FindClose(handle) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let name = data
            .cFileName
            .split(|unit| *unit == 0)
            .next()
            .expect("directory entry has a name");
        Ok(path.with_file_name(OsString::from_wide(name)))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::fs;
    use tokio::{task::yield_now, time::timeout};

    #[tokio::test]
    async fn replacement_waits_for_existing_readers_and_leaves_other_files_available() {
        let folder = tempfile::tempdir().unwrap();
        let a = folder.path().join("a.mp4");
        fs::write(&a, b"source").await.unwrap();
        let a = fs::canonicalize(a).await.unwrap();
        let b = a.with_file_name("b.mp4");
        let access = Arc::new(MediaAccess::default());
        let reader = access.read(&a).unwrap();
        let replacement = tokio::spawn({
            let access = Arc::clone(&access);
            let a = a.clone();
            async move { access.replace(&a, |_| Ok(())).await }
        });
        timeout(Duration::from_secs(2), async {
            while access.read(&a).is_ok() {
                yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(access.read(&b).is_ok());
        assert!(!replacement.is_finished());
        drop(reader);
        replacement.await.unwrap().unwrap();
        assert!(access.read(&a).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn replacing_a_link_does_not_wait_for_readers_of_the_old_target() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("target.mp4");
        let link = folder.path().join("link.mp4");
        let staged = folder.path().join("upload");
        fs::write(&target, b"old target").await.unwrap();
        fs::write(&staged, b"replacement").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let access = MediaAccess::default();
        let _reader = access.read(&target).unwrap();
        timeout(
            Duration::from_secs(2),
            access.replace(&link, move |destination| {
                std::fs::rename(staged, destination)?;
                Ok(())
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fs::read(target).await.unwrap(), b"old target");
        assert_eq!(fs::read(link).await.unwrap(), b"replacement");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn queued_upload_does_not_follow_a_link_swapped_into_its_destination() {
        let library = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let destination = library.path().join("video.mp4");
        let staged = library.path().join("upload");
        let unrelated = outside.path().join("unrelated.mp4");
        fs::write(&destination, b"old source").await.unwrap();
        fs::write(&staged, b"new source").await.unwrap();
        fs::write(&unrelated, b"outside source").await.unwrap();

        let access = Arc::new(MediaAccess::default());
        let reader = access.read(&destination).unwrap();
        let upload = tokio::spawn({
            let access = Arc::clone(&access);
            let destination = destination.clone();
            async move {
                access
                    .replace(&destination, move |entry| {
                        std::fs::rename(staged, entry)?;
                        Ok(())
                    })
                    .await
            }
        });
        timeout(Duration::from_secs(2), async {
            while access.read(&destination).is_ok() {
                yield_now().await;
            }
        })
        .await
        .unwrap();
        fs::remove_file(&destination).await.unwrap();
        std::os::unix::fs::symlink(&unrelated, &destination).unwrap();
        drop(reader);
        upload.await.unwrap().unwrap();

        assert_eq!(fs::read(destination).await.unwrap(), b"new source");
        assert_eq!(fs::read(unrelated).await.unwrap(), b"outside source");
    }
}
