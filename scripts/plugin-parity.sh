#!/usr/bin/env bash
# plugin-parity.sh — plugin-parity oracle over ONE plugin arm: the official
# analysis server's plugin face vs the dartq daemon's plugin host on the
# same corpus, same analysis_options.yaml, same vendored plugin. Directive:
# plugin-chained final results must be 1:1 with official.
#
# Arms for one plugin <name> and one parity corpus dir:
#   official — tools/parity_client.dart boots the pinned-SDK
#     analysis_server snapshot over stdio (classic analyzer protocol) with
#     ANALYZER_STATE_LOCATION_OVERRIDE on a fresh per-run dir; the corpus's
#     analysis_options.yaml declares the plugin through a `path:` source
#     into vendor/, with `plugins.dependency_overrides` pinning the
#     synthetic plugin package's framework deps to the vendored copies.
#   dartq — tools/ab_driver.dart (the plugin-ab invocation shape) runs one
#     daemon session with plugins enabled (--rounds 0: cold full analysis
#     only) and --dump-diagnostics records every `dartq/diagnostics` push
#     payload as one JSON array (dump contract: ab_driver.dart:37-40;
#     payload = {uri, diagnostics: [{range, severity, code, source,
#     message}]}, zero-based lines / UTF-16 columns — the daemon wire's
#     DiagnosticsParams/Diagnostic in crates/dartq-daemon/src/wire.rs).
#   compare — a python3 step computing Jaccard over (lowercased code,
#     corpus-relative file) per severity tier, both sides filtered to the
#     plugin's census codes (the <pkg>.census.txt format from
#     scripts/plugin-census.sh). Exit 0 only if every tier's entry sets
#     are equal; the per-tier report lands in <output-dir>/parity.json.
#
# Serial-phase tool: one analysis server or daemon instance at a time; the
# drivers bound their own waits; a single EXIT trap kills this lane's own
# survivors — the official server, the daemon, and the plugin sidecars,
# each identified by a recorded PID plus a command-identity check, never
# by a command pattern (both arms' process names are byte-identical across
# concurrent lanes, so pattern kills hit other lanes mid-run). Sidecars
# lead their own process groups by design — see plugin-ab.sh's trap.
# Requires DARTQ_DART_SDK (pinned Dart SDK root, >= 3.13) and cargo.
# Usage: scripts/plugin-parity.sh <plugin> <corpus-dir> <output-dir> [filter-file]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$REPO_ROOT/tests/plugin-validation"

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "usage: scripts/plugin-parity.sh <plugin> <corpus-dir> <output-dir> [filter-file]" >&2
  exit 2
fi
PLUGIN="$1"
CORPUS="$(cd "$2" && pwd)"
OUT="$3"
FILTER="${4:-$FIXTURE/expectations/$PLUGIN.census.txt}"

if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "plugin-parity: DARTQ_DART_SDK is required (pinned Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "plugin-parity: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
# A Flutter root passes the dart check above (its bin/dart shim) but carries
# no analysis_server snapshot — fail fast here rather than mid-official-arm.
if [ ! -f "$DARTQ_DART_SDK/bin/snapshots/analysis_server.dart.snapshot" ]; then
  echo "plugin-parity: no analysis_server snapshot at" \
    "$DARTQ_DART_SDK/bin/snapshots/analysis_server.dart.snapshot —" \
    "DARTQ_DART_SDK must be the plain Dart SDK root" \
    "(e.g. ~/fvm/default/bin/cache/dart-sdk), not the Flutter root" >&2
  exit 2
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "plugin-parity: cargo not found on PATH" >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "plugin-parity: python3 not found on PATH" >&2
  exit 2
fi
if [ ! -f "$CORPUS/analysis_options.yaml" ]; then
  echo "plugin-parity: no analysis_options.yaml in corpus $CORPUS" >&2
  exit 2
fi
if ! grep -qE "^[[:space:]]*$PLUGIN:" "$CORPUS/analysis_options.yaml"; then
  echo "plugin-parity: corpus $CORPUS does not declare plugin '$PLUGIN'" \
    "in its analysis_options.yaml plugins: section" >&2
  exit 2
