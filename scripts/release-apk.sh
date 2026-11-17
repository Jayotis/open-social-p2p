#!/usr/bin/env bash
# Builds the Android app for a release, signs it with OSP's release key, and
# adds it and its SHA-256 to that release's draft on GitHub.
#
#   scripts/release-apk.sh v0.1.0-alpha.6            build, check, upload
#   scripts/release-apk.sh --no-upload v0.1.0-alpha.6  build and check only
#
# Run it on the maintainer's computer, from a clean checkout of the tag, after
# the Release workflow has made the draft. The key never goes to GitHub, so CI
# can't do this step (app/README.md, "Release signing").
set -euo pipefail

# The SHA-256 of OSP's release certificate. An APK signed with any other key
# (the debug key, say, when android/key.properties is missing) is refused.
RELEASE_CERT_SHA256=821adaf4cbcfc946751c57227a469ebb646e0dc9a8e2a269c012cc341c2f605c
APK_NAME=osp-android-arm64.apk

upload=1
if [ "${1:-}" = --no-upload ]; then
  upload=0
  shift
fi
tag=${1:?usage: scripts/release-apk.sh [--no-upload] <tag>}

die() { echo "release-apk: $*" >&2; exit 1; }

root=$(git rev-parse --show-toplevel)
cd "$root"

# The APK must be built from exactly what the release was tagged from.
git rev-parse -q --verify "refs/tags/$tag" > /dev/null || die "no tag $tag here (git fetch --tags?)"
[ "$(git rev-parse HEAD)" = "$(git rev-parse "$tag^{commit}")" ] || die "HEAD isn't $tag; run git checkout $tag first"
[ -z "$(git status --porcelain)" ] || die "the checkout has changes; build from a clean one"

[ -f app/android/key.properties ] || die "app/android/key.properties is missing (copy it from ~/keys, see notes/runbook.md)"

apksigner=${APKSIGNER:-}
if [ -z "$apksigner" ]; then
  sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/sdk}}
  apksigner=$(ls -d "$sdk"/build-tools/*/apksigner 2> /dev/null | sort -V | tail -1)
fi
[ -x "$apksigner" ] || die "apksigner not found; set APKSIGNER or ANDROID_HOME"

if [ "$upload" = 1 ]; then
  draft=$(gh release view "$tag" --json isDraft -q .isDraft 2> /dev/null) || die "no release $tag on GitHub yet; push the tag and wait for the Release workflow"
  [ "$draft" = true ] || die "release $tag is already published; add the APK only to a draft"
fi

# Android installs an update only when its version code is higher than the
# installed one's. The commit count only grows along main.
build_number=$(git rev-list --count HEAD)
build_name=${tag#v}

echo "Building $build_name (build $build_number)..."
(cd app && flutter build apk --release --target-platform android-arm64 \
  --build-name="$build_name" --build-number="$build_number")
apk=app/build/app/outputs/flutter-apk/app-release.apk

cert=$("$apksigner" verify --print-certs "$apk" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p')
[ "$cert" = "$RELEASE_CERT_SHA256" ] || die "the APK is signed with $cert, not OSP's release key"
echo "Signed with OSP's release key."

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
cp "$apk" "$out/$APK_NAME"

if [ "$upload" = 0 ]; then
  (cd "$out" && sha256sum "$APK_NAME")
  keep="$root/app/build/$APK_NAME"
  cp "$out/$APK_NAME" "$keep"
  echo "Not uploaded (--no-upload). The APK is at $keep"
  exit 0
fi

# Add the APK's line to the draft's SHA256SUMS, replacing one from an earlier run.
gh release download "$tag" --pattern SHA256SUMS --dir "$out"
grep -v " $APK_NAME\$" "$out/SHA256SUMS" > "$out/SHA256SUMS.new" || true
(cd "$out" && sha256sum "$APK_NAME") >> "$out/SHA256SUMS.new"
mv "$out/SHA256SUMS.new" "$out/SHA256SUMS"
cat "$out/SHA256SUMS"

gh release upload "$tag" --clobber "$out/$APK_NAME" "$out/SHA256SUMS"
echo "Added $APK_NAME and its SHA-256 to the draft $tag."
