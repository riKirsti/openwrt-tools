#!/usr/bin/env bash
set -euo pipefail

patch=${1:?Missing patch file}
digest=${2:?Missing patch SHA-256}
printf '%s  %s\n' "$digest" "$patch" | sha256sum -c -
git apply --index --whitespace=error-all "$patch"
