#!/usr/bin/env bash
# sdk-matrix-versions.sh — resolve the multi-SDK matrix version set.
#
# The stable band is resolved LIVE from the dart-archive bucket (the
# authority for "released"): every 3.x minor line that has at least one
# published stable release, at its numerically newest patch. The dev band is
# the pinned snapshot in scripts/sdk-versions.txt (dev pins advance only
# green or by explicit bump — never auto-tracked, dev builds are yanked on
# regression). Output is one `version|channel` line per SDK, ascending.
#
# Modes:
#   scripts/sdk-matrix-versions.sh              stable (live) + dev (pinned)
#   scripts/sdk-matrix-versions.sh --stable     stable band only (live)
#   scripts/sdk-matrix-versions.sh --dev        dev band only (pinned snapshot)
#   scripts/sdk-matrix-versions.sh --stable-tip the single newest stable
#                                               release (live VERSION file)
#
# Any resolution failure — curl error, unparseable payload, truncated
# listing, empty band — exits 2 with the reason on stderr. There is no
# fallback release set: a silently stale matrix would gate against SDKs
# that no longer exist.

set -u -o pipefail
cd "$(dirname "$0")/.."

GCS_LISTING_URL="https://storage.googleapis.com/storage/v1/b/dart-archive/o?prefix=channels/stable/release/&delimiter=/&maxResults=1000"
STABLE_VERSION_URL="https://storage.googleapis.com/dart-archive/channels/stable/release/latest/VERSION"
SNAPSHOT="scripts/sdk-versions.txt"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "sdk-matrix-versions: $1" >&2
  exit 2
}

fetch() { # $1 = URL; stdout = body
  curl -fsSL --retry 3 --max-time 60 "$1" ||
    die "fetch failed: $1 (network required — the stable band resolves live)"
}

# The stable band: every released 3.x minor at its numerically newest patch.
# The listing is lexicographic from GCS, so patch picks are numeric maxima,
# not last-lexicographic. A truncated listing (nextPageToken) is an error —
# a partial band would silently drop minor lines.
stable_band() {
  fetch "$GCS_LISTING_URL" | python3 -c '
import json, re, sys
try:
    data = json.load(sys.stdin)
except ValueError as e:
    sys.exit("sdk-matrix-versions: GCS listing is not valid JSON: %s" % e)
if "nextPageToken" in data:
    sys.exit("sdk-matrix-versions: GCS listing truncated (nextPageToken present); raise the query")
rows = {}
for prefix in data.get("prefixes", []):
    m = re.fullmatch(r"channels/stable/release/(\d+)\.(\d+)\.(\d+)/", prefix)
    if not m:
        continue
    major, minor, patch = (int(x) for x in m.groups())
    if major != 3:
        continue
    key = (major, minor)
    if key not in rows or patch > rows[key]:
        rows[key] = patch
if not rows:
    sys.exit("sdk-matrix-versions: no 3.x stable releases in the GCS listing")
for major, minor in sorted(rows):
    print("%d.%d.%d|stable" % (major, minor, rows[(major, minor)]))
' || die "stable band resolution failed (see above)"
}

# The newest stable release, from the channel's live VERSION pointer.
stable_tip() {
  version="$(fetch "$STABLE_VERSION_URL" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except ValueError as e:
    sys.exit("sdk-matrix-versions: VERSION file is not valid JSON: %s" % e)
version = data.get("version", "")
import re
if not re.fullmatch(r"\d+\.\d+\.\d+", version):
    sys.exit("sdk-matrix-versions: VERSION file carries no release version: %r" % version)
print(version)
')" || die "stable tip resolution failed (see above)"
  printf '%s|stable\n' "$version"
}

# The dev band: pinned builds from the snapshot. A snapshot without dev
# lines is a resolution failure, not an empty result — an empty dev matrix
# must be a deliberate edit, never a parse accident.
dev_band() {
  [ -f "$SNAPSHOT" ] || die "$SNAPSHOT missing (the pinned dev snapshot)"
  band="$(grep -Ev '^\s*(#|$)' "$SNAPSHOT" | grep -E '^[0-9][^|]*\|dev$' || true)"
  [ -n "$band" ] || die "no dev pins in $SNAPSHOT"
  printf '%s\n' "$band"
}

mode="all"
while [ $# -gt 0 ]; do
  case "$1" in
  --stable) mode="stable" ;;
  --dev) mode="dev" ;;
  --stable-tip) mode="stable-tip" ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "sdk-matrix-versions: unknown argument '$1' (see scripts/sdk-matrix-versions.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

case "$mode" in
all)
  stable_band
  dev_band
  ;;
stable) stable_band ;;
dev) dev_band ;;
stable-tip) stable_tip ;;
esac
