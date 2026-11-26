#!/usr/bin/env bash
set -euo pipefail
target=${1:?Missing qualified publication branch}
if [[ "$target" != main ]]; then
  git push origin "HEAD:refs/heads/$target"
  exit
fi

: "${GH_TOKEN:?Missing check-writing token}"
: "${GITHUB_REPOSITORY:?Missing workflow repository}"
: "${GITHUB_RUN_ID:?Missing workflow identity}"
: "${GITHUB_RUN_ATTEMPT:?Missing workflow attempt}"
commit=$(git rev-parse HEAD)
staging="refs/heads/ci/qualified/$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
git push origin "$commit:$staging"
trap 'git push origin ":$staging" || printf "%s\n" "Could not remove temporary qualified ref" >&2' EXIT
gh api "repos/$GITHUB_REPOSITORY/check-runs" \
  -f name='build / build' -f head_sha="$commit" -f status=completed -f conclusion=success \
  -f details_url="https://github.com/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID" \
  --jq '.head_sha' | grep -Fx "$commit"
destination="refs/heads/$target"
for delay in 0 5 10 20; do
  if (( delay )); then sleep "$delay"; fi
  if output=$(git push origin "$commit:$destination" 2>&1); then
    printf '%s\n' "$output"
    exit 0
  fi
  printf '%s\n' "$output" >&2
  case "$output" in
    *commit_refs_rules*|*'Required status check'*|*'required status check'*) ;;
    *) exit 1 ;;
  esac
done
exit 1
