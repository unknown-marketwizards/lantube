use crate::{
    AppState,
    error::AppError,
    library::{
        check_revision, opened_file_revision, read_revision, require_revision, resolve_file,
    },
    transcode::{ReadLease, probe_chapter_duration},
};
use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

const MAX_CHAPTERS: usize = 200;
const MAX_TITLE_LENGTH: usize = 120;
const MIN_CHAPTER_MS: u64 = 500;
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
const CHECKSUM_CACHE_SIZE: usize = 128;
const CHECKSUM_WORKERS: usize = 2;

type Checksum = [u8; 32];
type CachedChecksum = Arc<AsyncMutex<Option<Checksum>>>;

pub struct ChapterStore {
    directory: PathBuf,
    writes: Mutex<()>,
    checksums: Mutex<VecDeque<(PathBuf, u64, CachedChecksum)>>,
    checksum_slots: Arc<Semaphore>,
    #[cfg(test)]
    checksum_reads: std::sync::atomic::AtomicUsize,
}

impl Default for ChapterStore {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            writes: Mutex::default(),
            checksums: Mutex::default(),
            checksum_slots: Arc::new(Semaphore::new(CHECKSUM_WORKERS)),
            #[cfg(test)]
            checksum_reads: std::sync::atomic::AtomicUsize::default(),
        }
    }
}

impl ChapterStore {
    pub fn open(directory: &Path) -> io::Result<Self> {
        Ok(Self {
            directory: resolve_directory_path(directory)?,
            ..Self::default()
        })
    }

    pub fn overlaps(&self, root: &Path) -> bool {
        if self.directory.as_os_str().is_empty() {
            return false;
        }
        let directory = self.directory.as_path();
        #[cfg(windows)]
        let (root, directory) = (
            PathBuf::from(root.to_string_lossy().to_lowercase()),
            PathBuf::from(directory.to_string_lossy().to_lowercase()),
        );
        root.starts_with(&directory) || directory.starts_with(root)
    }

    fn check_directory(&self) -> Result<(), AppError> {
        // 媒体路由以启动时解析出的路径隔离章节目录。
        if resolve_directory_path(&self.directory)? != self.directory {
            return Err(AppError::forbidden(
                "Chapter storage directory was redirected",
            ));
        }
        Ok(())
    }

    async fn cached_checksum(
        self: &Arc<Self>,
        input: &Path,
        revision: u64,
        read: &ReadLease,
    ) -> Result<Checksum, AppError> {
        let entry = {
            let mut cache = self
                .checksums
                .lock()
                .expect("checksum cache is not poisoned");
            let entry = cache
                .iter()
                .position(|(path, version, _)| path == input && *version == revision)
                .map(|index| {
                    cache
                        .remove(index)
                        .expect("position is in the checksum cache")
                        .2
                })
                .unwrap_or_default();
            // 正在校验或等待校验的请求共享同一个条目。
            while cache.len() >= CHECKSUM_CACHE_SIZE {
                let Some(index) = cache
                    .iter()
                    .rposition(|(_, _, item)| Arc::strong_count(item) == 1)
                else {
                    break;
                };
                cache.remove(index);
            }
            cache.push_front((input.to_owned(), revision, Arc::clone(&entry)));
            entry
        };
        let mut cached = entry.lock_owned().await;
        let actual = if let Some(actual) = *cached {
            drop(cached);
            actual
        } else {
            // 同一版本先合并请求，只有实际扫描才占用一个名额。
            let slot = Arc::clone(&self.checksum_slots)
                .acquire_owned()
                .await
                .expect("checksum semaphore must stay open");
            let path = input.to_owned();
            let read = Arc::clone(read);
            #[cfg(test)]
            let store = Arc::clone(self);
            tokio::task::spawn_blocking(move || {
                let _slot = slot;
                let _read = read;
                #[cfg(test)]
                store
                    .checksum_reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let actual = checksum(&path, revision)?;
                *cached = Some(actual);
                Ok::<_, AppError>(actual)
            })
            .await??
        };
        check_revision(input, revision).await?;
        Ok(actual)
    }

