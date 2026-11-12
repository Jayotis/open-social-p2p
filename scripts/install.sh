#!/bin/bash
# Installs an Open Social P2P home node for the invited alpha, from one release:
# the `osp` program, Tor and ffmpeg, a new node that reads the release's
# directories, and a service that starts it at boot.
#
#   curl -fsSLO https://github.com/Jayotis/open-social-p2p/releases/download/<version>/install.sh
#   bash install.sh
#
# Run it again to update `osp` to this release: an existing node and its
# config.toml are left as they are.
#
# For tests: OSP_VERSION, OSP_BASE (where the release's files are), OSP_HOME,
# and OSP_INIT_FLAGS (answers to `osp init`'s questions).
#
# For Linux on x86_64 or arm64 (64-bit Raspberry Pi OS and other SBCs), with
# systemd. Packages are installed with apt (Debian, Ubuntu, Raspberry Pi OS);
# elsewhere it says what to install.
#
# The directories come from the release's directories.txt, never from the
# program: `osp` has no built-in directory (ADR 0015).
set -euo pipefail

# The release job writes its tag here.
VERSION="${OSP_VERSION:-@VERSION@}"
REPO="Jayotis/open-social-p2p"
BASE="${OSP_BASE:-https://github.com/$REPO/releases/download/$VERSION}"
BIN="$HOME/.local/bin"
NODE_HOME="${OSP_HOME:-$HOME/.local/share/osp}"

say() { printf '%s\n' "$*"; }
stop() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# --- What this machine is ---

if [[ "$VERSION" == "@VERSION@" ]]; then
    stop "this copy isn't from a release; set OSP_VERSION to one (say, OSP_VERSION=v0.1.0)"
fi
[[ "$(uname -s)" == Linux ]] || stop "this installer is for Linux only"
[[ $EUID -ne 0 ]] || stop "run it as the user the node should run as, not as root (it asks for sudo when it needs it)"
case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    *) stop "no build for $(uname -m): there are builds for x86_64 and arm64 (a 64-bit OS on a Raspberry Pi)" ;;
esac
# The builds need glibc 2.35 or newer (Debian 12, Ubuntu 22.04, Raspberry Pi OS 12).
glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
if [[ -z "$glibc" ]] || [[ "$(printf '%s\n' 2.35 "$glibc" | sort -V | head -1)" != 2.35 ]]; then
    stop "needs glibc 2.35 or newer (found: ${glibc:-none}); Debian 12, Ubuntu 22.04 or newer work"
fi
command -v systemctl >/dev/null || stop "needs systemd, to run the node as a service"
for tool in curl sha256sum tar; do
    command -v "$tool" >/dev/null || stop "needs $tool"
done

say "Installing Open Social P2P $VERSION for $ARCH."

# --- Tor and ffmpeg ---

missing=()
command -v tor >/dev/null || missing+=(tor)
{ command -v ffmpeg >/dev/null && command -v ffprobe >/dev/null; } || missing+=(ffmpeg)
command -v heif-convert >/dev/null || missing+=(libheif-examples)
if ((${#missing[@]})); then
    if command -v apt-get >/dev/null; then
        # The decoder for iPhone photos is a separate package on newer releases.
        if apt-cache show libheif-plugin-libde265 >/dev/null 2>&1; then
            missing+=(libheif-plugin-libde265)
        fi
        say ""
        say "Installing with apt (sudo asks for your password): ${missing[*]}"
        sudo apt-get update -qq
        sudo apt-get install -y --no-install-recommends "${missing[@]}"
    else
        say ""
        say "Install these with your system's package manager, then run this again:"
        say "  Tor (the tor command), ffmpeg (ffmpeg and ffprobe), heif-convert (from libheif)"
        exit 1
    fi
fi

# --- The osp program ---

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() { curl -fsSL --retry 3 -o "$work/$1" "$BASE/$1" || stop "couldn't download $BASE/$1"; }
tarball="osp-linux-$ARCH.tar.gz"
fetch "$tarball"
fetch SHA256SUMS
fetch directories.txt
# This catches a damaged download, not a changed release: the sums come from
# the same place as the program.
(cd "$work" && grep " $tarball\$" SHA256SUMS | sha256sum -c --quiet -) || stop "$tarball doesn't match SHA256SUMS"
tar -xzf "$work/$tarball" -C "$work" osp

running=false
if systemctl --user is-active --quiet osp 2>/dev/null; then
    running=true
    say "Stopping the node to update it."
    systemctl --user stop osp
fi
mkdir -p "$BIN"
if [[ -e "$BIN/osp" ]]; then
    mv "$BIN/osp" "$BIN/osp.prev"
fi
install -m 755 "$work/osp" "$BIN/osp"
say "Installed $("$BIN/osp" --version) in $BIN."

# --- The node ---

if [[ -e "$NODE_HOME/config.toml" ]]; then
    say ""
    say "A node already exists in $NODE_HOME; its settings are left as they are."
    if $running; then
        systemctl --user start osp
    fi
else
    # Each line: a directory's machine name, then its public ID. # starts a comment.
    directories=()
    while read -r machine node _; do
        [[ -z "$machine" || "$machine" == \#* ]] && continue
        [[ -n "$node" ]] || stop "directories.txt: a line has a machine name and no public ID"
        directories+=(--directory "$machine" "$node")
    done < "$work/directories.txt"
    ((${#directories[@]})) || stop "directories.txt names no directories"
    say ""
    say "Creating your node. It asks two questions, shows your recovery words, then"
    say "asks for some of them back. Have paper and a pen ready."
    # Answers come from the terminal even when this script was piped in. Tests
    # answer with flags in OSP_INIT_FLAGS instead.
    if [[ -n "${OSP_INIT_FLAGS:-}" ]]; then
        # shellcheck disable=SC2086 # several flags
        "$BIN/osp" --home "$NODE_HOME" init "${directories[@]}" $OSP_INIT_FLAGS
    else
        "$BIN/osp" --home "$NODE_HOME" init "${directories[@]}" < /dev/tty
    fi
fi

if ! systemctl --user is-enabled --quiet osp 2>/dev/null; then
    say ""
    "$BIN/osp" --home "$NODE_HOME" service install
fi

say ""
say "Waiting for the node to start..."
for _ in $(seq 30); do
    "$BIN/osp" --home "$NODE_HOME" status >/dev/null 2>&1 && break
    sleep 1
done
"$BIN/osp" --home "$NODE_HOME" status || say "It isn't answering yet: try \`osp status\` in a minute."

say ""
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) say "$BIN isn't on your PATH: run it as $BIN/osp, or add that folder to PATH." ;;
esac
say "Done. \`osp --help\` lists what it does."
