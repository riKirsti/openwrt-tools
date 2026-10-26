#!/usr/bin/env bash
set -euo pipefail
: "${APK_DIR:?Missing APK directory}"
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
shopt -s nullglob
package_names=()
for recipe in package/*/Makefile; do
  package=${recipe%/Makefile}
  package_names+=("${package##*/}")
done
if (( ${#package_names[@]} == 0 )); then
  echo 'No package recipes found' >&2
  exit 1
fi
apks=()
for package in "${package_names[@]}"; do
  matches=("$APK_DIR"/"$package"-*.apk)
  if (( ${#matches[@]} != 1 )) || [[ ! -s ${matches[0]} ]]; then
    echo "Expected exactly one nonempty $package APK to publish" >&2
    exit 1
  fi
  apks+=("${matches[0]}")
done
mkdir -p build
for package in "${package_names[@]}"; do
  previous_packages=(build/"$package"-*.apk)
  if (( ${#previous_packages[@]} )); then
    rm -- "${previous_packages[@]}"
  fi
done
cp -- "${apks[@]}" build/
git add -- build/
if git diff --cached --quiet -- build/; then
  echo 'The APKs in build/ are already up to date'
  exit 0
fi
git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git commit -m 'Update tools APKs in build/'
git push origin HEAD:refs/heads/main