    fn read_record(&self, input: &Path) -> Result<Option<StoredChapters>, AppError> {
        self.check_directory()?;
        let path = self.path(input);
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(serde_json::from_reader(BufReader::new(file))?))
    }

    async fn read_snapshot(
        self: &Arc<Self>,
        input: &Path,
        revision: u64,
        read: &ReadLease,
    ) -> Result<Option<StoredChapters>, AppError> {
        let path = input.to_owned();
        let store = Arc::clone(self);
        let record = tokio::task::spawn_blocking(move || {
            require_revision(read_revision(&path)?, revision)?;
            store.read_record(&path)
        })
        .await??;
        // 播放版本受文件系统影响，章节归属按内容核对。
        let actual = if record
            .as_ref()
            .is_some_and(|record| record.media.revision != revision)
        {
            Some(self.cached_checksum(input, revision, read).await?)
        } else {
            None
        };
        matching_record(record, revision, actual)
    }

    async fn load(
        self: &Arc<Self>,
        input: &Path,
        revision: u64,
        read: &ReadLease,
    ) -> Result<ChapterSet, AppError> {
        let record = self.read_snapshot(input, revision, read).await?;
        let chapters = record.map(|record| record.chapters).unwrap_or_default();
        ChapterSet::new(input, revision, chapters)
    }

    async fn save(
        self: &Arc<Self>,
        input: &Path,
        update: ChapterUpdate,
        read: &ReadLease,
    ) -> Result<ChapterSet, AppError> {
        let revision = update.revision;
        let saved = self.read_snapshot(input, revision, read).await?;
        let next = ChapterSet::new(input, revision, update.chapters)?;
        let observed = ChapterSet::stored_version(input, revision, saved.as_ref())?;
        if next.version == observed {
            return Ok(next);
        }
        if observed != update.version {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "Chapters changed since they were loaded",
            ));
        }
        let checksum = match saved {
            Some(record) => record.checksum,
            None => self.cached_checksum(input, revision, read).await?,
        };
        let input = input.to_owned();
        let read = Arc::clone(read);
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _read = read;
            store.commit(&input, revision, checksum, &update.version, next)
        })
        .await?
    }

    fn commit(
        &self,
        input: &Path,
        revision: u64,
        checksum: Checksum,
        version: &str,
        next: ChapterSet,
    ) -> Result<ChapterSet, AppError> {
        let _write = self
            .writes
            .lock()
            .expect("chapter store lock is not poisoned");
        require_revision(read_revision(input)?, revision)?;
        let saved = matching_record(self.read_record(input)?, revision, Some(checksum))?;
        let current = ChapterSet::stored_version(input, revision, saved.as_ref())?;
        if next.version == current {
            return Ok(next);
        }
        if current != version {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "Chapters or media changed since they were loaded",
            ));
        }

        fs::create_dir_all(&self.directory)?;
        self.check_directory()?;
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(
            file.as_file_mut(),
            &StoredChapters {
                media: StoredMedia { revision },
                checksum,
                chapters: next.chapters.clone(),
            },
        )?;
        file.as_file().sync_all()?;
        require_revision(read_revision(input)?, revision)?;
        self.check_directory()?;
        file.persist(self.path(input))
            .map_err(|error| error.error)?;
        Ok(next)
    }

    fn path(&self, input: &Path) -> PathBuf {
        let digest = Sha256::digest(input.as_os_str().as_encoded_bytes());
        let suffix = ".verified.json";
        let mut name = String::with_capacity(digest.len() * 2 + suffix.len());
        for byte in digest {
            use std::fmt::Write;
            write!(name, "{byte:02x}").expect("writing to a string is infallible");
        }
        name.push_str(suffix);
        self.directory.join(name)
    }
}

fn resolve_directory_path(path: &Path) -> io::Result<PathBuf> {
    // 首次保存才创建目录，先解析已存在的父目录。
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                resolved.extend(missing.into_iter().rev());
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(ancestor) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => return Err(error),
                }
                let Some((parent, component)) = ancestor.parent().zip(ancestor.file_name()) else {
                    return Err(error);
                };
                missing.push(component);
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Chapter {
    start_ms: u64,
    title: String,
}

#[derive(Deserialize, Serialize)]
struct StoredChapters {
    media: StoredMedia,
    checksum: Checksum,
    chapters: Vec<Chapter>,
}

