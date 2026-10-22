//! Transcoding with ffmpeg to media profile v1 (ADR 0005).
//!
//! Only the transcoded rendition is ever published; the original file is
//! never modified. All metadata (GPS, device, capture time) is stripped.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail, ensure};
use osp_proto::MediaKind;
use serde::Deserialize;
use tokio::process::Command;

use crate::config::MediaConfig;

/// Name recorded in each attachment's `profile` field.
pub const PROFILE: &str = "v1-720p";

/// Fit inside 1280x720 (landscape) or 720x1280 (portrait), never upscale,
/// keep dimensions even for yuv420p.
const VIDEO_SCALE: &str = "scale=w='if(gte(iw,ih),min(1280,iw),min(720,iw))':h='if(gte(iw,ih),min(720,ih),min(1280,ih))':force_original_aspect_ratio=decrease:force_divisible_by=2";
const IMAGE_SCALE: &str =
    "scale=w='min(2048,iw)':h='min(2048,ih)':force_original_aspect_ratio=decrease";
const THUMB_SCALE: &str =
    "scale=w='min(480,iw)':h='min(480,ih)':force_original_aspect_ratio=decrease";

/// A finished rendition, ready to be imported into the blob store.
#[derive(Debug)]
pub struct Rendition {
    pub kind: MediaKind,
    pub media_type: &'static str,
    pub file: PathBuf,
    pub thumbnail: PathBuf,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
}

/// The parts of ffprobe's report we use.
#[derive(Debug)]
pub struct Probe {
    pub format_name: String,
    pub duration_secs: Option<f64>,
    pub bit_rate: Option<u64>,
    /// Width and height of the first video stream.
    pub video: Option<(u32, u32)>,
    pub has_audio: bool,
    /// Every metadata tag, from the container and from each stream.
    pub tags: Vec<(String, String)>,
}

impl Probe {
    /// ffmpeg's demuxer names for still images end in `_pipe` or are `image2`.
    fn is_still_image(&self) -> bool {
        self.format_name == "image2" || self.format_name.ends_with("_pipe")
    }
}

/// Whether ffmpeg and ffprobe can be run. Without them, posts with media fail.
pub async fn tools_available(cfg: &MediaConfig) -> bool {
    for tool in [&cfg.ffmpeg, &cfg.ffprobe] {
        let ran = Command::new(tool)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if !ran.is_ok_and(|status| status.success()) {
            return false;
        }
    }
    true
}

/// Transcodes `input` into a fresh directory under `work_dir`.
pub async fn transcode(cfg: &MediaConfig, input: &Path, work_dir: &Path) -> Result<Rendition> {
    let source = probe(cfg, input).await?;
    ensure!(
        source.video.is_some(),
        "{} has no picture; only images and videos can be attached",
        input.display()
    );
    let job_dir = new_job_dir(work_dir)?;
    if source.is_still_image() {
        transcode_image(cfg, input, &job_dir).await
    } else {
        transcode_video(cfg, input, &source, &job_dir).await
    }
}

