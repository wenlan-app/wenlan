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

# `timeout` stops the command it started, but an `apt-get` that command ran
# through sudo (Playwright's --with-deps) is outside its reach and keeps going.
# The next attempt then fails at once on the dpkg lock, so one slow mirror
# used to spend every attempt. Before a retry, wait (bounded by the same
# limit) for any apt-get or dpkg still running to finish. Without pgrep there
# is nothing to measure, and the retry goes ahead as before.
wait_for_leftover_apt() {
  command -v pgrep >/dev/null 2>&1 || return 0
  [[ -d /var/lib/dpkg ]] || return 0
  local waited=0
  while pgrep -x apt-get >/dev/null 2>&1 || pgrep -x dpkg >/dev/null 2>&1; do
    if [[ $waited -ge $limit ]]; then
      echo "::warning title=Setup retried::apt-get or dpkg still running after ${limit}s" >&2
      return 0
    fi
    sleep 5
    waited=$((waited + 5))
  done
}

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
    wait_for_leftover_apt
  fi
done
exit "$status"
