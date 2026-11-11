use super::{
    AUDIO_FILTER,
    access::ReadLease,
    check_revision,
    tools::{MediaTools, ToolProcess, ffmpeg_file},
};
use crate::error::AppError;
use axum::http::StatusCode;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as IndexMutex},
    time::Duration,
};
use tokio::{
    fs,
    io::AsyncReadExt,
    process::{ChildStdout, Command},
    sync::Mutex,
    time::{Instant, interval},
};

const AUDIO_CACHE_TRACKS: usize = 2;
const AUDIO_CACHE_WINDOWS: usize = 2;
const DECODER_IDLE_LIMIT: Duration = Duration::from_secs(90);
const DECODER_SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const SAMPLE_RATE: usize = 48_000;
const SAMPLE_BYTES: usize = 4; // 16-bit stereo
const BYTES_PER_SECOND: usize = SAMPLE_RATE * SAMPLE_BYTES;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AudioWindow {
    pub anchor: u32,
    pub start: u32,
    pub end: u32,
    pub limit: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct Source {
    input: PathBuf,
    revision: u64,
    track: usize,
    anchor: u32,
    limit: u32,
}

struct Decoder {
    // 关闭或淘汰轨道时结束等待输出的 FFmpeg。
    process: ToolProcess,
    output: ChildStdout,
    next: u32,
    last_used: Instant,
}

impl Decoder {
    async fn start(
        tools: &MediaTools,
        source: &Source,
        start: u32,
        read: &ReadLease,
    ) -> Result<Self, AppError> {
        let seek = (source.anchor > 0).then_some(f64::from(source.anchor));
        // 由 Rust 打开源文件，Windows 上允许在解码进程等待时替换文件。
        let file = fs::File::open(&source.input).await?.into_std().await;
        let mut command = ffmpeg_file(tools, file, seek)?;
        command
            .args([
                "-ss",
                &(start - source.anchor).to_string(),
                "-map",
                &format!("0:a:{}", source.track),
                "-af",
                AUDIO_FILTER,
                "-t",
                &(source.limit - start).to_string(),
                "-c:a",
                "pcm_s16le",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-f",
                "s16le",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut process = ToolProcess::start(&mut command, read)?;
        let output = process
            .child()
            .stdout
            .take()
            .expect("ffmpeg stdout is piped");
        Ok(Self {
            process,
            output,
            next: start,
            last_used: Instant::now(),
        })
    }

    async fn stop(self) -> Result<(), AppError> {
        self.process.stop().await
    }
}

#[derive(Default)]
struct AudioTrack {
    decoder: Option<Decoder>,
    windows: VecDeque<(u32, Arc<Vec<u8>>)>,
}

type CachedTrack = Arc<Mutex<AudioTrack>>;

#[derive(Default)]
pub struct AudioScratch {
    tracks: IndexMutex<VecDeque<(Source, CachedTrack)>>,
}

impl AudioScratch {
    pub async fn close(&self, target: &Path) -> Result<(), AppError> {
        let affected = {
            let mut tracks = self.tracks.lock().expect("audio index is not poisoned");
            let mut affected = Vec::new();
            tracks.retain(|(source, entry)| {
                if source.input == target {
                    affected.push(Arc::clone(entry));
                    false
                } else {
                    true
                }
            });
            affected
        };
        for entry in affected {
            let mut track = entry.lock().await;
            if let Some(decoder) = track.decoder.take() {
                decoder.stop().await?;
            }
        }
        Ok(())
    }

    pub fn start_reaper(self: &Arc<Self>) {
        let scratch = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut sweep = interval(DECODER_SWEEP_INTERVAL);
            loop {
                sweep.tick().await;
                let Some(scratch) = scratch.upgrade() else {
                    break;
                };
                let entries: Vec<_> = scratch
                    .tracks
                    .lock()
                    .expect("audio index is not poisoned")
                    .iter()
                    .map(|(_, entry)| Arc::clone(entry))
                    .collect();
                for entry in entries {
                    if let Ok(mut track) = entry.try_lock()
                        && track.decoder.as_ref().is_some_and(|decoder| {
                            decoder.last_used.elapsed() >= DECODER_IDLE_LIMIT
                        })
                        && let Some(decoder) = track.decoder.take()
                        && let Err(error) = decoder.stop().await
                    {
                        eprintln!("Could not stop idle audio decoder: {error:?}");
                    }
                }
            }
        });
    }

    pub async fn track(
        &self,
        tools: &MediaTools,
        input: &Path,
        revision: u64,
        track: usize,
        window: AudioWindow,
        read: &ReadLease,
    ) -> Result<Arc<Vec<u8>>, AppError> {
        let source = Source {
            input: input.to_owned(),
            revision,
            track,
            anchor: window.anchor,
            limit: window.limit,
        };
        let entry = {
            let mut tracks = self.tracks.lock().expect("audio index is not poisoned");
            let entry = tracks
                .iter()
                .position(|(key, _)| key == &source)
                .map(|index| {
                    tracks
                        .remove(index)
                        .expect("position is within the cache")
                        .1
                })
                .unwrap_or_default();
            tracks.push_front((source.clone(), Arc::clone(&entry)));
            tracks.truncate(AUDIO_CACHE_TRACKS);
            entry
        };
        let mut track = entry.lock().await;
        check_revision(input, revision).await?;
        if let Some(decoder) = track.decoder.as_mut() {
            decoder.last_used = Instant::now();
        }
        if let Some((_, pcm)) = track
            .windows
            .iter()
            .find(|(start, _)| *start == window.start)
        {
            return Ok(Arc::clone(pcm));
        }
        // 读到一半被取消时直接结束解码器，下次从完整窗口开始。
        let mut decoder = match track.decoder.take() {
            Some(mut decoder) if decoder.next == window.start => {
                decoder.process.set_read(Some(read.clone()));
                decoder
            }
            _ => Decoder::start(tools, &source, window.start, read).await?,
        };
        let mut pcm = vec![0; (window.end - window.start) as usize * BYTES_PER_SECOND];
        decoder
            .output
            .read_exact(&mut pcm)
            .await
            .map_err(|_| AppError::new(StatusCode::BAD_GATEWAY, "Incomplete audio window"))?;
        check_revision(input, revision).await?;
        if window.end < window.limit {
            decoder.process.set_read(None);
            decoder.next = window.end;
            decoder.last_used = Instant::now();
            track.decoder = Some(decoder);
        }
        let pcm = Arc::new(pcm);
        track.windows.push_front((window.start, Arc::clone(&pcm)));
        track.windows.truncate(AUDIO_CACHE_WINDOWS);
        Ok(pcm)
    }
}

pub fn audio_slice(
    pcm: &[u8],
    window: AudioWindow,
    offset: f64,
    length: f64,
) -> Result<&[u8], AppError> {
    let start = (offset as u32 - window.start) as usize * BYTES_PER_SECOND;
    let end = start + (length * SAMPLE_RATE as f64).round() as usize * SAMPLE_BYTES;
    pcm.get(start..end)
        .ok_or_else(|| AppError::new(StatusCode::BAD_GATEWAY, "Incomplete audio window"))
}

pub fn add_audio_input(command: &mut Command) {
    command.args(["-f", "s16le", "-ar", "48000", "-ac", "2", "-i", "pipe:0"]);
}