async fn transcode_video(
    cfg: &MediaConfig,
    input: &Path,
    source: &Probe,
    job_dir: &Path,
) -> Result<Rendition> {
    let duration = source.duration_secs.unwrap_or(0.0);
    ensure!(
        duration <= cfg.max_video_secs as f64,
        "video is {:.0} s long; the limit is {} s",
        duration,
        cfg.max_video_secs
    );
    let out = job_dir.join("video.mp4");
    let mut args: Vec<&str> = vec!["-i", path_str(input)?, "-map", "0:v:0", "-map", "0:a:0?"];
    args.extend([
        "-vf",
        VIDEO_SCALE,
        "-fpsmax",
        "30",
        "-c:v",
        "libx264",
        "-preset",
        &cfg.x264_preset,
        "-profile:v",
        "high",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "26",
        "-maxrate",
        "1500k",
        "-bufsize",
        "3000k",
        "-force_key_frames",
        "expr:gte(t,n_forced*2)",
        "-c:a",
        "aac",
        "-b:a",
        "96k",
        "-ac",
        "2",
        "-ar",
        "48000",
    ]);
    args.extend(STRIP_METADATA);
    args.extend(["-movflags", "+faststart", path_str(&out)?]);
    run_ffmpeg(cfg, &args).await?;

    let result = probe(cfg, &out).await?;
    let (width, height) = result.video.context("transcoded video has no picture")?;
    let duration_secs = result.duration_secs.unwrap_or(duration);

    let thumbnail = job_dir.join("thumb.webp");
    let seek = format!("{:.2}", (duration_secs / 2.0).min(1.0));
    let mut args = vec!["-ss", &seek, "-i", path_str(&out)?, "-frames:v", "1"];
    args.extend(["-vf", THUMB_SCALE, "-c:v", "libwebp", "-quality", "75"]);
    args.extend(STRIP_METADATA);
    args.push(path_str(&thumbnail)?);
    run_ffmpeg(cfg, &args).await?;

    Ok(Rendition {
        kind: MediaKind::Video,
        media_type: "video/mp4",
        file: out,
        thumbnail,
        width,
        height,
        duration_ms: Some((duration_secs * 1000.0).round() as u64),
    })
}

async fn transcode_image(cfg: &MediaConfig, input: &Path, job_dir: &Path) -> Result<Rendition> {
    let out = job_dir.join("image.webp");
    let mut args = vec!["-i", path_str(input)?, "-frames:v", "1"];
    args.extend(["-vf", IMAGE_SCALE, "-c:v", "libwebp", "-quality", "80"]);
    args.extend(STRIP_METADATA);
    args.push(path_str(&out)?);
    run_ffmpeg(cfg, &args).await?;

    let thumbnail = job_dir.join("thumb.webp");
    let mut args = vec!["-i", path_str(&out)?, "-frames:v", "1"];
    args.extend(["-vf", THUMB_SCALE, "-c:v", "libwebp", "-quality", "75"]);
    args.extend(STRIP_METADATA);
    args.push(path_str(&thumbnail)?);
    run_ffmpeg(cfg, &args).await?;

    let (width, height) = probe(cfg, &out)
        .await?
        .video
        .context("transcoded image has no picture")?;
    Ok(Rendition {
        kind: MediaKind::Image,
        media_type: "image/webp",
        file: out,
        thumbnail,
        width,
        height,
        duration_ms: None,
    })
}

/// Drops container, per-stream and chapter metadata. `-map_metadata -1` alone
/// only covers the container; stream tags would otherwise be copied.
const STRIP_METADATA: [&str; 8] = [
    "-map_metadata",
    "-1",
    "-map_metadata:s:v",
    "-1",
    "-map_metadata:s:a",
    "-1",
    "-map_chapters",
    "-1",
];

pub async fn probe(cfg: &MediaConfig, path: &Path) -> Result<Probe> {
    #[derive(Deserialize)]
    struct Output {
        #[serde(default)]
        streams: Vec<Stream>,
        format: Format,
    }
    #[derive(Deserialize)]
    struct Stream {
        codec_type: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        #[serde(default)]
        tags: HashMap<String, String>,
    }
    #[derive(Deserialize)]
    struct Format {
        format_name: String,
        duration: Option<String>,
        bit_rate: Option<String>,
        #[serde(default)]
        tags: HashMap<String, String>,
    }

    let output = Command::new(&cfg.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("running {} (is ffmpeg installed?)", cfg.ffprobe))?;
    if !output.status.success() {
        bail!(
            "{} is not a readable media file: {}",
            path.display(),
            last_lines(&output.stderr)
        );
    }
    let parsed: Output =
        serde_json::from_slice(&output.stdout).context("unexpected ffprobe output")?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .and_then(|s| Some((s.width?, s.height?)));
    let has_audio = parsed
        .streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));
    let mut tags: Vec<(String, String)> = parsed.format.tags.into_iter().collect();
    for s in parsed.streams {
        tags.extend(s.tags);
    }
    Ok(Probe {
        format_name: parsed.format.format_name,
        duration_secs: parsed.format.duration.and_then(|d| d.parse().ok()),
        bit_rate: parsed.format.bit_rate.and_then(|b| b.parse().ok()),
        video,
        has_audio,
        tags,
    })
}

