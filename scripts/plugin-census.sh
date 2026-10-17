#!/usr/bin/env bash
# plugin-census.sh — per-plugin registration census for the vendored
# third-party plugin fixture (tests/plugin-validation).
#
# Serial-phase tool: for each vendored plugin it spawns the sidecar Dart
# process exactly as the dartq core does (dart <entrypoint> inside the
# plugin dir), performs the wire v1 initialize handshake, and writes the
# reply's registration census to tests/plugin-validation/expectations/
# <pkg>.census.txt — the authoritative enablement ground truth (lint =
# opt-in, warning = default-on) for the preset options and the oracle
# differential.
#
# Requires DARTQ_DART_SDK (the pinned Dart SDK root; Dart >= 3.13). The
# sidecar runs plain `dart` from PATH, so the pinned SDK is prepended to
# PATH for the driver and every spawned child.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$REPO_ROOT/tests/plugin-validation"

PLUGINS=(
  leancode_lint-27.0.0
  clean_architecture_linter-2.8.0
  riverpod_lint-3.1.9
  dart_code_linter-4.4.2
  solid_lints-2.0.0
)

if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "plugin-census: DARTQ_DART_SDK is required (pinned Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "plugin-census: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi

mkdir -p "$FIXTURE/expectations"

fail=0
for plugin in "${PLUGINS[@]}"; do
  dir="$FIXTURE/vendor/$plugin"
  # The injected package_config.json is load-bearing (it maps
  # analysis_server_plugin to the in-repo shim and dartq_plugin_host to the
  # host package — neither is a pub dependency). A `dart pub get` inside the
  # vendored root silently replaces it with a pub-solved config; fail loud.
  if ! grep -q '"dartq_plugin_host"' "$dir/.dart_tool/package_config.json"; then
    echo "plugin-census: $plugin: .dart_tool/package_config.json lost its" \
      "dartq_plugin_host entry — overwritten by pub get? Restore the" \
      "injected config (see tests/plugin-validation/README.md)." >&2
    fail=1
    continue
  fi
  out="$FIXTURE/expectations/${plugin%-*}.census.txt"
  tmp="$out.tmp"
  echo "plugin-census: $plugin"
  if dart "$FIXTURE/tools/census_client.dart" "$dir" >"$tmp"; then
    mv "$tmp" "$out"
    grep '^#' "$out" | sed 's/^/  /'
  else
    rm -f "$tmp"
    echo "plugin-census: $plugin census FAILED (see stderr above)" >&2
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "plugin-census: one or more plugins failed; expectations are incomplete" >&2
  exit 1
fi
echo "plugin-census: all ${#PLUGINS[@]} censuses written to tests/plugin-validation/expectations/"
