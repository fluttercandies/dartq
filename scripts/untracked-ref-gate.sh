#!/usr/bin/env bash
# untracked-ref-gate.sh — forbid tracked files from referencing never-tracked
# local workspace material: vendored reference checkouts, the untracked
# evidence/ledger/charter paths, and absolute home paths. A fresh clone has
# none of these, so every such reference in tracked content dangles.
#
# The pattern below is written with character classes ([.], [d], [a-z]) so
# this script never literally contains what it forbids — otherwise it would
# flag itself. Matched forms: dot-prefixed vendored checkouts, untracked
# evidence/ledger directory references, the local charter file names, and
# absolute /Users/<user> home paths. .gitignore entries and Cargo.lock are
# out of scope (ignore rules legitimately name the paths; lockfiles are
# generated).
#
# PENDING (phase-B debt): the crates in PENDING_PATHS still carry references
# owned by in-flight purge lanes; the gate skips them and says so. Delete
# entries as those lanes land — the gate then covers the whole tracked tree.

set -euo pipefail
cd "$(dirname "$0")/.."

PATTERN='[.]third|(^|[^[:alnum:]_./])[d]ocs/|AGENTS[.]md|PRD[.]md|GOALS[.]md|REF[.]md|[.]goal/|/Users/[a-z]'

PENDING_PATHS=(
  crates/dartq-plugin-host
  crates/dartq-semantic
  crates/dartq-parser
  crates/dartq-lints
  crates/dartq-daemon
)

excludes=(':(exclude).gitignore' ':(exclude)scripts/untracked-ref-gate.sh' ':(exclude)Cargo.lock')
if [ "${#PENDING_PATHS[@]}" -gt 0 ]; then
  printf 'untracked-ref-gate: skipping %d pending path(s) (phase-B debt): %s\n' \
    "${#PENDING_PATHS[@]}" "${PENDING_PATHS[*]}" >&2
  for p in "${PENDING_PATHS[@]}"; do
    excludes+=(":(exclude)$p")
  done
fi

hits="$(git grep -nE "$PATTERN" -- . "${excludes[@]}" || true)"
if [ -n "$hits" ]; then
  echo "untracked-ref-gate: tracked files reference never-tracked local paths:" >&2
  printf '%s\n' "$hits" >&2
  echo "untracked-ref-gate: rewrite the reference (self-describing prose, an" >&2
  echo "untracked-ref-gate: env var, or the target/evidence root) and rerun." >&2
  exit 1
fi
echo "untracked-ref-gate: clean"
