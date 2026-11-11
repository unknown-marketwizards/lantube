use super::{
    FRAME_RATE, MAX_SEGMENTS, MIN_TAIL_SECONDS, SEGMENT_SECONDS,
    access::ReadLease,
    audio::AudioWindow,
    tools::{MediaTools, ToolProcess, check_output, ffprobe, run},
};
use crate::error::AppError;
use axum::http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap, fmt::Write, iter::once, ops::Range, path::Path, process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::timeout,
};

const TAIL_PROBE_PACKETS: usize = 20_000;
const FIRST_VIDEO_PACKETS: usize = 16;
const PLAYLIST_CHUNK_SEGMENTS: u32 = 128;
const AUDIO_WINDOW_SECONDS: u32 = 15 * SEGMENT_SECONDS;

#[derive(Clone, Copy, Default)]
struct PacketTiming {
    start: Option<f64>,
    end: Option<f64>,
}

struct TrackTiming {
    start: f64,
    end: Option<f64>,
}

#[derive(Deserialize, Serialize)]
pub struct PlaybackTimeline {
    picture: Range<f64>,
    pub duration: f64,
    audio: Option<usize>,
}

pub struct Segment {
    pub offset: f64,
    pub length: f64,
    pub picture: Option<Range<f64>>,
    pub seek: f64,
    pub audio: Option<usize>,
    pub audio_window: Option<AudioWindow>,
}

pub struct ThumbnailTiming {
    pub seek: Option<f64>,
    pub duration: Option<f64>,
}

pub async fn thumbnail_timing(
    tools: &MediaTools,
    input: &Path,
    read: &ReadLease,
) -> Result<ThumbnailTiming, AppError> {
    let probe = probe_media(tools, input, read).await?;
    let picture = probe.timing(probe.video()?, None);
    let audio_end = probe
        .audio()
        .map(|(_, track)| probe.timing(track, None).end);
    let duration = match (picture.end, audio_end) {
        (Some(end), None) => Some(end),
        (Some(end), Some(Some(audio_end))) => Some(end.max(audio_end)),
        _ => probe.container_duration(),
    };
    let seek = picture
        .end
        .map(|end| picture.start + ((end - picture.start) * 0.2).min(60.0));
    Ok(ThumbnailTiming { seek, duration })
}

impl PlaybackTimeline {
    fn rounded_duration(&self) -> f64 {
        (self.duration * 1_000_000.0).round() / 1_000_000.0
    }

    fn segment_count(&self) -> Result<u32, AppError> {
        let duration = self.rounded_duration();
        let step = f64::from(SEGMENT_SECONDS);
        let count = (duration / step).ceil();
        if !count.is_finite() || count < 1.0 || count > f64::from(MAX_SEGMENTS) {
            return Err(AppError::bad_request("Unsupported video duration"));
        }
        let mut count = count as u32;
        // 不足半秒的尾段可能没有可编码的画面，合并后四舍五入仍为 4 秒。
        if count > 1 && duration - f64::from(count - 1) * step < MIN_TAIL_SECONDS {
            count -= 1;
        }
        Ok(count)
    }

    fn segment_at(&self, index: u32, count: u32) -> Segment {
        let step = f64::from(SEGMENT_SECONDS);
        let offset = f64::from(index) * step;
        let length = if index + 1 == count {
            self.rounded_duration() - offset
        } else {
            step
        };
        let lead = (self.picture.start - offset).max(0.0);
        let tail = (self.picture.end - offset).min(length);
        let picture = (lead < tail).then_some(lead..tail);
        let seek = if self.audio.is_some() && (picture.is_none() || lead > 0.0) {
            // 无画面处快速定位可能跳到视频关键帧，先从可读到音频的位置解码。
            let guard = step * 2.0;
            if offset >= self.picture.end && self.picture.end - self.picture.start > guard {
                self.picture.end - guard
            } else {
                0.0
            }
        } else {
            offset
        };
        // 短预卷直接解码；长区间按固定窗口复用已解码的音频。
        let audio_window = (offset - seek > step * 2.0).then(|| {
            let start = offset as u32 / AUDIO_WINDOW_SECONDS * AUDIO_WINDOW_SECONDS;
            let anchor = seek.floor() as u32;
            let boundary = start + AUDIO_WINDOW_SECONDS;
            // 以实际分片边界为准，最后一段可能覆盖名义边界后的短尾。
            let end_at_boundary = |segment: u32| {
                if segment < count {
                    segment * SEGMENT_SECONDS
                } else {
                    self.rounded_duration().ceil() as u32
                }
            };
            let end = end_at_boundary(boundary / SEGMENT_SECONDS);
            let limit = if offset < self.picture.start {
                end_at_boundary((self.picture.start / step).ceil() as u32)
            } else {
                end_at_boundary(count)
            };
            AudioWindow {
                anchor,
                start: start.max(anchor),
                end: end.min(limit),
                limit,
            }
        });
        Segment {
            offset,
            length,
            picture,
            seek: if audio_window.is_some() { offset } else { seek },
            audio: self.audio,
            audio_window,
        }
    }