fi
if [ ! -f "$FILTER" ]; then
  echo "plugin-parity: no census filter file: $FILTER" >&2
  echo "plugin-parity: run scripts/plugin-census.sh first, or pass a filter" \
    "file (the <pkg>.census.txt format) as the fourth argument" >&2
  exit 2
fi

# One release build up front; the arms measure analysis, not compilation.
# Freshness gate: any crate source newer than the binary forces a rebuild —
# build-only-if-missing would silently measure a stale daemon after edits.
BIN="$REPO_ROOT/target/release/dartq"
if [ ! -x "$BIN" ] || [ -n "$(find "$REPO_ROOT/crates" -name '*.rs' -new "$BIN" -print -quit)" ]; then
  echo "plugin-parity: building dartq-cli (release; binary missing or older than crates/)"
  cargo build --release -p dartq-cli --quiet
fi

mkdir -p "$OUT"
STATE="$OUT/plugin-state"
rm -rf "$STATE"
mkdir -p "$STATE"
# Recorded-PID files for the survivor trap: the official server's PID is
# discovered while its client runs, the daemon's PID is written by the
# driver (--pidfile).
server_pid_file="$OUT/official.server.pid"
daemon_pid_file="$OUT/dartq.daemon.pid"

# Echoes the PID recorded in $1, or nothing when the file is absent, empty,
# or non-numeric (stale/reuse guards live at the kill sites).
pid_from() {
  local pid
  [ -f "$1" ] || return 0
  pid="$(cat "$1" 2>/dev/null)" || return 0
  case "$pid" in ''|*[!0-9]*) return 0 ;; esac
  printf '%s\n' "$pid"
}

# Scoped survivor cleanup (plugin-ab pattern): kills exactly this lane's
# recorded processes. The official server's PID comes from the official
# arm's discovery loop, the daemon's from the driver's --pidfile; every
# kill is gated on the PID's CURRENT command line matching its expected
# identity, so a dead-then-reused PID is skipped. No step pattern-matches
# across processes — the old pkill -f traps byte-matched any concurrent
# lane's server/daemon/sidecars and killed them mid-run (the A-B
# incident's suspected killer: both lanes share one binary path and one
# --client-id constant).
cleanup_survivors() {
  local pid sidecar
  pid="$(pid_from "$server_pid_file")"
  case "$(ps -o command= -p "$pid" 2>/dev/null)" in
    *analysis_server.dart.snapshot*) kill "$pid" 2>/dev/null || true ;;
  esac
  pid="$(pid_from "$daemon_pid_file")"
  [ -n "$pid" ] || return 0
  # Sidecars first: children of the still-live daemon — once the daemon
  # dies they reparent and become unattributable.
  for sidecar in $(pgrep -P "$pid" 2>/dev/null); do
    case "$(ps -o command= -p "$sidecar" 2>/dev/null)" in
      *bin/dartq_serve.dart*) kill "$sidecar" 2>/dev/null || true ;;
    esac
  done
  case "$(ps -o command= -p "$pid" 2>/dev/null)" in
    *"$BIN daemon"*) kill "$pid" 2>/dev/null || true ;;
  esac
}
trap cleanup_survivors EXIT

echo "plugin-parity: plugin=$PLUGIN corpus=${CORPUS#"$REPO_ROOT"/} out=${OUT#"$REPO_ROOT"/}"

# Package resolution for the dartq face (the official face reads only
# analysis_options.yaml; its synthetic plugin package lives under $STATE).
(cd "$CORPUS" && dart pub get >/dev/null)

echo "plugin-parity: official arm (analysis_server snapshot)"
# The client runs backgrounded so this lane's analysis-server PID can be
# recorded while it lives: the server is the client's direct child
# carrying the pinned snapshot path. The EXIT trap identity-checks before
# killing, so only THIS lane's survivor is ever targeted — the old
# pattern kill matched --client-id, a constant every concurrent parity
# lane shares. `wait` preserves the real exit status.
rm -f "$server_pid_file"
status=0
ANALYZER_STATE_LOCATION_OVERRIDE="$STATE" \
  dart "$FIXTURE/tools/parity_client.dart" "$CORPUS" "$OUT/official.jsonl" \
  >"$OUT/official.log" 2>&1 &
