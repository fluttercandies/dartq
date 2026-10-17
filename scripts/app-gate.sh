#!/usr/bin/env bash
# app-gate.sh — the --flutter-app corpus differential + RSS leg (#2063).
#
# The super-complex bar's Flutter-widget face. Regenerates the corpus-gen
# --flutter-app trees at 1k and 10k (tests/corpus/app/<scale>, seed 0 — the
# deterministic contract; git-ignore the tree like tests/corpus/mega/),
# materializes the hand-written .dart_tool/package_config.json the generated
# tree lacks (the app package plus every packages/* entry — the flutter stub
# included; languageVersion 3.13, the run-face floor), then runs the
# complex-gate differential idiom per scale: oracle dart analyze vs the
# release analyze face, ERROR-tier, threshold 1.0 (Jaccard). No
# --persist-regressions — same ruling as complex-gate: an unproven posture
# must not grow the frozen regression set mid-chain. --allow-vacuous follows
# the mega clean-face ruling (a genuinely clean oracle side records instead
# of failing; the adversarial overlay's errstorm packages normally carry
# rows anyway).
#
# Then the dartq-side rss-100k bench over the 10k tree asserts peak RSS
# <= APP_RSS_10K_LIMIT_KIB (default 2 GiB; charter math: ~520k LOC at the
# mega ratios x 200 MB/100k is ~1.0 GiB, padded for fixed overhead — the
# absolute-peak form avoids the A2 below-100k normalization distortion).
#
# Golden check (skip-if-absent, like the other optional legs): when
# goldens/app-x/slice/SHA256SUMS exists, the slice-scale tree is
# regenerated and the pinned files (see goldens/app-x/slice/README.md)
# are hash-compared — the byte-stability pin for the face's scale-invariant
# files. Absent pins = clean logged skip; the morning serial phase pins them.
#
# SERIAL PHASE ONLY: spawns dart + dartq over the 1k/10k trees and cargo
# builds corpus-gen when a scale tree is absent. HEAVY: safe to drop from a
# tight window via `serial-gates.sh --skip app-gate`.
#
# Usage: scripts/app-gate.sh [--label L]
#   --label L  evidence dir name under target/evidence/gate-app/ (default
#              APP_LABEL env, else app-r1). An existing dir refuses the run
#              — recorded evidence is never overwritten.
# Exit codes: 0 green; 1 a differential/RSS/golden breach; 2 environment.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

label="${APP_LABEL:-app-r1}"
rss_limit_kib="${APP_RSS_10K_LIMIT_KIB:-2097152}"
while [ $# -gt 0 ]; do
  case "$1" in
  --label)
    [ $# -ge 2 ] && { label="$2"; shift 2; } || {
      echo "app-gate: --label needs a value" >&2
      exit 2
    }
    ;;
  -h | --help)
    sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "app-gate: unknown argument '$1' (see scripts/app-gate.sh --help)" >&2
    exit 2
    ;;
  esac
done
out="target/evidence/gate-app/$label"

command -v jq >/dev/null 2>&1 || { echo "app-gate: jq is required (brew install jq)" >&2; exit 2; }
[ -e "$out" ] && { echo "app-gate: $out already exists — refusing to overwrite recorded evidence (bump --label/APP_LABEL)" >&2; exit 2; }
[ -x ./target/release/harness ] || { echo "app-gate: ./target/release/harness missing — release-rebuild first (cargo build --release -p dartq-cli -p harness)" >&2; exit 2; }
[ -x ./target/release/dartq ] || { echo "app-gate: ./target/release/dartq missing — release-rebuild first" >&2; exit 2; }
FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
[ -x "$dart_bin" ] || { echo "app-gate: fvm default dart not found at $dart_bin (set FVM_HOME)" >&2; exit 2; }
dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $sdk_root}"
export DART_SDK="$sdk_root"
# The 10k-file oracle batch exceeds the harness 120 s default (mega-run raises the same way).
export DARTQ_HARNESS_ORACLE_TIMEOUT_MS="${DARTQ_HARNESS_ORACLE_TIMEOUT_MS:-600000}"

