#!/usr/bin/env bash
set -euo pipefail

started=$SECONDS
BUILD_JOBS=$(nproc)
export BUILD_JOBS
echo "Build jobs: $BUILD_JOBS; build mode: $TOOLS_BUILD_MODE"
echo "Build jobs: $BUILD_JOBS; build mode: $TOOLS_BUILD_MODE" >> "$GITHUB_STEP_SUMMARY"
cache_dir="$RUNNER_TEMP/tools-cache"
output_dir="$RUNNER_TEMP/tools-apk"
mkdir -p "$cache_dir/downloads" "$cache_dir/cargo-target" "$output_dir"
download=
restore_runner_files() {
  if [[ -n "$download" ]]; then
    rm -f -- "$download"
  fi
  sudo chown -R "$(id -u):$(id -g)" "$cache_dir" "$output_dir"
}
trap restore_runner_files EXIT
recipe=package/protun-openwrt/Makefile
version=$(sed -n 's/^PKG_VERSION:=//p' "$recipe")
revision=$(sed -n 's/^PKG_SOURCE_COMMIT:=//p' "$recipe")
source_hash=$(sed -n 's/^PKG_HASH:=//p' "$recipe")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$revision" =~ ^[0-9a-f]{40}$ && "$source_hash" =~ ^[0-9a-f]{64}$ ]]
archive="$cache_dir/downloads/protunnext-$version.tar.gz"
if ! echo "$source_hash  $archive" | sha256sum -c --status; then
  download=$(mktemp "$cache_dir/downloads/.protunnext.XXXXXX")
  curl --fail --silent --show-error --location --proto '=https' --retry 3 --max-time 120 \
    "https://codeload.github.com/riKirsti/ProtunNext/tar.gz/$revision" --output "$download"
  echo "$source_hash  $download" | sha256sum -c -
  mv -- "$download" "$archive"
  download=
fi
sudo chown -R 1000:1000 "$cache_dir" "$output_dir"
smoke_mount=()
if [[ -n "${TAILSCALE_SMOKE_BASELINE:-}" ]]; then
  smoke_mount=(--mount "type=bind,source=$TAILSCALE_SMOKE_BASELINE,target=/baseline-tailscale-smoke.c,readonly")
fi
docker run --rm \
  "${smoke_mount[@]}" \
  --env SDK_ARCH \
  --env PACKAGES_COMMIT \
  --env RUST_VERSION \
  --env TOOLS_BUILD_MODE \
  --env TOOLS_RUST_ROOT=/opt/rust \
  --env BUILD_JOBS \
  --env TOOLS_CARGO_TARGET_DIR=/cache/target \
  --mount "type=bind,source=$GITHUB_WORKSPACE/package,target=/feed,readonly" \
  --mount "type=bind,source=$GITHUB_WORKSPACE/.github/scripts,target=/ci,readonly" \
  --mount "type=bind,source=$RUST_TOOLCHAIN_DIR,target=/opt/rust,readonly" \
  --mount "type=bind,source=$cache_dir/downloads,target=/builder/dl" \
  --mount "type=bind,source=$cache_dir/cargo-target,target=/cache/target" \
  --mount "type=bind,source=$output_dir,target=/out" \
  --workdir /builder --entrypoint /bin/bash \
  "$RESOLVED_SDK_IMAGE" /ci/build-sdk.sh
echo "SDK build and checks: $((SECONDS - started))s" >> "$GITHUB_STEP_SUMMARY"