client=$!
while kill -0 "$client" 2>/dev/null; do
  for pid in $(pgrep -P "$client" 2>/dev/null); do
    case "$(ps -o command= -p "$pid" 2>/dev/null)" in
      *analysis_server.dart.snapshot*) echo "$pid" >"$server_pid_file" ;;
    esac
  done
  sleep 0.2
done
wait "$client" || status=$?
if [ "$status" -ne 0 ]; then
  echo "plugin-parity: official arm FAILED (exit $status) — see ${OUT#"$REPO_ROOT"/}/official.log" >&2
  exit "$status"
fi

echo "plugin-parity: dartq arm (daemon session, plugins enabled)"
rm -f "$daemon_pid_file"
status=0
# The widened quiet window is the correctness gate against the driver's
# default 2 s cold-wave terminator: a plugin sidecar's first merge can
# publish later than that on a cold VM start and would otherwise be cut
# off by the session EOF, silently missing from the dump. The wider window
# costs one quiet wait at wave end, once per run.
dart "$FIXTURE/tools/ab_driver.dart" \
  --daemon "$BIN" \
  --dir "$CORPUS" \
  --sdk "$DARTQ_DART_SDK" \
  --rounds 0 \
  --pidfile "$daemon_pid_file" \
  --quiet-window-ms 30000 \
  --dump-diagnostics "$OUT/dartq.json" \
  >"$OUT/dartq.driver.json" 2>"$OUT/dartq.log" || status=$?
if [ "$status" -ne 0 ]; then
  echo "plugin-parity: dartq arm FAILED (exit $status) — see ${OUT#"$REPO_ROOT"/}/dartq.log" >&2
  exit "$status"
fi

echo "plugin-parity: comparing (Jaccard over (code, file) per severity tier, census-filtered)"
cmp_status=0
python3 - "$PLUGIN" "$CORPUS" "$FILTER" "$OUT/official.jsonl" "$OUT/dartq.json" \
  "$OUT/parity.json" <<'PYEOF' || cmp_status=$?
import json
import os
import sys
import urllib.parse

plugin, corpus, filter_path, official_path, dartq_path, report_path = sys.argv[1:7]
corpus = os.path.realpath(corpus)


def rel(path):
    return os.path.relpath(os.path.realpath(path), corpus)


# Census filter: the plugin-attributable code set — third tab-separated
# field of each census row, comma-joined name=severity pairs (the
# scripts/plugin-census.sh output format). Header lines are comments.
codes = set()
for line in open(filter_path):
    line = line.rstrip("\n")
    if not line or line.startswith("#"):
        continue
    parts = line.split("\t")
    if len(parts) < 3:
        continue
    for pair in parts[2].split(","):
        name = pair.split("=", 1)[0].strip().lower()
        if name:
            codes.add(name)
if not codes:
    print(
        "plugin-parity: warning: filter file has no codes; comparing unfiltered",
        file=sys.stderr,
    )

# Official side: parity_client's JSON-lines event log; plugin diagnostics
# arrive merged into analysis.errors notifications.
official = {}  # tier -> {(code, corpus-relative file), ...}
for line in open(official_path):
    line = line.strip()
    if not line:
        continue
    frame = json.loads(line)
    if frame.get("event") != "analysis.errors":
        continue
    params = frame.get("params") or {}
    path = params.get("file")
    if not path:
        continue
    # analysis.errors params carry no top-level severity — each error in
    # errors[] carries its own (analyzer-protocol AnalysisError), lowercased
    # here to the same error/warning/info keys the dartq side tiers by.
    for error in params.get("errors") or []:
        code = (error.get("code") or "").lower()
        if not code or (codes and code not in codes):
            continue
        tier = str(error.get("severity") or "").lower()
        official.setdefault(tier, set()).add((code, rel(path)))