for scale in 1k 10k; do
  dir="tests/corpus/app/$scale"
  if [ ! -f "$dir/manifest.json" ]; then
    echo "-- [$scale] tree absent; generating (corpus-gen --flutter-app --seed 0)"
    cargo run -q -p corpus-gen -- --out "$dir" --scale "$scale" --seed 0 --flutter-app ||
      { echo "app-gate: generation of $scale failed" >&2; exit 2; }
  fi
  config="$dir/.dart_tool/package_config.json"
  if [ ! -f "$config" ]; then
    echo "-- [$scale] materializing $config"
    mkdir -p "$dir/.dart_tool" || exit 2
    entries='[{"name":"app","rootUri":"..","packageUri":"lib/","languageVersion":"3.13"}]'
    for d in "$dir"/packages/*/; do
      [ -f "${d}pubspec.yaml" ] || continue
      name="$(sed -n 's/^name: *//p' "${d}pubspec.yaml" | head -n1)"
      [ -n "$name" ] || continue
      rel="../${d#"$dir"/}"
      rel="${rel%/}"
      entries="$(printf '%s' "$entries" | jq -c --arg n "$name" --arg r "$rel" \
        '. + [{name: $n, rootUri: $r, packageUri: "lib/", languageVersion: "3.13"}]')" || exit 2
    done
    jq -n --argjson p "$entries" '{configVersion: 2, packages: $p}' >"$config" || exit 2
  fi
done

# Golden byte-stability check — skip-if-absent like the other optional legs.
if [ -f goldens/app-x/slice/SHA256SUMS ]; then
  sums="$PWD/goldens/app-x/slice/SHA256SUMS"
  scratch="target/gate/app-x-slice"
  rm -rf "$scratch"
  cargo run -q -p corpus-gen -- --out "$scratch" --scale slice --seed 0 --flutter-app >/dev/null ||
    { echo "app-gate: slice regeneration failed" >&2; exit 2; }
  if (cd "$scratch" && shasum -a 256 -c "$sums"); then
    echo "-- golden slice hashes match (goldens/app-x/slice)"
  else
    echo "app-gate: golden slice hash mismatch — the pinned face bytes drifted (goldens/app-x/slice/SHA256SUMS)" >&2
    exit 1
  fi
else
  echo "-- SKIP golden check: goldens/app-x/slice/SHA256SUMS absent (the morning serial phase pins it; see the README there)"
fi

for scale in 1k 10k; do
  echo "-- [$scale] differential: oracle vs release analyze face (ERROR Jaccard >= 1.0)"
  if ! ./target/release/harness run \
    --corpus-dir "tests/corpus/app/$scale" \
    --oracle-dart "$dart_bin" \
    --severity-filter ERROR \
    --threshold 1.0 \
    --allow-vacuous \
    --dartq-cmd "$dartq_cmd" \
    --out "$out/app-$scale"; then
    echo "app-gate: differential at $scale below bar or harness error (report: $out/app-$scale)" >&2
    exit 1
  fi
done

echo "-- [10k] rss bench: peak RSS <= ${rss_limit_kib} KiB (override APP_RSS_10K_LIMIT_KIB)"
if ! ./target/release/harness bench --metric rss-100k --corpus-dir tests/corpus/app/10k \
  --out "$out" --dartq-cmd "$dartq_cmd" >/dev/null; then
  echo "app-gate: rss bench failed (harness error)" >&2
  exit 2
fi
peak_kib="$(jq -r '.peak_rss_kib // empty' "$out/rss-100k/10k/report.json" 2>/dev/null)" || peak_kib=""
[ -n "$peak_kib" ] || { echo "app-gate: rss report missing at $out/rss-100k/10k/report.json" >&2; exit 2; }
if [ "$peak_kib" -le "$rss_limit_kib" ]; then
  echo "app-gate: PASS (differentials 1k/10k at Jaccard 1.0; rss@10k peak ${peak_kib} KiB <= ${rss_limit_kib} KiB; reports under $out)"
  exit 0
fi
echo "app-gate: FAIL rss@10k peak ${peak_kib} KiB > ${rss_limit_kib} KiB" >&2
exit 1