async fn run_ffmpeg(cfg: &MediaConfig, args: &[&str]) -> Result<()> {
    let output = Command::new(&cfg.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running {} (is ffmpeg installed?)", cfg.ffmpeg))?;
    if !output.status.success() {
        bail!("ffmpeg failed: {}", last_lines(&output.stderr));
    }
    Ok(())
}

fn new_job_dir(work_dir: &Path) -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = work_dir.join(format!("job-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn last_lines(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a short clip with sound and personal metadata baked in.
    pub(crate) async fn make_clip(dir: &Path, size: &str, secs: u32) -> PathBuf {
        let out = dir.join(format!("clip-{size}.mp4"));
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!("testsrc=duration={secs}:size={size}:rate=30"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=440:duration={secs}"))
            .args([
                "-metadata",
                "location=+51.5074-000.1278/",
                "-metadata",
                "title=secret-title",
                "-metadata:s:v:0",
                "title=secret-stream",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&out)
            .status()
            .await
            .expect("ffmpeg must be installed to run media tests");
        assert!(status.success());
        out
    }

    fn assert_no_personal_metadata(probe: &Probe) {
        for (k, v) in &probe.tags {
            let k = k.to_lowercase();
            assert!(!k.contains("location"), "leaked tag {k}={v}");
            assert!(!v.contains("secret"), "leaked tag {k}={v}");
        }
    }

    #[tokio::test]
    async fn landscape_video_is_capped_at_720p_and_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = MediaConfig {
            x264_preset: "ultrafast".into(),
            ..MediaConfig::default()
        };
        let input = make_clip(dir.path(), "1920x1080", 2).await;
        let original = probe(&cfg, &input).await.unwrap();
        assert!(original.tags.iter().any(|(k, _)| k == "location"));

        let r = transcode(&cfg, &input, dir.path()).await.unwrap();
        assert_eq!(r.kind, MediaKind::Video);
        assert_eq!((r.width, r.height), (1280, 720));
        let out = probe(&cfg, &r.file).await.unwrap();
        assert!(out.has_audio);
        assert!(
            out.bit_rate.unwrap() <= 1_700_000,
            "bitrate {:?}",
            out.bit_rate
        );
        assert_no_personal_metadata(&out);
        assert!(r.thumbnail.exists());
    }

    #[tokio::test]
    async fn portrait_and_small_videos_keep_shape_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = MediaConfig {
            x264_preset: "ultrafast".into(),
            ..MediaConfig::default()
        };
        let portrait = make_clip(dir.path(), "1080x1920", 1).await;
        let r = transcode(&cfg, &portrait, dir.path()).await.unwrap();
        assert_eq!((r.width, r.height), (720, 1280));

        let small = make_clip(dir.path(), "640x360", 1).await;
        let r = transcode(&cfg, &small, dir.path()).await.unwrap();
        assert_eq!((r.width, r.height), (640, 360));
    }

    #[tokio::test]
    async fn images_become_webp() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = MediaConfig::default();
        let input = dir.path().join("photo.png");
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args(["-i", "testsrc=size=3000x2000", "-frames:v", "1"])
            .arg(&input)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let r = transcode(&cfg, &input, dir.path()).await.unwrap();
        assert_eq!(r.kind, MediaKind::Image);
        assert_eq!(r.media_type, "image/webp");
        assert_eq!(r.width, 2048);
        assert!((1365..=1366).contains(&r.height), "height {}", r.height);
    }

    #[tokio::test]
    async fn too_long_video_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = MediaConfig {
            max_video_secs: 1,
            ..MediaConfig::default()
        };
        let input = make_clip(dir.path(), "320x240", 3).await;
        assert!(transcode(&cfg, &input, dir.path()).await.is_err());
    }
}