# dartq side: ab_driver's --dump-diagnostics JSON array of `dartq/diagnostics`
# push payloads. Shape contract (ab_driver.dart:37-40 dump, :346-356 uri/row
# access; crates/dartq-daemon/src/wire.rs DiagnosticsParams/Diagnostic):
#   {uri: "file://...", diagnostics: [{range, severity, code, source,
#   message}]}, positions zero-based lines / UTF-16 columns, severity a
#   required int (1 error · 2 warning · 3 information — closed domain, no
#   LSP hint tier), code a required snake_case string. The `dartq/`
#   namespace is the host's own tool-diagnostic channel (plugin_crashed /
#   plugin_timeout today) — it is the arm reporting the plugin's health,
#   not a plugin finding, so it is pulled out of the compared sets. Two
#   assertions pin the contract: any dartq/-namespaced code OUTSIDE the
#   known internal set, and any row whose severity or code falls outside
#   the shapes above, mean the daemon's wire drifted from this comparator
#   and the comparison must fail loudly rather than silently filter rows
#   (a silently dropped dartq row could turn a real mismatch into a false
#   parity pass).
dartq_tier = {1: "error", 2: "warning", 3: "info"}
dartq_internal_codes = {"dartq/plugin_crashed", "dartq/plugin_timeout"}
dartq = {}
dartq_internal = []  # (code, file, tier) rows excluded from the comparison
shape_breaches = []  # rows off the dartq-RPC contract pinned above
for publish in json.load(open(dartq_path)):
    uri = publish.get("uri") or ""
    if not uri.startswith("file://"):
        continue
    path = urllib.parse.unquote(urllib.parse.urlparse(uri).path)
    rfile = rel(path)
    for diagnostic in publish.get("diagnostics") or []:
        code = diagnostic.get("code")
        if not isinstance(code, str) or not code:
            shape_breaches.append(f"{uri}: non-string code {code!r}")
            continue
        code = code.lower()
        tier = dartq_tier.get(diagnostic.get("severity"))
        if tier is None:
            shape_breaches.append(
                f"{uri}: severity {diagnostic.get('severity')!r} outside 1/2/3"
            )
            continue
        if code.startswith("dartq/"):
            dartq_internal.append((code, rfile, tier))
            continue
        if codes and code not in codes:
            continue
        dartq.setdefault(tier, set()).add((code, rfile))

unknown_internal = sorted(
    {code for code, _f, _t in dartq_internal} - dartq_internal_codes
)
if shape_breaches or unknown_internal:
    for breach in shape_breaches[:20]:
        print("plugin-parity: dartq-side shape breach: " + breach, file=sys.stderr)
    if unknown_internal:
        print(
            "plugin-parity: unexpected dartq-internal diagnostic code(s) "
            + ", ".join(unknown_internal)
            + " — the comparator's exclusion is stale; refusing to compare",
            file=sys.stderr,
        )
    sys.exit(2)

report = {
    "plugin": plugin,
    "caliber": (
        "Jaccard over (lowercased code, corpus-relative file) per severity "
        "tier, both sides filtered to the plugin's census codes"
    ),
    "equal": True,
    "dartq_internal_excluded": sorted(
        f"{code} {rfile} {tier}" for code, rfile, tier in dartq_internal
    ),
    "tiers": {},
}
for tier in sorted(set(official) | set(dartq)):
    o = official.get(tier, set())
    d = dartq.get(tier, set())
    union = o | d
    report["tiers"][tier] = {
        "jaccard": len(o & d) / len(union) if union else 1.0,
        "official": len(o),
        "dartq": len(d),
        "intersection": len(o & d),
        "union": len(union),
        "official_only_count": len(o - d),
        "dartq_only_count": len(d - o),
        "official_only": sorted(f"{c} {f}" for c, f in (o - d))[:20],
        "dartq_only": sorted(f"{c} {f}" for c, f in (d - o))[:20],
    }
    if o != d:
        report["equal"] = False
with open(report_path, "w") as f:
    json.dump(report, f, indent=2, sort_keys=True)
    f.write("\n")

for tier, data in report["tiers"].items():
    print(
        f"plugin-parity: {plugin} {tier}: jaccard={data['jaccard']:.4f} "
        f"official={data['official']} dartq={data['dartq']} "
        f"official_only={data['official_only_count']} "
        f"dartq_only={data['dartq_only_count']}"
    )
sys.exit(0 if report["equal"] else 1)
PYEOF

if [ "$cmp_status" -eq 0 ]; then
  echo "plugin-parity: PARITY — every tier's (code, file) sets are equal (${OUT#"$REPO_ROOT"/}/parity.json)"
else
  echo "plugin-parity: MISMATCH — see ${OUT#"$REPO_ROOT"/}/parity.json (official_only/dartq_only per tier)" >&2
fi
exit "$cmp_status"