#[derive(Deserialize, Serialize)]
struct StoredMedia {
    revision: u64,
}

fn matching_record(
    record: Option<StoredChapters>,
    revision: u64,
    actual: Option<Checksum>,
) -> Result<Option<StoredChapters>, AppError> {
    let record = record
        .filter(|record| record.media.revision == revision || actual == Some(record.checksum));
    if let Some(record) = &record {
        validate(&record.chapters).map_err(|_| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "Invalid saved chapters")
        })?;
    }
    Ok(record)
}

#[derive(Serialize)]
pub struct ChapterSet {
    version: String,
    chapters: Vec<Chapter>,
}

impl ChapterSet {
    fn stored_version(
        input: &Path,
        revision: u64,
        record: Option<&StoredChapters>,
    ) -> Result<String, AppError> {
        Self::version(
            input,
            revision,
            record.map_or(&[], |record| &record.chapters),
        )
    }

    fn new(input: &Path, media: u64, chapters: Vec<Chapter>) -> Result<Self, AppError> {
        Ok(Self {
            version: Self::version(input, media, &chapters)?,
            chapters,
        })
    }

    fn version(input: &Path, media: u64, chapters: &[Chapter]) -> Result<String, AppError> {
        let mut digest = Sha256::new();
        digest.update(input.as_os_str().as_encoded_bytes());
        digest.update(serde_json::to_vec(&(media, &chapters))?);
        Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
    }
}

#[derive(Deserialize)]
pub struct ChapterQuery {
    path: String,
    revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChapterUpdate {
    path: String,
    #[serde(deserialize_with = "revision_from_string")]
    revision: u64,
    version: String,
    chapters: Vec<Chapter>,
}

fn revision_from_string<'de, D: serde::Deserializer<'de>>(input: D) -> Result<u64, D::Error> {
    String::deserialize(input)?
        .parse()
        .map_err(serde::de::Error::custom)
}

pub async fn load(
    State(state): State<AppState>,
    Query(query): Query<ChapterQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], Json<ChapterSet>), AppError> {
    let input = resolve_file(&state, &query.path).await?;
    let read = state.media.read(&input)?;
    let result = state.chapters.load(&input, query.revision, &read).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(result)))
}

fn checksum(input: &Path, revision: u64) -> Result<Checksum, AppError> {
    let mut file = fs::File::open(input)?;
    require_revision(opened_file_revision(&file)?, revision)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    require_revision(opened_file_revision(&file)?, revision)?;
    Ok(digest.finalize().into())
}

pub async fn save(
    State(state): State<AppState>,
    Json(update): Json<ChapterUpdate>,
) -> Result<Json<ChapterSet>, AppError> {
    validate(&update.chapters)?;
    let input = resolve_file(&state, &update.path).await?;
    let read = state.media.read(&input)?;
    check_revision(&input, update.revision).await?;
    if let Some(last) = update
        .chapters
        .last()
        .filter(|chapter| chapter.start_ms > 0)
    {
        let duration = probe_chapter_duration(&state.tools, &input, &read).await;
        check_revision(&input, update.revision).await?;
        let duration = duration?;
        validate_last_cut(last.start_ms, duration)?;
    }
    let result = state.chapters.save(&input, update, &read).await?;
    Ok(Json(result))
}

fn validate_last_cut(start_ms: u64, duration: f64) -> Result<(), AppError> {
    if start_ms as f64 / 1000.0 >= duration {
        return Err(AppError::bad_request(
            "Chapter starts must be before the end of the media",
        ));
    }
    Ok(())
}

