#!/usr/bin/env bash
set -euo pipefail

patch=${1:?Missing patch file}
digest=${2:?Missing patch SHA-256}
base=${3:?Missing baseline commit}
if [[ $(git rev-parse HEAD) != "$base" ]]; then
  echo 'Candidate baseline does not match the checked-out commit' >&2
  exit 1
fi
printf '%s  %s\n' "$digest" "$patch" | sha256sum -c -
git apply --index --whitespace=error-all "$patch"
