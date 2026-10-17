#!/usr/bin/env bash
# plugin-precompile.sh — move a vendored plugin's startup compile out of the
# sidecar process (#1300; attack-list row 1 of the sidecar-memory
# re-attribution): compile the manifest entrypoint ahead of time and flip
# the vendored manifest to the precompiled launch shape.
#
# Default (kernel) mode:
#   dart compile kernel <entrypoint> → <stem>.dill beside the entrypoint,
#   manifest flipped to entrypoint_kind "kernel". At spawn the VM then
#   loads the precompiled kernel directly (magic-number sniff; upstream
#   Dart SDK source, runtime/bin/dfe.cc) — the in-process CFE compile of
#   the plugin's import graph (the measured ~500-850 MiB transient,
#   sidecar-memory re-attribution R2-B) never runs in the sidecar.
#
# --aot mode (pilot; dart_code_linter is the measured worst offender):
#   dart compile exe <entrypoint> → <stem>.exe beside the entrypoint,
#   manifest flipped to entrypoint_kind "exe". The sidecar becomes a
#   self-contained AOT executable — no dart front end and no JIT in the
#   process at all. A manifest already flipped to "kernel" must be
#   reverted before an --aot pass (the script refuses double flips).
#
# The manifest edit is a run-time action on the vendored (untracked) tree:
# the original source manifest is preserved once as dartq_plugin.json.bak
# (never overwritten by later flips), revert = copy it back. Only the
# entrypoint line is rewritten; timeouts, budgets, and capabilities are
# preserved verbatim.
#
# Serial-phase tool (executes dart). Requires DARTQ_DART_SDK (pinned Dart
# SDK root, >= 3.13); the produced snapshot is version-locked to that SDK's
# VM — recompile the plugins whenever the pinned SDK changes.
# Usage: scripts/plugin-precompile.sh <plugin-dir> [--aot]
set -euo pipefail

usage() {
  echo "usage: scripts/plugin-precompile.sh <plugin-dir> [--aot]" >&2
  echo "  <plugin-dir>  vendored plugin package (holds dartq_plugin.json)" >&2
  echo "  --aot         dart compile exe pilot (the exe is spawned directly)" >&2
}

AOT=0
DIR=""
for arg in "$@"; do
  case "$arg" in
    --aot) AOT=1 ;;
    -h|--help) usage; exit 0 ;;
    -*)
      echo "plugin-precompile: unknown flag '$arg'" >&2
      usage
      exit 2
      ;;
    *)
      if [ -n "$DIR" ]; then
        echo "plugin-precompile: exactly one plugin-dir is required (got '$DIR' and '$arg')" >&2
        exit 2
      fi
      DIR="$arg"
      ;;
  esac
done
if [ -z "$DIR" ]; then
  echo "plugin-precompile: <plugin-dir> is required" >&2
  usage
  exit 2
fi
# Fails when the directory does not exist; pins an absolute path so the
# closing summary can name the artifacts unambiguously.
DIR="$(cd "$DIR" && pwd)"
MANIFEST="$DIR/dartq_plugin.json"
if [ ! -f "$MANIFEST" ]; then
  echo "plugin-precompile: no dartq_plugin.json in $DIR" >&2
  exit 2
fi
if grep -q '"entrypoint_kind"' "$MANIFEST"; then
  echo "plugin-precompile: $MANIFEST already names an entrypoint_kind — refusing to double-flip" >&2
  echo "  (revert first: cp \"$MANIFEST.bak\" \"$MANIFEST\")" >&2
  exit 2
fi

# The entrypoint is the daemon's own discovery input: the manifest's
# `entrypoint` field (`bin/dartq_serve.dart` for all five vendored plugins).
ENTRY="$(sed -n 's/^[[:space:]]*"entrypoint"[[:space:]]*:[[:space:]]*"\([^"]*\)".*$/\1/p' "$MANIFEST")"
if [ -z "$ENTRY" ]; then
  echo "plugin-precompile: cannot read the entrypoint field from $MANIFEST" >&2
  exit 2
fi
if [ ! -f "$DIR/$ENTRY" ]; then
  echo "plugin-precompile: entrypoint script missing: $DIR/$ENTRY" >&2
  exit 2
fi
# dart compile resolves package: imports through the package config; a
# vendored plugin without one is not pub-resolved yet.
if [ ! -f "$DIR/.dart_tool/package_config.json" ]; then
  echo "plugin-precompile: no .dart_tool/package_config.json in $DIR —" >&2
  echo "  run 'dart pub get' inside the plugin dir first" >&2
  exit 2
fi

if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "plugin-precompile: DARTQ_DART_SDK is required (pinned Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "plugin-precompile: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi

# Output beside the entrypoint: <stem>.dill / <stem>.exe
# (bin/dartq_serve.dart → bin/dartq_serve.dill|exe) — exactly what the
# manifest flip below points at.
STEM="${ENTRY%.*}"
if [ "$AOT" -eq 1 ]; then
  KIND="exe"
  OUT="$STEM.exe"
  echo "plugin-precompile: dart compile exe $ENTRY -> $OUT (in $DIR)"
  (cd "$DIR" && dart compile exe "$ENTRY" -o "$OUT")
else
  KIND="kernel"
  OUT="$STEM.dill"
  echo "plugin-precompile: dart compile kernel $ENTRY -> $OUT (in $DIR)"
  (cd "$DIR" && dart compile kernel "$ENTRY" -o "$OUT")
fi
if [ ! -f "$DIR/$OUT" ]; then
  echo "plugin-precompile: compile reported success but $DIR/$OUT is missing" >&2
  exit 1
fi

# Flip the vendored manifest: the entrypoint now names the precompiled
# artifact and entrypoint_kind names the launch shape. The original is
# backed up exactly once so later experiments still have the source
# manifest to revert to.
if [ ! -f "$MANIFEST.bak" ]; then
  cp "$MANIFEST" "$MANIFEST.bak"
fi
awk -v entry="$OUT" -v kind="$KIND" '
  !flipped && /"entrypoint":/ {
    printf "  \"entrypoint\": \"%s\",\n", entry
    printf "  \"entrypoint_kind\": \"%s\",\n", kind
    flipped = 1
    next
  }
  { print }
' "$MANIFEST" > "$MANIFEST.tmp"
mv "$MANIFEST.tmp" "$MANIFEST"

echo "plugin-precompile: done — $MANIFEST now launches $KIND style"
echo "  entrypoint: $OUT"
echo "  source manifest preserved at dartq_plugin.json.bak"