    pub fn segment(&self, index: u32) -> Result<Segment, AppError> {
        let count = self.segment_count()?;
        if index >= count {
            return Err(AppError::bad_request("Invalid video segment"));
        }
        Ok(self.segment_at(index, count))
    }

    pub fn playlist_chunks(self, token: String) -> Result<impl Iterator<Item = String>, AppError> {
        let count = self.segment_count()?;
        let header = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{SEGMENT_SECONDS}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-INDEPENDENT-SEGMENTS\n"
        );
        // 相对 URI 复用清单路径；分批生成，避免在内存里展开整份长清单。
        let chunks = (0..count)
            .step_by(PLAYLIST_CHUNK_SEGMENTS as usize)
            .map(move |start| {
                let end = (start + PLAYLIST_CHUNK_SEGMENTS).min(count);
                let mut chunk = String::with_capacity((end - start) as usize * (token.len() + 64));
                for index in start..end {
                    let length = self.segment_at(index, count).length;
                    // 每段独立复用，MPEG-TS 连续性计数器会重新开始。
                    if index > 0 {
                        chunk.push_str("#EXT-X-DISCONTINUITY\n");
                    }
                    writeln!(chunk, "#EXTINF:{length:.6},\n?segment={index}&plan={token}")
                        .expect("writing to a String cannot fail");
                }
                chunk
            });
        Ok(once(header)
            .chain(chunks)
            .chain(once("#EXT-X-ENDLIST\n".into())))
    }
}

#[derive(Deserialize)]
struct MediaProbe {
    #[serde(default)]
    streams: Vec<ProbedStream>,
    format: ProbedFormat,
}

#[derive(Deserialize)]
struct ProbedFormat {
    format_name: Option<String>,
    #[serde(flatten)]
    timing: ProbedTiming,
}

#[derive(Deserialize)]
struct ProbedStream {
    index: u32,
    #[serde(default)]
    codec_type: String,
    #[serde(flatten)]
    timing: ProbedTiming,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: HashMap<String, u8>,
}

#[derive(Deserialize)]
struct ProbedTiming {
    start_time: Option<String>,
    duration: Option<String>,
}

impl MediaProbe {
    fn origin(&self) -> f64 {
        finite(self.format.timing.start_time.as_deref()).unwrap_or_default()
    }

    fn container_duration(&self) -> Option<f64> {
        finite(self.format.timing.duration.as_deref()).filter(|duration| *duration > 0.0)
    }

    fn needs_video_end_check(&self, video_end: Option<f64>, has_audio: bool) -> bool {
        // 部分容器的轨道估算值会沿用容器时长。
        has_audio
            && (self.is_matroska() || self.has_transport_clock())
            && video_end
                .zip(self.container_duration())
                .is_some_and(|(video_end, total)| video_end >= total - 1.0 / f64::from(FRAME_RATE))
    }

    fn has_transport_clock(&self) -> bool {
        matches!(self.format.format_name.as_deref(), Some("mpegts" | "mpeg"))
    }

    fn is_matroska(&self) -> bool {
        matches!(
            self.format.format_name.as_deref(),
            Some("matroska,webm" | "matroska" | "webm")
        )
    }

    fn needs_video_start_check(&self, video: &ProbedStream, has_audio: bool) -> bool {
        video.start().is_none_or(|start| {
            has_audio
                && (self.is_matroska() || self.has_transport_clock())
                && start - self.origin() <= 1.0 / f64::from(FRAME_RATE)
        })
    }

