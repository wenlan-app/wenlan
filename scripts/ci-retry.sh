#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Run a download or install command up to ATTEMPTS times, each bounded by
# TIMEOUT seconds. Used only for network setup steps (browser and tool
# downloads), never for tests: a hung mirror must cost minutes, not the whole
# job's time budget, and a transient failure must not fail a merge-queue run.
# The last attempt's failure is still a failure.
#
#   bash scripts/ci-retry.sh ATTEMPTS TIMEOUT_SECONDS command [args...]
set -uo pipefail

if [[ $# -lt 3 || ! "$1" =~ ^[1-9][0-9]*$ || ! "$2" =~ ^[1-9][0-9]*$ ]]; then
  echo "usage: ci-retry.sh ATTEMPTS TIMEOUT_SECONDS command [args...]" >&2
  exit 2
fi
attempts="$1"
limit="$2"
shift 2

bounded=()
if command -v timeout >/dev/null 2>&1; then
  bounded=(timeout --kill-after=30 "$limit")
elif command -v gtimeout >/dev/null 2>&1; then
  bounded=(gtimeout --kill-after=30 "$limit")
fi

status=1
for ((attempt = 1; attempt <= attempts; attempt++)); do
  ${bounded[@]+"${bounded[@]}"} "$@"
  status=$?
  if [[ $status -eq 0 ]]; then
    exit 0
  fi
  echo "::warning title=Setup retried::attempt $attempt/$attempts of '$*' exited $status" >&2
  if [[ $attempt -lt $attempts ]]; then
    sleep $((attempt * 10))
  fi
done
exit "$status"
