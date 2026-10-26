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
- HDR video, HLG as iPhones record by default or PQ (HDR10), is tone-mapped to SDR BT.709 (`zscale` and `tonemap`, Hable's curve) after scaling. Without it, its colours come out washed out. It makes the transcode about 1.7 times as long: 8.2 s instead of 4.9 s for an 18-second 1080p iPhone clip on a desktop.

**Images:** WebP, at most 2048 px on the long side, quality 80, turned upright as the photo says (EXIF orientation, which ffmpeg applies).

**HEIC photos,** as iPhones take them, are decoded first with `heif-convert` from libheif, into a PNG that ffmpeg transcodes as above. ffmpeg can't read them whole: iPhones store a photo as a grid of 512-pixel tiles, and ffmpeg 7.1 shows one tile as a 1-frame video, while ffmpeg 6.1 can't open the file at all. libheif applies the photo's rotation. The decoded PNG keeps the photo's metadata, so it is deleted as soon as the WebP is made. A file is taken as HEIC from the brands at its start (`heic`, `heix`, `heim` or `heis`, or the general HEIF brand with one of those among its compatible brands); other HEIF files, such as AVIF, are left to ffmpeg.

**Thumbnails:** WebP, 480 px on the long side; for video, taken from about one second in.

**Privacy:** all container and stream metadata is removed (`-map_metadata -1 -map_chapters -1`), which strips GPS, device model and capture time. The rendition is checked with ffprobe as part of the test suite.

**Transcoding limits:** so that many posts at once, or a file that makes ffmpeg hang, can't tie up the machine:

- One transcode runs at a time; other posts wait their turn, in order. At most four posts with media can be in progress at once (arriving from a phone, waiting, or transcoding); more are refused until one is done. Posts with only text aren't affected.
- A video transcode is stopped after 30 minutes by default: `max_transcode_secs` under `[media]` in `config.toml`, which a slow machine can raise. Transcoding an image or making a thumbnail is stopped after 2 minutes, and reading a file with ffprobe after 1 minute.
- ffmpeg also stops at the duration limit (`-t`), so a file that doesn't give its length, or gives a wrong one, can't produce a longer video.
- A failed transcode's scratch files are deleted.

**No adaptive streaming yet:** one rendition per video. 1080p, 4K and multiple renditions (HLS or DASH) are deliberately out of scope for Stage 1.

## Options considered

- **480p at about 0.8 Mbit/s:** lighter, but noticeably soft on modern phone screens.
- **Multiple renditions with adaptive streaming:** better quality on good connections, but multiplies storage, transcoding time and the number of blobs to replicate. Revisit once replication and scoring work.
- **AV1, VP9 or HEVC:** better compression, but slow software encoding on modest desktops and patchy hardware decoding on older Android phones. H.264 plays everywhere.

## Consequences

- Transcoding uses the home node's CPU; a 10-minute clip can take a few minutes on a modest machine. The node reports progress.
- ffmpeg becomes a runtime dependency of the home node (packaged with it or installed from the distribution). It needs the `zscale` filter (libzimg) for HDR video, as Debian's and Ubuntu's builds have.
- `heif-convert` (libheif-examples; newer versions also call it `heif-dec`) is needed for HEIC photos. Without it, those photos fail to post, and the node's status says what to install. `heif_convert` under `[media]` in `config.toml` names another program.
- Photos in a wide colour space, such as Display P3 as iPhones take them, lose their colour profile and look slightly duller. Converting them to sRGB is left for later.
- Publishing never alters the person's original file.

## Validation and revisit conditions

- Tests transcode a generated clip and assert the resolution, bitrate cap and absence of metadata.
- Tests decode a small HEIC made like an iPhone's (a grid of tiles, a rotation, EXIF with a GPS position) and assert the whole picture, upright, with no EXIF left. Others check a rotated JPEG is turned upright and HLG and PQ video come out as BT.709.
- Revisit when Stage 3 playback testing shows people need higher quality, or when replication capacity makes larger files affordable.
