mod access;
mod audio;
mod plan;
mod timeline;
mod tools;

pub use access::MediaAccess;
pub use tools::MediaTools;

use crate::{AppState, error::AppError, library::resolve_file};
use access::ReadLease;
use audio::{add_audio_input, audio_slice};
use axum::{
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use plan::PlaybackPlan;
use serde::Deserialize;
use std::{
    convert::Infallible,
    fmt::Write,
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    time::Duration,
};
use timeline::{Segment, playback_timeline, thumbnail_timing};
use tokio::{fs, sync::Semaphore, time::timeout};
use tools::{ffmpeg, ffmpeg_command, run, run_with_audio};

const FRAME_RATE: u32 = 30;
const SEGMENT_SECONDS: u32 = 4;
const MIN_TAIL_SECONDS: f64 = 0.5;
const MAX_SEGMENTS: u32 = 100_000;
const VIDEO_FILTER: &str = "scale=w='min(1920,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2";
const AUDIO_FILTER: &str = "aresample=async=1:first_pts=0,apad";

static PLAYBACK_SLOTS: Semaphore = Semaphore::const_new(2);
static THUMBNAIL_SLOTS: Semaphore = Semaphore::const_new(1);

#[derive(Deserialize)]
pub struct PlaybackQuery {
    segment: Option<u32>,
    plan: Option<String>,
}

pub async fn playback(
    State(state): State<AppState>,
    RoutePath(path): RoutePath<String>,
    Query(query): Query<PlaybackQuery>,
    method: Method,
) -> Result<Response, AppError> {
    let input = resolve_file(&state.data, &path).await?;
    let read = state.media.read(&input)?;
    if let Some(index) = query.segment {
        let token = query
            .plan
            .as_deref()
            .ok_or_else(|| AppError::bad_request("Missing playback plan"))?;
        let plan = PlaybackPlan::decode(&path, token)?;
        let segment = plan.timeline.segment(index)?;
        let bytes = with_media_slot(
            &PLAYBACK_SLOTS,
            encode_segment(&state, &input, plan.revision, segment, &read),
        )
        .await?;
        return Ok((
            [
                (header::CONTENT_TYPE, "video/mp2t"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            bytes,
        )
            .into_response());
    }

    let revision = file_revision(&input).await?;
    let timeline = with_media_slot(
        &PLAYBACK_SLOTS,
        playback_timeline(&state.tools, &input, method, &read),
    )
    .await?;
    check_revision(&input, revision).await?;
    let headers = [
        (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
        (header::CACHE_CONTROL, "no-store"),
    ];
    let Some(timeline) = timeline else {
        // 原生 HLS 的 HEAD 只确认有视频轨，逐包探测留给实际请求。
        return Ok((headers, ()).into_response());
    };
    let plan = PlaybackPlan { revision, timeline };
    let token = plan.encode(&path)?;
    let duration = plan.timeline.duration.to_string();
    let chunks = plan.timeline.playlist_chunks(token)?;
    Ok((
        headers,
        [(
            header::HeaderName::from_static("x-media-duration"),
            duration,
        )],
        Body::from_stream(stream::iter(chunks.map(Ok::<_, Infallible>))),
    )
        .into_response())
}

async fn encode_segment(
    state: &AppState,
    input: &Path,
    revision: u64,
    segment: Segment,
    read: &ReadLease,
) -> Result<Vec<u8>, AppError> {
    check_revision(input, revision).await?;
    let derived = if let (Some(track), Some(window)) = (segment.audio, segment.audio_window) {
        Some((
            window,
            state
                .media
                .audio
                .track(&state.tools, input, revision, track, window, read)
                .await?,
        ))
    } else {
        None
    };
    let trim = segment.offset - segment.seek;
    let needs_original = derived.is_none() || segment.picture.is_some();
    // 音频已单独定位时，从实际首帧开始找画面，避免部分容器在首帧之前定位失败。
    let picture_delay = segment
        .picture
        .as_ref()
        .filter(|_| derived.is_some())
        .map_or(0.0, |picture| picture.start);
    let video_seek = segment.seek + picture_delay;
    let mut command = if needs_original {
        ffmpeg(
            &state.tools,
            input,
            (video_seek > 0.0).then_some(video_seek),
        )?
    } else {
        ffmpeg_command(&state.tools)?
    };

    let video = if segment.picture.is_some() {
        "0:V:0"
    } else {
        command.args([
            "-itsoffset",
            &trim.to_string(),
            "-f",
            "lavfi",
            "-i",
            "color=s=640x360:r=30",
        ]);
        if needs_original { "1:v:0" } else { "0:v:0" }
    };
    let pcm = if let Some((window, track)) = &derived {
        add_audio_input(&mut command);
        Some(audio_slice(track, *window, segment.offset, segment.length)?)
    } else {
        None
    };
    if let Some(picture) = &segment.picture {
        let mut filter = if picture_delay > 0.0 {
            hold_frames(
                &format!("{VIDEO_FILTER},setpts=PTS+{picture_delay}/TB"),
                trim,
            )
        } else {
            hold_frames(VIDEO_FILTER, trim)
        };
        if picture.start > 0.0 || picture.end < segment.length {
            write!(
                filter,
                ",drawbox=c=black:t=fill:enable='lt(t,{})+gte(t,{})'",
                trim + picture.start,
                trim + picture.end,
            )
            .expect("writing to a String cannot fail");
        }
        command.args(["-vf", &filter]);
    }
    if trim > 0.0 {
        command.args(["-ss", &trim.to_string()]);
    }
    command.args(["-map", video]);
    if derived.is_some() {
        command.args(["-map", "1:a:0"]);
    } else if let Some(audio) = segment.audio {
        command.args(["-map", &format!("0:a:{audio}")]);
    }
    command.args([
        "-t",
        &segment.length.to_string(),
        "-af",
        AUDIO_FILTER,
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "23",
        "-maxrate",
        "4M",
        "-bufsize",
        "8M",
        "-pix_fmt",
        "yuv420p",
        "-g",
        "60",
        "-bf",
        "0",
        "-threads",
        "2",
        "-c:a",
        "aac",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-b:a",
        "128k",
        "-muxdelay",
        "0",
        "-muxpreload",
        "0",
        "-output_ts_offset",
        &segment.offset.to_string(),
        "-f",
        "mpegts",
        "pipe:1",
    ]);
    let bytes = match pcm {
        Some(audio) => run_with_audio(&mut command, audio, read).await?,
        None => run(&mut command, "ffmpeg", read).await?,
    };
    check_revision(input, revision).await?;
    Ok(bytes)
}

pub async fn thumbnail(
    State(state): State<AppState>,
    RoutePath(path): RoutePath<String>,
) -> Result<Response, AppError> {
    let input = resolve_file(&state.data, &path).await?;
    let read = state.media.read(&input)?;
    let revision = file_revision(&input).await?;
    let (bytes, duration) = with_media_slot(&THUMBNAIL_SLOTS, async {
        let timing = thumbnail_timing(&state.tools, &input, &read).await?;
        let filter = hold_frames(
            "scale=w='min(480,iw)':h='min(360,ih)':force_original_aspect_ratio=decrease",
            0.0,
        );
        let bytes = run(
            ffmpeg(&state.tools, &input, timing.seek)?.args([
                "-map",
                "0:V:0",
                "-an",
                "-frames:v",
                "1",
                "-vf",
                &filter,
                "-c:v",
                "mjpeg",
                "-pix_fmt",
                "yuvj420p",
                "-threads",
                "1",
                "-q:v",
                "4",
                "-f",
                "image2pipe",
                "pipe:1",
            ]),
            "ffmpeg",
            &read,
        )
        .await?;
        Ok((bytes, timing.duration))
    })
    .await?;
    check_revision(&input, revision).await?;
    let mut response = (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response();
    if let Some(duration) = duration {
        response.headers_mut().insert(
            "x-media-duration",
            HeaderValue::from_str(&duration.to_string()).expect("duration is a valid header value"),
        );
    }
    Ok(response)
}

async fn file_revision(path: &Path) -> Result<u64, AppError> {
    let metadata = fs::metadata(path).await?;
    let mut revision = DefaultHasher::new();
    metadata.modified()?.hash(&mut revision);
    metadata.len().hash(&mut revision);
    metadata.created().ok().hash(&mut revision);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.ino().hash(&mut revision);
    }
    Ok(revision.finish())
}

async fn check_revision(path: &Path, expected: u64) -> Result<(), AppError> {
    if file_revision(path).await? != expected {
        return Err(AppError::new(StatusCode::CONFLICT, "Media file changed"));
    }
    Ok(())
}

fn hold_frames(scale: &str, start: f64) -> String {
    format!("{scale},tpad=stop_mode=clone:stop=-1,fps={FRAME_RATE}:start_time={start}")
}

async fn with_media_slot<T>(
    slots: &Semaphore,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    // 探测和转码共用一次排队与处理时限。
    timeout(Duration::from_secs(45), async {
        let _permit = slots
            .acquire()
            .await
            .expect("media semaphore must stay open");
        work.await
    })
    .await
    .map_err(|_| AppError::new(StatusCode::GATEWAY_TIMEOUT, "Media conversion timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_a_segment_after_the_source_is_replaced() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("video.mp4");
        fs::write(&file, b"old").await.unwrap();
        let revision = file_revision(&file).await.unwrap();
        fs::write(&file, b"replacement").await.unwrap();
        let state = AppState {
            data: std::collections::HashMap::from([("media".into(), folder.path().to_owned())]),
            ..Default::default()
        };
        let path = "media/video.mp4";
        let plan = PlaybackPlan {
            revision,
            timeline: serde_json::from_str(r#"{"picture":{"start":0,"end":4},"duration":4}"#)
                .unwrap(),
        };
        let result = playback(
            State(state),
            RoutePath(path.into()),
            Query(PlaybackQuery {
                segment: Some(0),
                plan: Some(plan.encode(path).unwrap()),
            }),
            Method::GET,
        )
        .await;
        assert_eq!(
            result.unwrap_err().into_response().status(),
            StatusCode::CONFLICT
        );
    }
}