fn validate(chapters: &[Chapter]) -> Result<(), AppError> {
    if chapters.len() > MAX_CHAPTERS {
        return Err(AppError::bad_request("Too many chapters"));
    }
    if chapters.first().is_some_and(|first| first.start_ms != 0) {
        return Err(AppError::bad_request("Chapters must start at zero"));
    }
    if chapters.iter().any(|chapter| {
        chapter.start_ms > MAX_SAFE_INTEGER
            || chapter.title.chars().count() > MAX_TITLE_LENGTH
            || chapter.title.chars().any(char::is_control)
    }) {
        return Err(AppError::bad_request("Invalid chapter time or title"));
    }
    if chapters
        .windows(2)
        .any(|pair| pair[1].start_ms.saturating_sub(pair[0].start_ms) < MIN_CHAPTER_MS)
    {
        return Err(AppError::bad_request(
            "Chapter starts must be at least 500 ms apart",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, response::IntoResponse};
    use std::{collections::HashMap, sync::atomic::Ordering, time::Duration};
    use tower::ServiceExt;

    fn chapter(start_ms: u64) -> Chapter {
        Chapter {
            start_ms,
            title: String::new(),
        }
    }

    async fn store_test_chapters(store: &Arc<ChapterStore>, input: &Path) -> u64 {
        fs::write(input, b"original").unwrap();
        let revision = read_revision(input).unwrap();
        let read = crate::transcode::MediaAccess::default()
            .read(input)
            .unwrap();
        let initial = store.load(input, revision, &read).await.unwrap();
        store
            .save(
                input,
                ChapterUpdate {
                    path: String::new(),
                    revision,
                    version: initial.version,
                    chapters: vec![chapter(0), chapter(1750)],
                },
                &read,
            )
            .await
            .unwrap();
        revision
    }

    fn change_modified_time(input: &Path) -> u64 {
        let file = fs::OpenOptions::new().write(true).open(input).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified + Duration::from_secs(60)))
            .unwrap();
        read_revision(input).unwrap()
    }

    #[test]
    fn rejects_missing_zero_unsorted_nearby_or_unsafe_chapter_starts() {
        for chapters in [
            vec![chapter(500)],
            vec![chapter(0), chapter(0)],
            vec![chapter(0), chapter(499)],
            vec![chapter(0), chapter(1500), chapter(500)],
            vec![chapter(0), chapter(MAX_SAFE_INTEGER + 1)],
            vec![chapter(0); MAX_CHAPTERS + 1],
        ] {
            assert_eq!(
                validate(&chapters).unwrap_err().into_response().status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert!(validate(&[chapter(0), chapter(500)]).is_ok());
        assert!(validate(&[]).is_ok());
    }

    #[test]
    fn rejects_long_and_multiline_titles() {
        for title in ["章".repeat(MAX_TITLE_LENGTH + 1), "line\nbreak".into()] {
            assert!(validate(&[Chapter { start_ms: 0, title }]).is_err());
        }
        assert!(
            validate(&[Chapter {
                start_ms: 0,
                title: "章".repeat(MAX_TITLE_LENGTH)
            }])
            .is_ok()
        );
    }

    #[tokio::test]
    async fn preserves_chapters_across_restarts_and_rejects_stale_edits() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("example.mp4");
        let directory = folder.path().join("chapters");
        fs::write(&input, b"original").unwrap();
        let store = Arc::new(ChapterStore::open(&directory).unwrap());
        let read = crate::transcode::MediaAccess::default()
            .read(&input)
            .unwrap();
        let load = async |store: &Arc<ChapterStore>| {
            let revision = read_revision(&input).unwrap();
            store.load(&input, revision, &read).await.unwrap()
        };
        let first = load(&store).await;
        assert!(first.chapters.is_empty());
        assert!(!directory.exists());
        let revision = read_revision(&input).unwrap();
        let update = |version, end| ChapterUpdate {
            path: String::new(),
            revision,
            version,
            chapters: vec![chapter(0), chapter(end)],
        };
        let saved = store
            .save(&input, update(first.version.clone(), 1750), &read)
            .await
            .unwrap();
        let restarted = Arc::new(ChapterStore::open(&directory).unwrap());
        assert_eq!(load(&restarted).await.chapters[1].start_ms, 1750);
        assert_eq!(fs::read(&input).unwrap(), b"original");

        let repeated = restarted
            .save(&input, update(first.version.clone(), 1750), &read)
            .await
            .unwrap();
        assert_eq!(repeated.version, saved.version);
        let conflict = restarted
            .save(&input, update(first.version, 3000), &read)
            .await
            .err()
            .unwrap();
        assert_eq!(conflict.into_response().status(), StatusCode::CONFLICT);

        let edited = restarted
            .save(&input, update(saved.version, 2500), &read)
            .await
            .unwrap();
        assert_eq!(load(&restarted).await.chapters[1].start_ms, 2500);
        let original = fs::metadata(&input).unwrap();
        let mut replacement = tempfile::NamedTempFile::new_in(folder.path()).unwrap();
        io::Write::write_all(&mut replacement, b"replaced").unwrap();
        let times = fs::FileTimes::new().set_modified(original.modified().unwrap());
        #[cfg(windows)]
        let times = {
            use std::os::windows::fs::FileTimesExt;
            times.set_created(original.created().unwrap())
        };
        replacement.as_file().set_times(times).unwrap();
        replacement.persist(&input).unwrap();
        let replaced = fs::metadata(&input).unwrap();
        assert_eq!(original.len(), replaced.len());
        assert_eq!(original.modified().unwrap(), replaced.modified().unwrap());
        assert!(load(&restarted).await.chapters.is_empty());
        let conflict = restarted
            .save(&input, update(edited.version, 3000), &read)
            .await
            .err()
            .unwrap();
        assert_eq!(conflict.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn preserves_chapters_after_metadata_changes_but_rejects_modified_content() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("example.mp4");
        let store = Arc::new(ChapterStore::open(&folder.path().join("chapters")).unwrap());
        let before = store_test_chapters(&store, &input).await;
        let read = crate::transcode::MediaAccess::default()
            .read(&input)
            .unwrap();

        let metadata = fs::metadata(&input).unwrap();
        let mut permissions = metadata.permissions();
        let original_permissions = permissions.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() ^ 0o100);
        }
        #[cfg(windows)]
        permissions.set_readonly(true);
        fs::set_permissions(&input, permissions).unwrap();

        let changed = read_revision(&input).unwrap();
        assert_ne!(changed, before);
        assert_eq!(
            store.load(&input, changed, &read).await.unwrap().chapters[1].start_ms,
            1750
        );
        fs::set_permissions(&input, original_permissions).unwrap();

        // 让权限调整和内容覆盖跨过文件系统的 ctime 时钟刻度。
        tokio::time::sleep(Duration::from_millis(20)).await;
        fs::write(&input, b"replaced").unwrap();
        let file = fs::OpenOptions::new().write(true).open(&input).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        let replaced = read_revision(&input).unwrap();
        assert_ne!(replaced, changed);
        assert!(
            store
                .load(&input, replaced, &read)
                .await
                .unwrap()
                .chapters
                .is_empty()
        );
    }

    #[tokio::test]
    async fn preserves_chapters_when_only_the_modified_time_changes() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("example.mp4");
        let directory = folder.path().join("chapters");
        let before =
            store_test_chapters(&Arc::new(ChapterStore::open(&directory).unwrap()), &input).await;
        let changed = change_modified_time(&input);
        assert_ne!(changed, before);

        let restarted = Arc::new(ChapterStore::open(&directory).unwrap());
        let read = crate::transcode::MediaAccess::default()
            .read(&input)
            .unwrap();
        let chapters = restarted
            .load(&input, changed, &read)
            .await
            .unwrap()
            .chapters;
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].start_ms, 1750);
    }

    #[tokio::test]
    async fn preserves_chapters_for_identical_media_with_a_different_file_identifier() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("example.mp4");
        let directory = folder.path().join("chapters");
        let before =
            store_test_chapters(&Arc::new(ChapterStore::open(&directory).unwrap()), &input).await;
        let original = fs::metadata(&input).unwrap();
        let mut remounted = tempfile::NamedTempFile::new_in(folder.path()).unwrap();
        io::Write::write_all(&mut remounted, b"original").unwrap();
        remounted
            .as_file()
            .set_times(fs::FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        remounted.persist(&input).unwrap();
        let revision = read_revision(&input).unwrap();
        assert_ne!(revision, before);

        let access = crate::transcode::MediaAccess::default();
        let read = access.read(&input).unwrap();
        let restarted = Arc::new(ChapterStore::open(&directory).unwrap());
        let saved = restarted.load(&input, revision, &read).await.unwrap();
        assert_eq!(saved.chapters[1].start_ms, 1750);
        assert_eq!(restarted.checksum_reads.load(Ordering::Relaxed), 1);
        let stale = restarted.load(&input, before, &read).await.err().unwrap();
        assert_eq!(stale.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_chapter_requests_share_one_checksum_per_media_version() {
        let folder = tempfile::tempdir().unwrap();
        let share = folder.path().join("share");
        fs::create_dir(&share).unwrap();
        let share = fs::canonicalize(share).unwrap();
        let input = share.join("example.mp4");
        let directory = folder.path().join("chapters");
        store_test_chapters(&Arc::new(ChapterStore::open(&directory).unwrap()), &input).await;
        let store = Arc::new(ChapterStore::open(&directory).unwrap());
        let app = crate::router(
            AppState {
                data: HashMap::from([("share".to_owned(), share)]),
                chapters: Arc::clone(&store),
                ..AppState::default()
            },
            crate::auth::Auth::default(),
        );
        let read = |revision| {
            let request = Request::get(format!(
                "/api/chapters?path=%2Fshare%2Fexample.mp4&revision={revision}"
            ))
            .body(Body::empty())
            .unwrap();
            app.clone().oneshot(request)
        };
        let revision = change_modified_time(&input);
        let start = tokio::sync::Barrier::new(8);
        let requests = (0..8).map(|_| async {
            start.wait().await;
            read(revision).await.unwrap()
        });
        for response in futures_util::future::join_all(requests).await {
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let saved: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(saved["chapters"][1]["startMs"], 1750);
        }
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), 1);

        assert_eq!(read(revision).await.unwrap().status(), StatusCode::OK);
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), 1);

        let revised = change_modified_time(&input);
        assert_ne!(revised, revision);
        assert_eq!(read(revision).await.unwrap().status(), StatusCode::CONFLICT);
        assert_eq!(read(revised).await.unwrap().status(), StatusCode::OK);
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn checksums_for_different_files_wait_for_a_shared_scan_slot() {
        let folder = tempfile::tempdir().unwrap();
        let store = Arc::new(ChapterStore::open(&folder.path().join("chapters")).unwrap());
        let access = crate::transcode::MediaAccess::default();
        let inputs: Vec<_> = (0..=CHECKSUM_WORKERS)
            .map(|index| {
                let path = folder.path().join(format!("{index}.mp4"));
                fs::write(&path, format!("video {index}")).unwrap();
                let revision = read_revision(&path).unwrap();
                let read = access.read(&path).unwrap();
                (path, revision, read)
            })
            .collect();

        // 模拟扫描名额都被占用，多个文件需异步等待，不能启动阻塞任务。
        let slots: Vec<_> = (0..CHECKSUM_WORKERS)
            .map(|_| store.checksum_slots.try_acquire().unwrap())
            .collect();
        assert!(store.checksum_slots.try_acquire().is_err());
        let mut pending: Vec<_> = inputs
            .iter()
            .map(|(path, revision, read)| Box::pin(store.cached_checksum(path, *revision, read)))
            .collect();
        for checksum in &mut pending {
            assert!(futures_util::poll!(checksum.as_mut()).is_pending());
        }
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), 0);

        drop(slots);
        for (index, checksum) in pending.into_iter().enumerate() {
            assert_eq!(
                checksum.await.unwrap(),
                <Checksum>::from(Sha256::digest(format!("video {index}")))
            );
        }
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), inputs.len());

        let permits = store
            .checksum_slots
            .acquire_many(CHECKSUM_WORKERS as u32)
            .await
            .unwrap();
        let (path, revision, read) = &inputs[0];
        tokio::time::timeout(
            Duration::from_secs(2),
            store.cached_checksum(path, *revision, read),
        )
        .await
        .expect("a cache hit does not need a scan slot")
        .unwrap();
        assert_eq!(store.checksum_reads.load(Ordering::Relaxed), inputs.len());
        drop(permits);
    }

    #[test]
    fn rejects_cuts_at_or_beyond_the_end_of_the_media() {
        assert!(validate_last_cut(9_999, 10.0).is_ok());
        for start in [10_000, 60_000] {
            let error = validate_last_cut(start, 10.0).unwrap_err();
            assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        }
    }
}
