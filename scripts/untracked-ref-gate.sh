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

set -euo pipefail
cd "$(dirname "$0")/.."

PATTERN='[.]third|(^|[^[:alnum:]_./])[d]ocs/|AGENTS[.]md|PRD[.]md|GOALS[.]md|REF[.]md|[.]goal/|/Users/[a-z]'

excludes=(':(exclude).gitignore' ':(exclude)scripts/untracked-ref-gate.sh' ':(exclude)Cargo.lock')

hits="$(git grep -nE "$PATTERN" -- . "${excludes[@]}" || true)"
if [ -n "$hits" ]; then
  echo "untracked-ref-gate: tracked files reference never-tracked local paths:" >&2
  printf '%s\n' "$hits" >&2
  echo "untracked-ref-gate: rewrite the reference (self-describing prose, an" >&2
  echo "untracked-ref-gate: env var, or the target/evidence root) and rerun." >&2
  exit 1
fi
echo "untracked-ref-gate: clean"
