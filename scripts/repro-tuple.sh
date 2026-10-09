#!/usr/bin/env bash
# repro-tuple.sh — emit the PRD §19.5 reproducibility tuple for a dartiq run.
#
# What it captures (the environment identity face of §19.5):
#   - git_head:            HEAD commit the working tree is based on
#   - worktree_patch_sha256: hash over `git diff HEAD` bytes — every staged
#                          or unstaged change to tracked files, so two runs
#                          of the same commit with clean trees share one
#                          identity, and any tracked edit splits it
#   - dirty:               whether any tracked file differs from HEAD
#   - rustc / cargo:       exact toolchain versions (`rustc -V`, `cargo -V`)
#   - toolchain_pin:       channel from rust-toolchain.toml
#   - output_sha256:       hash of the run-output file given via --output
#
# Output-face note: crates/dartq-engine/tests/reproducibility.rs already
# pins §19.5's OUTPUT determinism (byte-identical machine-format lines
# across repeated in-process runs). This script pins the identity of the
# ENVIRONMENT those runs happen in, so CI can assert "same tuple ⇒ same
# output digest" across runs and machines. The per-Context faces of the
# tuple (SDK, PackageGraph, LanguageVersions, Options, Target,
# ConditionalEnv) are pinned by the harness run inputs themselves and are
# not re-derived here.
#
# Digest hook (serial verification phase): after the oracle run writes its
# output file, pass it through --output to seal the tuple:
#
#   scripts/repro-tuple.sh --output <oracle-run-output-file>
#
# The emitted line is deliberately timestamp-free: two invocations in the
# same repository state produce byte-identical tuples, so gate assertions
# can compare the lines (or their hashes) directly.
#
# Identity scope: tracked-file content only. Untracked workspace material
# (docs/, .third/, .goal/) is intentionally not part of the identity — it
# is not commit-tracked repo content. For CI-grade tuples, run from a clean
# committed tree (dirty: false).

set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/repro-tuple.sh [--output FILE]

  --output FILE  Hash this run-output file into the tuple's output_sha256
                 field (the serial-phase oracle run output, or any artifact
                 whose reproducibility is being asserted).

Emits exactly one JSON object per invocation; identical repository and
toolchain state yield byte-identical output.
EOF
}

OUTPUT_FILE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output)
      [ $# -ge 2 ] || { echo "error: --output needs a value" >&2; exit 2; }
      OUTPUT_FILE="$2"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

# Hash stdin with the first available sha256 tool (macOS: shasum; Linux:
# sha256sum). Failing loud — a tuple without a working digest is useless.
hash_stdin() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  else
    echo "error: need shasum or sha256sum to compute digests" >&2
    exit 2
  fi
}

json_escape() { local s=$1; s=${s//\\/\\\\}; s=${s//\"/\\\"}; printf '%s' "$s"; }

command -v git >/dev/null 2>&1 || { echo "error: git not on PATH" >&2; exit 2; }
GIT_TOP="$(git rev-parse --show-toplevel 2>/dev/null)" \
  || { echo "error: not inside a git repository; the tuple needs a repo identity" >&2; exit 2; }
cd "$GIT_TOP"

GIT_HEAD="$(git rev-parse HEAD)"

# Worktree identity: exact patch bytes vs HEAD. Flags pin away external diff
# drivers and color so the hash depends only on content.
WORKTREE_PATCH_SHA="$(git --no-pager diff HEAD --no-color --no-ext-diff | hash_stdin)"

if [ -z "$(git status --porcelain --untracked-files=no)" ]; then
  DIRTY=false
else
  DIRTY=true
fi

RUSTC_V="$(rustc -V)"
CARGO_V="$(cargo -V)"
TOOLCHAIN_PIN="$(awk -F'"' '/^channel/ {print $2; exit}' rust-toolchain.toml)"
[ -n "$TOOLCHAIN_PIN" ] || { echo "error: no channel found in rust-toolchain.toml" >&2; exit 2; }

OUTPUT_SHA="null"
if [ -n "$OUTPUT_FILE" ]; then
  [ -f "$OUTPUT_FILE" ] || { echo "error: output file not found: $OUTPUT_FILE" >&2; exit 2; }
  OUTPUT_SHA="\"$(hash_stdin < "$OUTPUT_FILE")\""
fi

printf '{"record":"repro_tuple","git_head":"%s","worktree_patch_sha256":"%s","dirty":%s,"rustc":"%s","cargo":"%s","toolchain_pin":"%s","output_sha256":%s}\n' \
  "$GIT_HEAD" \
  "$WORKTREE_PATCH_SHA" \
  "$DIRTY" \
  "$(json_escape "$RUSTC_V")" \
  "$(json_escape "$CARGO_V")" \
  "$TOOLCHAIN_PIN" \
  "$OUTPUT_SHA"