    fn timing(&self, track: &ProbedStream, packet_start: Option<f64>) -> TrackTiming {
        let origin = self.origin();
        let start = packet_start.or_else(|| track.start()).unwrap_or(origin);
        let stream_end = finite(track.timing.duration.as_deref())
            .filter(|duration| *duration > 0.0)
            .map(|duration| start + duration);
        let tag_end = track.tagged_end();
        let end = if self.is_matroska() {
            tag_end.or(stream_end)
        } else {
            stream_end.or(tag_end)
        };
        let start = (start - origin).max(0.0);
        let end = end
            .map(|end| end - origin)
            .filter(|end| end.is_finite() && *end > start);
        TrackTiming { start, end }
    }

    fn video(&self) -> Result<&ProbedStream, AppError> {
        // 和 FFmpeg 的 V:0 一样，跳过附带的封面图。
        self.streams
            .iter()
            .find(|track| {
                track.codec_type == "video" && track.disposition.get("attached_pic") != Some(&1)
            })
            .ok_or_else(|| AppError::new(StatusCode::UNPROCESSABLE_ENTITY, "No video track"))
    }

    fn audio(&self) -> Option<(usize, &ProbedStream)> {
        let mut tracks = self
            .streams
            .iter()
            .filter(|track| track.codec_type == "audio")
            .enumerate();
        tracks
            .clone()
            .find(|(_, track)| track.disposition.get("default") == Some(&1))
            .or_else(|| tracks.next())
    }
}

impl ProbedStream {
    fn start(&self) -> Option<f64> {
        finite(self.timing.start_time.as_deref())
    }

    fn tagged_end(&self) -> Option<f64> {
        // Matroska 的 DURATION 标签记录轨道末尾的时间戳。
        let (hours, value) = self.tags.get("DURATION")?.split_once(':')?;
        let (minutes, seconds) = value.split_once(':')?;
        Some(finite(Some(hours))? * 3600.0 + finite(Some(minutes))? * 60.0 + finite(Some(seconds))?)
    }
}

async fn probe_media(
    tools: &MediaTools,
    input: &Path,
    read: &ReadLease,
) -> Result<MediaProbe, AppError> {
    let output = run(
        ffprobe(tools)?
            .args([
                "-show_entries",
                "stream=index,codec_type,start_time,duration:stream_tags=DURATION:stream_disposition=attached_pic,default:format=format_name,start_time,duration",
                "-of",
                "json",
                "-i",
            ])
            .arg(input),
        "ffprobe",
        read,
    )
    .await?;
    serde_json::from_slice(&output)
        .map_err(|_| AppError::new(StatusCode::BAD_GATEWAY, "Invalid media probe response"))
}

pub async fn playback_timeline(
    tools: &MediaTools,
    input: &Path,
    method: Method,
    read: &ReadLease,
) -> Result<Option<PlaybackTimeline>, AppError> {
    let probe = probe_media(tools, input, read).await?;
    let video = probe.video()?;
    tools.encoder()?;
    let selected_audio = probe.audio();
    let audio = selected_audio.map(|(_, track)| track);
    let origin = probe.origin();
    let check_video_start = probe.needs_video_start_check(video, audio.is_some());
    let audio_end = audio.and_then(|track| probe.timing(track, None).end);
    let container_end = probe.container_duration();
    let metadata_video_end = probe.timing(video, None).end;
    let check_video_end = video.tagged_end().is_none()
        && probe.needs_video_end_check(metadata_video_end, audio.is_some());
    let missing_ends = [
        (metadata_video_end.is_none()
            || check_video_end
            || (check_video_start && container_end.is_none()))
        .then_some(video.index),
        audio
            .filter(|_| audio_end.is_none())
            .map(|track| track.index),
    ];
    if method == Method::HEAD && (check_video_start || missing_ends != [None, None]) {
        return Ok(None);
    }
    let [video_packets, audio_packets] = if missing_ends == [None, None] {
        [PacketTiming::default(); 2]
    } else if let Some(end) = container_end {
        // 容器已有时长时，只在结尾附近查找轨道终点。
        let tail_start = (end - 24.0).max(0.0);
        let interval = if tail_start > 0.0 {
            format!("{}%+#{TAIL_PROBE_PACKETS}", origin + tail_start)
        } else {
            format!("%+#{TAIL_PROBE_PACKETS}")
        };
        match timeout(
            Duration::from_secs(3),
            probe_packet_timing(
                tools,
                input,
                missing_ends,
                Some((&interval, TAIL_PROBE_PACKETS)),
                read,
            ),
        )
        .await
        {
            Ok(Ok(ends)) => ends,
            _ => [PacketTiming::default(); 2],
        }
    } else {
        probe_packet_timing(tools, input, missing_ends, None, read).await?
    };
    let packet_start = if check_video_start && container_end.is_some() {
        let interval = format!("%+#{FIRST_VIDEO_PACKETS}");
        let [first, _] = probe_packet_timing(
            tools,
            input,
            [Some(video.index), None],
            Some((&interval, FIRST_VIDEO_PACKETS)),
            read,
        )
        .await?;
        first.start
    } else if check_video_start {
        video_packets.start
    } else {
        None
    };
    let picture = probe.timing(video, packet_start);
    let video_end = picture
        .end
        .filter(|_| !check_video_end)
        .or(video_packets.end.map(|end| end - origin))
        .or(picture.end)
        .or(container_end)
        .filter(|end| *end > picture.start)
        .ok_or_else(|| AppError::bad_request("Unsupported video duration"))?;
    let audio_end = audio.and_then(|_| {
        audio_end
            .or(audio_packets.end.map(|end| end - origin))
            .or(container_end)
    });
    let duration = video_end.max(audio_end.unwrap_or_default());
    Ok(Some(PlaybackTimeline {
        picture: picture.start..video_end,
        duration,
        audio: selected_audio.map(|(index, _)| index),
    }))
}

