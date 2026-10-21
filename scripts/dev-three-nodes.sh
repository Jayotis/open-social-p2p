#!/usr/bin/env bash
# Runs three home nodes (alice, bob, carol) on this machine and walks through
# the Stage 1 scenario: Alice posts a video, Bob reshares it and hosts a copy,
# Alice goes offline, and Carol still gets the video, from Bob.
#
# Usage:
#   scripts/dev-three-nodes.sh [VIDEO] [--internet]
#
#   VIDEO       a video file to post (default: a generated 1080p test clip
#               with fake GPS metadata, to show it gets stripped)
#   --internet  use n0's public relays and DNS lookup instead of loopback only
#
# Node homes and logs are kept in .osp-dev/ for inspection afterwards.
set -euo pipefail

cd "$(dirname "$0")/.."
DEV=.osp-dev
VIDEO=""
INTERNET=0
for arg in "$@"; do
    case "$arg" in
        --internet) INTERNET=1 ;;
        *) VIDEO="$arg" ;;
    esac
done

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

cargo build --quiet --bin osp
OSP="$PWD/target/debug/osp"
osp() { local who=$1; shift; "$OSP" --home "$DEV/$who" "$@"; }

rm -rf "$DEV"
mkdir -p "$DEV"

if [[ -z "$VIDEO" ]]; then
    VIDEO="$DEV/test-1080p.mp4"
    ffmpeg -hide_banner -loglevel error -y \
        -f lavfi -i testsrc2=duration=8:size=1920x1080:rate=30 \
        -f lavfi -i sine=frequency=330:duration=8 \
        -metadata location="+51.5074-000.1278/" -metadata title="private title" \
        -c:v libx264 -preset ultrafast -crf 18 -c:a aac -shortest "$VIDEO"
fi

step "Creating nodes"
for who in alice bob carol; do
    name="$(tr '[:lower:]' '[:upper:]' <<<"${who:0:1}")${who:1}"
    if [[ $INTERNET == 1 ]]; then
        osp "$who" init --name "$name" >/dev/null
    else
        osp "$who" init --name "$name" --relay disabled --bind 127.0.0.1:0 --no-address-lookup >/dev/null
    fi
done

declare -A PIDS
start() {
    local who=$1
    "$OSP" --home "$DEV/$who" daemon >"$DEV/$who.log" 2>&1 &
    PIDS[$who]=$!
    for _ in $(seq 50); do [[ -S "$DEV/$who/osp.sock" ]] && return; sleep 0.1; done
    echo "daemon $who did not start; see $DEV/$who.log" >&2
    exit 1
}
cleanup() { for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done; }
trap cleanup EXIT
for who in alice bob carol; do start "$who"; done
echo "alice, bob and carol are running (logs in $DEV/*.log)"

step "Alice posts a video (transcoding to 720p)"
POST=$(osp alice post "Hello from Alice" --attach "$VIDEO" | awk '{print $2}')
echo "post id: $POST"

step "Bob follows Alice and syncs"
osp bob follow "$(osp alice invite --no-qr)"
osp bob sync
osp bob feed

step "Bob reshares and hosts the video"
osp bob reshare "$POST" --host

step "Carol follows Bob"
osp carol follow "$(osp bob invite --no-qr)"

step "Alice goes offline"
kill "${PIDS[alice]}"
wait "${PIDS[alice]}" 2>/dev/null || true
unset 'PIDS[alice]'

step "Carol syncs and fetches the video (from Bob)"
osp carol sync
osp carol feed
osp carol fetch "$POST" --out "$DEV/carol-out"

step "What Carol received"
OUT=$(ls "$DEV"/carol-out/*.mp4)
ffprobe -v error -show_entries stream=codec_name,width,height:format=duration,bit_rate,size \
    -of default=noprint_wrappers=1 "$OUT"
echo "metadata tags left in the file:"
ffprobe -v error -show_entries format_tags:stream_tags -of default=noprint_wrappers=1 "$OUT" \
    | grep -viE '^TAG:(handler_name|vendor_id|encoder|major_brand|minor_version|compatible_brands|language)=' \
    || echo "  (none besides technical tags)"

step "Status"
osp bob status
echo
osp carol status

step "Done"
echo "Alice's post reached Carol through Bob's hosted copy while Alice was offline."
