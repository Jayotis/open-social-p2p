# ADR 0005: Media profile v1, 720p at low bitrate with metadata stripped

**Status:** Accepted (2026-10-21)

## Context

Home upload speeds are limited, relayed traffic costs someone money, and in a young network popular videos may be hosted by only a few peers. Small files keep the network responsive. Phone photos and videos also carry metadata, including GPS location, that people rarely mean to publish.

## Decision

The home node transcodes everything it publishes with ffmpeg (run as a separate process). Originals stay private on the home node; **only the transcoded rendition is published.**

**Video, profile `v1-720p`**

- H.264 High profile, `yuv420p`, at most 1280 px on the long side and 720 px on the short side (portrait videos stay portrait), at most 30 fps.
- CRF 26 with a cap: `-maxrate 1500k -bufsize 3000k`. Roughly 11 MB per minute at the cap, often less.
- Keyframe every 2 seconds (useful for seeking now and for segmenting later).
- AAC, 96 kbit/s, stereo, 48 kHz.
- MP4 with `-movflags +faststart`, so playback can start before the download finishes.
- Maximum duration 10 minutes by default (configurable per node).

**Images:** WebP, at most 2048 px on the long side, quality 80.

**Thumbnails:** WebP, 480 px on the long side; for video, taken from about one second in.

**Privacy:** all container and stream metadata is removed (`-map_metadata -1 -map_chapters -1`), which strips GPS, device model and capture time. The rendition is checked with ffprobe as part of the test suite.

**No adaptive streaming yet:** one rendition per video. 1080p, 4K and multiple renditions (HLS or DASH) are deliberately out of scope for Stage 1.

## Options considered

- **480p at about 0.8 Mbit/s:** lighter, but noticeably soft on modern phone screens.
- **Multiple renditions with adaptive streaming:** better quality on good connections, but multiplies storage, transcoding time and the number of blobs to replicate. Revisit once replication and scoring work.
- **AV1, VP9 or HEVC:** better compression, but slow software encoding on modest desktops and patchy hardware decoding on older Android phones. H.264 plays everywhere.

## Consequences

- Transcoding uses the home node's CPU; a 10-minute clip can take a few minutes on a modest machine. The node reports progress.
- ffmpeg becomes a runtime dependency of the home node (packaged with it or installed from the distribution).
- Publishing never alters the person's original file.

## Validation and revisit conditions

- Tests transcode a generated clip and assert the resolution, bitrate cap and absence of metadata.
- Revisit when Stage 3 playback testing shows people need higher quality, or when replication capacity makes larger files affordable.