fn finite(value: Option<&str>) -> Option<f64> {
    value?.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn packet_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.split('|').find_map(|value| value.strip_prefix(field))
}

async fn probe_packet_timing(
    tools: &MediaTools,
    input: &Path,
    tracks: [Option<u32>; 2],
    interval: Option<(&str, usize)>,
    read: &ReadLease,
) -> Result<[PacketTiming; 2], AppError> {
    let mut command = ffprobe(tools)?;
    if let Some((interval, _)) = interval {
        command.args(["-read_intervals", interval]);
    }
    if let [Some(index), None] | [None, Some(index)] = tracks {
        command.args(["-select_streams", &index.to_string()]);
    }
    let command = command
        .args([
            "-show_entries",
            "packet=stream_index,pts_time,dts_time,duration_time",
            "-of",
            "compact=p=0",
            "-i",
        ])
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ToolProcess::start(command, read)?;
    let mut packets = BufReader::new(
        child
            .child()
            .stdout
            .take()
            .expect("ffprobe stdout is piped"),
    )
    .lines();
    let read_timing = async {
        let mut starts = [f64::INFINITY; 2];
        let mut ends = [f64::NEG_INFINITY; 2];
        let mut count = 0;
        while let Some(line) = packets.next_line().await? {
            let stream = packet_field(&line, "stream_index=").and_then(|id| id.parse().ok());
            count += usize::from(stream.is_some());
            let Some(index) = tracks.iter().position(|id| id.is_some() && *id == stream) else {
                continue;
            };
            let Some(pts) = finite(packet_field(&line, "pts_time="))
                .or_else(|| finite(packet_field(&line, "dts_time=")))
            else {
                continue;
            };
            let duration = finite(packet_field(&line, "duration_time="))
                .filter(|duration| *duration > 0.0)
                .unwrap_or(1.0 / f64::from(FRAME_RATE));
            starts[index] = starts[index].min(pts);
            ends[index] = ends[index].max(pts + duration);
        }
        let reached_end = interval.is_none_or(|(_, limit)| count < limit);
        Ok::<_, AppError>(std::array::from_fn(|index| PacketTiming {
            start: starts[index].is_finite().then_some(starts[index]),
            end: (reached_end && ends[index].is_finite()).then_some(ends[index]),
        }))
    };
    let (timing, output) = tokio::try_join!(read_timing, child.output())?;
    check_output(&output, "ffprobe", true)?;
    Ok(timing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_window_covers_a_short_tail_merged_across_its_boundary() {
        for picture in [0.0..4.0, 59.8..60.125] {
            let timeline = PlaybackTimeline {
                picture,
                duration: 60.125,
                audio: Some(0),
            };
            let last = timeline.segment(14).unwrap();
            assert_eq!(last.offset + last.length, 60.125);
            assert_eq!(last.audio_window.unwrap().end, 61);
            assert!(timeline.segment(15).is_err());
        }
    }

    #[test]
    fn checks_transport_stream_video_duration_against_packets_when_it_matches_container() {
        let probe: MediaProbe = serde_json::from_str(
            r#"{"format":{"format_name":"mpegts","start_time":"1.4","duration":"50"}}"#,
        )
        .unwrap();
        assert!(probe.needs_video_end_check(Some(50.0), true));
        assert!(!probe.needs_video_end_check(Some(1.0), true));
        assert!(!probe.needs_video_end_check(Some(50.0), false));
    }

    #[test]
    fn checks_transport_stream_video_start_when_it_matches_container() {
        let probe: MediaProbe = serde_json::from_str(
            r#"{
            "streams":[{"index":0,"codec_type":"video","start_time":"1.4"}],
            "format":{"format_name":"mpegts","start_time":"1.4","duration":"36"}
        }"#,
        )
        .unwrap();
        let track = probe.video().unwrap();
        assert!(probe.needs_video_start_check(track, true));
        assert!(!probe.needs_video_start_check(track, false));
    }

    #[test]
    fn segments_that_start_outside_the_picture_seek_from_a_safe_audio_position() {
        let timeline = PlaybackTimeline {
            picture: 13.5..28.0,
            duration: 128.0,
            audio: Some(0),
        };
        let leading = timeline.segment(1).unwrap();
        assert_eq!(leading.picture, None);
        assert_eq!(leading.seek, 0.0);
        assert!(leading.audio_window.is_none());
        let start = timeline.segment(3).unwrap();
        assert_eq!(start.picture, Some(1.5..4.0));
        assert_eq!(start.seek, 12.0);
        let leading_window = start.audio_window.unwrap();
        assert_eq!(
            (
                leading_window.anchor,
                leading_window.start,
                leading_window.end
            ),
            (0, 0, 16)
        );
        let trailing = timeline.segment(7).unwrap();
        assert_eq!(trailing.picture, None);
        assert_eq!(trailing.seek, 20.0);
        assert!(trailing.audio_window.is_none());
        for index in [8, 9, 10] {
            let later = timeline.segment(index).unwrap();
            assert_eq!(later.seek, later.offset);
            let window = later.audio_window.unwrap();
            assert_eq!((window.anchor, window.start, window.end), (20, 20, 60));
        }
        let later = timeline.segment(15).unwrap().audio_window.unwrap();
        assert_eq!((later.anchor, later.start, later.end), (20, 60, 120));
        let last = timeline.segment(31).unwrap().audio_window.unwrap();
        assert_eq!((last.anchor, last.start, last.end), (20, 120, 128));
    }

    fn timeline(picture: Range<f64>, duration: f64) -> PlaybackTimeline {
        PlaybackTimeline {
            picture,
            duration,
            audio: None,
        }
    }

    fn playlist(timeline: PlaybackTimeline) -> Result<String, AppError> {
        Ok(timeline.playlist_chunks("token".into())?.collect())
    }

    #[test]
    fn prefers_the_container_default_audio_track() {
        let probe: MediaProbe = serde_json::from_str(
            r#"{
            "streams":[
                {"index":0,"codec_type":"video"},
                {"index":1,"codec_type":"audio"},
                {"index":2,"codec_type":"subtitle"},
                {"index":3,"codec_type":"audio","disposition":{"default":1}}
            ],"format":{}
        }"#,
        )
        .unwrap();
        let (audio, track) = probe.audio().unwrap();
        assert_eq!(audio, 1);
        assert_eq!(track.index, 3);
    }

    #[test]
    fn playlist_uses_relative_uris_and_preserves_the_partial_final_segment() {
        let result = playlist(timeline(0.0..9.25, 9.25)).unwrap();
        assert!(result.contains("#EXT-X-TARGETDURATION:4\n"));
        assert!(result.contains("#EXTINF:4.000000,\n?segment=0&plan=token\n"));
        assert!(
            result.contains("#EXT-X-DISCONTINUITY\n#EXTINF:1.250000,\n?segment=2&plan=token\n")
        );
        assert!(result.ends_with("#EXT-X-ENDLIST\n"));
        assert_eq!(result.matches("#EXTINF:").count(), 3);
        assert_eq!(result.matches("#EXT-X-DISCONTINUITY\n").count(), 2);
    }

    #[test]
    fn streams_long_playlists_in_bounded_chunks() {
        let duration = f64::from(MAX_SEGMENTS * SEGMENT_SECONDS);
        let mut chunks = timeline(0.0..duration, duration)
            .playlist_chunks("token".into())
            .unwrap();
        assert!(chunks.next().unwrap().starts_with("#EXTM3U\n"));
        let first = chunks.next().unwrap();
        assert_eq!(
            first.matches("#EXTINF:").count(),
            PLAYLIST_CHUNK_SEGMENTS as usize
        );
        let second = chunks.next().unwrap();
        assert!(second.starts_with("#EXT-X-DISCONTINUITY\n#EXTINF:4.000000,\n?segment=128&"));
    }

    #[test]
    fn playlist_rejects_invalid_or_excessive_duration() {
        for duration in [
            0.0,
            f64::INFINITY,
            f64::from(MAX_SEGMENTS * SEGMENT_SECONDS) + 1.0,
        ] {
            assert!(playlist(timeline(0.0..1.0, duration)).is_err());
        }
    }

    #[test]
    fn playlist_omits_sub_microsecond_rounding_remainder() {
        let result = playlist(timeline(0.023..4.0, 4.000000000000001)).unwrap();
        assert_eq!(result.matches("#EXTINF:").count(), 1);
        assert!(result.contains("#EXTINF:4.000000,"));
    }

    #[test]
    fn playlist_merges_a_short_tail_without_shifting_the_previous_segments() {
        let timeline_with_tail = timeline(0.0..8.008, 8.008);
        let last = timeline_with_tail.segment(1).unwrap();
        assert!(timeline_with_tail.segment(2).is_err());
        let result = playlist(timeline_with_tail).unwrap();
        assert_eq!(result.matches("#EXTINF:").count(), 2);
        assert!(result.contains("#EXTINF:4.008000,\n?segment=1&plan=token\n"));
        assert_eq!(last.offset, 4.0);
        assert!((last.length - 4.008).abs() < 1e-6);
        assert_eq!(timeline(0.0..8.5, 8.5).segment_count().unwrap(), 3);
        assert_eq!(timeline(0.0..0.2, 0.2).segment_count().unwrap(), 1);
    }

    #[test]
    fn keeps_audio_only_segments_on_both_sides_of_the_picture() {
        let timeline = PlaybackTimeline {
            picture: 6.0..11.0,
            duration: 17.0,
            audio: Some(1),
        };
        for (index, picture) in [None, Some(2.0..4.0), Some(0.0..3.0), None, None]
            .into_iter()
            .enumerate()
        {
            let segment = timeline.segment(index as u32).unwrap();
            assert_eq!(segment.picture, picture);
            assert_eq!(segment.audio, Some(1));
        }
        assert!(timeline.segment(5).is_err());
        assert!(timeline.segment(99_999).is_err());
    }

    #[test]
    fn reads_container_start_after_duplicate_program_stream() {
        let probe: MediaProbe = serde_json::from_str(r#"{
            "programs":[{"streams":[{"index":1,"codec_type":"video","start_time":"7.5"}]}],
            "streams":[
                {"index":3},
                {"index":0,"codec_type":"video","disposition":{"attached_pic":1}},
                {"index":1,"codec_type":"video","start_time":"7.5","duration":"9"},
                {"index":2,"codec_type":"audio","start_time":"1.5","tags":{"DURATION":"00:00:17.500000000"}}
            ],
            "format":{"start_time":"1.5"}
        }"#).unwrap();
        let video = probe.timing(probe.video().unwrap(), None);
        assert_eq!(video.start, 6.0);
        assert_eq!(video.end, Some(15.0));
        assert_eq!(probe.timing(&probe.streams[3], None).end, Some(16.0));
    }

    #[test]
    fn missing_start_uses_the_first_video_packet_for_track_duration() {
        let probe: MediaProbe = serde_json::from_str(
            r#"{"streams":[{"index":1,"codec_type":"video","duration":"4.0"}],"format":{"start_time":"1.0"}}"#,
        ).unwrap();
        let timing = probe.timing(probe.video().unwrap(), Some(7.0));
        assert_eq!(timing.start, 6.0);
        assert_eq!(timing.end, Some(10.0));
    }

    #[test]
    fn matroska_uses_the_packet_start_and_track_tag_instead_of_container_estimates() {
        let probe: MediaProbe = serde_json::from_str(
            r#"{
            "streams":[{"index":0,"codec_type":"video","start_time":"0","duration":"20.023",
                        "tags":{"DURATION":"00:00:17.520000000"}}],
            "format":{"format_name":"matroska,webm","start_time":"0","duration":"20.023"}
        }"#,
        )
        .unwrap();
        let video = probe.video().unwrap();
        assert!(probe.needs_video_start_check(video, true));
        let timing = probe.timing(video, Some(13.52));
        assert_eq!(timing.start, 13.52);
        assert_eq!(timing.end, Some(17.52));
    }
}
